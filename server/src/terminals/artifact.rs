//! Claude Code's `Artifact` tool: pages a session publishes to claude.ai.
//!
//! The page is a file of the project that leaves this computer (claude.ai stores it, private to the
//! account until its owner shares it), so Workbench says so where it asks for permission, and keeps the
//! link a publish returns on the session (`AgentInfo.artifacts`) so the user can open it from Workbench.
//!
//! What the tool answers is not documented, so the link is looked for in the whole `tool_response`,
//! whatever its shape, and only a claude.ai artifact URL counts: a page cannot make Workbench offer
//! another site.

use std::sync::LazyLock;

use serde_json::Value;

use super::permission::{redact_patterns, show_invisible};
use super::transcript::clean_line;
use super::{ArtifactLink, MAX_ARTIFACTS};

/// `https://claude.ai/artifact/<id>` and `https://claude.ai/code/artifact/<id>`.
static ARTIFACT_URL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https://claude\.ai/(?:code/)?artifact/[A-Za-z0-9][A-Za-z0-9_-]{5,80}").unwrap());

/// The one claude.ai artifact URL in `text`, if it is exactly one (`url` must match whole).
pub fn valid_url(url: &str) -> bool {
    ARTIFACT_URL.find(url).is_some_and(|m| m.start() == 0 && m.end() == url.len())
}

fn text(input: Option<&Value>, k: &str) -> Option<String> {
    input.and_then(|i| i.get(k)).and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| redact_patterns(&show_invisible(s)))
}

/// What the call does: a page is published or updated unless `action` says otherwise.
fn action(input: Option<&Value>) -> &str {
    input.and_then(|i| i.get("action")).and_then(Value::as_str).filter(|a| !a.is_empty()).unwrap_or("publish")
}

fn is_asset_upload(input: Option<&Value>) -> bool {
    input.and_then(|i| i.get("asset")).and_then(Value::as_bool).unwrap_or(false)
}

/// The published paths of a call's `files` (a map, or a list of `{path}` / strings).
fn file_names(input: Option<&Value>) -> Vec<String> {
    match input.and_then(|i| i.get("files")) {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|f| f.as_str().or_else(|| f.get("path").and_then(Value::as_str)))
            .map(str::to_string)
            .collect(),
        _ => vec![],
    }
}

/// The runtime capabilities a page declares (`db`, `claude`…): what it can do once it is open.
fn capabilities(input: Option<&Value>) -> Vec<String> {
    match input.and_then(|i| i.get("capabilities")) {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        _ => vec![],
    }
}

/// The verb of the one-line permission text: `publish dashboard.html to claude.ai`.
pub fn summary(input: Option<&Value>, cwd: Option<&str>) -> Option<String> {
    let short = |p: &str| cwd.and_then(|c| crate::util::os::path::below_dir(p, c)).unwrap_or(p).to_string();
    let file = text(input, "file_path").map(|p| clean_line(&short(&p), 120));
    let url = text(input, "url").map(|u| clean_line(&u, 120));
    Some(match (action(input), is_asset_upload(input)) {
        ("publish", true) => match &url {
            Some(u) => format!("upload a file to the page {u} on claude.ai"),
            None => "upload a file to a page on claude.ai".to_string(),
        },
        ("publish", false) => match (&url, &file) {
            (Some(u), Some(f)) => format!("update the page {u} on claude.ai with {f}"),
            (Some(u), None) => format!("update the page {u} on claude.ai"),
            (None, Some(f)) => format!("publish {f} to claude.ai"),
            (None, None) => "publish a page to claude.ai".to_string(),
        },
        ("delete", _) => match &url {
            Some(u) => format!("delete the page {u} on claude.ai"),
            None => "delete a page on claude.ai".to_string(),
        },
        ("pin" | "unpin", _) => format!("{} a page in your claude.ai sidebar", action(input)),
        (a @ ("read" | "list" | "open" | "quickstart"), _) => format!("{a} pages on claude.ai"),
        _ => return None,
    })
}

