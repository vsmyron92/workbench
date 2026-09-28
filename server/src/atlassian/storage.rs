//! Confluence storage format helpers: inline-comment markers (the safety check on
//! page updates), validation before sending an edit, and a readable plain-text /
//! markdown-ish rendering for agents.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use super::html::{decode_entities, escape};
use super::xml::{self, Node};

static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<ac:inline-comment-marker\b[^>]*?\bac:ref\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());

/// The `ac:ref` of every inline-comment marker, in document order, without duplicates.
pub fn inline_marker_refs(storage: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    MARKER
        .captures_iter(storage)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)).map(|m| m.as_str().to_string()))
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

/// Markers present in `before` but missing from `after`.
pub fn dropped_markers(before: &str, after: &str) -> Vec<String> {
    let kept: BTreeSet<String> = inline_marker_refs(after).into_iter().collect();
    inline_marker_refs(before).into_iter().filter(|r| !kept.contains(r)).collect()
}

/// Check that an edit is well-formed storage XML (Confluence answers malformed
/// storage with an opaque 400; this gives the line and column instead).
pub fn validate(storage: &str) -> Result<(), String> {
    xml::parse(storage).map(|_| ()).map_err(|e| format!("The page source is not valid storage format ({e})"))
}

// ---------------------------------------------------------------- plain text

/// A readable, markdown-like rendering of storage format for agents and previews.
/// Macros are summarized (`code` becomes a fenced block, panels become quotes).
pub fn to_text(storage: &str) -> String {
    match xml::parse(storage) {
        Ok(nodes) => {
            let out = blocks(&nodes);
            collapse_blank_lines(&out)
        }
        Err(_) => {
            // Not well-formed (legacy pages): strip tags as a fallback.
            static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
            decode_entities(&TAG.replace_all(storage, " ")).split_whitespace().collect::<Vec<_>>().join(" ")
        }
    }
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim().to_string()
}

const BLOCK_TAGS: &[&str] = &[
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "tbody", "thead", "tfoot", "tr", "td", "th",
    "pre", "blockquote", "hr", "div", "ac:layout", "ac:layout-section", "ac:layout-cell", "ac:task-list", "colgroup",
];

fn is_block(n: &Node) -> bool {
    match n {
        Node::Element { name, .. } => {
            if name == "ac:structured-macro" {
                !matches!(n.attr("ac:name"), Some("status" | "jira" | "anchor" | "emoticon"))
            } else {
                BLOCK_TAGS.contains(&name.as_str())
            }
        }
        _ => false,
    }
}

/// Render a list of nodes in block context: inline runs become paragraphs.
fn blocks(nodes: &[Node]) -> String {
    blocks_sep(nodes, "\n\n")
}

fn blocks_sep(nodes: &[Node], sep: &str) -> String {
    let mut parts: Vec<String> = vec![];
    let mut run: Vec<&Node> = vec![];
    let flush = |run: &mut Vec<&Node>, parts: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        let text = inline(run.iter().copied());
        if !text.trim().is_empty() {
            parts.push(text.trim().to_string());
        }
        run.clear();
    };
    for n in nodes {
        if is_block(n) {
            flush(&mut run, &mut parts);
            let b = block(n);
            if !b.trim().is_empty() {
                parts.push(b);
            }
        } else {
            run.push(n);
        }
    }
    flush(&mut run, &mut parts);
    parts.join(sep)
}

