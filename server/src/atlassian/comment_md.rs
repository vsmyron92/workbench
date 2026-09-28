//! Comment bodies as markdown for editing, and whether saving that markdown back would
//! change the comment (`CommentOut.editLossy`).
//!
//! Unlike `storage::to_text` (a readable rendering for agents), the markdown here is
//! written to be parsed again by `markdown::to_storage`: text is escaped (`src/*.rs` stays
//! literal, `__init__` stays `__init__`, a paragraph starting `1. ` stays a paragraph),
//! `<br/>` is a hard break (`\` at the end of the line) and Confluence task lists are
//! `- [ ]` items. `editLossy` is decided by that round trip: the storage the markdown
//! turns back into is compared with the original after normalizing what carries no
//! meaning (ids such as `local-id`, whitespace, `<b>` for `<strong>`, a paragraph inside a
//! list item or not, table column widths); any other difference is flagged.

use std::collections::HashMap;

use super::markdown;
use super::storage;
use super::xml::{self, Node};

/// A comment body as markdown for editing, and whether saving it back unchanged would
/// alter the comment. Mentions become `[@Name](mention:<accountId>)`, which
/// `markdown::to_storage` turns back into mentions.
pub fn to_markdown(storage_body: &str, names: &HashMap<String, String>) -> (String, bool) {
    let Ok(nodes) = xml::parse(storage_body) else {
        return (storage::to_text(storage_body), true);
    };
    let md = Emitter { names }.blocks(&nodes);
    let lossy = !markdown_safe(&nodes) || !round_trips(&nodes, &md);
    (md, lossy)
}

/// Whether `md`, saved as a comment, gives back what `nodes` say.
fn round_trips(nodes: &[Node], md: &str) -> bool {
    match xml::parse(&markdown::to_storage(md)) {
        Ok(back) => canonical(nodes) == canonical(&back),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------- safe subset

/// Elements markdown can express. Anything else is lossy without further checks.
const MARKDOWN_SAFE: &[&str] = &[
    "p", "br", "strong", "b", "em", "i", "del", "s", "strike", "code", "ul", "ol", "li", "h1", "h2", "h3", "h4", "h5",
    "h6", "blockquote", "pre", "hr", "table", "tbody", "thead", "tr", "th", "td", "colgroup", "col", "ac:task-list",
    "ac:task", "ac:task-id", "ac:task-uuid", "ac:task-status", "ac:task-body", "ac:plain-text-body",
];

fn markdown_safe(nodes: &[Node]) -> bool {
    nodes.iter().all(|n| match n {
        Node::Text(_) | Node::CData(_) => true,
        Node::Element { name, children, .. } => {
            let ok = match name.as_str() {
                "a" => n.attr("href").is_some_and(|h| !h.is_empty()),
                // A mention (`[@Name](mention:id)` in markdown).
                "ac:link" => children.len() == 1 && n.child("ri:user").is_some_and(|u| u.attr("ri:account-id").is_some()),
                "ri:user" => true,
                "ac:structured-macro" => n.attr("ac:name") == Some("code"),
                "ac:parameter" => n.attr("ac:name") == Some("language"),
                other => MARKDOWN_SAFE.contains(&other),
            };
            ok && markdown_safe(children)
        }
    })
}

// ---------------------------------------------------------------- storage → markdown

const BLOCK_TAGS: &[&str] = &[
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "tbody", "thead", "tfoot", "tr", "td", "th",
    "pre", "blockquote", "hr", "div", "ac:layout", "ac:layout-section", "ac:layout-cell", "ac:task-list", "colgroup",
];

fn is_block(n: &Node) -> bool {
    match n {
        Node::Element { name, .. } if name == "ac:structured-macro" => {
            !matches!(n.attr("ac:name"), Some("status" | "jira" | "anchor" | "emoticon"))
        }
        Node::Element { name, .. } => BLOCK_TAGS.contains(&name.as_str()),
        _ => false,
    }
}

fn macro_param<'a>(n: &'a Node, name: &str) -> Option<&'a Node> {
    n.children().iter().find(|c| c.name() == "ac:parameter" && c.attr("ac:name") == Some(name))
}