/// The whole request as text, for the approval's detail view: what leaves the computer and where it is read from.
pub fn detail(input: Option<&Value>) -> String {
    let mut out = vec![format!("Artifact tool: {}{}", action(input), if is_asset_upload(input) { " (upload a file)" } else { "" })];
    if let Some(f) = text(input, "file_path") {
        out.push(format!("Page file: {f}"));
    }
    if let Some(u) = text(input, "url") {
        out.push(format!("Page: {u}"));
    }
    let names = file_names(input);
    if !names.is_empty() {
        let shown: Vec<String> = names.iter().take(10).map(|n| redact_patterns(&show_invisible(n))).collect();
        let more = names.len().saturating_sub(shown.len());
        out.push(format!("Supporting files ({}): {}{}", names.len(), shown.join(", "), if more > 0 { format!(", and {more} more") } else { String::new() }));
    }
    let caps = capabilities(input);
    if !caps.is_empty() {
        out.push(format!("Page capabilities: {}", caps.iter().map(|c| redact_patterns(&show_invisible(c))).collect::<Vec<_>>().join(", ")));
    }
    if let Some(d) = text(input, "description") {
        out.push(format!("Description: {d}"));
    }
    if matches!(action(input), "publish") {
        out.push("The page file is sent to claude.ai: it stays private to your account until you share it. Open the file to see what is sent.".into());
    }
    out.join("\n")
}

/// The page a finished `Artifact` call published or updated, if it did.
pub fn published(v: &Value, now: i64) -> Option<ArtifactLink> {
    if v.get("tool_name").and_then(Value::as_str) != Some("Artifact") {
        return None;
    }
    let input = v.get("tool_input");
    if action(input) != "publish" || is_asset_upload(input) {
        return None;
    }
    // An update names its page; a new page's address is in the answer.
    let from_input = input.and_then(|i| i.get("url")).and_then(Value::as_str).filter(|u| valid_url(u)).map(str::to_string);
    let url = from_input.or_else(|| {
        let answer = match v.get("tool_response")? {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        ARTIFACT_URL.find(&answer).map(|m| m.as_str().to_string())
    })?;
    let title = input
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            let f = input?.get("file_path")?.as_str()?;
            std::path::Path::new(f).file_stem().and_then(|s| s.to_str()).map(str::to_string)
        })
        .map(|t| clean_line(&redact_patterns(&show_invisible(&t)), 80))
        .filter(|t| !t.is_empty());
    Some(ArtifactLink { url, title, at: now })
}

