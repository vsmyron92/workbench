//! A small, strict XML reader for Confluence storage format.
//!
//! Storage format is XHTML plus `ac:`/`ri:` elements, CDATA sections and HTML named
//! entities (`&nbsp;`, `&rsquo;`…), which generic XML parsers reject without a DTD.
//! This reader knows the HTML 4 entity set, reports errors with line and column, and
//! builds a tree for the plain-text conversion and for validating edits before they
//! are sent to Confluence.

use super::entities;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Element { name: String, attrs: Vec<(String, String)>, children: Vec<Node> },
    Text(String),
    /// `<![CDATA[…]]>` content (kept apart from text: code macros use it).
    CData(String),
}

impl Node {
    pub fn attr(&self, key: &str) -> Option<&str> {
        match self {
            Node::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Node::Element { name, .. } => name,
            _ => "",
        }
    }

    pub fn children(&self) -> &[Node] {
        match self {
            Node::Element { children, .. } => children,
            _ => &[],
        }
    }

    /// Concatenated text and CDATA of this subtree.
    pub fn text(&self) -> String {
        let mut out = String::new();
        fn walk(n: &Node, out: &mut String) {
            match n {
                Node::Text(t) | Node::CData(t) => out.push_str(t),
                Node::Element { children, .. } => children.iter().for_each(|c| walk(c, out)),
            }
        }
        walk(self, &mut out);
        out
    }

    /// First child element with this name.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children().iter().find(|c| c.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct XmlError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

/// Nesting deeper than this is refused (stack safety for hostile input).
const MAX_DEPTH: usize = 100;

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ':' | '_' | '-' | '.')
}

