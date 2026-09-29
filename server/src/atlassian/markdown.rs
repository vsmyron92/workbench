//! Markdown in, Atlassian formats out: Confluence storage XHTML (comments, pages
//! written by agents) and Atlassian Document Format (Jira descriptions and comments).
//! Both serializers walk one small tree built from pulldown-cmark events. Also ADF →
//! markdown, so Jira text can be shown to agents and edited as markdown in the UI.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::{Map, Value, json};

use super::html::escape;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Para(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Code { lang: Option<String>, text: String },
    Quote(Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<Item> },
    Rule,
    Table { aligns: Vec<Alignment>, head: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    /// A raw HTML block in the markdown source.
    Html(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Em(Vec<Inline>),
    Strike(Vec<Inline>),
    Sup(Vec<Inline>),
    Sub(Vec<Inline>),
    Link { href: String, children: Vec<Inline> },
    Image { src: String, alt: String },
    HardBreak,
    SoftBreak,
}

struct TreeBuilder<'a, I: Iterator<Item = Event<'a>>> {
    it: I,
    pending_task: Option<bool>,
}

impl<'a, I: Iterator<Item = Event<'a>>> TreeBuilder<'a, I> {
    fn blocks_until(&mut self, end: Option<TagEnd>) -> Vec<Block> {
        let mut out = vec![];
        let mut loose: Vec<Inline> = vec![];
        let flush = |loose: &mut Vec<Inline>, out: &mut Vec<Block>| {
            if !loose.is_empty() {
                out.push(Block::Para(std::mem::take(loose)));
            }
        };
        while let Some(ev) = self.it.next() {
            match ev {
                Event::End(e) if Some(e) == end => break,
                Event::Start(tag) => match tag {
                    Tag::Paragraph => {
                        flush(&mut loose, &mut out);
                        out.push(Block::Para(self.inlines_until(TagEnd::Paragraph)));
                    }
                    Tag::Heading { level, .. } => {
                        flush(&mut loose, &mut out);
                        out.push(Block::Heading(level as u8, self.inlines_until(TagEnd::Heading(level))));
                    }
                    Tag::BlockQuote(kind) => {
                        flush(&mut loose, &mut out);
                        out.push(Block::Quote(self.blocks_until(Some(TagEnd::BlockQuote(kind)))));
                    }
                    Tag::CodeBlock(kind) => {
                        flush(&mut loose, &mut out);
                        let lang = match kind {
                            CodeBlockKind::Fenced(info) => info.split_whitespace().next().map(str::to_string),
                            CodeBlockKind::Indented => None,
                        };
                        let mut text = String::new();
                        for ev in self.it.by_ref() {
                            match ev {
                                Event::Text(t) => text.push_str(&t),
                                Event::End(TagEnd::CodeBlock) => break,
                                _ => {}
                            }
                        }
                        if text.ends_with('\n') {
                            text.pop();
                        }
                        out.push(Block::Code { lang: lang.filter(|l| !l.is_empty()), text });
                    }
                    Tag::HtmlBlock => {
                        flush(&mut loose, &mut out);
                        let mut raw = String::new();
                        for ev in self.it.by_ref() {
                            match ev {
                                Event::Html(t) | Event::Text(t) => raw.push_str(&t),
                                Event::End(TagEnd::HtmlBlock) => break,
                                _ => {}
                            }
                        }
                        out.push(Block::Html(raw.trim_end().to_string()));
                    }
                    Tag::List(start) => {
                        flush(&mut loose, &mut out);
                        let mut items = vec![];
                        while let Some(ev) = self.it.next() {
                            match ev {
                                Event::Start(Tag::Item) => {
                                    self.pending_task = None;
                                    let mut blocks = self.blocks_until(Some(TagEnd::Item));
                                    let task = self.pending_task.take();
                                    if blocks.is_empty() {
                                        blocks.push(Block::Para(vec![]));
                                    }
                                    items.push(Item { task, blocks });
                                }
                                Event::End(TagEnd::List(_)) => break,
                                _ => {}
                            }
                        }
                        out.push(Block::List { ordered: start.is_some(), start: start.unwrap_or(1), items });
                    }
                    Tag::Table(aligns) => {
                        flush(&mut loose, &mut out);
                        out.push(self.table(aligns));
                    }
                    Tag::FootnoteDefinition(_)
                    | Tag::DefinitionList
                    | Tag::DefinitionListTitle
                    | Tag::DefinitionListDefinition
                    | Tag::MetadataBlock(_) => {
                        flush(&mut loose, &mut out);
                        let end = tag.to_end();
                        out.extend(self.blocks_until(Some(end)));
                    }
                    Tag::Item | Tag::TableHead | Tag::TableRow | Tag::TableCell => {}
                    inline => loose.extend(self.inline_tag(inline)),
                },
                Event::Rule => {
                    flush(&mut loose, &mut out);
                    out.push(Block::Rule);
                }
                Event::TaskListMarker(done) => self.pending_task = Some(done),
                Event::End(_) => {}
                other => {
                    if let Some(i) = self.leaf(other) {
                        loose.push(i);
                    }
                }
            }
        }
        flush(&mut loose, &mut out);
        out
    }

    fn leaf(&mut self, ev: Event<'a>) -> Option<Inline> {
        Some(match ev {
            Event::Text(t) => Inline::Text(t.into_string()),
            Event::Code(c) | Event::InlineMath(c) | Event::DisplayMath(c) => Inline::Code(c.into_string()),
            Event::SoftBreak => Inline::SoftBreak,
            Event::HardBreak => Inline::HardBreak,
            // Inline HTML is never trusted as markup: it is kept as literal text.
            Event::Html(h) | Event::InlineHtml(h) => Inline::Text(h.into_string()),
            Event::FootnoteReference(r) => Inline::Text(format!("[^{r}]")),
            Event::TaskListMarker(done) => {
                self.pending_task = Some(done);
                return None;
            }
            _ => return None,
        })
    }

    /// An inline container; unknown containers contribute their children.
    fn inline_tag(&mut self, tag: Tag<'a>) -> Vec<Inline> {
        vec![match tag {
            Tag::Emphasis => Inline::Em(self.inlines_until(TagEnd::Emphasis)),
            Tag::Strong => Inline::Strong(self.inlines_until(TagEnd::Strong)),
            Tag::Strikethrough => Inline::Strike(self.inlines_until(TagEnd::Strikethrough)),
            Tag::Superscript => Inline::Sup(self.inlines_until(TagEnd::Superscript)),
            Tag::Subscript => Inline::Sub(self.inlines_until(TagEnd::Subscript)),
            Tag::Link { dest_url, .. } => {
                Inline::Link { href: dest_url.into_string(), children: self.inlines_until(TagEnd::Link) }
            }
            Tag::Image { dest_url, .. } => {
                let alt = plain(&self.inlines_until(TagEnd::Image));
                Inline::Image { src: dest_url.into_string(), alt }
            }
            other => {
                let end = other.to_end();
                return self.inlines_until(end);
            }
        }]
    }

    fn inlines_until(&mut self, end: TagEnd) -> Vec<Inline> {
        let mut out = vec![];
        while let Some(ev) = self.it.next() {
            match ev {
                Event::End(e) if e == end => break,
                Event::Start(tag) => out.extend(self.inline_tag(tag)),
                Event::End(_) => {}
                other => {
                    if let Some(i) = self.leaf(other) {
                        out.push(i);
                    }
                }
            }
        }
        out
    }

    fn cells(&mut self, end: TagEnd) -> Vec<Vec<Inline>> {
        let mut cells = vec![];
        while let Some(ev) = self.it.next() {
            match ev {
                Event::Start(Tag::TableCell) => cells.push(self.inlines_until(TagEnd::TableCell)),
                Event::End(e) if e == end => break,
                _ => {}
            }
        }
        cells
    }

    fn table(&mut self, aligns: Vec<Alignment>) -> Block {
        let mut head = vec![];
        let mut rows = vec![];
        while let Some(ev) = self.it.next() {
            match ev {
                Event::Start(Tag::TableHead) => head = self.cells(TagEnd::TableHead),
                Event::Start(Tag::TableRow) => rows.push(self.cells(TagEnd::TableRow)),
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        Block::Table { aligns, head, rows }
    }
}

/// Parse markdown (GFM tables, strikethrough, task lists).
pub fn parse(md: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = TreeBuilder { it: Parser::new_ext(md, opts), pending_task: None };
    b.blocks_until(None)
}

/// Plain text of inline content.
pub fn plain(inl: &[Inline]) -> String {
    let mut s = String::new();
    for i in inl {
        match i {
            Inline::Text(t) | Inline::Code(t) => s.push_str(t),
            Inline::Strong(c) | Inline::Em(c) | Inline::Strike(c) | Inline::Sup(c) | Inline::Sub(c) => s.push_str(&plain(c)),
            Inline::Link { children, .. } => s.push_str(&plain(children)),
            Inline::Image { alt, .. } => s.push_str(alt),
            Inline::HardBreak => s.push('\n'),
            Inline::SoftBreak => s.push(' '),
        }
    }
    s
}

// ---------------------------------------------------------------- storage

/// Language names the Confluence code macro understands, from common markdown aliases.
fn code_language(lang: &str) -> String {
    let l = lang.to_ascii_lowercase();
    match l.as_str() {
        "js" | "jsx" | "mjs" => "javascript".into(),
        "ts" | "tsx" => "typescript".into(),
        "sh" | "shell" | "zsh" | "console" => "bash".into(),
        "py" => "python".into(),
        "yml" => "yaml".into(),
        "rs" => "rust".into(),
        "c++" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h++" => "cpp".into(),
        "h" => "c".into(),
        "v" | "sv" | "svh" | "systemverilog" => "verilog".into(),
        "vhd" => "vhdl".into(),
        "cs" | "c#" => "csharp".into(),
        "kt" => "kotlin".into(),
        "md" => "markdown".into(),
        _ => l,
    }
}

fn cdata(text: &str) -> String {
    format!("<![CDATA[{}]]>", text.replace("]]>", "]]]]><![CDATA[>"))
}

/// Markdown → Confluence storage XHTML. Fenced code becomes the `code` macro, task
/// lists become `ac:task-list`, and well-formed raw HTML blocks pass through (so an
/// agent can include macros); anything else is escaped.
pub fn to_storage(md: &str) -> String {
    let mut out = String::new();
    for b in parse(md) {
        storage_block(&b, &mut out);
    }
    out
}

fn storage_blocks(bs: &[Block], out: &mut String) {
    for b in bs {
        storage_block(b, out);
    }
}

fn align_style(a: Option<&Alignment>) -> &'static str {
    match a {
        Some(Alignment::Center) => " style=\"text-align: center;\"",
        Some(Alignment::Right) => " style=\"text-align: right;\"",
        _ => "",
    }
}

fn storage_block(b: &Block, out: &mut String) {
    match b {
        Block::Para(inl) => {
            out.push_str("<p>");
            storage_inlines(inl, out);
            out.push_str("</p>");
        }
        Block::Heading(l, inl) => {
            out.push_str(&format!("<h{l}>"));
            storage_inlines(inl, out);
            out.push_str(&format!("</h{l}>"));
        }
        Block::Code { lang, text } => {
            out.push_str(r#"<ac:structured-macro ac:name="code" ac:schema-version="1">"#);
            if let Some(l) = lang {
                out.push_str(&format!(r#"<ac:parameter ac:name="language">{}</ac:parameter>"#, escape(&code_language(l))));
            }
            out.push_str(&format!("<ac:plain-text-body>{}</ac:plain-text-body></ac:structured-macro>", cdata(text)));
        }
        Block::Quote(bs) => {
            out.push_str("<blockquote>");
            storage_blocks(bs, out);
            out.push_str("</blockquote>");
        }
        Block::List { ordered, start, items } => {
            if !items.is_empty() && items.iter().all(|i| i.task.is_some()) {
                out.push_str("<ac:task-list>");
                for i in items {
                    let status = if i.task == Some(true) { "complete" } else { "incomplete" };
                    out.push_str(&format!("<ac:task><ac:task-status>{status}</ac:task-status><ac:task-body>"));
                    let mut rest: &[Block] = &i.blocks;
                    if let Some(Block::Para(inl)) = i.blocks.first() {
                        storage_inlines(inl, out);
                        rest = &i.blocks[1..];
                    }
                    storage_blocks(rest, out);
                    out.push_str("</ac:task-body></ac:task>");
                }
                out.push_str("</ac:task-list>");
                return;
            }
            let tag = if *ordered { "ol" } else { "ul" };
            if *ordered && *start != 1 {
                out.push_str(&format!("<ol start=\"{start}\">"));
            } else {
                out.push_str(&format!("<{tag}>"));
            }
            for i in items {
                out.push_str("<li>");
                if let Some(done) = i.task {
                    // A task inside an ordinary list: keep the checkbox as text.
                    out.push_str(if done { "[x] " } else { "[ ] " });
                }
                storage_blocks(&i.blocks, out);
                out.push_str("</li>");
            }
            out.push_str(&format!("</{tag}>"));
        }
        Block::Rule => out.push_str("<hr />"),
        Block::Table { aligns, head, rows } => {
            out.push_str("<table><tbody>");
            if !head.is_empty() {
                out.push_str("<tr>");
                for (i, c) in head.iter().enumerate() {
                    out.push_str(&format!("<th{}><p>", align_style(aligns.get(i))));
                    storage_inlines(c, out);
                    out.push_str("</p></th>");
                }
                out.push_str("</tr>");
            }
            for r in rows {
                out.push_str("<tr>");
                for (i, c) in r.iter().enumerate() {
                    out.push_str(&format!("<td{}><p>", align_style(aligns.get(i))));
                    storage_inlines(c, out);
                    out.push_str("</p></td>");
                }
                out.push_str("</tr>");
            }
            out.push_str("</tbody></table>");
        }
        Block::Html(raw) => {
            if super::xml::parse(raw).is_ok() {
                out.push_str(raw);
            } else {
                out.push_str("<p>");
                out.push_str(&escape(raw));
                out.push_str("</p>");
            }
        }
    }
}

/// `mention:<accountId>` links are user mentions (`[@Ann](mention:557058:…)`).
pub fn mention_id(href: &str) -> Option<&str> {
    let id = href.strip_prefix("mention:")?;
    (!id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_')))
        .then_some(id)
}

fn is_http(url: &str) -> bool {
    let l = url.to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://")
}

fn storage_inlines(inl: &[Inline], out: &mut String) {
    for i in inl {
        match i {
            Inline::Text(t) => out.push_str(&escape(t)),
            Inline::Code(c) => out.push_str(&format!("<code>{}</code>", escape(c))),
            Inline::Strong(c) => wrap_storage("strong", c, out),
            Inline::Em(c) => wrap_storage("em", c, out),
            Inline::Strike(c) => wrap_storage("del", c, out),
            Inline::Sup(c) => wrap_storage("sup", c, out),
            Inline::Sub(c) => wrap_storage("sub", c, out),
            Inline::Link { href, .. } if mention_id(href).is_some() => {
                let id = mention_id(href).unwrap_or_default();
                out.push_str(&format!("<ac:link><ri:user ri:account-id=\"{}\" /></ac:link>", escape(id)));
            }
            Inline::Link { href, children } => {
                out.push_str(&format!("<a href=\"{}\">", escape(href)));
                if children.is_empty() {
                    out.push_str(&escape(href));
                } else {
                    storage_inlines(children, out);
                }
                out.push_str("</a>");
            }
            Inline::Image { src, alt } => {
                if is_http(src) {
                    out.push_str(&format!("<ac:image ac:alt=\"{}\"><ri:url ri:value=\"{}\" /></ac:image>", escape(alt), escape(src)));
                } else {
                    out.push_str(&escape(&format!("![{alt}]({src})")));
                }
            }
            Inline::HardBreak => out.push_str("<br />"),
            Inline::SoftBreak => out.push(' '),
        }
    }
}

fn wrap_storage(tag: &str, children: &[Inline], out: &mut String) {
    out.push_str(&format!("<{tag}>"));
    storage_inlines(children, out);
    out.push_str(&format!("</{tag}>"));
}

// ---------------------------------------------------------------- ADF

/// Markdown → Atlassian Document Format (`{"version":1,"type":"doc",…}`).
pub fn to_adf(md: &str) -> Value {
    let content: Vec<Value> = parse(md).iter().flat_map(|b| adf_block(b, false)).collect();
    json!({ "version": 1, "type": "doc", "content": content })
}

fn node(kind: &str, attrs: Option<Value>, content: Vec<Value>) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), Value::String(kind.into()));
    if let Some(a) = attrs {
        m.insert("attrs".into(), a);
    }
    if !content.is_empty() {
        m.insert("content".into(), Value::Array(content));
    }
    Value::Object(m)
}

fn local_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `restricted`: inside blockquote/list items, where headings are not allowed.
fn adf_block(b: &Block, restricted: bool) -> Vec<Value> {
    match b {
        Block::Para(inl) => vec![node("paragraph", None, adf_inlines(inl))],
        Block::Heading(l, inl) if !restricted => vec![node("heading", Some(json!({ "level": l })), adf_inlines(inl))],
        Block::Heading(_, inl) => vec![node("paragraph", None, adf_inlines(&[Inline::Strong(inl.clone())]))],
        Block::Code { lang, text } => {
            let attrs = lang.as_ref().map(|l| json!({ "language": code_language(l) }));
            let content = if text.is_empty() { vec![] } else { vec![json!({ "type": "text", "text": text })] };
            vec![node("codeBlock", attrs, content)]
        }
        Block::Quote(bs) => {
            let inner: Vec<Value> = bs.iter().flat_map(|b| adf_block(b, true)).collect();
            vec![node("blockquote", None, inner)]
        }
        Block::List { ordered, start, items } => {
            if !items.is_empty() && items.iter().all(|i| i.task.is_some()) {
                let tasks = items
                    .iter()
                    .map(|i| {
                        let state = if i.task == Some(true) { "DONE" } else { "TODO" };
                        let inl: Vec<Inline> = i
                            .blocks
                            .iter()
                            .flat_map(|b| match b {
                                Block::Para(x) | Block::Heading(_, x) => x.clone(),
                                other => vec![Inline::Text(block_plain(other))],
                            })
                            .collect();
                        node("taskItem", Some(json!({ "localId": local_id(), "state": state })), adf_inlines(&inl))
                    })
                    .collect();
                return vec![node("taskList", Some(json!({ "localId": local_id() })), tasks)];
            }
            let list_items = items
                .iter()
                .map(|i| {
                    let mut content: Vec<Value> = i.blocks.iter().flat_map(|b| adf_block(b, true)).collect();
                    // A list item must start with a paragraph.
                    if content.first().and_then(|c| c.get("type")).and_then(Value::as_str) != Some("paragraph") {
                        content.insert(0, node("paragraph", None, vec![]));
                    }
                    if let Some(done) = i.task {
                        if let Some(first) = content.first_mut() {
                            let prefix = json!({ "type": "text", "text": if done { "[x] " } else { "[ ] " } });
                            match first.get_mut("content").and_then(Value::as_array_mut) {
                                Some(arr) => arr.insert(0, prefix),
                                None => first["content"] = json!([prefix]),
                            }
                        }
                    }
                    node("listItem", None, content)
                })
                .collect();
            if *ordered {
                vec![node("orderedList", Some(json!({ "order": start })), list_items)]
            } else {
                vec![node("bulletList", None, list_items)]
            }
        }
        Block::Rule if !restricted => vec![node("rule", None, vec![])],
        Block::Rule => vec![],
        Block::Table { head, rows, .. } if !restricted => {
            let cell = |kind: &str, inl: &Vec<Inline>| node(kind, Some(json!({})), vec![node("paragraph", None, adf_inlines(inl))]);
            let mut trs = vec![];
            if !head.is_empty() {
                trs.push(node("tableRow", None, head.iter().map(|c| cell("tableHeader", c)).collect()));
            }
            for r in rows {
                trs.push(node("tableRow", None, r.iter().map(|c| cell("tableCell", c)).collect()));
            }
            vec![node("table", Some(json!({ "isNumberColumnEnabled": false, "layout": "default" })), trs)]
        }
        Block::Table { .. } => vec![node("paragraph", None, adf_inlines(&[Inline::Text(block_plain(b))]))],
        Block::Html(raw) => vec![node("paragraph", None, adf_inlines(&[Inline::Text(raw.clone())]))],
    }
}

fn block_plain(b: &Block) -> String {
    match b {
        Block::Para(i) | Block::Heading(_, i) => plain(i),
        Block::Code { text, .. } => text.clone(),
        Block::Quote(bs) => bs.iter().map(block_plain).collect::<Vec<_>>().join("\n"),
        Block::List { items, .. } => items
            .iter()
            .map(|i| i.blocks.iter().map(block_plain).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Rule => String::new(),
        Block::Table { head, rows, .. } => std::iter::once(head)
            .chain(rows.iter())
            .map(|r| r.iter().map(|c| plain(c)).collect::<Vec<_>>().join(" | "))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Html(h) => h.clone(),
    }
}

fn adf_inlines(inl: &[Inline]) -> Vec<Value> {
    let mut out: Vec<Value> = vec![];
    flatten_adf(inl, &[], &mut out);
    // Merge adjacent text nodes with identical marks.
    let mut merged: Vec<Value> = vec![];
    for v in out {
        if let (Some(last), Some("text")) = (merged.last_mut(), v.get("type").and_then(Value::as_str)) {
            if last.get("type").and_then(Value::as_str) == Some("text") && last.get("marks") == v.get("marks") {
                let t = format!(
                    "{}{}",
                    last.get("text").and_then(Value::as_str).unwrap_or(""),
                    v.get("text").and_then(Value::as_str).unwrap_or("")
                );
                last["text"] = Value::String(t);
                continue;
            }
        }
        merged.push(v);
    }
    merged
}

fn text_node(t: &str, marks: &[Value]) -> Option<Value> {
    if t.is_empty() {
        return None;
    }
    let mut v = json!({ "type": "text", "text": t });
    if !marks.is_empty() {
        v["marks"] = Value::Array(marks.to_vec());
    }
    Some(v)
}

fn flatten_adf(inl: &[Inline], marks: &[Value], out: &mut Vec<Value>) {
    let with = |m: Value| {
        let mut v = marks.to_vec();
        if !v.contains(&m) {
            v.push(m);
        }
        v
    };
    for i in inl {
        match i {
            Inline::Text(t) => out.extend(text_node(t, marks)),
            Inline::Code(c) => {
                // ADF allows `code` only together with `link`.
                let mut m: Vec<Value> = marks.iter().filter(|m| m["type"] == "link").cloned().collect();
                m.push(json!({ "type": "code" }));
                out.extend(text_node(c, &m));
            }
            Inline::Strong(c) => flatten_adf(c, &with(json!({ "type": "strong" })), out),
            Inline::Em(c) => flatten_adf(c, &with(json!({ "type": "em" })), out),
            Inline::Strike(c) => flatten_adf(c, &with(json!({ "type": "strike" })), out),
            Inline::Sup(c) => flatten_adf(c, &with(json!({ "type": "subsup", "attrs": { "type": "sup" } })), out),
            Inline::Sub(c) => flatten_adf(c, &with(json!({ "type": "subsup", "attrs": { "type": "sub" } })), out),
            Inline::Link { href, children } => {
                let m = with(json!({ "type": "link", "attrs": { "href": href } }));
                if children.is_empty() {
                    out.extend(text_node(href, &m));
                } else {
                    flatten_adf(children, &m, out);
                }
            }
            Inline::Image { src, alt } => {
                let label = if alt.is_empty() { src.as_str() } else { alt.as_str() };
                out.extend(text_node(label, &with(json!({ "type": "link", "attrs": { "href": src } }))));
            }
            Inline::HardBreak => out.push(json!({ "type": "hardBreak" })),
            Inline::SoftBreak => out.extend(text_node(" ", marks)),
        }
    }
}

// ---------------------------------------------------------------- ADF → markdown

/// Node types `adf_to_markdown` renders faithfully enough to edit and save back.
const EDITABLE_NODES: &[&str] = &[
    "doc", "paragraph", "heading", "text", "bulletList", "orderedList", "listItem", "codeBlock", "blockquote", "rule",
    "hardBreak", "table", "tableRow", "tableHeader", "tableCell", "taskList", "taskItem",
];
const EDITABLE_MARKS: &[&str] = &["strong", "em", "code", "strike", "link", "subsup"];

/// True when ADF contains content that a markdown round trip would lose (media,
/// mentions, panels, colours…), so the UI can warn before saving an edit.
pub fn adf_is_lossy(v: &Value) -> bool {
    match v {
        Value::Object(m) => {
            if let Some(t) = m.get("type").and_then(Value::as_str) {
                if !EDITABLE_NODES.contains(&t) {
                    return true;
                }
            }
            if let Some(marks) = m.get("marks").and_then(Value::as_array) {
                if marks.iter().any(|mk| !EDITABLE_MARKS.contains(&mk.get("type").and_then(Value::as_str).unwrap_or(""))) {
                    return true;
                }
            }
            m.get("content").and_then(Value::as_array).is_some_and(|c| c.iter().any(adf_is_lossy))
        }
        _ => false,
    }
}

/// Render ADF as markdown (for agents, and as the starting point of a markdown edit).
pub fn adf_to_markdown(v: &Value) -> String {
    let out = adf_blocks(v.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
    out.trim().to_string()
}

fn children(v: &Value) -> &[Value] {
    v.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

fn attr<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get("attrs").and_then(|a| a.get(k))
}

fn adf_blocks(nodes: &[Value]) -> String {
    nodes.iter().map(adf_block_md).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n")
}

fn prefix_lines(s: &str, first: &str, rest: &str) -> String {
    s.lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { format!("{first}{l}") } else if l.is_empty() { String::new() } else { format!("{rest}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn adf_block_md(n: &Value) -> String {
    let kind = n.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "paragraph" => adf_inline_md(children(n)),
        "heading" => {
            let level = attr(n, "level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), adf_inline_md(children(n)))
        }
        "bulletList" | "orderedList" => {
            let mut num = attr(n, "order").and_then(Value::as_u64).unwrap_or(1);
            children(n)
                .iter()
                .map(|li| {
                    let marker = if kind == "orderedList" {
                        let m = format!("{num}. ");
                        num += 1;
                        m
                    } else {
                        "- ".to_string()
                    };
                    let pad = " ".repeat(marker.len());
                    let body = children(li).iter().map(adf_block_md).collect::<Vec<_>>().join("\n");
                    prefix_lines(&body, &marker, &pad)
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        "taskList" => children(n)
            .iter()
            .map(|t| {
                if t.get("type").and_then(Value::as_str) == Some("taskList") {
                    return prefix_lines(&adf_block_md(t), "  ", "  ");
                }
                let done = attr(t, "state").and_then(Value::as_str) == Some("DONE");
                format!("- [{}] {}", if done { "x" } else { " " }, adf_inline_md(children(t)))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "decisionList" => children(n).iter().map(|d| format!("- ✓ {}", adf_inline_md(children(d)))).collect::<Vec<_>>().join("\n"),
        "codeBlock" => {
            let lang = attr(n, "language").and_then(Value::as_str).unwrap_or("");
            let text: String = children(n).iter().filter_map(|t| t.get("text").and_then(Value::as_str)).collect();
            format!("```{lang}\n{text}\n```")
        }
        "blockquote" => prefix_lines(&adf_blocks(children(n)), "> ", "> "),
        "panel" => {
            let t = attr(n, "panelType").and_then(Value::as_str).unwrap_or("info");
            let mut label = t.to_string();
            if let Some(f) = label.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            prefix_lines(&format!("**{label}:**\n{}", adf_blocks(children(n))), "> ", "> ")
        }
        "rule" => "---".into(),
        "expand" | "nestedExpand" => {
            let t = attr(n, "title").and_then(Value::as_str).unwrap_or("Details");
            format!("**▸ {t}**\n\n{}", adf_blocks(children(n)))
        }
        "table" => {
            let rows: Vec<(bool, Vec<String>)> = children(n)
                .iter()
                .map(|r| {
                    let cells = children(r);
                    let header = !cells.is_empty() && cells.iter().all(|c| c.get("type").and_then(Value::as_str) == Some("tableHeader"));
                    let texts = cells
                        .iter()
                        .map(|c| adf_blocks(children(c)).replace('\n', " ").replace('|', "\\|"))
                        .collect();
                    (header, texts)
                })
                .collect();
            if rows.is_empty() {
                return String::new();
            }
            let width = rows.iter().map(|r| r.1.len()).max().unwrap_or(1).max(1);
            let line = |c: &[String]| {
                let mut v = c.to_vec();
                v.resize(width, String::new());
                format!("| {} |", v.join(" | "))
            };
            let sep = format!("|{}|", vec!["---"; width].join("|"));
            let mut out = vec![];
            if rows[0].0 {
                out.push(line(&rows[0].1));
                out.push(sep);
                out.extend(rows[1..].iter().map(|r| line(&r.1)));
            } else {
                out.push(line(&vec![String::new(); width]));
                out.push(sep);
                out.extend(rows.iter().map(|r| line(&r.1)));
            }
            out.join("\n")
        }
        "mediaSingle" | "mediaGroup" | "media" => "![attachment]".into(),
        "blockCard" | "embedCard" => attr(n, "url").and_then(Value::as_str).unwrap_or("").to_string(),
        "layoutSection" | "layoutColumn" | "bodiedExtension" => adf_blocks(children(n)),
        _ => {
            let c = children(n);
            if c.is_empty() { String::new() } else { adf_blocks(c) }
        }
    }
}

fn adf_inline_md(nodes: &[Value]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                let mut t = n.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                let marks = n.get("marks").and_then(Value::as_array).cloned().unwrap_or_default();
                let mut link = None;
                for m in &marks {
                    match m.get("type").and_then(Value::as_str).unwrap_or("") {
                        "code" => t = format!("`{t}`"),
                        "strong" => t = format!("**{t}**"),
                        "em" => t = format!("*{t}*"),
                        "strike" => t = format!("~~{t}~~"),
                        "link" => link = attr(m, "href").and_then(Value::as_str).map(str::to_string),
                        _ => {}
                    }
                }
                if let Some(h) = link {
                    t = format!("[{t}]({h})");
                }
                out.push_str(&t);
            }
            "hardBreak" => out.push('\n'),
            "mention" => {
                let t = attr(n, "text").and_then(Value::as_str).unwrap_or("user");
                if !t.starts_with('@') {
                    out.push('@');
                }
                out.push_str(t);
            }
            "emoji" => out.push_str(attr(n, "text").or(attr(n, "shortName")).and_then(Value::as_str).unwrap_or("")),
            "inlineCard" => out.push_str(attr(n, "url").and_then(Value::as_str).unwrap_or("")),
            "status" => out.push_str(&format!("[{}]", attr(n, "text").and_then(Value::as_str).unwrap_or("").to_uppercase())),
            "date" => {
                let ts = attr(n, "timestamp").and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|i| i.to_string())));
                let s = ts
                    .and_then(|t| t.parse::<i64>().ok())
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                out.push_str(&s);
            }
            _ => out.push_str(&adf_inline_md(children(n))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Title\n\nSome **bold** and *em* and `code` and ~~gone~~ with a [link](https://x.dev).\nSoft break.  \nHard break.\n\n- one\n- two\n  1. nested\n\n* [x] done\n* [ ] todo\n\n```rs\nfn main() { if a < b && c {} }\n]]> tricky\n```\n\n> quoted\n\n| a | b |\n|:-:|--:|\n| 1 | <2> |\n\n---\n\n<p>raw <b>ok</b></p>\n\n<div>broken";

    #[test]
    fn code_languages_use_the_macro_names() {
        assert_eq!(code_language("C++"), "cpp");
        assert_eq!(code_language("hxx"), "cpp");
        assert_eq!(code_language("c"), "c");
        assert_eq!(code_language("sv"), "verilog");
        assert_eq!(code_language("SystemVerilog"), "verilog");
        assert_eq!(code_language("vhd"), "vhdl");
        assert_eq!(code_language("VHDL"), "vhdl");
    }

    #[test]
    fn markdown_to_storage() {
        let s = to_storage(MD);
        assert!(s.starts_with("<h1>Title</h1><p>Some <strong>bold</strong> and <em>em</em> and <code>code</code> and <del>gone</del> with a <a href=\"https://x.dev\">link</a>. Soft break.<br />Hard break.</p>"), "{s}");
        assert!(s.contains("<ul><li><p>one</p></li><li><p>two</p><ol><li><p>nested</p></li></ol></li></ul>"), "{s}");
        assert!(s.contains("<ac:task-list><ac:task><ac:task-status>complete</ac:task-status><ac:task-body>done</ac:task-body></ac:task><ac:task><ac:task-status>incomplete</ac:task-status><ac:task-body>todo</ac:task-body></ac:task></ac:task-list>"), "{s}");
        assert!(s.contains(r#"<ac:structured-macro ac:name="code" ac:schema-version="1"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[fn main() { if a < b && c {} }
]]]]><![CDATA[> tricky]]></ac:plain-text-body></ac:structured-macro>"#), "{s}");
        assert!(s.contains("<blockquote><p>quoted</p></blockquote>"));
        assert!(s.contains(r#"<th style="text-align: center;"><p>a</p></th><th style="text-align: right;"><p>b</p></th>"#), "{s}");
        assert!(s.contains("<td style=\"text-align: right;\"><p>&lt;2&gt;</p></td>"), "{s}");
        assert!(s.contains("<hr />"));
        assert!(s.contains("<p>raw <b>ok</b></p>"), "well-formed raw HTML blocks pass through");
        assert!(s.contains("<p>&lt;div&gt;broken</p>"), "malformed raw HTML is escaped: {s}");
        // Whatever markdown produces must be valid storage.
        super::super::storage::validate(&s).unwrap();
    }

    #[test]
    fn markdown_to_adf() {
        let v = to_adf(MD);
        assert_eq!(v["type"], "doc");
        let c = v["content"].as_array().unwrap();
        assert_eq!(c[0], json!({"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"Title"}]}));
        let p = &c[1]["content"];
        assert_eq!(p[1], json!({"type":"text","text":"bold","marks":[{"type":"strong"}]}));
        assert_eq!(p[5], json!({"type":"text","text":"code","marks":[{"type":"code"}]}));
        assert_eq!(p[9], json!({"type":"text","text":"link","marks":[{"type":"link","attrs":{"href":"https://x.dev"}}]}));
        assert!(p.as_array().unwrap().iter().any(|n| n["type"] == "hardBreak"));
        assert_eq!(c[2]["type"], "bulletList");
        assert_eq!(c[2]["content"][1]["content"][1]["type"], "orderedList");
        assert_eq!(c[3]["type"], "taskList");
        assert_eq!(c[3]["content"][0]["attrs"]["state"], "DONE");
        assert_eq!(c[4]["type"], "codeBlock");
        assert_eq!(c[4]["attrs"]["language"], "rust");
        assert!(c[4]["content"][0]["text"].as_str().unwrap().ends_with("]]> tricky"));
        assert_eq!(c[5]["type"], "blockquote");
        assert_eq!(c[6]["type"], "table");
        assert_eq!(c[6]["content"][0]["content"][0]["type"], "tableHeader");
        assert_eq!(c[7]["type"], "rule");
    }

    #[test]
    fn code_marks_only_combine_with_links() {
        let v = to_adf("**`x`** [`y`](https://y.dev)");
        let p = &v["content"][0]["content"];
        assert_eq!(p[0]["marks"], json!([{"type":"code"}]));
        assert_eq!(p[2]["marks"], json!([{"type":"link","attrs":{"href":"https://y.dev"}},{"type":"code"}]));
    }

    #[test]
    fn list_items_start_with_a_paragraph_and_quotes_have_no_headings() {
        let v = to_adf("- ```\n  code\n  ```\n\n> # H");
        assert_eq!(v["content"][0]["content"][0]["content"][0]["type"], "paragraph");
        assert_eq!(v["content"][1]["content"][0]["type"], "paragraph");
    }

    #[test]
    fn adf_round_trips_through_markdown() {
        let md = "## Steps\n\n1. Open **the** app\n2. Click `Run`\n\n- [ ] check\n\n```rust\nfn x() {}\n```\n\n> note\n\n| a | b |\n|---|---|\n| 1 | 2 |";
        let adf = to_adf(md);
        assert!(!adf_is_lossy(&adf));
        let back = adf_to_markdown(&adf);
        assert_eq!(back, md);
    }

    #[test]
    fn detects_lossy_adf() {
        let adf = json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"mention","attrs":{"id":"1","text":"@Ann"}}]}]});
        assert!(adf_is_lossy(&adf));
        assert_eq!(adf_to_markdown(&adf), "@Ann");
        let colored = json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"x","marks":[{"type":"textColor","attrs":{"color":"#f00"}}]}]}]});
        assert!(adf_is_lossy(&colored));
    }

    #[test]
    fn renders_rich_adf_for_agents() {
        let adf = json!({"type":"doc","version":1,"content":[
            {"type":"panel","attrs":{"panelType":"warning"},"content":[{"type":"paragraph","content":[{"type":"text","text":"Careful"}]}]},
            {"type":"paragraph","content":[{"type":"status","attrs":{"text":"In progress"}},{"type":"text","text":" "},{"type":"inlineCard","attrs":{"url":"https://a.b"}}]},
            {"type":"mediaSingle","content":[{"type":"media","attrs":{"id":"x"}}]}
        ]});
        assert_eq!(adf_to_markdown(&adf), "> **Warning:**\n> Careful\n\n[IN PROGRESS] https://a.b\n\n![attachment]");
    }
}