fn indent(s: &str, first: &str, rest: &str) -> String {
    s.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { format!("{first}{l}") } else if l.is_empty() { String::new() } else { format!("{rest}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn macro_param<'a>(n: &'a Node, name: &str) -> Option<&'a Node> {
    n.children().iter().find(|c| c.name() == "ac:parameter" && c.attr("ac:name") == Some(name))
}

fn block(n: &Node) -> String {
    let name = n.name();
    let kids = n.children();
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = name[1..].parse::<usize>().unwrap_or(1);
            format!("{} {}", "#".repeat(level), inline(kids.iter()).trim())
        }
        "p" => inline(kids.iter()).trim().to_string(),
        "hr" => "---".into(),
        "pre" => format!("```\n{}\n```", n.text().trim_end()),
        "blockquote" => indent(&blocks(kids), "> ", "> "),
        "ul" | "ol" => {
            let ordered = name == "ol";
            let start = n.attr("start").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
            kids.iter()
                .filter(|c| c.name() == "li")
                .enumerate()
                .map(|(i, li)| {
                    let marker = if ordered { format!("{}. ", start + i) } else { "- ".into() };
                    let pad = " ".repeat(marker.len());
                    // Tight: a list item's paragraphs and nested lists stay on adjacent lines.
                    indent(&blocks_sep(li.children(), "\n"), &marker, &pad)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "li" => blocks(kids),
        "table" => table(n),
        "ac:task-list" => kids
            .iter()
            .filter(|c| c.name() == "ac:task")
            .map(|t| {
                let done = t.child("ac:task-status").map(|s| s.text().trim() == "complete").unwrap_or(false);
                let body = t.child("ac:task-body").map(|b| inline(b.children().iter())).unwrap_or_default();
                format!("- [{}] {}", if done { "x" } else { " " }, body.trim())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "ac:structured-macro" => structured_macro(n),
        "colgroup" => String::new(),
        _ => blocks(kids),
    }
}

fn structured_macro(n: &Node) -> String {
    let name = n.attr("ac:name").unwrap_or("macro");
    let body = n.child("ac:rich-text-body");
    let plain = n.child("ac:plain-text-body");
    let title = macro_param(n, "title").map(|p| p.text());
    match name {
        "code" | "noformat" => {
            let lang = macro_param(n, "language").map(|p| p.text()).unwrap_or_default();
            let code = plain.map(|p| p.text()).unwrap_or_default();
            let head = title.map(|t| format!("**{}**\n", t.trim())).unwrap_or_default();
            format!("{head}```{}\n{}\n```", lang.trim(), code.trim_end_matches('\n'))
        }
        "info" | "note" | "warning" | "tip" | "panel" => {
            let label = match name {
                "info" => "Info",
                "note" => "Note",
                "warning" => "Warning",
                "tip" => "Tip",
                _ => "Panel",
            };
            let head = match &title {
                Some(t) if !t.trim().is_empty() => format!("**{label}: {}**", t.trim()),
                _ => format!("**{label}:**"),
            };
            let inner = body.map(|b| blocks(b.children())).unwrap_or_default();
            indent(&format!("{head}\n{inner}"), "> ", "> ")
        }
        "expand" => {
            let t = title.unwrap_or_else(|| "Details".into());
            let inner = body.map(|b| blocks(b.children())).unwrap_or_default();
            format!("**▸ {}**\n\n{inner}", t.trim())
        }
        "toc" => "[Table of contents]".into(),
        "children" => "[Child pages]".into(),
        _ => match body {
            Some(b) => blocks(b.children()),
            None => match plain {
                Some(p) => format!("```\n{}\n```", p.text().trim_end()),
                None => format!("[{name} macro]"),
            },
        },
    }
}

fn table(n: &Node) -> String {
    let mut rows: Vec<(bool, Vec<String>)> = vec![];
    fn collect(n: &Node, rows: &mut Vec<(bool, Vec<String>)>) {
        for c in n.children() {
            match c.name() {
                "tr" => {
                    let cells: Vec<&Node> = c.children().iter().filter(|x| matches!(x.name(), "td" | "th")).collect();
                    let header = !cells.is_empty() && cells.iter().all(|x| x.name() == "th");
                    let texts = cells
                        .iter()
                        .map(|x| blocks(x.children()).replace('\n', " ").replace('|', "\\|").trim().to_string())
                        .collect();
                    rows.push((header, texts));
                }
                "tbody" | "thead" | "tfoot" => collect(c, rows),
                _ => {}
            }
        }
    }
    collect(n, &mut rows);
    if rows.is_empty() {
        return String::new();
    }
    let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(1);
    let line = |cells: &[String]| {
        let mut v: Vec<String> = cells.to_vec();
        v.resize(width, String::new());
        format!("| {} |", v.join(" | "))
    };
    let mut out = vec![];
    let (first_header, first) = &rows[0];
    if *first_header {
        out.push(line(first));
        out.push(format!("|{}|", vec!["---"; width].join("|")));
        out.extend(rows[1..].iter().map(|r| line(&r.1)));
    } else {
        out.push(line(&vec![String::new(); width]));
        out.push(format!("|{}|", vec!["---"; width].join("|")));
        out.extend(rows.iter().map(|r| line(&r.1)));
    }
    out.join("\n")
}

fn inline<'a>(nodes: impl Iterator<Item = &'a Node>) -> String {
    let mut out = String::new();
    for n in nodes {
        inline_node(n, &mut out);
    }
    out
}

fn push_text(out: &mut String, t: &str) {
    // Collapse whitespace the way HTML rendering does.
    let mut last_space = out.ends_with(' ') || out.ends_with('\n') || out.is_empty();
    for c in t.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(if c == '\u{a0}' { ' ' } else { c });
            last_space = false;
        }
    }
}

fn wrap(out: &mut String, mark: &str, inner: String) {
    let t = inner.trim();
    if t.is_empty() {
        out.push_str(&inner);
        return;
    }
    if inner.starts_with(' ') && !out.ends_with(' ') {
        out.push(' ');
    }
    out.push_str(mark);
    out.push_str(t);
    out.push_str(mark);
    if inner.ends_with(' ') {
        out.push(' ');
    }
}

fn inline_node(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) => push_text(out, t),
        Node::CData(t) => out.push_str(t),
        Node::Element { name, children, .. } => match name.as_str() {
            "br" => {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push('\n');
            }
            "strong" | "b" => wrap(out, "**", inline(children.iter())),
            "em" | "i" => wrap(out, "*", inline(children.iter())),
            "del" | "s" | "strike" => wrap(out, "~~", inline(children.iter())),
            "code" => wrap(out, "`", n.text()),
            "a" => {
                let text = inline(children.iter());
                match n.attr("href") {
                    Some(h) if !h.is_empty() => out.push_str(&format!("[{}]({h})", text.trim())),
                    _ => out.push_str(&text),
                }
            }
            "img" => out.push_str(&format!("![{}]({})", n.attr("alt").unwrap_or(""), n.attr("src").unwrap_or(""))),
            "time" => out.push_str(n.attr("datetime").unwrap_or("")),
            "ac:link" => {
                let body = n
                    .child("ac:link-body")
                    .or_else(|| n.child("ac:plain-text-link-body"))
                    .map(|b| b.text())
                    .filter(|t| !t.trim().is_empty());
                let target = if let Some(p) = n.child("ri:page") {
                    p.attr("ri:content-title").map(|t| format!("[[{t}]]"))
                } else if let Some(u) = n.child("ri:user") {
                    Some(format!("@{}", u.attr("ri:account-id").or(u.attr("ri:username")).unwrap_or("user")))
                } else if let Some(a) = n.child("ri:attachment") {
                    a.attr("ri:filename").map(|f| format!("[attachment: {f}]"))
                } else {
                    None
                };
                let anchor = n.attr("ac:anchor").map(|a| format!("#{a}"));
                match (body, target) {
                    (Some(b), Some(t)) => out.push_str(&format!("{} ({t})", b.trim())),
                    (Some(b), None) => out.push_str(b.trim()),
                    (None, Some(t)) => out.push_str(&t),
                    (None, None) => out.push_str(&anchor.unwrap_or_default()),
                }
            }
            "ac:image" => {
                let src = n
                    .child("ri:attachment")
                    .and_then(|a| a.attr("ri:filename"))
                    .or_else(|| n.child("ri:url").and_then(|u| u.attr("ri:value")))
                    .unwrap_or("image");
                out.push_str(&format!("![{src}]"));
            }
            "ac:emoticon" => out.push_str(
                n.attr("ac:emoji-fallback").or(n.attr("ac:emoji-shortname")).or(n.attr("ac:name")).unwrap_or(""),
            ),
            "ac:structured-macro" => match n.attr("ac:name") {
                Some("status") => {
                    let t = macro_param(n, "title").map(|p| p.text()).unwrap_or_default();
                    out.push_str(&format!("[{}]", t.trim().to_uppercase()));
                }
                Some("jira") => {
                    let k = macro_param(n, "key").map(|p| p.text()).unwrap_or_default();
                    out.push_str(&format!("[Jira {}]", k.trim()));
                }
                Some("anchor") => {}
                _ => out.push_str(&structured_macro(n)),
            },
            "ac:placeholder" | "ac:parameter" | "colgroup" | "col" => {}
            n2 if n2.starts_with("ri:") => {}
            _ => {
                for c in children {
                    inline_node(c, out);
                }
            }
        },
    }
}