impl<'a> Parser<'a> {
    fn err(&self, at: usize, message: impl Into<String>) -> XmlError {
        let before = &self.src[..at.min(self.src.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
        XmlError { message: message.into(), line, column }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    /// Decode entity references in `s` (text or attribute value starting at `at`).
    fn decode(&self, s: &str, at: usize) -> Result<String, XmlError> {
        if !s.contains('&') {
            return Ok(s.to_string());
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        let mut offset = at;
        while let Some(i) = rest.find('&') {
            out.push_str(&rest[..i]);
            let after = &rest[i + 1..];
            let Some(end) = after.find(';') else {
                return Err(self.err(offset + i, "'&' must start an entity like &amp;"));
            };
            let name = &after[..end];
            let ch = if let Some(num) = name.strip_prefix('#') {
                let n = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse::<u32>().ok(),
                };
                n.filter(|&n| n != 0).and_then(char::from_u32)
            } else {
                entities::lookup(name)
            };
            match ch {
                Some(c) => out.push(c),
                None => return Err(self.err(offset + i, format!("unknown entity &{name};"))),
            }
            let consumed = i + 1 + end + 1;
            rest = &rest[consumed..];
            offset += consumed;
        }
        out.push_str(rest);
        Ok(out)
    }

    fn parse_nodes(&mut self, depth: usize, parent: Option<(&str, usize)>) -> Result<Vec<Node>, XmlError> {
        if depth > MAX_DEPTH {
            return Err(self.err(self.pos, "elements are nested too deeply"));
        }
        let mut out = vec![];
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                return match parent {
                    Some((name, at)) => Err(self.err(at, format!("<{name}> is never closed"))),
                    None => Ok(out),
                };
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").ok_or_else(|| self.err(self.pos, "unterminated CDATA section"))?;
                out.push(Node::CData(body[..end].to_string()));
                self.pos += 9 + end + 3;
            } else if let Some(body) = rest.strip_prefix("<!--") {
                let end = body.find("-->").ok_or_else(|| self.err(self.pos, "unterminated comment"))?;
                self.pos += 4 + end + 3;
            } else if rest.starts_with("<?") {
                let end = rest.find("?>").ok_or_else(|| self.err(self.pos, "unterminated processing instruction"))?;
                self.pos += end + 2;
            } else if rest.starts_with("<!") {
                return Err(self.err(self.pos, "DOCTYPE declarations are not allowed in storage format"));
            } else if let Some(body) = rest.strip_prefix("</") {
                let at = self.pos;
                let end = body.find('>').ok_or_else(|| self.err(at, "unterminated end tag"))?;
                let name = body[..end].trim();
                return match parent {
                    Some((open, _)) if open == name => {
                        self.pos += 2 + end + 1;
                        Ok(out)
                    }
                    Some((open, _)) => Err(self.err(at, format!("expected </{open}> but found </{name}>"))),
                    None => Err(self.err(at, format!("unexpected </{name}>"))),
                };
            } else if rest.starts_with('<') {
                out.push(self.parse_element(depth)?);
            } else {
                let end = rest.find('<').unwrap_or(rest.len());
                let raw = &rest[..end];
                let text = self.decode(raw, self.pos)?;
                if raw.contains('>') && raw.contains("]]>") {
                    return Err(self.err(self.pos, "']]>' is not allowed in text"));
                }
                out.push(Node::Text(text));
                self.pos += end;
            }
        }
    }

    fn parse_element(&mut self, depth: usize) -> Result<Node, XmlError> {
        let start = self.pos;
        self.pos += 1;
        let name_len = self.rest().find(|c: char| !is_name_char(c)).unwrap_or(self.rest().len());
        if name_len == 0 {
            return Err(self.err(start, "'<' must start a tag; write &lt; for a literal <"));
        }
        let name = self.rest()[..name_len].to_string();
        self.pos += name_len;
        let mut attrs: Vec<(String, String)> = vec![];
        loop {
            let ws = self.rest().len() - self.rest().trim_start().len();
            self.pos += ws;
            let rest = self.rest();
            if rest.starts_with("/>") {
                self.pos += 2;
                return Ok(Node::Element { name, attrs, children: vec![] });
            }
            if rest.starts_with('>') {
                self.pos += 1;
                let children = self.parse_nodes(depth + 1, Some((&name, start)))?;
                return Ok(Node::Element { name, attrs, children });
            }
            if rest.is_empty() {
                return Err(self.err(start, format!("<{name}> tag is not closed")));
            }
            if ws == 0 {
                return Err(self.err(self.pos, "expected whitespace between attributes"));
            }
            let at = self.pos;
            let key_len = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
            if key_len == 0 {
                return Err(self.err(at, format!("malformed attribute in <{name}>")));
            }
            let key = rest[..key_len].to_string();
            self.pos += key_len;
            let after = self.rest().trim_start();
            let Some(after_eq) = after.strip_prefix('=') else {
                return Err(self.err(at, format!("attribute {key} needs a quoted value")));
            };
            let value_part = after_eq.trim_start();
            let quote = value_part.chars().next().filter(|c| *c == '"' || *c == '\'');
            let Some(q) = quote else {
                return Err(self.err(at, format!("attribute {key} needs a quoted value")));
            };
            let body = &value_part[1..];
            let end = body.find(q).ok_or_else(|| self.err(at, format!("unterminated value of {key}")))?;
            let raw = &body[..end];
            if raw.contains('<') {
                return Err(self.err(at, format!("'<' is not allowed in the value of {key}")));
            }
            let value_at = self.src.len() - body.len();
            let value = self.decode(raw, value_at)?;
            if attrs.iter().any(|(k, _)| *k == key) {
                return Err(self.err(at, format!("duplicate attribute {key}")));
            }
            attrs.push((key, value));
            self.pos = value_at + end + 1;
        }
    }
}

/// Parse a storage-format fragment (any number of top-level nodes).
pub fn parse(src: &str) -> Result<Vec<Node>, XmlError> {
    let mut p = Parser { src, pos: 0 };
    p.parse_nodes(0, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_storage_with_macros_cdata_and_entities() {
        let src = r#"<h1>A &amp; B&nbsp;&rsquo;</h1><ac:structured-macro ac:name="code"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[if a < b && c {}]]></ac:plain-text-body></ac:structured-macro><p />"#;
        let nodes = parse(src).unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].text(), "A & B\u{a0}\u{2019}");
        assert_eq!(nodes[1].attr("ac:name"), Some("code"));
        assert_eq!(nodes[1].child("ac:plain-text-body").unwrap().text(), "if a < b && c {}");
        assert_eq!(nodes[2], Node::Element { name: "p".into(), attrs: vec![], children: vec![] });
    }

    #[test]
    fn reports_errors_with_positions() {
        let e = parse("<p>one</p>\n<p>two <b>bold</p>").unwrap_err();
        assert_eq!((e.line, e.message.as_str()), (2, "expected </b> but found </p>"));
        assert!(parse("<p>a < b</p>").unwrap_err().message.contains("&lt;"));
        assert!(parse("<p>&bogus;</p>").unwrap_err().message.contains("unknown entity"));
        assert!(parse("<p>AT&T</p>").is_err());
        assert!(parse("<p class=x>").is_err());
        assert!(parse("<p>").unwrap_err().message.contains("never closed"));
        assert!(parse("</p>").is_err());
        assert!(parse(r#"<p a="1" a="2"/>"#).is_err());
        assert!(parse("<!DOCTYPE x><p/>").is_err());
    }

    #[test]
    fn refuses_hostile_nesting() {
        let deep = "<b>".repeat(150) + &"</b>".repeat(150);
        assert!(parse(&deep).unwrap_err().message.contains("too deeply"));
    }

    #[test]
    fn parses_the_template_storage_sample() {
        let nodes = parse(include_str!("testdata/design_storage.xml")).unwrap();
        assert_eq!(nodes[0].name(), "ac:layout");
    }
}
