//! Rendering Confluence `view` HTML and Jira `renderedFields` HTML for the browser.
//!
//! Two passes:
//! 1. `rewrite_tags` edits `<img>` and `<a>` start tags of the raw upstream HTML:
//!    attachment images go through Workbench's proxy (the browser has no Atlassian
//!    session and API tokens cannot load `/wiki/download/…`), internal page and issue
//!    links get `data-wb-page` / `data-wb-issue` so the UI opens them in panels, and
//!    other relative links become absolute links to the site.
//! 2. `ammonia` sanitizes the result with an allow-list that keeps Confluence's layout
//!    structure (tables, code panels, info panels, status lozenges, layouts) and drops
//!    scripts, styles, event handlers, forms and unknown URLs.
//!
//! The first pass only needs to be right for well-formed input: whatever it produces
//! is sanitized afterwards.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::client::Product;
use super::entities;

/// Context for rewriting links in one document.
pub struct Rewrite<'a> {
    /// `https://x.atlassian.net`
    pub site: &'a str,
    pub product: Product,
    /// Project whose credentials fetched the document; proxy URLs carry it so they
    /// use the same site and token.
    pub project_id: Option<String>,
}

/// `path?k=v&…` (values percent-encoded; empty list → `path`).
fn with_query(path: String, params: &[(&str, &str)]) -> String {
    let mut out = path;
    for (i, (k, v)) in params.iter().enumerate() {
        out.push(if i == 0 { '?' } else { '&' });
        out.push_str(k);
        out.push('=');
        out.push_str(&urlencoding::encode(v));
    }
    out
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<(img|a)(\s(?:[^>"']|"[^"]*"|'[^']*')*)?>"#).unwrap());
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([^\s=/>"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"'`=]+)))?"#).unwrap()
});
static PAGE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/wiki/spaces/[^/]+/pages/(?:edit-v2/)?(\d+)(?:[/?#]|$)").unwrap());
static PAGE_ID_PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/wiki/pages/viewpage\.action\?(?:.*&)?pageId=(\d+)").unwrap());
static ATTACHMENT_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/wiki/download/(?:attachments|thumbnails)/(\d+)/([^?#]+)").unwrap());
static JIRA_ATTACHMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/(?:secure/attachment|rest/api/[23]/attachment/content|secure/thumbnail|rest/api/[23]/attachment/thumbnail)/(\d+)")
        .unwrap()
});
static JIRA_BROWSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/browse/([A-Z][A-Z0-9_]+-\d+)(?:[/?#]|$)").unwrap());

/// Decode the entities in an attribute value or text run (`&amp;`, `&#39;`, `&rsquo;`…).
pub fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map(|e| e + 1);
        match end {
            Some(e) if rest[e..].starts_with(';') && e > 1 => {
                let name = &rest[1..e];
                let ch = if let Some(num) = name.strip_prefix('#') {
                    let n = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => num.parse::<u32>().ok(),
                    };
                    n.and_then(char::from_u32)
                } else {
                    entities::lookup(name)
                };
                match ch {
                    Some(c) => {
                        out.push(c);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Escape text for an HTML attribute value (double-quoted) or element content.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-decode then re-encode a path segment (normalizes `my%20pic.png` / `my pic.png`).
fn reencode(seg: &str) -> String {
    let name = urlencoding::decode(seg).map(|n| n.into_owned()).unwrap_or_else(|_| seg.to_string());
    urlencoding::encode(&name).into_owned()
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    ATTR.captures_iter(s)
        .map(|c| {
            let name = c[1].to_ascii_lowercase();
            let raw = c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)).map(|m| m.as_str()).unwrap_or("");
            (name, decode_entities(raw).into_owned())
        })
        .collect()
}

fn render_tag(name: &str, attrs: &[(String, String)]) -> String {
    let mut out = format!("<{name}");
    for (k, v) in attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&escape(v));
        out.push('"');
    }
    out.push('>');
    out
}

fn get<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn set(attrs: &mut Vec<(String, String)>, name: &str, value: String) {
    match attrs.iter_mut().find(|(k, _)| k == name) {
        Some(slot) => slot.1 = value,
        None => attrs.push((name.to_string(), value)),
    }
}

fn remove(attrs: &mut Vec<(String, String)>, names: &[&str]) {
    attrs.retain(|(k, _)| !names.contains(&k.as_str()));
}

impl Rewrite<'_> {
    /// The path part of `url` if it points at this site (absolute) or is root-relative.
    fn site_path<'u>(&self, url: &'u str) -> Option<&'u str> {
        if url.starts_with('/') && !url.starts_with("//") {
            return Some(url);
        }
        let rest = url.strip_prefix(self.site)?;
        (rest.is_empty() || rest.starts_with('/')).then_some(if rest.is_empty() { "/" } else { rest })
    }

    fn push_project<'s>(&'s self, params: &mut Vec<(&'s str, &'s str)>) {
        if let Some(p) = &self.project_id {
            params.push(("projectId", p.as_str()));
        }
    }

    fn all_digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }

    fn rewrite_img(&self, attrs: &mut Vec<(String, String)>) {
        let src = get(attrs, "src").unwrap_or("").to_string();
        let container = get(attrs, "data-linked-resource-container-id").map(str::to_string);
        let att = get(attrs, "data-linked-resource-id").map(str::to_string);
        let kind = get(attrs, "data-linked-resource-type").unwrap_or("attachment");
        let version = get(attrs, "data-linked-resource-version").filter(|v| Self::all_digits(v)).map(str::to_string);
        let new_src = match self.product {
            Product::Confluence => match (container, att) {
                (Some(c), Some(a)) if kind == "attachment" && Self::all_digits(&c) && Self::all_digits(a.trim_start_matches("att")) => {
                    let a = a.trim_start_matches("att");
                    let mut params = vec![];
                    if let Some(v) = &version {
                        params.push(("v", v.as_str()));
                    }
                    self.push_project(&mut params);
                    Some(with_query(format!("/api/confluence/attachments/{c}/att{a}"), &params))
                }
                _ => self.site_path(&src).and_then(|p| {
                    let c = ATTACHMENT_PATH.captures(p)?;
                    let mut params = vec![];
                    self.push_project(&mut params);
                    Some(with_query(format!("/api/confluence/attachments/{}/by-name/{}", &c[1], reencode(&c[2])), &params))
                }),
            },
            Product::Jira => self.site_path(&src).and_then(|p| {
                let c = JIRA_ATTACHMENT.captures(p)?;
                let mut params = vec![];
                if p.contains("thumbnail") {
                    params.push(("thumb", "1"));
                }
                self.push_project(&mut params);
                Some(with_query(format!("/api/jira/attachments/{}", &c[1]), &params))
            }),
        };
        // Confluence sizes embedded images from data-width (the file's own size can be larger).
        if get(attrs, "width").is_none() {
            if let Some(w) = get(attrs, "data-width").filter(|w| Self::all_digits(w)).map(str::to_string) {
                set(attrs, "width", w);
            }
        }
        match new_src {
            Some(s) => {
                set(attrs, "src", s);
                remove(attrs, &["srcset", "data-image-src"]);
            }
            None => {
                // Other site-relative images (emoticons, static icons) load from the site.
                if let Some(p) = self.site_path(&src) {
                    if src.starts_with('/') {
                        set(attrs, "src", format!("{}{p}", self.site));
                    }
                }
                remove(attrs, &["srcset"]);
            }
        }
    }

    fn rewrite_a(&self, attrs: &mut Vec<(String, String)>) {
        let href = get(attrs, "href").unwrap_or("").to_string();
        if href.is_empty() || href.starts_with('#') {
            return;
        }
        let path = self.site_path(&href).map(str::to_string);
        match self.product {
            Product::Confluence => {
                let mut page = None;
                if get(attrs, "data-linked-resource-type") == Some("page") {
                    page = get(attrs, "data-linked-resource-id").filter(|v| Self::all_digits(v)).map(str::to_string);
                }
                if let Some(p) = &path {
                    if page.is_none() {
                        page = PAGE_PATH
                            .captures(p)
                            .or_else(|| PAGE_ID_PARAM.captures(p))
                            .map(|c| c[1].to_string());
                    }
                    if page.is_none() {
                        if let Some(c) = ATTACHMENT_PATH.captures(p) {
                            let mut params = vec![("download", "1")];
                            self.push_project(&mut params);
                            let url = with_query(format!("/api/confluence/attachments/{}/by-name/{}", &c[1], reencode(&c[2])), &params);
                            set(attrs, "href", url);
                            return;
                        }
                    }
                }
                if let Some(id) = page {
                    set(attrs, "data-wb-page", id);
                    // Keep an in-page anchor (`…/pages/1/Title#Heading`) for scrolling.
                    if let Some((_, frag)) = href.split_once('#') {
                        set(attrs, "data-wb-anchor", frag.to_string());
                    }
                }
            }
            Product::Jira => {
                let key = get(attrs, "data-issue-key")
                    .map(str::to_string)
                    .or_else(|| path.as_deref().and_then(|p| JIRA_BROWSE.captures(p)).map(|c| c[1].to_string()));
                if let Some(k) = key {
                    set(attrs, "data-wb-issue", k);
                }
            }
        }
        if let Some(p) = path {
            if href.starts_with('/') {
                set(attrs, "href", format!("{}{p}", self.site));
            }
        }
    }

    /// Pass 1: rewrite `<img>` and `<a>` start tags.
    pub fn rewrite_tags(&self, html: &str) -> String {
        TAG.replace_all(html, |c: &regex::Captures| {
            let name = c[1].to_ascii_lowercase();
            let mut attrs = parse_attrs(c.get(2).map(|m| m.as_str()).unwrap_or(""));
            if name == "img" {
                self.rewrite_img(&mut attrs);
            } else {
                self.rewrite_a(&mut attrs);
            }
            render_tag(&name, &attrs)
        })
        .into_owned()
    }

    /// Both passes: the HTML the UI may insert into the page.
    pub fn render(&self, html: &str) -> String {
        sanitizer().clean(&self.rewrite_tags(html)).to_string()
    }
}

const TAGS: &[&str] = &[
    "a", "abbr", "b", "blockquote", "br", "caption", "cite", "code", "col", "colgroup", "dd", "del", "details", "dfn",
    "div", "dl", "dt", "em", "figcaption", "figure", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "ins", "kbd",
    "li", "mark", "ol", "p", "pre", "q", "s", "samp", "small", "span", "strike", "strong", "sub", "summary", "sup",
    "table", "tbody", "td", "tfoot", "th", "thead", "time", "tr", "u", "ul", "var", "wbr",
];

/// Tags removed together with their content.
const DROP_WITH_CONTENT: &[&str] = &[
    "script", "style", "button", "form", "template", "noscript", "select", "textarea", "iframe", "object", "embed",
    "svg", "math", "audio", "video", "canvas", "input",
];

const GENERIC_ATTRS: &[&str] = &["title", "id", "style", "data-macro-name", "data-layout", "data-type", "data-ref", "lang", "dir"];

const TAG_ATTRS: &[(&str, &[&str])] = &[
    ("a", &["href", "data-wb-page", "data-wb-anchor", "data-wb-issue"]),
    ("img", &["src", "alt", "width", "height", "loading", "data-linked-resource-default-alias"]),
    ("td", &["colspan", "rowspan", "data-highlight-colour"]),
    ("th", &["colspan", "rowspan", "scope", "data-highlight-colour"]),
    ("col", &["span"]),
    ("ol", &["start", "type"]),
    ("li", &["data-inline-task-id", "value"]),
    ("time", &["datetime"]),
    ("details", &["open"]),
    ("span", &["data-emoji-id", "data-emoji-short-name"]),
];

/// Classes the Confluence styles in the UI rely on, per tag.
const CLASSES: &[(&str, &[&str])] = &[
    (
        "div",
        &[
            "contentLayout2", "columnLayout", "single", "fixed-width", "full-width", "two-equal", "two-left-sidebar",
            "two-right-sidebar", "three-equal", "three-with-sidebars", "four-equal", "five-equal", "cell", "normal", "aside",
            "sidebars", "innerCell", "table-wrap", "panel", "panelContent", "panelHeader", "code", "codeContent",
            "codeHeader", "pdl", "confluence-information-macro", "confluence-information-macro-information",
            "confluence-information-macro-note", "confluence-information-macro-warning", "confluence-information-macro-tip",
            "confluence-information-macro-body", "expand-container", "expand-control", "expand-content", "expand-hidden",
            "conf-macro", "output-block", "toc-macro", "client-side-toc-macro", "preformatted", "preformattedContent",
            "syntaxhighlighter", "plugin-tabmeta-details", "details", "error", "aui-message",
        ],
    ),
    (
        "span",
        &[
            "confluence-embedded-file-wrapper", "image-center-wrapper", "image-left-wrapper", "image-right-wrapper",
            "confluence-embedded-manual-size", "status-macro", "aui-lozenge", "aui-lozenge-visual", "aui-lozenge-subtle",
            "aui-lozenge-success", "aui-lozenge-error", "aui-lozenge-current", "aui-lozenge-complete", "aui-lozenge-moved",
            "aui-lozenge-default", "aui-lozenge-inprogress", "aui-lozenge-new", "confluence-anchor-link", "expand-control-text",
            "expand-control-icon", "aui-icon", "aui-icon-small", "aui-iconfont-info", "aui-iconfont-warning",
            "aui-iconfont-error", "aui-iconfont-approve", "confluence-information-macro-icon", "inline-comment-marker",
            "valid", "conf-macro", "output-inline", "emoticon", "confluence-jim-macro", "jira-issue", "jira-status",
            "user-mention", "mention", "code",
        ],
    ),
    ("table", &["confluenceTable", "wrapped", "relative-table", "fixed-table", "aui"]),
    ("th", &["confluenceTh", "numberingColumn", "highlight-grey", "highlight-blue", "highlight-green", "highlight-red", "highlight-yellow", "highlight-purple", "highlight-teal"]),
    ("td", &["confluenceTd", "numberingColumn", "highlight-grey", "highlight-blue", "highlight-green", "highlight-red", "highlight-yellow", "highlight-purple", "highlight-teal"]),
    ("img", &["confluence-embedded-image", "image-center", "image-left", "image-right", "emoticon", "confluence-embedded-manual-size", "emoji"]),
    ("pre", &["syntaxhighlighter-pre", "code-java", "code-javascript"]),
    ("a", &["confluence-link", "external-link", "unresolved", "confluence-userlink", "user-mention", "current-user-mention", "issue-link", "user-hover"]),
    ("p", &["auto-cursor-target"]),
    ("ul", &["inline-task-list"]),
    ("li", &["checked"]),
    ("code", &["language-java"]),
];

const STYLE_PROPS: &[&str] = &[
    "text-align", "width", "max-width", "color", "text-decoration", "padding-left", "margin-left", "font-weight",
    "font-style", "vertical-align", "list-style-type",
];

/// Relative URLs may only point at Workbench's own attachment proxies or in-page anchors;
/// the rewrite pass has already made every other site link absolute.
fn relative_url(url: &str) -> Option<Cow<'_, str>> {
    let ok = url.starts_with('#')
        || url.starts_with("/api/confluence/attachments/")
        || url.starts_with("/api/jira/attachments/");
    ok.then_some(Cow::Borrowed(url))
}

fn sanitizer() -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::default();
    b.tags(TAGS.iter().copied().collect())
        .clean_content_tags(DROP_WITH_CONTENT.iter().copied().collect())
        .generic_attributes(GENERIC_ATTRS.iter().copied().collect())
        .tag_attributes(TAG_ATTRS.iter().map(|(t, a)| (*t, a.iter().copied().collect::<HashSet<_>>())).collect())
        .allowed_classes(CLASSES.iter().map(|(t, c)| (*t, c.iter().copied().collect::<HashSet<_>>())).collect::<HashMap<_, _>>())
        .filter_style_properties(STYLE_PROPS.iter().copied().collect())
        .url_schemes(["http", "https", "mailto", "tel"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::Custom(Box::new(relative_url)))
        .link_rel(Some("noopener noreferrer"))
        // Heading ids become `cf-…` so they cannot clobber globals; the UI maps `#x` anchors.
        .id_prefix(Some("cf-"))
        .strip_comments(true);
    b
}

/// The text content of (sanitized) HTML, as the browser's `textContent` gives it:
/// tags dropped, entities decoded, whitespace kept. Inline-comment selections are
/// counted in it.
pub fn text_content(html: &str) -> String {
    static TAG_OR_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->|<[^>]*>").unwrap());
    decode_entities(&TAG_OR_COMMENT.replace_all(html, "")).into_owned()
}

/// Non-overlapping occurrences of `needle` in `hay`, scanning from the start.
pub fn occurrences(hay: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        out.push(from + i);
        from += i + needle.len();
    }
    out
}