/// Put a page first in a session's list (a republish moves it up); returns whether the list changed.
pub fn remember(list: &mut Vec<ArtifactLink>, link: ArtifactLink) -> bool {
    let title = link.title.clone();
    let before = list.clone();
    let kept_title = list.iter().find(|l| l.url == link.url).and_then(|l| l.title.clone());
    list.retain(|l| l.url != link.url);
    list.insert(0, ArtifactLink { title: title.or(kept_title), ..link });
    list.truncate(MAX_ARTIFACTS);
    *list != before
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const URL: &str = "https://claude.ai/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab";

    fn done(input: Value, response: Value) -> Value {
        json!({ "hook_event_name": "PostToolUse", "tool_name": "Artifact", "tool_input": input, "tool_response": response })
    }

    #[test]
    fn only_a_claude_ai_artifact_address_is_a_link() {
        assert!(valid_url(URL));
        assert!(valid_url("https://claude.ai/code/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab"));
        assert!(!valid_url("https://claude.ai/artifact/x"), "an id has some length");
        assert!(!valid_url("http://claude.ai/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab"));
        assert!(!valid_url("https://claude.ai.evil.example/artifact/9f2c1d7e-aaaa-4bbb-8ccc-0123456789ab"));
        assert!(!valid_url("https://evil.example/https://claude.ai/artifact/9f2c1d7e-aaaa-4bbb"));
        assert!(!valid_url(&format!("{URL} and more")));
        assert!(!valid_url("javascript:alert(1)"));
    }

    #[test]
    fn a_publish_leaves_a_link_whatever_shape_the_answer_has() {
        let input = json!({ "file_path": "/w/p/out/ci-dashboard.html", "icon": "chart" });
        for response in [
            json!(format!("Published. Artifact URL: {URL}")),
            json!({ "url": URL, "ok": true }),
            json!({ "content": [{ "type": "text", "text": format!("done — {URL} (private)") }] }),
        ] {
            let link = published(&done(input.clone(), response), 5).expect("a link");
            assert_eq!((link.url.as_str(), link.title.as_deref(), link.at), (URL, Some("ci-dashboard"), 5));
        }
        // A title from the call wins over the file name.
        let link = published(&done(json!({ "file_path": "/a/b.html", "title": "CI health" }), json!(URL)), 1).unwrap();
        assert_eq!(link.title.as_deref(), Some("CI health"));
        // An update names its page, which the answer need not repeat.
        let link = published(&done(json!({ "file_path": "/a/b.html", "url": URL }), json!("Updated.")), 1).unwrap();
        assert_eq!(link.url, URL);
    }

    #[test]
    fn what_is_not_a_publish_or_not_claude_ais_leaves_no_link() {
        let p = json!({ "file_path": "/a/b.html" });
        // Not the Artifact tool, or listing and reading pages, uploading assets, a refusal that names no page.
        assert!(published(&json!({ "tool_name": "Bash", "tool_input": p, "tool_response": URL }), 1).is_none());
        assert!(published(&done(json!({ "action": "list" }), json!(URL)), 1).is_none());
        assert!(published(&done(json!({ "action": "read", "url": URL }), json!("<html>")), 1).is_none());
        assert!(published(&done(json!({ "action": "delete", "url": URL }), json!("deleted")), 1).is_none());
        assert!(published(&done(json!({ "file_path": "/a/x.png", "asset": true, "url": URL }), json!("ok")), 1).is_none());
        assert!(published(&done(p.clone(), json!("refused: the page is too large")), 1).is_none());
        // Another site's address, even inside the answer or in the call, is not offered.
        assert!(published(&done(p.clone(), json!("see https://evil.example/artifact/9f2c1d7e-aaaa-4bbb")), 1).is_none());
        assert!(published(&done(json!({ "file_path": "/a/b.html", "url": "https://evil.example/x" }), json!("ok")), 1).is_none());
    }

    #[test]
    fn a_session_remembers_its_newest_pages_first() {
        let mk = |u: &str, t: &str, at| ArtifactLink { url: format!("https://claude.ai/artifact/{u}"), title: Some(t.into()), at };
        let mut l = vec![];
        assert!(remember(&mut l, mk("aaaaaa", "A", 1)));
        assert!(remember(&mut l, mk("bbbbbb", "B", 2)));
        // A republish moves the page up and keeps what it was called when the call names nothing.
        let mut again = mk("aaaaaa", "", 3);
        again.title = None;
        assert!(remember(&mut l, again));
        assert_eq!(l.iter().map(|x| (x.url.rsplit('/').next().unwrap(), x.title.as_deref(), x.at)).collect::<Vec<_>>(), [("aaaaaa", Some("A"), 3), ("bbbbbb", Some("B"), 2)]);
        for i in 0..20 {
            remember(&mut l, mk(&format!("page{i:03}"), "x", 10 + i));
        }
        assert_eq!(l.len(), MAX_ARTIFACTS);
        assert!(l[0].url.ends_with("page019"));
    }

    #[test]
    fn the_permission_text_says_the_page_leaves_this_computer() {
        let s = |v: Value| summary(Some(&v), Some("/w/p")).unwrap();
        assert_eq!(s(json!({ "file_path": "/w/p/out/ci.html" })), "publish out/ci.html to claude.ai");
        assert_eq!(s(json!({ "action": "publish", "file_path": "/w/p/ci.html", "url": URL })), format!("update the page {URL} on claude.ai with ci.html"));
        assert_eq!(s(json!({ "action": "delete", "url": URL })), format!("delete the page {URL} on claude.ai"));
        assert_eq!(s(json!({ "file_path": "/w/p/a.png", "asset": true, "url": URL })), format!("upload a file to the page {URL} on claude.ai"));
        assert!(summary(Some(&json!({ "action": "something-new" })), None).is_none(), "an unknown action falls back to the generic text");

        let d = detail(Some(&json!({
            "file_path": "/w/p/ci.html",
            "files": { "app.js": "/w/p/app.js", "style.css": { "from": "/w/p/s.css" } },
            "capabilities": { "db": {}, "claude": {} },
            "description": "CI health, token=hunter2hunter2"
        })));
        assert!(d.contains("Artifact tool: publish") && d.contains("Page file: /w/p/ci.html"), "{d}");
        assert!(d.contains("Supporting files (2): app.js, style.css"), "{d}");
        assert!(d.contains("Page capabilities: claude, db"), "{d}");
        assert!(d.contains("sent to claude.ai") && d.contains("private to your account"), "{d}");
        assert!(!d.contains("hunter2hunter2"), "credentials are masked: {d}");
    }
}