/// Prefix the first line with `first` and the others (non-empty) with `rest`.
fn indent(s: &str, first: &str, rest: &str) -> String {
    s.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { format!("{first}{l}") } else if l.is_empty() { String::new() } else { format!("{rest}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fenced code block whose fence is longer than any backtick run in `code`.
fn fence(code: &str, lang: &str) -> String {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let f = "`".repeat(longest.max(2) + 1);
    format!("{f}{}\n{}\n{f}", lang.trim(), code.trim_end_matches('\n'))
}

/// An inline code span that keeps `code` exactly (backticks and edge spaces included).
fn code_span(code: &str) -> String {
    let code = code.replace(['\n', '\r'], " ");
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest + 1);
    let pad = code.starts_with('`') || code.ends_with('`') || (code.starts_with(' ') && code.ends_with(' ') && !code.trim().is_empty());
    let sp = if pad { " " } else { "" };
    format!("{ticks}{sp}{code}{sp}{ticks}")
}

/// Text that would start an entity reference (`&amp;`, `&#42;`) if left alone.
fn entity_like(rest: &str) -> bool {
    let body: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '#').take(33).collect();
    !body.is_empty() && body.len() <= 32 && rest[body.len()..].starts_with(';')
}

/// A link destination markdown reads back as `href`.
fn link_dest(href: &str) -> String {
    let mut out = String::new();
    let angle = href.contains(|c: char| c.is_whitespace());
    for (i, c) in href.char_indices() {
        match c {
            '\\' | '<' | '>' => {
                out.push('\\');
                out.push(c);
            }
            '(' | ')' if !angle => {
                out.push('\\');
                out.push(c);
            }
            '&' if entity_like(&href[i + 1..]) => out.push_str("\\&"),
            _ => out.push(c),
        }
    }
    if angle { format!("<{out}>") } else { out }
}

/// Escape what a line of paragraph text must not start with: an ATX heading, a quote, a
/// list item, a setext underline or a thematic break.
fn escape_line_starts(s: &str) -> String {
    s.split('\n')
        .map(|line| {
            if line.starts_with(['#', '>', '-', '+', '=']) {
                return format!("\\{line}");
            }
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=9).contains(&digits) && line[digits..].starts_with(['.', ')']) {
                return format!("{}\\{}", &line[..digits], &line[digits..]);
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Emitter<'a> {
    names: &'a HashMap<String, String>,
}

impl Emitter<'_> {
    fn blocks(&self, nodes: &[Node]) -> String {
        self.block_parts(nodes).join("\n\n")
    }

    /// Blocks of a node list; runs of inline nodes become paragraphs.
    fn block_parts(&self, nodes: &[Node]) -> Vec<String> {
        let mut parts: Vec<String> = vec![];
        let mut run: Vec<&Node> = vec![];
        let flush = |run: &mut Vec<&Node>, parts: &mut Vec<String>| {
            let p = self.para(run.as_slice());
            if !p.is_empty() {
                parts.push(p);
            }
            run.clear();
        };
        for n in nodes {
            if is_block(n) {
                flush(&mut run, &mut parts);
                let b = self.block(n);
                if !b.trim().is_empty() {
                    parts.push(b);
                }
            } else {
                run.push(n);
            }
        }
        flush(&mut run, &mut parts);
        parts
    }

    /// A paragraph of inline nodes: trimmed, hard breaks at its ends dropped (markdown
    /// cannot write them there), line starts escaped.
    fn para(&self, nodes: &[&Node]) -> String {
        let s = self.inline(nodes.iter().copied());
        let mut t = s.as_str();
        loop {
            let before = t;
            t = t.trim_matches(' ');
            t = t.strip_prefix("\\\n").unwrap_or(t);
            t = t.strip_suffix("\\\n").unwrap_or(t);
            if t == before {
                break;
            }
        }
        escape_line_starts(t)
    }

    fn block(&self, n: &Node) -> String {
        let name = n.name();
        let kids = n.children();
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse::<usize>().unwrap_or(1);
                let mut t = self.para(&kids.iter().collect::<Vec<_>>()).replace("\\\n", " ");
                // A closing `#` run would be taken for the heading's optional closing sequence.
                if t.ends_with('#') && !t.ends_with("\\#") {
                    t.insert(t.len() - 1, '\\');
                }
                format!("{} {t}", "#".repeat(level))
            }
            "p" => self.para(&kids.iter().collect::<Vec<_>>()),
            "hr" => "---".into(),
            "pre" => fence(&n.text(), ""),
            "blockquote" => {
                let inner = self.blocks(kids);
                inner.lines().map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect::<Vec<_>>().join("\n")
            }
            "ul" | "ol" => self.list(n, name == "ol"),
            "table" => self.table(n),
            "ac:task-list" => kids
                .iter()
                .filter(|c| c.name() == "ac:task")
                .map(|t| {
                    let done = t.child("ac:task-status").is_some_and(|s| s.text().trim() == "complete");
                    let body = t.child("ac:task-body").map(|b| self.item_body(b.children())).unwrap_or_default();
                    indent(&body, if done { "- [x] " } else { "- [ ] " }, "  ")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            "ac:structured-macro" => self.structured_macro(n),
            "colgroup" => String::new(),
            _ => self.blocks(kids),
        }
    }

    /// A list item's blocks: a nested list follows its paragraph directly, other blocks
    /// are separated by a blank line (so a second paragraph is not a lazy continuation).
    fn item_body(&self, nodes: &[Node]) -> String {
        let parts = self.block_parts(nodes);
        let mut out = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                let list_next = p.starts_with("- ") || p.split_once(". ").is_some_and(|(d, _)| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
                out.push_str(if list_next { "\n" } else { "\n\n" });
            }
            out.push_str(p);
        }
        out
    }

    fn list(&self, n: &Node, ordered: bool) -> String {
        let start = n.attr("start").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
        n.children()
            .iter()
            .filter(|c| c.name() == "li")
            .enumerate()
            .map(|(i, li)| {
                let marker = if ordered { format!("{}. ", start + i) } else { "- ".into() };
                let pad = " ".repeat(marker.len());
                let body = self.item_body(li.children());
                if body.is_empty() { marker.trim_end().to_string() } else { indent(&body, &marker, &pad) }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn table(&self, n: &Node) -> String {
        fn rows<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
            for c in n.children() {
                match c.name() {
                    "tr" => out.push(c),
                    "tbody" | "thead" | "tfoot" => rows(c, out),
                    _ => {}
                }
            }
        }
        let mut trs = vec![];
        rows(n, &mut trs);
        let rows: Vec<(bool, Vec<String>)> = trs
            .iter()
            .map(|tr| {
                let cells: Vec<&Node> = tr.children().iter().filter(|x| matches!(x.name(), "td" | "th")).collect();
                let header = !cells.is_empty() && cells.iter().all(|x| x.name() == "th");
                (header, cells.iter().map(|c| self.blocks(c.children()).replace("\\\n", " ").replace('\n', " ")).collect())
            })
            .collect();
        if rows.is_empty() {
            return String::new();
        }
        let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(0).max(1);
        let line = |cells: &[String]| {
            let mut v = cells.to_vec();
            v.resize(width, String::new());
            format!("| {} |", v.join(" | "))
        };
        let rule = format!("|{}|", vec!["---"; width].join("|"));
        let (first_header, first) = &rows[0];
        let mut out = vec![];
        if *first_header {
            out.push(line(first));
            out.push(rule);
            out.extend(rows[1..].iter().map(|r| line(&r.1)));
        } else {
            out.push(line(&vec![String::new(); width]));
            out.push(rule);
            out.extend(rows.iter().map(|r| line(&r.1)));
        }
        out.join("\n")
    }

    fn structured_macro(&self, n: &Node) -> String {
        let name = n.attr("ac:name").unwrap_or("macro");
        let body = n.child("ac:rich-text-body");
        let plain = n.child("ac:plain-text-body");
        let title = macro_param(n, "title").map(|p| p.text());
        match name {
            "code" | "noformat" => {
                let lang = macro_param(n, "language").map(|p| p.text()).unwrap_or_default();
                fence(&plain.map(|p| p.text()).unwrap_or_default(), &lang)
            }
            "info" | "note" | "warning" | "tip" | "panel" | "expand" => {
                // Not expressible: the body is kept, the panel is flagged lossy.
                let head = title.filter(|t| !t.trim().is_empty()).map(|t| format!("**{}**\n\n", escape_text(t.trim(), None)));
                format!("{}{}", head.unwrap_or_default(), body.map(|b| self.blocks(b.children())).unwrap_or_default())
            }
            _ => match (body, plain) {
                (Some(b), _) => self.blocks(b.children()),
                (None, Some(p)) => fence(&p.text(), ""),
                (None, None) => escape_text(&format!("[{name} macro]"), None),
            },
        }
    }

    fn inline<'n>(&self, nodes: impl Iterator<Item = &'n Node>) -> String {
        let mut out = String::new();
        for n in nodes {
            self.inline_node(n, &mut out);
        }
        out
    }

    fn inline_node(&self, n: &Node, out: &mut String) {
        match n {
            Node::Text(t) | Node::CData(t) => push_text(out, t),
            Node::Element { name, children, .. } => match name.as_str() {
                "br" => {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str("\\\n");
                }
                "strong" | "b" => wrap(out, "**", self.inline(children.iter())),
                "em" | "i" => wrap(out, "*", self.inline(children.iter())),
                "del" | "s" | "strike" => wrap(out, "~~", self.inline(children.iter())),
                "code" => out.push_str(&code_span(&n.text())),
                "a" => {
                    let text = self.inline(children.iter());
                    match n.attr("href") {
                        Some(h) if !h.is_empty() => {
                            wrap_with(out, "[", &format!("]({})", link_dest(h)), text);
                        }
                        _ => out.push_str(&text),
                    }
                }
                "ac:link" => {
                    if let Some(id) = n.child("ri:user").and_then(|u| u.attr("ri:account-id")) {
                        let name = self.names.get(id).map(String::as_str).unwrap_or("user").replace(['[', ']'], "");
                        out.push_str(&format!("[@{}](mention:{id})", escape_text(&name, None)));
                        return;
                    }
                    // Page and attachment links have no markdown form: their text is kept.
                    let body = n
                        .child("ac:link-body")
                        .or_else(|| n.child("ac:plain-text-link-body"))
                        .map(|b| b.text())
                        .filter(|t| !t.trim().is_empty());
                    let target = n
                        .child("ri:page")
                        .and_then(|p| p.attr("ri:content-title"))
                        .or_else(|| n.child("ri:attachment").and_then(|a| a.attr("ri:filename")));
                    if let Some(t) = body.as_deref().or(target) {
                        push_text(out, t.trim());
                    }
                }
                "ac:image" | "img" => {
                    let src = n
                        .child("ri:attachment")
                        .and_then(|a| a.attr("ri:filename"))
                        .or_else(|| n.child("ri:url").and_then(|u| u.attr("ri:value")))
                        .or(n.attr("src"))
                        .unwrap_or("image");
                    push_text(out, &format!("[image: {src}]"));
                }
                "time" => push_text(out, n.attr("datetime").unwrap_or("")),
                "ac:emoticon" => push_text(out, n.attr("ac:emoji-fallback").or(n.attr("ac:emoji-shortname")).or(n.attr("ac:name")).unwrap_or("")),
                "ac:structured-macro" => match n.attr("ac:name") {
                    Some("status") => {
                        let t = macro_param(n, "title").map(|p| p.text()).unwrap_or_default();
                        push_text(out, &t.trim().to_uppercase());
                    }
                    Some("jira") => push_text(out, macro_param(n, "key").map(|p| p.text()).unwrap_or_default().trim()),
                    Some("anchor") => {}
                    _ => out.push_str(&self.structured_macro(n)),
                },
                "ac:placeholder" | "ac:parameter" | "colgroup" | "col" => {}
                n2 if n2.starts_with("ri:") => {}
                _ => {
                    for c in children {
                        self.inline_node(c, out);
                    }
                }
            },
        }
    }
}

/// Escape markdown syntax in text so it reads back as the same text. `prev` is the
/// character before the text (for `_`, which only needs escaping at word edges).
fn escape_text(t: &str, prev: Option<char>) -> String {
    let mut out = String::with_capacity(t.len() + 8);
    let chars: Vec<(usize, char)> = t.char_indices().collect();
    let mut prev = prev;
    for (k, &(i, c)) in chars.iter().enumerate() {
        let next = chars.get(k + 1).map(|&(_, n)| n);
        let escape = match c {
            '*' | '`' | '[' | ']' | '~' | '|' => true,
            '\\' => next.is_none_or(|n| n.is_ascii_punctuation()),
            '_' => !(prev.is_some_and(char::is_alphanumeric) && next.is_some_and(char::is_alphanumeric)),
            '<' => next.is_none_or(|n| n.is_ascii_alphabetic() || matches!(n, '/' | '!' | '?')),
            '&' => entity_like(&t[i + c.len_utf8()..]),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Append text, collapsing whitespace the way HTML rendering does, escaped.
fn push_text(out: &mut String, t: &str) {
    let mut collapsed = String::with_capacity(t.len());
    let mut last_space = out.is_empty() || out.ends_with(' ') || out.ends_with('\n');
    for c in t.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !last_space {
                collapsed.push(' ');
                last_space = true;
            }
        } else {
            collapsed.push(c);
            last_space = false;
        }
    }
    let prev = out.chars().next_back();
    out.push_str(&escape_text(&collapsed, prev));
}

/// `mark` around `inner`, with the spaces at its edges moved outside (markdown does not
/// open or close emphasis next to a space).
fn wrap(out: &mut String, mark: &str, inner: String) {
    wrap_with(out, mark, mark, inner)
}

fn wrap_with(out: &mut String, open: &str, close: &str, inner: String) {
    let t = inner.trim_matches(' ');
    if t.is_empty() && open == close {
        out.push_str(&inner);
        return;
    }
    if inner.starts_with(' ') && !out.is_empty() && !out.ends_with([' ', '\n']) {
        out.push(' ');
    }
    out.push_str(open);
    out.push_str(t);
    out.push_str(close);
    if inner.ends_with(' ') {
        out.push(' ');
    }
}

// ---------------------------------------------------------------- canonical form

/// Attributes that identify or lay out an element without changing what it says.
fn ignored_attr(name: &str) -> bool {
    name.ends_with("local-id")
        || matches!(name, "ac:macro-id" | "ac:schema-version" | "data-layout" | "data-table-width" | "ac:task-list-id")
}

const CANON_BLOCKS: &[&str] = &[
    "p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "table", "tr", "td", "th", "pre", "blockquote", "hr",
    "div", "ac:task-list", "ac:task", "ac:task-status", "codeblock", "ac:rich-text-body",
];

/// A normalized form of storage nodes for comparing a comment with its markdown round
/// trip: synonyms unified (`b` → `strong`, `pre` / code macro → `codeblock`), ids and
/// layout dropped, whitespace collapsed, inline runs in containers wrapped in `p`, empty
/// paragraphs and hard breaks at the ends of a block dropped.
fn canonical(nodes: &[Node]) -> String {
    let mut out = String::new();
    canon_container(nodes, &mut out);
    out
}

fn canon_container(nodes: &[Node], out: &mut String) {
    let mut run: Vec<&Node> = vec![];
    let flush = |run: &mut Vec<&Node>, out: &mut String| {
        if !run.is_empty() {
            let inner = canon_inline_block(run.as_slice());
            if !inner.is_empty() {
                out.push_str(&format!("<p>{inner}</p>"));
            }
            run.clear();
        }
    };
    for n in nodes {
        if let Some(name) = canon_name(n).filter(|name| CANON_BLOCKS.contains(&name.as_str())) {
            flush(&mut run, out);
            canon_block(n, &name, out);
        } else if matches!(n.name(), "tbody" | "thead" | "tfoot") {
            flush(&mut run, out);
            canon_container(n.children(), out);
        } else if !matches!(n.name(), "colgroup" | "col" | "ac:task-id" | "ac:task-uuid") {
            run.push(n);
        }
    }
    flush(&mut run, out);
}

/// The element's normalized name (`None` for text).
fn canon_name(n: &Node) -> Option<String> {
    let name = match n {
        Node::Element { name, .. } => name.as_str(),
        _ => return None,
    };
    Some(
        match name {
            "b" => "strong",
            "i" => "em",
            "s" | "strike" => "del",
            "thead" | "tbody" | "tfoot" => name,
            "pre" if n.children().iter().all(|c| !matches!(c, Node::Element { .. })) => "codeblock",
            "ac:structured-macro" if n.attr("ac:name") == Some("code") => "codeblock",
            "ac:structured-macro" if !matches!(n.attr("ac:name"), Some("status" | "jira" | "anchor" | "emoticon")) => "div",
            "ac:task-body" => "li",
            other => other,
        }
        .to_string(),
    )
}

fn canon_attrs(n: &Node) -> String {
    let Node::Element { attrs, name, .. } = n else { return String::new() };
    let mut a: Vec<(&str, &str)> = attrs
        .iter()
        .filter(|(k, v)| !ignored_attr(k) && !(name == "ol" && k == "start" && v == "1"))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    a.sort();
    a.iter().map(|(k, v)| format!(" {k}={v:?}")).collect()
}

fn canon_block(n: &Node, name: &str, out: &mut String) {
    match name {
        "codeblock" => {
            let (lang, text, params) = if n.name() == "pre" {
                (String::new(), n.text(), String::new())
            } else {
                let lang = macro_param(n, "language").map(|p| p.text().trim().to_ascii_lowercase()).unwrap_or_default();
                let mut params: Vec<String> = n
                    .children()
                    .iter()
                    .filter(|c| c.name() == "ac:parameter" && c.attr("ac:name") != Some("language"))
                    .map(|c| format!("{}={:?}", c.attr("ac:name").unwrap_or(""), c.text()))
                    .collect();
                params.sort();
                (lang, n.child("ac:plain-text-body").map(|p| p.text()).unwrap_or_default(), params.join(","))
            };
            out.push_str(&format!("<codeblock lang={lang:?} params={params:?}>{:?}</codeblock>", text.trim_end_matches('\n')));
        }
        "hr" => out.push_str("<hr/>"),
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let inner = canon_inline_block(&n.children().iter().collect::<Vec<_>>());
            if !inner.is_empty() || name != "p" {
                out.push_str(&format!("<{name}{}>{inner}</{name}>", canon_attrs(n)));
            }
        }
        "ac:task-status" => out.push_str(&format!("<status>{}</status>", n.text().trim())),
        _ => {
            out.push_str(&format!("<{name}{}>", canon_attrs(n)));
            canon_container(n.children(), out);
            out.push_str(&format!("</{name}>"));
        }
    }
}

/// Inline content of one block: whitespace collapsed (a no-break space counts as a
/// space), trimmed, without hard breaks at either end.
fn canon_inline_block(nodes: &[&Node]) -> String {
    let mut s = String::new();
    for n in nodes {
        canon_inline(n, &mut s);
    }
    let mut t = s.as_str();
    loop {
        let trimmed = t.trim_matches(' ');
        let next = trimmed.strip_prefix("<br/>").or_else(|| trimmed.strip_suffix("<br/>"));
        match next {
            Some(rest) => t = rest,
            None => return trimmed.to_string(),
        }
    }
}

fn canon_inline(n: &Node, out: &mut String) {
    match n {
        Node::Text(t) | Node::CData(t) => {
            for c in t.chars() {
                if c.is_whitespace() {
                    if !out.ends_with(' ') && !out.ends_with("<br/>") {
                        out.push(' ');
                    }
                } else {
                    // Escaped so text never looks like a tag of the canonical form.
                    match c {
                        '<' => out.push_str("&lt;"),
                        '&' => out.push_str("&amp;"),
                        _ => out.push(c),
                    }
                }
            }
        }
        Node::Element { children, .. } => {
            let name = canon_name(n).unwrap_or_default();
            match name.as_str() {
                "br" => {
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    out.push_str("<br/>");
                }
                "code" => out.push_str(&format!("<code>{:?}</code>", n.text().split_whitespace().collect::<Vec<_>>().join(" "))),
                _ => {
                    // `<strong>bold </strong>next` reads as `<strong>bold</strong> next`.
                    let mut inner = String::new();
                    for c in children {
                        canon_inline(c, &mut inner);
                    }
                    if inner.starts_with(' ') && !out.is_empty() && !out.ends_with(' ') && !out.ends_with("<br/>") {
                        out.push(' ');
                    }
                    out.push_str(&format!("<{name}{}>{}</{name}>", canon_attrs(n), inner.trim_matches(' ')));
                    if inner.ends_with(' ') && inner.trim_matches(' ') != "" {
                        out.push(' ');
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(storage: &str) -> (String, bool) {
        let names: HashMap<String, String> = [("557058:abc".to_string(), "Ann Lee".to_string())].into();
        to_markdown(storage, &names)
    }

    /// Saving the markdown unchanged gives back the same comment.
    fn assert_round_trip(storage: &str) -> String {
        let (m, lossy) = md(storage);
        assert!(!lossy, "flagged lossy: {storage}\n→ {m}\n→ {}", markdown::to_storage(&m));
        let back = markdown::to_storage(&m);
        assert_eq!(canonical(&xml::parse(storage).unwrap()), canonical(&xml::parse(&back).unwrap()), "{m}");
        m
    }

    #[test]
    fn comments_become_editable_markdown() {
        let m = assert_round_trip(
            r#"<p local-id="x">Hi <ac:link><ri:user ri:account-id="557058:abc" /></ac:link>, <strong>bold</strong> and <a href="https://x.dev">x</a></p>"#,
        );
        assert_eq!(m, "Hi [@Ann Lee](mention:557058:abc), **bold** and [x](https://x.dev)");
        let (_, lossy) = md(r#"<p>See <ac:link><ri:page ri:content-title="GDD" /></ac:link></p>"#);
        assert!(lossy, "page links cannot be written in markdown");
        let (m, lossy) = md(r#"<ac:structured-macro ac:name="info"><ac:rich-text-body><p>x</p></ac:rich-text-body></ac:structured-macro>"#);
        assert!(lossy && m == "x", "{m}");
        let m = assert_round_trip(
            r#"<ac:structured-macro ac:name="code" ac:schema-version="1" ac:macro-id="m1"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[fn x() { `a` }]]></ac:plain-text-body></ac:structured-macro>"#,
        );
        assert_eq!(m, "```rust\nfn x() { `a` }\n```");
        // Round trip through markdown keeps the mention.
        let back = markdown::to_storage("Hi [@Ann Lee](mention:557058:abc)");
        assert_eq!(back, r#"<p>Hi <ac:link><ri:user ri:account-id="557058:abc" /></ac:link></p>"#);
    }

    #[test]
    fn markdown_syntax_in_text_stays_text() {
        let m = assert_round_trip("<p>Compute a*b*c, match src/*.rs and tests/*.rs, not __init__.py or [x](y)</p><p>1. not a list</p>");
        assert_eq!(m, "Compute a\\*b\\*c, match src/\\*.rs and tests/\\*.rs, not \\_\\_init\\_\\_.py or \\[x\\](y)\n\n1\\. not a list");
        // Identifiers keep their inner underscores readable.
        assert_eq!(assert_round_trip("<p>my_var_name and snake_case</p>"), "my_var_name and snake_case");
        assert_round_trip("<p># not a heading</p><p>- not an item</p><p>&gt; not a quote</p><p>+ plus</p><p>2) two</p>");
        assert_round_trip("<p>a &lt;div&gt; tag, a &lt; b, &amp;amp; and &amp;copy; stay, C:\\Users\\x and trailing \\</p>");
        assert_round_trip("<p>~~not struck~~, a | b, `not code`, &lt;https://x.dev&gt;</p>");
        assert_round_trip("<p><code>a `tick` b</code> and <code> spaced </code> and <em>it*al</em></p>");
        assert_round_trip(r#"<p><a href="https://x.dev/a_(b)?q=1&amp;r=2">link *text*</a></p>"#);
    }

    #[test]
    fn hard_breaks_headings_lists_quotes_tables() {
        let m = assert_round_trip("<p>line one<br/>line two<br />- three</p>");
        assert_eq!(m, "line one\\\nline two\\\n\\- three");
        assert_round_trip("<p>trailing break<br/></p>");
        assert_round_trip("<h2>Title #1 #</h2><p>x</p>");
        assert_round_trip(
            r#"<ul><li><p>one</p><ul><li><p>nested *a*</p></li></ul></li><li><p>two</p><p>second para</p></li></ul><ol start="3"><li>three</li><li>four</li></ol>"#,
        );
        assert_round_trip("<blockquote><p>quoted</p><p>twice</p></blockquote><hr/><p>after</p>");
        assert_round_trip(
            r#"<table data-layout="default" ac:local-id="t1"><colgroup><col style="width: 20px;"/></colgroup><tbody><tr><th><p>A | B</p></th><th><p>C</p></th></tr><tr><td><p><strong>1</strong></p></td><td><p>2</p></td></tr></tbody></table>"#,
        );
    }

    #[test]
    fn markdown_written_comments_edit_back_unchanged() {
        // Comments posted from the composer (markdown) are edited as the same markdown.
        for src in [
            "Compute a\\*b\\*c, see \\[1\\] and \\<tag> or 2 < 3.\n\n1\\. not a list",
            "## Plan\n\n| Task | Owner |\n|---|---|\n| Ship | Ann |\n\n- [ ] write tests\n- [x] done\n\n> quoted\n\nline one\\\nline two",
            "Ping [@Ann Lee](mention:557058:abc): see `cfg.rs` and **bold *nested***.\n\n```ts\nconst a = 1\n```",
        ] {
            let storage = markdown::to_storage(src);
            let m = assert_round_trip(&storage);
            assert_eq!(markdown::to_storage(&m), storage, "{src}\n→ {m}");
        }
    }

    #[test]
    fn task_lists_stay_task_lists() {
        let s = r#"<ac:task-list ac:local-id="l1"><ac:task><ac:task-id>1</ac:task-id><ac:task-uuid>u1</ac:task-uuid><ac:task-status>incomplete</ac:task-status><ac:task-body>write tests</ac:task-body></ac:task><ac:task><ac:task-id>2</ac:task-id><ac:task-status>complete</ac:task-status><ac:task-body>ship *it*</ac:task-body></ac:task></ac:task-list>"#;
        let m = assert_round_trip(s);
        assert_eq!(m, "- [ ] write tests\n- [x] ship \\*it\\*");
        assert!(markdown::to_storage(&m).contains("<ac:task-list>"));
    }

    #[test]
    fn what_markdown_cannot_say_is_flagged() {
        for s in [
            // Two lists in a row merge into one in markdown.
            "<ul><li>a</li></ul><ul><li>b</li></ul>",
            // A table without a header row gains an empty one.
            "<table><tbody><tr><td><p>a</p></td></tr></tbody></table>",
            // Colours, underline, sub/superscript, smart links, code parameters.
            r#"<p><span style="color: red;">red</span></p>"#,
            "<p><u>under</u> H<sub>2</sub>O</p>",
            r#"<p><a href="https://x.dev" data-card-appearance="inline">https://x.dev</a></p>"#,
            r#"<ac:structured-macro ac:name="code"><ac:parameter ac:name="title">T</ac:parameter><ac:plain-text-body><![CDATA[x]]></ac:plain-text-body></ac:structured-macro>"#,
            "<p>broken <b>",
        ] {
            assert!(md(s).1, "{s} should be flagged");
        }
    }
}