// ---------------------------------------------------------------- HTML

const HTML_TAGS: &[&str] = &[
    "p", "br", "strong", "b", "em", "i", "u", "s", "del", "strike", "code", "pre", "a", "ul", "ol", "li", "h1", "h2",
    "h3", "h4", "h5", "h6", "blockquote", "table", "thead", "tbody", "tfoot", "tr", "th", "td", "hr", "span", "sub",
    "sup", "div", "colgroup", "col", "time",
];
const HTML_ATTRS: &[&str] = &["href", "style", "colspan", "rowspan", "datetime", "title"];

/// Render storage format as HTML for display where Confluence does not give us its
/// own `view` rendering (v2 comments). Mentions use `names` (account id → name);
/// macros are summarized. The result must still go through the sanitizer.
pub fn to_html(storage: &str, names: &HashMap<String, String>) -> String {
    match xml::parse(storage) {
        Ok(nodes) => {
            let mut out = String::new();
            for n in &nodes {
                html_node(n, names, &mut out);
            }
            out
        }
        Err(_) => format!("<p>{}</p>", escape(&to_text(storage))),
    }
}

/// Account ids mentioned with `<ri:user ri:account-id=…>`.
pub fn mentioned_accounts(storage: &str) -> Vec<String> {
    static USER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"ri:account-id\s*=\s*"([^"]+)""#).unwrap());
    USER.captures_iter(storage).map(|c| c[1].to_string()).collect()
}

fn html_children(n: &Node, names: &HashMap<String, String>, out: &mut String) {
    for c in n.children() {
        html_node(c, names, out);
    }
}

fn html_node(n: &Node, names: &HashMap<String, String>, out: &mut String) {
    match n {
        Node::Text(t) | Node::CData(t) => out.push_str(&escape(t)),
        Node::Element { name, attrs, .. } => match name.as_str() {
            tag if HTML_TAGS.contains(&tag) => {
                out.push('<');
                out.push_str(tag);
                for (k, v) in attrs.iter().filter(|(k, _)| HTML_ATTRS.contains(&k.as_str())) {
                    out.push_str(&format!(" {k}=\"{}\"", escape(v)));
                }
                if matches!(tag, "br" | "hr" | "col") {
                    out.push('>');
                    return;
                }
                out.push('>');
                html_children(n, names, out);
                out.push_str(&format!("</{tag}>"));
            }
            "ac:link" => {
                let body = n.child("ac:link-body").or_else(|| n.child("ac:plain-text-link-body")).map(|b| b.text());
                if let Some(u) = n.child("ri:user") {
                    let id = u.attr("ri:account-id").unwrap_or("");
                    let name = names.get(id).cloned().unwrap_or_else(|| "user".into());
                    out.push_str(&format!("<span class=\"user-mention\">@{}</span>", escape(&name)));
                } else if let Some(p) = n.child("ri:page") {
                    let title = p.attr("ri:content-title").unwrap_or("page");
                    let label = body.filter(|b| !b.trim().is_empty()).unwrap_or_else(|| title.to_string());
                    out.push_str(&format!("<span class=\"confluence-link\" title=\"{}\">{}</span>", escape(title), escape(&label)));
                } else {
                    out.push_str(&escape(&body.unwrap_or_default()));
                }
            }
            "ac:inline-comment-marker" => {
                out.push_str("<span class=\"inline-comment-marker\">");
                html_children(n, names, out);
                out.push_str("</span>");
            }
            "ac:emoticon" => out.push_str(&escape(
                n.attr("ac:emoji-fallback").or(n.attr("ac:emoji-shortname")).or(n.attr("ac:name")).unwrap_or(""),
            )),
            "ac:image" => {
                let f = n.child("ri:attachment").and_then(|a| a.attr("ri:filename")).unwrap_or("image");
                out.push_str(&format!("<em>[image: {}]</em>", escape(f)));
            }
            "ac:task-list" => {
                out.push_str("<ul>");
                for t in n.children().iter().filter(|c| c.name() == "ac:task") {
                    let done = t.child("ac:task-status").is_some_and(|s| s.text().trim() == "complete");
                    out.push_str(if done { "<li>☑ " } else { "<li>☐ " });
                    if let Some(b) = t.child("ac:task-body") {
                        html_children(b, names, out);
                    }
                    out.push_str("</li>");
                }
                out.push_str("</ul>");
            }
            "ac:structured-macro" => {
                let mname = n.attr("ac:name").unwrap_or("");
                match mname {
                    "code" | "noformat" => {
                        let code = n.child("ac:plain-text-body").map(|b| b.text()).unwrap_or_default();
                        out.push_str(&format!("<pre>{}</pre>", escape(&code)));
                    }
                    "status" => {
                        let t = macro_param(n, "title").map(|p| p.text()).unwrap_or_default();
                        out.push_str(&format!(
                            "<span class=\"status-macro aui-lozenge aui-lozenge-visual\">{}</span>",
                            escape(t.trim())
                        ));
                    }
                    "info" | "note" | "warning" | "tip" | "panel" => {
                        out.push_str(&format!(
                            "<div class=\"confluence-information-macro confluence-information-macro-{}\"><div class=\"confluence-information-macro-body\">",
                            if mname == "panel" { "information" } else { mname }
                        ));
                        if let Some(b) = n.child("ac:rich-text-body") {
                            html_children(b, names, out);
                        }
                        out.push_str("</div></div>");
                    }
                    _ => match n.child("ac:rich-text-body") {
                        Some(b) => html_children(b, names, out),
                        None => out.push_str(&format!("<em>[{} macro]</em>", escape(mname))),
                    },
                }
            }
            n2 if n2.starts_with("ac:") || n2.starts_with("ri:") => {}
            _ => html_children(n, names, out),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_compares_inline_markers() {
        let before = r#"<p>a <ac:inline-comment-marker ac:ref="r1">x</ac:inline-comment-marker> b <ac:inline-comment-marker ac:ref='r2'>y</ac:inline-comment-marker><ac:inline-comment-marker ac:ref="r1">z</ac:inline-comment-marker></p>"#;
        assert_eq!(inline_marker_refs(before), vec!["r1", "r2"]);
        let after = r#"<p>a <ac:inline-comment-marker ac:ref="r1">x</ac:inline-comment-marker> b y</p>"#;
        assert_eq!(dropped_markers(before, after), vec!["r2"]);
        assert!(dropped_markers(before, before).is_empty());
        assert!(dropped_markers("<p>none</p>", "<p/>").is_empty());
    }

    #[test]
    fn validates_storage() {
        assert!(validate("<p>ok &nbsp;</p><ac:structured-macro ac:name=\"toc\"/>").is_ok());
        let e = validate("<p>broken <b></p>").unwrap_err();
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn renders_text_for_agents() {
        let s = r#"<h2>Army cap</h2><p>Your <strong>army</strong> size&nbsp;is <em>capped</em>.<br />Next line</p><ul><li><p>one</p><ul><li>nested</li></ul></li><li>two</li></ul><table><tbody><tr><th><p>Tier</p></th><th><p>Cap</p></th></tr><tr><td><p>I</p></td><td><p>30</p></td></tr></tbody></table><ac:structured-macro ac:name="code"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[fn main() {}]]></ac:plain-text-body></ac:structured-macro><ac:structured-macro ac:name="info"><ac:rich-text-body><p>Careful</p></ac:rich-text-body></ac:structured-macro><p>See <ac:link><ri:page ri:content-title="GDD" /></ac:link> and <ac:structured-macro ac:name="status"><ac:parameter ac:name="title">done</ac:parameter></ac:structured-macro> <ac:inline-comment-marker ac:ref="x">marked</ac:inline-comment-marker></p>"#;
        let t = to_text(s);
        assert!(t.contains("## Army cap"), "{t}");
        assert!(t.contains("Your **army** size is *capped*.\nNext line"), "{t}");
        assert!(t.contains("- one\n  - nested\n- two"), "{t}");
        assert!(t.contains("| Tier | Cap |\n|---|---|\n| I | 30 |"), "{t}");
        assert!(t.contains("```rust\nfn main() {}\n```"), "{t}");
        assert!(t.contains("> **Info:**\n> Careful"), "{t}");
        assert!(t.contains("See [[GDD]] and [DONE] marked"), "{t}");
    }

    #[test]
    fn renders_the_template_page() {
        let t = to_text(include_str!("testdata/design_storage.xml"));
        assert!(t.contains("## Welcome to your design team space!"), "{t}");
        assert!(t.contains("![chantz.svg]"));
        assert!(t.contains("Template - Design Sprint ([[Template - Design Sprint]])"));
    }

    #[test]
    fn falls_back_on_malformed_storage() {
        assert_eq!(to_text("<p>a <b>b</p> &amp; c"), "a b & c");
    }

    #[test]
    fn renders_comment_storage_as_html() {
        let names: HashMap<String, String> = [("acc-1".to_string(), "Ann <Admin>".to_string())].into();
        let s = r#"<p>Hi <ac:link><ri:user ri:account-id="acc-1" /></ac:link>, see <ac:link><ri:page ri:content-title="GDD" /></ac:link> &amp; <a href="https://x.dev" onclick="x()">x</a></p><ac:structured-macro ac:name="code"><ac:plain-text-body><![CDATA[a < b]]></ac:plain-text-body></ac:structured-macro><script>alert(1)</script>"#;
        let h = to_html(s, &names);
        assert!(h.contains(r#"<span class="user-mention">@Ann &lt;Admin&gt;</span>"#), "{h}");
        assert!(h.contains(r#"<span class="confluence-link" title="GDD">GDD</span>"#), "{h}");
        assert!(h.contains(r#"<a href="https://x.dev">x</a>"#), "attributes are allow-listed: {h}");
        assert!(h.contains("<pre>a &lt; b</pre>"));
        assert!(h.contains("alert(1)") && !h.contains("<script"), "unknown elements keep only their text: {h}");
        assert_eq!(mentioned_accounts(s), vec!["acc-1"]);
        assert_eq!(to_html("<p>broken", &names), "<p>broken</p>");
    }
}