static HL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@@@(?:end)?hl@@@").unwrap());
static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TAGS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());

/// Clean a CQL search title/excerpt: highlight markers, tags and entities removed,
/// whitespace collapsed.
pub fn clean_search_text(s: &str) -> String {
    let s = HL.replace_all(s, "");
    let s = TAGS_RE.replace_all(&s, "");
    let s = decode_entities(&s);
    WS.replace_all(s.trim(), " ").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf() -> Rewrite<'static> {
        Rewrite { site: "https://example.atlassian.net", product: Product::Confluence, project_id: None }
    }

    const DESIGN_VIEW: &str = include_str!("testdata/design_view.html");
    const MACROS_VIEW: &str = include_str!("testdata/macros_view.html");

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &rsquo;&#39;&#x41;&mdash;"), "a & b <c> \u{2019}'A\u{2014}");
        assert_eq!(decode_entities("&unknown; & &;"), "&unknown; & &;");
        assert_eq!(decode_entities("plain"), "plain");
    }

    #[test]
    fn text_content_and_occurrences() {
        let t = text_content("<p>Hello <strong>world</strong> &amp; hello&nbsp;world</p><!-- x --><p>world</p>");
        assert_eq!(t, "Hello world & hello\u{a0}worldworld");
        assert_eq!(occurrences(&t, "world"), vec![6, 21, 26], "byte offsets");
        assert_eq!(occurrences("aaa", "aa"), vec![0], "non-overlapping");
        assert!(occurrences("abc", "").is_empty());
    }

    #[test]
    fn cleans_cql_excerpts() {
        assert_eq!(
            clean_search_text("The @@@hl@@@Ashen@@@endhl@@@ Choir &amp; friends\n  &mdash; roster"),
            "The Ashen Choir & friends \u{2014} roster"
        );
        // `\s` is Unicode-aware, so a non-breaking space collapses to a plain one.
        assert_eq!(clean_search_text("<b>x</b>&nbsp;y"), "x y");
    }

    #[test]
    fn design_page_images_go_through_the_proxy() {
        let out = cf().render(DESIGN_VIEW);
        assert!(out.contains(r#"src="/api/confluence/attachments/229492/att229517?v=1""#), "{out}");
        assert!(out.contains(r#"width="178""#), "display size comes from data-width");
        assert!(out.contains(r#"width="680""#), "an explicit width wins");
        assert!(out.contains("/api/confluence/attachments/229492/att229542?v=1"));
        assert!(!out.contains("srcset"));
        assert!(!out.contains("/wiki/download/"), "no direct attachment URLs remain");
        assert!(!out.contains("data-image-src"));
    }

    #[test]
    fn design_page_internal_links_are_marked() {
        let out = cf().render(DESIGN_VIEW);
        assert!(out.contains(r#"data-wb-page="229547""#), "{out}");
        assert!(out.contains(r#"href="https://example.atlassian.net/wiki/spaces/DESIGN/pages/229547/Template+-+Design+Sprint""#));
        assert!(out.contains(r#"rel="noopener noreferrer""#));
    }

    #[test]
    fn design_page_structure_is_kept_and_junk_dropped() {
        let out = cf().render(DESIGN_VIEW);
        for keep in [r#"class="columnLayout three-equal""#, r#"class="innerCell""#, r#"class="confluenceTable""#, r#"id="cf-Design-Meettheteam""#] {
            assert!(out.contains(keep), "missing {keep}");
        }
        for gone in ["<style", "data-colorid", "<button", "Create blog post", "background-color", "data-media-id"] {
            assert!(!out.contains(gone), "still has {gone}");
        }
        assert!(out.contains(r#"style="text-align:center""#) || out.contains(r#"style="text-align: center""#));
    }

    #[test]
    fn macros_keep_their_classes() {
        let out = cf().render(MACROS_VIEW);
        assert!(out.contains(r#"class="status-macro aui-lozenge aui-lozenge-visual aui-lozenge-success conf-macro output-inline""#), "{out}");
        assert!(out.contains("confluence-information-macro confluence-information-macro-information"));
        assert!(out.contains(r#"<pre class="syntaxhighlighter-pre">"#));
        assert!(out.contains(r#"<span class="inline-comment-marker" data-ref="c0ffee-1">"#));
        assert!(out.contains(r#"data-wb-page="1409026""#));
        assert!(out.contains(r#"data-wb-anchor="Section-2""#));
    }

    #[test]
    fn hostile_markup_is_removed() {
        let html = r#"<p onclick="x()">hi<script>alert(1)</script><img src="x" onerror="y()"><a href="javascript:alert(1)">j</a><a href="/api/terminals">t</a><iframe src="https://evil"></iframe><a href="//evil.example/x">p</a></p>"#;
        let out = cf().render(html);
        assert!(!out.contains("onclick") && !out.contains("onerror") && !out.contains("script"));
        assert!(!out.contains("javascript:"));
        assert!(!out.contains("iframe"));
        assert!(!out.contains(r#"href="/api/terminals""#), "relative link outside the proxies is dropped");
        // An img src that is neither a Confluence attachment nor absolute is dropped.
        assert!(!out.contains(r#"src="x""#));
        assert!(!out.contains("evil.example"), "protocol-relative links are dropped");
    }

    #[test]
    fn filename_only_images_use_the_by_name_proxy() {
        let r = Rewrite { site: "https://s.atlassian.net", product: Product::Confluence, project_id: Some("shop".into()) };
        let out = r.render(r#"<img src="https://s.atlassian.net/wiki/download/attachments/42/my%20pic.png?api=v2">"#);
        assert!(out.contains(r#"src="/api/confluence/attachments/42/by-name/my%20pic.png?projectId=shop""#), "{out}");
        let out = r.render(r#"<img data-linked-resource-container-id="42" data-linked-resource-id="7" src="x">"#);
        assert!(out.contains(r#"src="/api/confluence/attachments/42/att7?projectId=shop""#), "{out}");
    }

    #[test]
    fn jira_attachments_and_issue_links() {
        let r = Rewrite { site: "https://s.atlassian.net", product: Product::Jira, project_id: None };
        let out = r.render(
            r#"<p><img src="/rest/api/3/attachment/content/10001"> <img src="https://s.atlassian.net/secure/thumbnail/10002/x.png"> <a href="https://s.atlassian.net/browse/ABC-12">ABC-12</a></p>"#,
        );
        assert!(out.contains(r#"src="/api/jira/attachments/10001""#), "{out}");
        assert!(out.contains(r#"src="/api/jira/attachments/10002?thumb=1""#), "{out}");
        assert!(out.contains(r#"data-wb-issue="ABC-12""#));
    }

    /// Runs on the raw research samples when `WORKBENCH_ATLASSIAN_SAMPLES` points at them
    /// (they are the owner's pages, so they are not committed).
    #[test]
    fn research_samples_render_cleanly() {
        let Ok(dir) = std::env::var("WORKBENCH_ATLASSIAN_SAMPLES") else { return };
        let dir = std::path::Path::new(&dir);
        if let Ok(text) = std::fs::read_to_string(dir.join("page_view.json")) {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            let html = v.pointer("/body/view/value").and_then(|x| x.as_str()).unwrap();
            let out = cf().render(html);
            assert!(out.contains("confluenceTable"));
            assert!(!out.contains("<script"));
            assert!(out.len() > html.len() / 2, "sanitizing kept most of the page");
        }
    }
}
