//! A local mock of the Confluence Cloud (v2 + the v1 bits we use) and Jira v3 APIs,
//! plus tests of the write paths against it (real sites are read-only in tests).
//!
//! The mock also runs standalone for UI work:
//! `WB_ATLASSIAN_MOCK_PORT=4010 cargo test -- --ignored serve_mock_forever --nocapture`
//! then point an isolated Workbench's `[atlassian] site` at `http://127.0.0.1:4010`
//! (any email; token secret `mock-token`).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::client::Product;
use super::{api_for, confluence, jira, jira_api, markdown};
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths, SecretRef, global::AtlassianConfig};

pub const MOCK_TOKEN: &str = "mock-token-0123456789";

#[derive(Clone)]
struct Page {
    id: String,
    title: String,
    space_id: String,
    parent_id: Option<String>,
    status: String,
    kind: String,
    position: i64,
    /// (number, storage, message, createdAt)
    versions: Vec<(u32, String, String, String)>,
    labels: Vec<String>,
}

#[derive(Clone)]
struct Comment {
    id: String,
    page_id: String,
    parent_id: Option<String>,
    inline: bool,
    storage: String,
    created: String,
    selection: Option<String>,
    marker_ref: Option<String>,
    resolution: Option<String>,
    version: u32,
    resolved_by: Option<String>,
}

#[derive(Clone)]
struct Attachment {
    id: String,
    page_id: String,
    title: String,
    media_type: String,
    data: Vec<u8>,
    version: u32,
    comment: String,
    status: String,
}

#[derive(Clone)]
struct Issue {
    key: String,
    summary: String,
    description: Value,
    status: (String, String),
    labels: Vec<String>,
    assignee: Option<String>,
    comments: Vec<(String, Value, String)>,
    sprint: Option<u64>,
}

/// Mock people: (account id, display name).
const USERS: &[(&str, &str)] = &[("u1", "Mock User"), ("u2", "Ann Lee"), ("u3", "Bob Stone")];

/// Jira status ids of the mock workflow.
fn status_id(name: &str) -> &'static str {
    match name {
        "To Do" => "10000",
        "In Progress" => "3",
        "In Review" => "10002",
        _ => "10001",
    }
}

pub struct MockData {
    pages: BTreeMap<String, Page>,
    comments: Vec<Comment>,
    attachments: Vec<Attachment>,
    watching: std::collections::BTreeSet<String>,
    issues: BTreeMap<String, Issue>,
    pub requests: Vec<(String, String, Option<Value>)>,
    pub flaky: u32,
    pub jira: bool,
    next_id: u64,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

const DIAGRAM_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="80"><rect width="240" height="80" rx="8" fill="#3574f0"/><text x="120" y="46" font-family="sans-serif" font-size="18" fill="#fff" text-anchor="middle">mock diagram</text></svg>"##;

const ARCH_STORAGE: &str = r#"<h1>Architecture</h1><p>The <strong>server</strong> is written in Rust. <ac:inline-comment-marker ac:ref="m-1">This sentence has a comment.</ac:inline-comment-marker></p><ac:structured-macro ac:name="info" ac:schema-version="1"><ac:rich-text-body><p>Everything here is mock data.</p></ac:rich-text-body></ac:structured-macro><h2>Components</h2><table><tbody><tr><th><p>Name</p></th><th><p>Language</p></th></tr><tr><td><p>server</p></td><td><p>Rust</p></td></tr><tr><td><p>web</p></td><td><p>TypeScript</p></td></tr></tbody></table><ac:structured-macro ac:name="code" ac:schema-version="1"><ac:parameter ac:name="language">rust</ac:parameter><ac:plain-text-body><![CDATA[fn main() {
    println!("hello <world> & friends");
}]]></ac:plain-text-body></ac:structured-macro><ul><li><p>See <a href="https://example.com">example.com</a></p></li><li><p>Status: <ac:structured-macro ac:name="status" ac:schema-version="1"><ac:parameter ac:name="colour">Green</ac:parameter><ac:parameter ac:name="title">Done</ac:parameter></ac:structured-macro></p></li></ul><p><ac:image ac:alt="diagram"><ri:attachment ri:filename="diagram.svg" /></ac:image></p><p>Next: <a href="/wiki/spaces/DEV/pages/2002/Runbook">the runbook</a>.</p>"#;

impl MockData {
    pub fn seeded() -> Self {
        let t = "2026-09-20T10:00:00.000Z".to_string();
        let page = |id: &str, title: &str, parent: Option<&str>, status: &str, pos: i64, storage: &str| Page {
            id: id.into(),
            title: title.into(),
            space_id: "1000".into(),
            parent_id: parent.map(str::to_string),
            status: status.into(),
            kind: "page".into(),
            position: pos,
            versions: vec![(1, storage.into(), "Created".into(), t.clone())],
            labels: vec![],
        };
        let mut pages = BTreeMap::new();
        for p in [
            page("2000", "Dev Home", None, "current", 0, "<p>Welcome to the <strong>mock</strong> space.</p>"),
            page("2001", "Architecture", Some("2000"), "current", 1, ARCH_STORAGE),
            page("2002", "Runbook", Some("2000"), "current", 2, "<h2>Deploy</h2><ol><li><p>Build</p></li><li><p>Ship</p></li></ol>"),
            page("2003", "Old notes", None, "archived", 3, "<p>Archived content.</p>"),
            page("2004", "API", Some("2001"), "current", 1, "<p>REST routes live under <code>/api</code>.</p>"),
        ] {
            pages.insert(p.id.clone(), p);
        }
        if let Some(a) = pages.get_mut("2001") {
            a.versions.insert(0, (1, "<h1>Architecture</h1><p>First draft.</p>".into(), "Created".into(), t.clone()));
            a.versions[1].0 = 2;
            a.versions[1].2 = "Filled in".into();
            a.labels = vec!["design".into(), "backend".into()];
        }
        let folder = Page { kind: "folder".into(), ..page("2005", "Drafts", Some("2000"), "current", 3, "") };
        pages.insert(folder.id.clone(), folder);
        let comments = vec![
            Comment {
                id: "3001".into(),
                page_id: "2001".into(),
                parent_id: None,
                inline: false,
                storage: "<p>Looks good to me.</p>".into(),
                created: t.clone(),
                selection: None,
                marker_ref: None,
                resolution: None,
                version: 1,
                resolved_by: None,
            },
            Comment {
                id: "3002".into(),
                page_id: "2001".into(),
                parent_id: Some("3001".into()),
                inline: false,
                storage: "<p>Thanks!</p>".into(),
                created: t.clone(),
                selection: None,
                marker_ref: None,
                resolution: None,
                version: 1,
                resolved_by: None,
            },
            Comment {
                id: "3003".into(),
                page_id: "2001".into(),
                parent_id: None,
                inline: true,
                storage: "<p>Is this still true?</p>".into(),
                created: t.clone(),
                selection: Some("This sentence has a comment.".into()),
                marker_ref: Some("m-1".into()),
                resolution: Some("open".into()),
                version: 1,
                resolved_by: None,
            },
        ];
        let attachments = vec![Attachment {
            id: "att7001".into(),
            page_id: "2001".into(),
            title: "diagram.svg".into(),
            media_type: "image/svg+xml".into(),
            data: DIAGRAM_SVG.as_bytes().to_vec(),
            version: 1,
            comment: String::new(),
            status: "current".into(),
        }];
        let adf = |t: &str| markdown::to_adf(t);
        let mut issues = BTreeMap::new();
        for (key, summary, desc, status) in [
            ("WB-1", "Render Confluence tables", "Tables should use **tokens**.\n\n- keep borders\n- no colours", ("To Do", "new")),
            ("WB-2", "Proxy attachment images", "Images need the `download` endpoint.", ("In Progress", "indeterminate")),
            ("WB-3", "Ship Jira panel", "", ("Done", "done")),
        ] {
            issues.insert(
                key.to_string(),
                Issue {
                    key: key.into(),
                    summary: summary.into(),
                    description: if desc.is_empty() { Value::Null } else { adf(desc) },
                    status: (status.0.into(), status.1.into()),
                    labels: vec!["workbench".into()],
                    assignee: (key != "WB-3").then(|| "u1".to_string()),
                    comments: vec![("5001".into(), adf("First comment with `code`."), t.clone())],
                    // WB-1 and WB-2 are in the active sprint; WB-3 waits in the backlog.
                    sprint: (key != "WB-3").then_some(11),
                },
            );
        }
        MockData {
            pages,
            comments,
            attachments,
            watching: Default::default(),
            issues,
            requests: vec![],
            flaky: 1,
            jira: true,
            next_id: 9000,
        }
    }
}

#[derive(Clone)]
pub struct Mock {
    pub data: Arc<Mutex<MockData>>,
    base: Arc<Mutex<String>>,
}

fn j(status: StatusCode, v: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

fn query(uri: &Uri) -> BTreeMap<String, String> {
    uri.query()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), urlencoding::decode(&v.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default()))
        .collect()
}

/// A crude "view" rendering of storage for the mock: enough structure for the UI.
fn render_view(storage: &str) -> String {
    let re = |p: &str| regex::Regex::new(p).unwrap();
    let s = re(r#"<ac:inline-comment-marker ac:ref="([^"]*)">"#)
        .replace_all(storage, r#"<span class="inline-comment-marker" data-ref="$1">"#)
        .replace("</ac:inline-comment-marker>", "</span>");
    let s = re(r#"(?s)<ac:structured-macro ac:name="code"[^>]*>.*?<ac:plain-text-body><!\[CDATA\[(.*?)\]\]></ac:plain-text-body></ac:structured-macro>"#)
        .replace_all(&s, |c: &regex::Captures| {
            format!(
                r#"<div class="code panel pdl conf-macro output-block" data-macro-name="code"><div class="codeContent panelContent pdl"><pre class="syntaxhighlighter-pre">{}</pre></div></div>"#,
                super::html::escape(&c[1])
            )
        });
    let s = re(r#"(?s)<ac:structured-macro ac:name="(info|note|warning|tip)"[^>]*><ac:rich-text-body>(.*?)</ac:rich-text-body></ac:structured-macro>"#)
        .replace_all(&s, r#"<div class="confluence-information-macro confluence-information-macro-$1 conf-macro output-block" data-macro-name="$1"><div class="confluence-information-macro-body">$2</div></div>"#);
    let s = re(r#"(?s)<ac:structured-macro ac:name="status"[^>]*>.*?<ac:parameter ac:name="title">(.*?)</ac:parameter></ac:structured-macro>"#)
        .replace_all(&s, r#"<span class="status-macro aui-lozenge aui-lozenge-visual aui-lozenge-success conf-macro output-inline" data-macro-name="status">$1</span>"#);
    let s = re(r#"<ac:image[^>]*><ri:attachment ri:filename="([^"]*)" ?/></ac:image>"#).replace_all(
        &s,
        r#"<span class="confluence-embedded-file-wrapper"><img class="confluence-embedded-image" src="/wiki/download/attachments/PAGE/$1?api=v2" data-mock-att="$1"></span>"#,
    );
    s.into_owned()
}

fn transitions_json() -> Value {
    json!([
        { "id": "11", "name": "To Do", "to": { "id": "10000", "name": "To Do", "statusCategory": { "key": "new" } }, "isAvailable": true },
        { "id": "21", "name": "Start progress", "to": { "id": "3", "name": "In Progress", "statusCategory": { "key": "indeterminate" } }, "isAvailable": true },
        { "id": "31", "name": "Done", "to": { "id": "10001", "name": "Done", "statusCategory": { "key": "done" } }, "isAvailable": true },
    ])
}

fn board_json(id: u64) -> Option<Value> {
    let (name, kind) = match id {
        1 => ("WB board", "scrum"),
        2 => ("WB kanban", "kanban"),
        _ => return None,
    };
    Some(json!({ "id": id, "name": name, "type": kind, "self": format!("/rest/agile/1.0/board/{id}"),
        "location": { "projectId": 1, "projectKey": "WB", "projectName": "Workbench", "displayName": "Workbench (WB)" } }))
}

fn sprints_json() -> Vec<Value> {
    vec![
        json!({ "id": 10, "name": "WB Sprint 1", "state": "closed", "startDate": "2026-09-01T09:00:00.000Z", "endDate": "2026-09-14T17:00:00.000Z", "completeDate": "2026-09-14T17:00:00.000Z", "originBoardId": 1 }),
        json!({ "id": 11, "name": "WB Sprint 2", "state": "active", "goal": "Ship the board", "startDate": "2026-09-15T09:00:00.000Z", "endDate": "2026-09-28T17:00:00.000Z", "originBoardId": 1 }),
        json!({ "id": 12, "name": "WB Sprint 3", "state": "future", "originBoardId": 1 }),
    ]
}

fn attachment_json(a: &Attachment) -> Value {
    json!({
        "id": a.id, "status": a.status, "title": a.title, "pageId": a.page_id, "mediaType": a.media_type,
        "fileSize": a.data.len(), "comment": a.comment, "createdAt": "2026-09-20T10:00:00.000Z",
        "version": { "number": a.version, "authorId": "u1", "createdAt": "2026-09-20T10:00:00.000Z" },
        "downloadLink": format!("/rest/api/content/{}/child/attachment/{}/download", a.page_id, a.id),
        "_links": { "download": format!("/rest/api/content/{}/child/attachment/{}/download", a.page_id, a.id) },
    })
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A minimal multipart/form-data reader: (field name, file name, content type, bytes).
fn multipart_parts(content_type: &str, body: &[u8]) -> Vec<(String, Option<String>, Option<String>, Vec<u8>)> {
    let Some(b) = content_type.split("boundary=").nth(1) else { return vec![] };
    let delim = format!("--{}", b.trim_matches('"'));
    let delim = delim.as_bytes();
    let mut starts = vec![];
    let mut i = 0;
    while let Some(p) = find_bytes(&body[i..], delim) {
        starts.push(i + p);
        i += p + delim.len();
    }
    let field = |head: &str, key: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r#"(?i)[; ]{key}="([^"]*)""#)).unwrap();
        re.captures(head).map(|c| c[1].to_string())
    };
    let mut out = vec![];
    for w in starts.windows(2) {
        let part = &body[w[0] + delim.len()..w[1]];
        let part = part.strip_prefix(b"\r\n").unwrap_or(part);
        let part = part.strip_suffix(b"\r\n").unwrap_or(part);
        let Some(h) = find_bytes(part, b"\r\n\r\n") else { continue };
        let head = String::from_utf8_lossy(&part[..h]).to_string();
        let ct = head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_string()));
        out.push((field(&head, "name").unwrap_or_default(), field(&head, "filename"), ct, part[h + 4..].to_vec()));
    }
    out
}

/// Wrap occurrence `index` of `selection` in the storage text with an inline-comment
/// marker, the way Confluence anchors a new inline comment. `None` when the
/// occurrences in the markup do not line up with the page text (left dangling).
fn insert_marker(storage: &str, selection: &str, index: usize, marker: &str) -> Option<String> {
    let needle = super::html::escape(selection);
    let mut hits = vec![];
    let mut from = 0;
    while let Some(i) = storage[from..].find(&needle) {
        let at = from + i;
        let before = &storage[..at];
        let in_tag = before.rfind('<') > before.rfind('>');
        let in_cdata = before.rfind("<![CDATA[").is_some_and(|c| !before[c..].contains("]]>"));
        if !in_tag && !in_cdata {
            hits.push(at);
        }
        from = at + needle.len();
    }
    let at = *hits.get(index)?;
    Some(format!(
        "{}<ac:inline-comment-marker ac:ref=\"{marker}\">{needle}</ac:inline-comment-marker>{}",
        &storage[..at],
        &storage[at + needle.len()..]
    ))
}

/// Confluence's view of `ac:link`s (user mentions, page and attachment links) and of
/// embedded attachments (which carry the attachment's id and version).
fn render_links(view: &str, ids: &BTreeMap<String, String>, page_id: &str, atts: &[Attachment]) -> String {
    let re = |p: &str| regex::Regex::new(p).unwrap();
    let view = re(r#"src="/wiki/download/attachments/PAGE/([^"?]*)\?api=v2" data-mock-att="[^"]*""#).replace_all(view, |c: &regex::Captures| {
        let name = &c[1];
        match atts.iter().find(|a| a.page_id == page_id && a.title == name && a.status == "current") {
            Some(a) => format!(
                r#"src="/wiki/download/attachments/{page_id}/{name}?api=v2" data-linked-resource-id="{}" data-linked-resource-container-id="{page_id}" data-linked-resource-type="attachment" data-linked-resource-version="{}""#,
                a.id.trim_start_matches("att"),
                a.version
            ),
            None => format!(r#"src="/wiki/download/attachments/{page_id}/{name}?api=v2""#),
        }
    });
    let s = re(r#"<ac:link><ri:user ri:account-id="([^"]*)" ?/></ac:link>"#).replace_all(&view, |c: &regex::Captures| {
        let name = USERS.iter().find(|u| u.0 == &c[1]).map(|u| u.1).unwrap_or("Unknown user");
        format!(r#"<a href="/wiki/people/{0}" class="confluence-userlink user-mention" data-account-id="{0}">{name}</a>"#, &c[1])
    });
    let s = re(r#"(?s)<ac:link><ri:page (?:ri:space-key="[^"]*" )?ri:content-title="([^"]*)" ?/>(?:<ac:plain-text-link-body><!\[CDATA\[(.*?)\]\]></ac:plain-text-link-body>)?</ac:link>"#).replace_all(&s, |c: &regex::Captures| {
        let title = super::html::decode_entities(&c[1]).into_owned();
        let text = c.get(2).map(|m| super::html::escape(m.as_str())).unwrap_or_else(|| c[1].to_string());
        match ids.get(&title) {
            Some(pid) => format!(r#"<a href="/wiki/spaces/DEV/pages/{pid}" data-linked-resource-id="{pid}" data-linked-resource-type="page">{text}</a>"#),
            None => format!(r#"<a href="/wiki/pages/createpage.action?title={}" class="createlink">{text}</a>"#, urlencoding::encode(&title)),
        }
    });
    re(r#"<ac:link><ri:attachment ri:filename="([^"]*)" ?/></ac:link>"#)
        .replace_all(&s, |c: &regex::Captures| format!(r#"<a href="/wiki/download/attachments/{page_id}/{0}">{0}</a>"#, &c[1]))
        .into_owned()
}

impl Mock {
    fn page_json(&self, p: &Page, version: Option<u32>, format: Option<&str>, labels: bool) -> Option<Value> {
        let v = match version {
            Some(n) => p.versions.iter().find(|v| v.0 == n)?,
            None => p.versions.last()?,
        };
        let mut out = json!({
            "id": p.id, "title": p.title, "status": p.status, "spaceId": p.space_id,
            "parentId": p.parent_id, "parentType": p.parent_id.as_ref().map(|_| "page"),
            "version": { "number": v.0, "message": v.2, "minorEdit": false, "authorId": "u1", "createdAt": v.3 },
            "_links": { "webui": format!("/spaces/DEV/pages/{}/{}", p.id, p.title.replace(' ', "+")), "edituiv2": format!("/spaces/DEV/pages/edit-v2/{}", p.id) },
        });
        match format {
            Some("storage") => out["body"] = json!({ "storage": { "representation": "storage", "value": v.1 } }),
            Some("view") => out["body"] = json!({ "view": { "representation": "view", "value": render_view(&v.1) } }),
            _ => {}
        }
        if labels {
            out["labels"] = json!({ "results": p.labels.iter().map(|l| json!({"name": l, "prefix": "global"})).collect::<Vec<_>>() });
        }
        Some(out)
    }

    fn comment_json(&self, c: &Comment) -> Value {
        let mut v = json!({
            "id": c.id, "status": "current", "pageId": c.page_id,
            "version": { "number": c.version, "authorId": "u1", "createdAt": c.created },
            "body": { "storage": { "representation": "storage", "value": c.storage } },
            "_links": { "webui": format!("/spaces/DEV/pages/{}?focusedCommentId={}", c.page_id, c.id) },
        });
        if let Some(p) = &c.parent_id {
            v["parentCommentId"] = json!(p);
        }
        if c.inline {
            v["resolutionStatus"] = json!(c.resolution);
            v["properties"] = json!({ "inlineMarkerRef": c.marker_ref, "inlineOriginalSelection": c.selection });
            if let Some(by) = &c.resolved_by {
                v["resolutionLastModifierId"] = json!(by);
                v["resolutionLastModifiedAt"] = json!("2026-09-27T10:00:00.000Z");
            }
        }
        v
    }

    fn issue_json(&self, i: &Issue, base: &str) -> Value {
        let rendered = super::html::escape(&markdown::adf_to_markdown(&i.description)).replace('\n', "<br/>");
        json!({
            "id": i.key.trim_start_matches("WB-"), "key": i.key,
            "fields": {
                "summary": i.summary,
                "status": { "id": status_id(&i.status.0), "name": i.status.0, "statusCategory": { "key": i.status.1 } },
                "assignee": i.assignee.as_ref().map(|a| json!({ "accountId": a, "displayName": USERS.iter().find(|u| u.0 == a).map(|u| u.1).unwrap_or("Mock User") })),
                "reporter": { "accountId": "u1", "displayName": "Mock User" },
                "priority": { "id": "3", "name": "Medium" },
                "issuetype": { "id": "10001", "name": "Task" },
                "labels": i.labels,
                "created": "2026-09-01T10:00:00.000+0000",
                "updated": "2026-09-25T10:00:00.000+0000",
                "project": { "key": "WB", "name": "Workbench" },
                "description": i.description,
            },
            "renderedFields": { "description": format!("<p>{rendered}</p><p><img src=\"{base}/rest/api/3/attachment/content/10001\"></p>") },
            "transitions": transitions_json(),
            "editmeta": { "fields": { "summary": {}, "description": {}, "labels": {}, "priority": { "allowedValues": [ { "id": "1", "name": "Highest" }, { "id": "3", "name": "Medium" }, { "id": "5", "name": "Lowest" } ] }, "assignee": {} } },
        })
    }

    fn handle(&self, method: Method, uri: Uri, headers: &HeaderMap, raw: &[u8], body: Option<Value>) -> Response {
        let path = uri.path().to_string();
        let qs = query(&uri);
        let base = self.base.lock().clone();
        let mut d = self.data.lock();
        let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let parts = if content_type.starts_with("multipart/form-data") { multipart_parts(&content_type, raw) } else { vec![] };
        // Multipart uploads are recorded as a summary of their parts (and the XSRF header).
        let recorded = if parts.is_empty() {
            body.clone()
        } else {
            Some(json!({
                "xsrf": headers.get("x-atlassian-token").and_then(|v| v.to_str().ok()),
                "parts": parts.iter().map(|(n, f, ct, data)| json!({ "name": n, "filename": f, "contentType": ct, "len": data.len(),
                    "text": String::from_utf8(data.clone()).ok().filter(|t| t.len() < 200) })).collect::<Vec<_>>(),
            }))
        };
        d.requests.push((method.to_string(), uri.to_string(), recorded));
        let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
        // Like the real v2 API: comments come only as storage / ADF / markdown.
        if path.contains("-comments") && qs.get("body-format").is_some_and(|f| f == "view") {
            return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "Provided value {view} for 'body-format' is not the correct type" }] }));
        }
        let not_found = || j(StatusCode::NOT_FOUND, json!({ "errors": [{ "status": 404, "code": "NOT_FOUND", "title": "Not found" }] }));
        match (method.clone(), seg.as_slice()) {
            (Method::GET, ["flaky"]) => {
                if d.flaky > 0 {
                    d.flaky -= 1;
                    return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "0")], "slow down").into_response();
                }
                j(StatusCode::OK, json!({ "ok": true }))
            }
            (Method::GET, ["leak"]) => j(StatusCode::BAD_REQUEST, json!({ "message": format!("bad token {MOCK_TOKEN} for you") })),
            (Method::GET, ["wiki", "rest", "api", "user", "current"]) => {
                j(StatusCode::OK, json!({ "type": "known", "accountId": "u1", "displayName": "Mock User", "email": "me@example.com" }))
            }
            (Method::GET, ["wiki", "rest", "api", "user"]) => {
                let id = qs.get("accountId").cloned().unwrap_or_default();
                let name = USERS.iter().find(|u| u.0 == id).map(|u| u.1).unwrap_or("Mock User");
                j(StatusCode::OK, json!({ "accountId": id, "displayName": name }))
            }
            (Method::GET, ["wiki", "rest", "api", "search", "user"]) => {
                let cql = qs.get("cql").cloned().unwrap_or_default();
                let Some(c) = regex::Regex::new(r#"^user\.fullname ~ "((?:[^"\\]|\\.)*)"$"#).unwrap().captures(&cql) else {
                    return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "Could not parse cql" }));
                };
                let want = c[1].to_lowercase();
                let results: Vec<Value> = USERS
                    .iter()
                    .filter(|(_, n)| n.to_lowercase().split(' ').any(|w| w.starts_with(&want)) || n.to_lowercase().starts_with(&want))
                    .map(|(id, n)| json!({ "user": { "type": "known", "accountId": id, "displayName": n, "publicName": n }, "title": n, "entityType": "user" }))
                    .collect();
                j(StatusCode::OK, json!({ "results": results, "start": 0, "limit": 25, "size": results.len(), "totalSize": results.len() }))
            }
            (Method::GET | Method::POST | Method::DELETE, ["wiki", "rest", "api", "user", "watch", "content", id]) => {
                if method != Method::GET && headers.get("x-atlassian-token").is_none() {
                    return j(StatusCode::FORBIDDEN, json!({ "message": "XSRF check failed" }));
                }
                if !d.pages.contains_key(*id) {
                    return j(StatusCode::NOT_FOUND, json!({ "message": "No content" }));
                }
                match method {
                    Method::POST => {
                        d.watching.insert(id.to_string());
                        StatusCode::NO_CONTENT.into_response()
                    }
                    Method::DELETE => {
                        d.watching.remove(*id);
                        StatusCode::NO_CONTENT.into_response()
                    }
                    _ => j(StatusCode::OK, json!({ "watching": d.watching.contains(*id) })),
                }
            }
            (Method::GET, ["wiki", "api", "v2", "spaces"]) => {
                let space = json!({ "id": "1000", "key": "DEV", "name": "Development", "type": "global", "status": "current", "homepageId": "2000" });
                let keys = qs.get("keys");
                let list = if keys.is_none_or(|k| k.split(',').any(|k| k == "DEV")) { vec![space] } else { vec![] };
                j(StatusCode::OK, json!({ "results": list, "_links": {} }))
            }
            (Method::GET, ["wiki", "api", "v2", "spaces", "1000"]) => {
                j(StatusCode::OK, json!({ "id": "1000", "key": "DEV", "name": "Development", "type": "global", "status": "current", "homepageId": "2000" }))
            }
            (Method::GET, ["wiki", "api", "v2", "spaces", "1000", "pages"]) => {
                let status = qs.get("status").cloned();
                let roots: Vec<Value> = d
                    .pages
                    .values()
                    .filter(|p| p.parent_id.is_none() && p.kind == "page" && status.as_ref().is_none_or(|s| *s == p.status))
                    .filter_map(|p| self.page_json(p, None, None, false))
                    .collect();
                j(StatusCode::OK, json!({ "results": roots, "_links": {} }))
            }
            (Method::GET, ["wiki", "api", "v2", "pages"]) => {
                let ids: Vec<String> = qs.get("id").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default();
                let list: Vec<Value> =
                    d.pages.values().filter(|p| ids.contains(&p.id)).filter_map(|p| self.page_json(p, None, None, false)).collect();
                j(StatusCode::OK, json!({ "results": list, "_links": {} }))
            }
            (Method::POST, ["wiki", "api", "v2", "pages"]) => {
                let b = body.unwrap_or(Value::Null);
                d.next_id += 1;
                let id = d.next_id.to_string();
                let p = Page {
                    id: id.clone(),
                    title: b["title"].as_str().unwrap_or("Untitled").into(),
                    space_id: b["spaceId"].as_str().unwrap_or("1000").into(),
                    parent_id: b["parentId"].as_str().map(str::to_string).or(Some("2000".into())),
                    status: "current".into(),
                    kind: "page".into(),
                    position: 99,
                    versions: vec![(1, b["body"]["value"].as_str().unwrap_or("").into(), String::new(), now())],
                    labels: vec![],
                };
                let out = self.page_json(&p, None, None, false);
                d.pages.insert(id, p);
                j(StatusCode::OK, out.unwrap_or(Value::Null))
            }
            (Method::GET, ["wiki", "api", "v2", "pages", id]) => {
                let Some(p) = d.pages.get(*id) else { return not_found() };
                // Like Confluence: a trashed page is only found when asked for by status.
                if p.status == "trashed" && !qs.get("status").is_some_and(|s| s.split(',').any(|x| x == "trashed")) {
                    return not_found();
                }
                let version = qs.get("version").and_then(|v| v.parse().ok());
                match self.page_json(p, version, qs.get("body-format").map(String::as_str), qs.contains_key("include-labels")) {
                    Some(mut v) => {
                        // Mentions and page links render as Confluence renders them: links.
                        if let Some(view) = v.pointer("/body/view/value").and_then(Value::as_str) {
                            let ids: BTreeMap<String, String> = d.pages.values().map(|p| (p.title.clone(), p.id.clone())).collect();
                            v["body"]["view"]["value"] = json!(render_links(view, &ids, id, &d.attachments));
                        }
                        j(StatusCode::OK, v)
                    }
                    None => not_found(),
                }
            }
            (Method::PUT, ["wiki", "api", "v2", "pages", id]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(p) = d.pages.get_mut(*id) else { return not_found() };
                let cur = p.versions.last().map(|v| v.0).unwrap_or(0);
                let want = b["version"]["number"].as_u64().unwrap_or(0) as u32;
                if want != cur + 1 {
                    return j(StatusCode::CONFLICT, json!({ "errors": [{ "status": 409, "title": format!("Version must be incremented (current {cur})") }] }));
                }
                if b["body"]["representation"] != "storage" || b["status"] != "current" {
                    return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "bad body" }] }));
                }
                if p.status == "trashed" {
                    // Restoring: only the status changes (the version still counts up).
                    p.status = "current".into();
                    let last = p.versions.last().cloned().unwrap_or_default();
                    p.versions.push((want, last.1, b["version"]["message"].as_str().unwrap_or("").into(), now()));
                    let p = p.clone();
                    return j(StatusCode::OK, self.page_json(&p, None, None, false).unwrap_or(Value::Null));
                }
                p.title = b["title"].as_str().unwrap_or(&p.title).to_string();
                p.versions.push((want, b["body"]["value"].as_str().unwrap_or("").into(), b["version"]["message"].as_str().unwrap_or("").into(), now()));
                let p = p.clone();
                j(StatusCode::OK, self.page_json(&p, None, None, false).unwrap_or(Value::Null))
            }
            (Method::GET, ["wiki", "api", "v2", "pages", id, "ancestors"]) => {
                let mut chain = vec![];
                let mut cur = d.pages.get(*id).and_then(|p| p.parent_id.clone());
                while let Some(pid) = cur {
                    let Some(pp) = d.pages.get(&pid) else { break };
                    chain.push(json!({ "id": pp.id, "type": pp.kind }));
                    cur = pp.parent_id.clone();
                }
                chain.reverse();
                j(StatusCode::OK, json!({ "results": chain }))
            }
            (Method::GET, ["wiki", "api", "v2", "pages" | "folders", id, kind @ ("descendants" | "direct-children")]) => {
                let limit: usize = qs.get("limit").and_then(|l| l.parse().ok()).unwrap_or(25);
                let offset: usize = qs.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
                let mut all: Vec<Value> = vec![];
                let kids = |parent: &str| -> Vec<Page> {
                    let mut v: Vec<Page> = d.pages.values().filter(|p| p.parent_id.as_deref() == Some(parent)).cloned().collect();
                    v.sort_by_key(|p| p.position);
                    v
                };
                for c in kids(id) {
                    all.push(json!({ "id": c.id, "status": c.status, "title": c.title, "type": c.kind, "parentId": id, "depth": 1, "childPosition": c.position }));
                    if *kind == "descendants" {
                        for g in kids(&c.id) {
                            all.push(json!({ "id": g.id, "status": g.status, "title": g.title, "type": g.kind, "parentId": c.id, "depth": 2, "childPosition": g.position }));
                        }
                    }
                }
                let page: Vec<Value> = all.iter().skip(offset).take(limit).cloned().collect();
                let mut links = json!({});
                if offset + limit < all.len() {
                    links["next"] = json!(format!("/wiki/api/v2/pages/{id}/{kind}?limit={limit}&cursor={}", offset + limit));
                }
                j(StatusCode::OK, json!({ "results": page, "_links": links }))
            }
            (Method::GET, ["wiki", "api", "v2", "pages", id, "versions"]) => {
                let Some(p) = d.pages.get(*id) else { return not_found() };
                let limit: usize = qs.get("limit").and_then(|l| l.parse().ok()).unwrap_or(25);
                let offset: usize = qs.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
                let all: Vec<Value> = p
                    .versions
                    .iter()
                    .rev()
                    .map(|v| json!({ "number": v.0, "message": v.2, "minorEdit": false, "authorId": "u1", "createdAt": v.3 }))
                    .collect();
                let mut links = json!({});
                if offset + limit < all.len() {
                    links["next"] = json!(format!("/wiki/api/v2/pages/{id}/versions?limit={limit}&cursor={}", offset + limit));
                }
                j(StatusCode::OK, json!({ "results": all.into_iter().skip(offset).take(limit).collect::<Vec<_>>(), "_links": links }))
            }
            (Method::GET, ["wiki", "api", "v2", "pages", id, kind @ ("footer-comments" | "inline-comments")]) => {
                let inline = *kind == "inline-comments";
                let list: Vec<Value> = d
                    .comments
                    .iter()
                    .filter(|c| c.page_id == *id && c.inline == inline && c.parent_id.is_none())
                    .map(|c| self.comment_json(c))
                    .collect();
                j(StatusCode::OK, json!({ "results": list, "_links": {} }))
            }
            (Method::GET, ["wiki", "api", "v2", "footer-comments" | "inline-comments", cid, "children"]) => {
                let list: Vec<Value> =
                    d.comments.iter().filter(|c| c.parent_id.as_deref() == Some(*cid)).map(|c| self.comment_json(c)).collect();
                j(StatusCode::OK, json!({ "results": list, "_links": {} }))
            }
            // A new top-level inline comment: anchored by text selection, count and index.
            (Method::POST, ["wiki", "api", "v2", "inline-comments"]) if body.as_ref().is_some_and(|b| b.get("parentCommentId").is_none()) => {
                let b = body.unwrap_or(Value::Null);
                let page_id = b["pageId"].as_str().unwrap_or_default().to_string();
                let props = &b["inlineCommentProperties"];
                let (Some(sel), Some(count), Some(index)) = (
                    props["textSelection"].as_str(),
                    props["textSelectionMatchCount"].as_u64(),
                    props["textSelectionMatchIndex"].as_u64(),
                ) else {
                    return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "inlineCommentProperties are required for a top-level inline comment" }] }));
                };
                let Some(p) = d.pages.get(&page_id) else { return not_found() };
                let storage = p.versions.last().map(|v| v.1.clone()).unwrap_or_default();
                let text = super::html::text_content(&render_view(&storage));
                let actual = super::html::occurrences(&text, sel).len() as u64;
                if actual != count || index >= count {
                    return j(
                        StatusCode::BAD_REQUEST,
                        json!({ "errors": [{ "title": format!("textSelectionMatchCount {count} does not match the {actual} occurrence(s) on the page") }] }),
                    );
                }
                d.next_id += 1;
                let id = d.next_id.to_string();
                let marker = format!("mk-{id}");
                if let Some(anchored) = insert_marker(&storage, sel, index as usize, &marker) {
                    if let Some(v) = d.pages.get_mut(&page_id).and_then(|p| p.versions.last_mut()) {
                        v.1 = anchored;
                    }
                }
                let c = Comment {
                    id: id.clone(),
                    page_id,
                    parent_id: None,
                    inline: true,
                    storage: b["body"]["value"].as_str().unwrap_or("").into(),
                    created: now(),
                    selection: Some(sel.to_string()),
                    marker_ref: Some(marker),
                    resolution: Some("open".into()),
                    version: 1,
                    resolved_by: None,
                };
                let out = self.comment_json(&c);
                d.comments.push(c);
                j(StatusCode::OK, out)
            }
            (Method::POST, ["wiki", "api", "v2", kind @ ("footer-comments" | "inline-comments")]) => {
                let b = body.unwrap_or(Value::Null);
                d.next_id += 1;
                let id = d.next_id.to_string();
                let parent = b["parentCommentId"].as_str().map(str::to_string);
                let page_id = match (&parent, b["pageId"].as_str()) {
                    (_, Some(p)) => p.to_string(),
                    (Some(par), None) => d.comments.iter().find(|c| &c.id == par).map(|c| c.page_id.clone()).unwrap_or_default(),
                    _ => return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "pageId required" }] })),
                };
                d.comments.push(Comment {
                    id: id.clone(),
                    page_id,
                    parent_id: parent,
                    inline: *kind == "inline-comments",
                    storage: b["body"]["value"].as_str().unwrap_or("").into(),
                    created: now(),
                    selection: None,
                    marker_ref: None,
                    resolution: None,
                    version: 1,
                    resolved_by: None,
                });
                j(StatusCode::OK, json!({ "id": id }))
            }
            (Method::GET, ["wiki", "api", "v2", kind @ ("footer-comments" | "inline-comments"), cid]) => {
                let inline = *kind == "inline-comments";
                match d.comments.iter().find(|c| c.id == *cid && c.inline == inline) {
                    Some(c) => j(StatusCode::OK, self.comment_json(c)),
                    None => not_found(),
                }
            }
            (Method::PUT, ["wiki", "api", "v2", kind @ ("footer-comments" | "inline-comments"), cid]) => {
                let b = body.unwrap_or(Value::Null);
                let inline = *kind == "inline-comments";
                let Some(c) = d.comments.iter_mut().find(|c| c.id == *cid && c.inline == inline) else { return not_found() };
                let want = b["version"]["number"].as_u64().unwrap_or(0) as u32;
                if want != c.version + 1 {
                    return j(StatusCode::CONFLICT, json!({ "errors": [{ "status": 409, "title": format!("Version must be incremented (current {})", c.version) }] }));
                }
                if b["body"]["representation"] != "storage" || b["body"]["value"].as_str().is_none_or(str::is_empty) {
                    return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "body is required" }] }));
                }
                if let Some(r) = b.get("resolved").and_then(Value::as_bool) {
                    if !inline {
                        return j(StatusCode::BAD_REQUEST, json!({ "errors": [{ "title": "resolved is not a footer comment field" }] }));
                    }
                    let now_resolved = c.resolution.as_deref() == Some("resolved");
                    if r != now_resolved {
                        c.resolution = Some(if r { "resolved" } else { "reopened" }.into());
                        c.resolved_by = Some("u1".into());
                    }
                }
                c.version = want;
                c.storage = b["body"]["value"].as_str().unwrap_or_default().to_string();
                let c = c.clone();
                j(StatusCode::OK, self.comment_json(&c))
            }
            (Method::DELETE, ["wiki", "api", "v2", kind @ ("footer-comments" | "inline-comments"), cid]) => {
                let inline = *kind == "inline-comments";
                if !d.comments.iter().any(|c| c.id == *cid && c.inline == inline) {
                    return not_found();
                }
                d.comments.retain(|c| c.id != *cid && c.parent_id.as_deref() != Some(*cid));
                StatusCode::NO_CONTENT.into_response()
            }
            (Method::GET, ["wiki", "api", "v2", "pages", id, "attachments"]) => {
                let name = qs.get("filename").cloned();
                let limit: usize = qs.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
                let offset: usize = qs.get("cursor").and_then(|c| c.parse().ok()).unwrap_or(0);
                let mut all: Vec<&Attachment> = d
                    .attachments
                    .iter()
                    .filter(|a| a.page_id == *id && a.status == "current" && name.as_ref().is_none_or(|n| *n == a.title))
                    .collect();
                if qs.get("sort").is_some_and(|s| s == "-created-date") {
                    all.reverse();
                }
                let page: Vec<Value> = all.iter().skip(offset).take(limit).map(|a| attachment_json(a)).collect();
                let mut links = json!({});
                if offset + limit < all.len() {
                    links["next"] = json!(format!("/wiki/api/v2/pages/{id}/attachments?limit={limit}&cursor={}", offset + limit));
                }
                j(StatusCode::OK, json!({ "results": page, "_links": links }))
            }
            (Method::GET, ["wiki", "rest", "api", "content", _page, "child", "attachment", att, "download"]) => {
                (StatusCode::FOUND, [(header::LOCATION, format!("{base}/media/{att}"))]).into_response()
            }
            (Method::GET, ["media", att]) => match d.attachments.iter().find(|a| a.id == *att) {
                Some(a) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, a.media_type.clone()), (header::CONTENT_DISPOSITION, format!("inline; filename=\"{}\"", a.title))],
                    a.data.clone(),
                )
                    .into_response(),
                None => (StatusCode::OK, [(header::CONTENT_TYPE, "image/svg+xml")], DIAGRAM_SVG).into_response(),
            },
            (Method::GET, ["wiki", "rest", "api", "search"]) => {
                let cql = qs.get("cql").cloned().unwrap_or_default();
                let text = regex::Regex::new(r#"text ~ "((?:[^"\\]|\\.)*)""#).unwrap().captures(&cql).map(|c| c[1].to_lowercase()).unwrap_or_default();
                // `title ~ "arch*"`: a title prefix (or contained word) search, as page pickers send.
                let title = regex::Regex::new(r#"title ~ "((?:[^"\\]|\\.)*)""#)
                    .unwrap()
                    .captures(&cql)
                    .map(|c| c[1].trim_end_matches('*').replace("\\\"", "\"").to_lowercase());
                let results: Vec<Value> = d
                    .pages
                    .values()
                    .filter(|p| p.kind == "page" && p.status != "trashed")
                    .filter(|p| title.as_ref().is_none_or(|t| p.title.to_lowercase().split(' ').any(|w| w.starts_with(t.as_str())) || p.title.to_lowercase().starts_with(t.as_str())))
                    .filter(|p| text.is_empty() || p.title.to_lowercase().contains(&text) || p.versions.last().is_some_and(|v| v.1.to_lowercase().contains(&text)))
                    .map(|p| {
                        let hl = if text.is_empty() { p.title.clone() } else { p.title.replacen(&p.title, &format!("@@@hl@@@{}@@@endhl@@@", p.title), 1) };
                        json!({
                            "content": { "id": p.id, "type": "page", "status": p.status, "title": p.title, "_links": { "webui": format!("/spaces/DEV/pages/{}", p.id) } },
                            "title": hl.replace('&', "&amp;"),
                            "excerpt": format!("…mock &amp; excerpt for {}…", p.title),
                            "url": format!("/spaces/DEV/pages/{}", p.id),
                            "resultGlobalContainer": { "title": "Development", "displayUrl": "/spaces/DEV" },
                            "lastModified": "2026-09-20T10:00:00.000Z",
                            "entityType": "content",
                        })
                    })
                    .collect();
                j(StatusCode::OK, json!({ "results": results, "start": 0, "limit": 25, "size": results.len(), "totalSize": results.len(), "_links": {} }))
            }
            (Method::POST | Method::PUT, ["wiki", "rest", "api", "content", page_id, "child", "attachment"]) => {
                let xsrf = headers.get("x-atlassian-token").and_then(|v| v.to_str().ok());
                if !matches!(xsrf, Some("no-check" | "nocheck")) {
                    return j(StatusCode::FORBIDDEN, json!({ "message": "XSRF check failed" }));
                }
                if !d.pages.contains_key(*page_id) {
                    return j(StatusCode::NOT_FOUND, json!({ "statusCode": 404, "message": "No content found with id" }));
                }
                let Some((_, Some(filename), ct, data)) = parts.iter().find(|p| p.0 == "file").cloned() else {
                    return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "No file part" }));
                };
                if !parts.iter().any(|p| p.0 == "minorEdit") {
                    return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "minorEdit is required" }));
                }
                let comment = parts.iter().find(|p| p.0 == "comment").map(|p| String::from_utf8_lossy(&p.3).into_owned()).unwrap_or_default();
                let media_type = ct.unwrap_or_else(|| "application/octet-stream".into());
                let existing = d.attachments.iter().position(|a| a.page_id == *page_id && a.title == filename && a.status == "current");
                let a = match (existing, method.clone()) {
                    (Some(_), Method::POST) => {
                        return j(
                            StatusCode::BAD_REQUEST,
                            json!({ "statusCode": 400, "message": format!("Cannot add a new attachment with same file name as an existing attachment: {filename}") }),
                        );
                    }
                    (Some(i), _) => {
                        let a = &mut d.attachments[i];
                        a.version += 1;
                        a.data = data;
                        a.media_type = media_type;
                        a.comment = comment;
                        a.clone()
                    }
                    (None, _) => {
                        d.next_id += 1;
                        let a = Attachment {
                            id: format!("att{}", d.next_id),
                            page_id: page_id.to_string(),
                            title: filename,
                            media_type,
                            data,
                            version: 1,
                            comment,
                            status: "current".into(),
                        };
                        d.attachments.push(a.clone());
                        a
                    }
                };
                j(
                    StatusCode::OK,
                    json!({ "results": [{
                        "id": a.id, "type": "attachment", "status": "current", "title": a.title,
                        "version": { "number": a.version, "when": now(), "by": { "type": "known", "accountId": "u1", "displayName": "Mock User" } },
                        "extensions": { "mediaType": a.media_type, "fileSize": a.data.len(), "comment": a.comment },
                        "_links": { "download": format!("/download/attachments/{}/{}", a.page_id, a.title) },
                    }], "size": 1 }),
                )
            }
            (Method::GET, ["wiki", "api", "v2", "attachments", att]) => match d.attachments.iter().find(|a| a.id == *att || a.id == format!("att{att}")) {
                Some(a) => j(StatusCode::OK, attachment_json(a)),
                None => not_found(),
            },
            (Method::DELETE, ["wiki", "api", "v2", "attachments", att]) => match d.attachments.iter_mut().find(|a| a.id == *att && a.status == "current") {
                Some(a) => {
                    a.status = "trashed".into();
                    StatusCode::NO_CONTENT.into_response()
                }
                None => not_found(),
            },
            (Method::GET, ["wiki", "rest", "api", "content", id, "label"]) => match d.pages.get(*id) {
                Some(p) => j(StatusCode::OK, json!({ "results": p.labels.iter().map(|l| json!({ "prefix": "global", "name": l, "label": l })).collect::<Vec<_>>(), "size": p.labels.len() })),
                None => not_found(),
            },
            (Method::POST, ["wiki", "rest", "api", "content", id, "label"]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(p) = d.pages.get_mut(*id) else { return not_found() };
                let Some(list) = b.as_array() else {
                    return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "expected a list of labels" }));
                };
                for l in list {
                    let (Some(name), Some("global")) = (l["name"].as_str(), l["prefix"].as_str()) else {
                        return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "label needs name and prefix" }));
                    };
                    if !p.labels.iter().any(|x| x == name) {
                        p.labels.push(name.to_lowercase());
                    }
                }
                j(StatusCode::OK, json!({ "results": p.labels.iter().map(|l| json!({ "prefix": "global", "name": l })).collect::<Vec<_>>(), "size": p.labels.len() }))
            }
            (Method::DELETE, ["wiki", "rest", "api", "content", id, "label"]) => {
                let name = qs.get("name").cloned().unwrap_or_default();
                let Some(p) = d.pages.get_mut(*id) else { return not_found() };
                if !p.labels.contains(&name) {
                    return j(StatusCode::NOT_FOUND, json!({ "statusCode": 404, "message": "Label not found" }));
                }
                p.labels.retain(|l| *l != name);
                StatusCode::NO_CONTENT.into_response()
            }
            (Method::PUT, ["wiki", "rest", "api", "content", id, "move", position, target]) => {
                if !d.pages.contains_key(*id) || !d.pages.contains_key(*target) {
                    return j(StatusCode::NOT_FOUND, json!({ "statusCode": 404, "message": "page not found" }));
                }
                // Not under itself or its own descendants.
                let mut cur = Some(target.to_string());
                while let Some(c) = cur {
                    if c == *id {
                        return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "Cannot move a page under itself" }));
                    }
                    cur = d.pages.get(&c).and_then(|p| p.parent_id.clone());
                }
                let target_page = d.pages[*target].clone();
                let (parent, pos) = match *position {
                    "append" => {
                        let max = d.pages.values().filter(|p| p.parent_id.as_deref() == Some(*target)).map(|p| p.position).max().unwrap_or(0);
                        (Some(target.to_string()), max + 1)
                    }
                    "before" => (target_page.parent_id.clone(), target_page.position * 2 - 1),
                    "after" => (target_page.parent_id.clone(), target_page.position * 2 + 1),
                    _ => return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "bad position" })),
                };
                if *position != "append" {
                    // Make room: double the siblings' positions so before/after fit between them.
                    for p in d.pages.values_mut().filter(|p| p.parent_id == parent) {
                        p.position *= 2;
                    }
                }
                if let Some(p) = d.pages.get_mut(*id) {
                    p.parent_id = parent;
                    p.position = pos;
                }
                j(StatusCode::OK, json!({ "pageId": id }))
            }
            (Method::POST, ["wiki", "rest", "api", "content", id, "copy"]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(orig) = d.pages.get(*id).cloned() else { return not_found() };
                let title = b["pageTitle"].as_str().unwrap_or(&orig.title).to_string();
                let parent = match (b["destination"]["type"].as_str(), b["destination"]["value"].as_str()) {
                    (Some("parent_page"), Some(p)) if d.pages.contains_key(p) => Some(p.to_string()),
                    (Some("space"), Some("DEV")) => None,
                    _ => return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "invalid destination" })),
                };
                if d.pages.values().any(|p| p.title == title && p.status != "trashed") {
                    return j(StatusCode::BAD_REQUEST, json!({ "statusCode": 400, "message": "A page with this title already exists" }));
                }
                d.next_id += 1;
                let new_id = d.next_id.to_string();
                let storage = orig.versions.last().map(|v| v.1.clone()).unwrap_or_default();
                let copy = Page {
                    id: new_id.clone(),
                    title: title.clone(),
                    parent_id: parent,
                    position: 98,
                    versions: vec![(1, storage, "Copied".into(), now())],
                    labels: if b["copyLabels"] == true { orig.labels.clone() } else { vec![] },
                    ..orig
                };
                d.pages.insert(new_id.clone(), copy);
                if b["copyAttachments"] == true {
                    let copies: Vec<Attachment> = d.attachments.iter().filter(|a| a.page_id == *id && a.status == "current").cloned().collect();
                    for mut a in copies {
                        d.next_id += 1;
                        a.id = format!("att{}", d.next_id);
                        a.page_id = new_id.clone();
                        d.attachments.push(a);
                    }
                }
                j(StatusCode::OK, json!({ "id": new_id, "type": "page", "status": "current", "title": title, "version": { "number": 1 },
                    "space": { "key": "DEV" }, "_links": { "webui": format!("/spaces/DEV/pages/{new_id}") } }))
            }
            (Method::DELETE, ["wiki", "api", "v2", "pages", id]) => match d.pages.get_mut(*id) {
                Some(p) if p.status == "current" || p.status == "archived" => {
                    p.status = "trashed".into();
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => not_found(),
            },
            // ------------------------------------------------ Jira
            (_, ["rest", "api", "3", ..]) if !d.jira => j(StatusCode::NOT_FOUND, json!({ "errorMessage": "Page not found" })),
            (Method::GET, ["rest", "api", "3", "serverInfo"]) => j(StatusCode::OK, json!({ "serverTitle": "Mock Jira", "deploymentType": "Cloud" })),
            (Method::GET, ["rest", "api", "3", "myself"]) => j(StatusCode::OK, json!({ "accountId": "u1", "displayName": "Mock User", "emailAddress": "me@example.com" })),
            (Method::GET, ["rest", "api", "3", "project", "search"]) => {
                j(StatusCode::OK, json!({ "values": [{ "id": "1", "key": "WB", "name": "Workbench" }], "isLast": true }))
            }
            (Method::GET, ["rest", "api", "3", "search", "jql"]) => {
                if qs.get("fields").is_none() {
                    return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["fields missing (the mock insists)"] }));
                }
                let jql = qs.get("jql").cloned().unwrap_or_default();
                if jql.contains("BROKEN") {
                    return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["Error in the JQL Query: unexpected BROKEN"] }));
                }
                let max: usize = qs.get("maxResults").and_then(|m| m.parse().ok()).unwrap_or(50);
                let offset: usize = qs.get("nextPageToken").and_then(|t| t.strip_prefix("tok-")).and_then(|t| t.parse().ok()).unwrap_or(0);
                let all: Vec<Value> = d
                    .issues
                    .values()
                    .filter(|i| !jql.contains("statusCategory != Done") || i.status.1 != "done")
                    .map(|i| self.issue_json(i, &base))
                    .collect();
                let page: Vec<Value> = all.iter().skip(offset).take(max).cloned().collect();
                let last = offset + max >= all.len();
                let mut out = json!({ "issues": page, "isLast": last });
                if !last {
                    out["nextPageToken"] = json!(format!("tok-{}", offset + max));
                }
                j(StatusCode::OK, out)
            }
            (Method::GET, ["rest", "api", "3", "issue", "createmeta", "WB", "issuetypes"]) => j(
                StatusCode::OK,
                json!({ "issueTypes": [{ "id": "10001", "name": "Task", "subtask": false }, { "id": "10002", "name": "Bug", "subtask": false }, { "id": "10003", "name": "Sub-task", "subtask": true }] }),
            ),
            (Method::POST, ["rest", "api", "3", "issue"]) => {
                let b = body.unwrap_or(Value::Null);
                let n = d.issues.len() + 1;
                let key = format!("WB-{n}");
                d.issues.insert(
                    key.clone(),
                    Issue {
                        key: key.clone(),
                        summary: b["fields"]["summary"].as_str().unwrap_or("").into(),
                        description: b["fields"]["description"].clone(),
                        status: ("To Do".into(), "new".into()),
                        labels: vec![],
                        assignee: None,
                        comments: vec![],
                        sprint: None,
                    },
                );
                j(StatusCode::CREATED, json!({ "id": n.to_string(), "key": key, "self": format!("{base}/rest/api/3/issue/{n}") }))
            }
            (Method::GET, ["rest", "api", "3", "issue", key]) => match d.issues.get(*key) {
                Some(i) => j(StatusCode::OK, self.issue_json(i, &base)),
                None => j(StatusCode::NOT_FOUND, json!({ "errorMessages": ["Issue does not exist or you do not have permission to see it."] })),
            },
            (Method::PUT, ["rest", "api", "3", "issue", key]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(i) = d.issues.get_mut(*key) else { return not_found() };
                if let Some(s) = b["fields"]["summary"].as_str() {
                    i.summary = s.into();
                }
                if b["fields"].get("description").is_some() {
                    i.description = b["fields"]["description"].clone();
                }
                if let Some(l) = b["fields"]["labels"].as_array() {
                    i.labels = l.iter().filter_map(Value::as_str).map(str::to_string).collect();
                }
                StatusCode::NO_CONTENT.into_response()
            }
            (Method::PUT, ["rest", "api", "3", "issue", key, "assignee"]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(i) = d.issues.get_mut(*key) else { return not_found() };
                i.assignee = b["accountId"].as_str().map(str::to_string);
                StatusCode::NO_CONTENT.into_response()
            }
            (Method::POST, ["rest", "api", "3", "issue", key, "transitions"]) => {
                let b = body.unwrap_or(Value::Null);
                let Some(i) = d.issues.get_mut(*key) else { return not_found() };
                i.status = match b["transition"]["id"].as_str() {
                    Some("11") => ("To Do".into(), "new".into()),
                    Some("21") => ("In Progress".into(), "indeterminate".into()),
                    Some("31") => ("Done".into(), "done".into()),
                    _ => return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["no such transition"] })),
                };
                if let Some(c) = b.pointer("/update/comment/0/add/body") {
                    i.comments.push(("5999".into(), c.clone(), now()));
                }
                StatusCode::NO_CONTENT.into_response()
            }
            (Method::GET, ["rest", "api", "3", "issue", key, "comment"]) => {
                let Some(i) = d.issues.get(*key) else { return not_found() };
                let list: Vec<Value> = i
                    .comments
                    .iter()
                    .map(|(id, adf, at)| {
                        json!({ "id": id, "author": { "accountId": "u1", "displayName": "Mock User" }, "created": at, "updated": at, "body": adf,
                                "renderedBody": format!("<p>{}</p>", super::html::escape(&markdown::adf_to_markdown(adf))) })
                    })
                    .collect();
                j(StatusCode::OK, json!({ "startAt": 0, "maxResults": 100, "total": list.len(), "comments": list }))
            }
            (Method::POST, ["rest", "api", "3", "issue", key, "comment"]) => {
                let b = body.unwrap_or(Value::Null);
                d.next_id += 1;
                let id = d.next_id.to_string();
                let Some(i) = d.issues.get_mut(*key) else { return not_found() };
                i.comments.push((id.clone(), b["body"].clone(), now()));
                j(StatusCode::CREATED, json!({ "id": id }))
            }
            (Method::GET, ["rest", "api", "3", "attachment", "content" | "thumbnail", _id]) => {
                (StatusCode::FOUND, [(header::LOCATION, format!("{base}/media/jira"))]).into_response()
            }
            (Method::GET, ["rest", "api", "3", "issue", key, "transitions"]) => match d.issues.get(*key) {
                Some(_) => j(StatusCode::OK, json!({ "expand": "transitions", "transitions": transitions_json() })),
                None => not_found(),
            },
            // ------------------------------------------------ Jira Software (agile)
            (_, ["rest", "agile", ..]) if !d.jira => j(StatusCode::NOT_FOUND, json!({ "errorMessages": ["Page not found"] })),
            (Method::GET, ["rest", "agile", "1.0", "board"]) => {
                let name = qs.get("name").map(|n| n.to_lowercase());
                let start: usize = qs.get("startAt").and_then(|s| s.parse().ok()).unwrap_or(0);
                let max: usize = qs.get("maxResults").and_then(|s| s.parse().ok()).unwrap_or(50);
                let all: Vec<Value> = [1, 2]
                    .into_iter()
                    .filter_map(board_json)
                    .filter(|b| name.as_ref().is_none_or(|n| b["name"].as_str().unwrap_or("").to_lowercase().contains(n)))
                    .filter(|b| qs.get("projectKeyOrId").is_none_or(|p| b["location"]["projectKey"] == p.as_str()))
                    .collect();
                let page: Vec<Value> = all.iter().skip(start).take(max).cloned().collect();
                j(StatusCode::OK, json!({ "startAt": start, "maxResults": max, "total": all.len(), "isLast": start + max >= all.len(), "values": page }))
            }
            (Method::GET, ["rest", "agile", "1.0", "board", id]) => match id.parse().ok().and_then(board_json) {
                Some(b) => j(StatusCode::OK, b),
                None => j(StatusCode::NOT_FOUND, json!({ "errorMessages": ["Board does not exist or you do not have permission to see it."] })),
            },
            (Method::GET, ["rest", "agile", "1.0", "board", id, "configuration"]) => {
                if id.parse().ok().and_then(board_json).is_none() {
                    return not_found();
                }
                let col = |name: &str, ids: &[&str]| json!({ "name": name, "statuses": ids.iter().map(|i| json!({ "id": i, "self": format!("{base}/rest/api/2/status/{i}") })).collect::<Vec<_>>() });
                j(
                    StatusCode::OK,
                    json!({ "id": id.parse::<u64>().unwrap_or(0), "name": "config", "filter": { "id": "10000" },
                        "columnConfig": { "columns": [col("To Do", &["10000"]), col("In Progress", &["3"]), col("Done", &["10001"])], "constraintType": "issueCount" },
                        "estimation": { "type": "field", "field": { "fieldId": "customfield_10016", "displayName": "Story point estimate" } } }),
                )
            }
            (Method::GET, ["rest", "agile", "1.0", "board", id, "quickfilter"]) => {
                let values = if *id == "1" { vec![json!({ "id": 1, "boardId": 1, "name": "Only my issues", "jql": "assignee = currentUser()", "position": 0 })] } else { vec![] };
                j(StatusCode::OK, json!({ "startAt": 0, "maxResults": 50, "total": values.len(), "isLast": true, "values": values }))
            }
            (Method::GET, ["rest", "agile", "1.0", "board", id, "sprint"]) => {
                if *id != "1" {
                    return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["The board does not support sprints"] }));
                }
                let states: Vec<String> = qs.get("state").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default();
                let values: Vec<Value> =
                    sprints_json().into_iter().filter(|s| states.is_empty() || states.iter().any(|x| s["state"] == x.as_str())).collect();
                j(StatusCode::OK, json!({ "startAt": 0, "maxResults": 50, "isLast": true, "values": values }))
            }
            (Method::GET, ["rest", "agile", "1.0", "board", id, rest @ ..]) if id.parse().ok().and_then(board_json).is_some() => {
                let scope = match rest {
                    ["issue"] => None,
                    ["backlog"] => Some(None),
                    ["sprint", sid, "issue"] => match sid.parse::<u64>() {
                        Ok(n) => Some(Some(n)),
                        Err(_) => return not_found(),
                    },
                    _ => return not_found(),
                };
                if qs.get("fields").is_none() {
                    return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["fields missing (the mock insists)"] }));
                }
                let jql = qs.get("jql").cloned().unwrap_or_default();
                if jql.contains("BROKEN") {
                    return j(StatusCode::BAD_REQUEST, json!({ "errorMessages": ["Error in the JQL Query"] }));
                }
                let start: usize = qs.get("startAt").and_then(|s| s.parse().ok()).unwrap_or(0);
                let max: usize = qs.get("maxResults").and_then(|s| s.parse().ok()).unwrap_or(50);
                let all: Vec<Value> = d
                    .issues
                    .values()
                    .filter(|i| match scope {
                        None => true,
                        Some(None) => i.sprint.is_none(),
                        Some(Some(s)) => i.sprint == Some(s),
                    })
                    .filter(|i| !jql.contains("assignee = currentUser()") || i.assignee.as_deref() == Some("u1"))
                    .map(|i| self.issue_json(i, &base))
                    .collect();
                let page: Vec<Value> = all.iter().skip(start).take(max).cloned().collect();
                j(StatusCode::OK, json!({ "startAt": start, "maxResults": max, "total": all.len(), "issues": page }))
            }
            _ => not_found(),
        }
    }
}

async fn mock_handler(State(m): State<Mock>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    // Everything except the media host requires the right Basic credentials; like the
    // real v2 API, a failure there is reported as 404.
    if !uri.path().starts_with("/media/") {
        let want = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("me@example.com:{MOCK_TOKEN}"))
        );
        let ok = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) == Some(want.as_str());
        if !ok {
            return if uri.path().starts_with("/wiki/api/v2/") {
                j(StatusCode::NOT_FOUND, json!({ "errors": [{ "status": 404, "code": "NOT_FOUND" }] }))
            } else {
                j(StatusCode::UNAUTHORIZED, json!({ "message": "Client must be authenticated" }))
            };
        }
    }
    let json = if body.is_empty() { None } else { serde_json::from_slice(&body).ok() };
    m.handle(method, uri, &headers, &body, json)
}

/// Start the mock on `addr`; returns its base URL and shared state.
pub async fn start_mock(addr: &str) -> (String, Mock) {
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind mock");
    let local: SocketAddr = listener.local_addr().unwrap();
    let base = format!("http://{local}");
    let mock = Mock { data: Arc::new(Mutex::new(MockData::seeded())), base: Arc::new(Mutex::new(base.clone())) };
    let app = Router::new().fallback(mock_handler).with_state(mock.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    (base, mock)
}

/// An isolated AppState whose `[atlassian]` points at `site`.
async fn state_for(site: &str, dir: &tempfile::TempDir) -> AppState {
    let token_file = dir.path().join("atlassian_token");
    std::fs::write(&token_file, MOCK_TOKEN).unwrap();
    crate::util::fs::set_mode(&token_file, 0o600);
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.atlassian = Some(AtlassianConfig { site: site.into(), email: "me@example.com".into(), token: "atlassian".into() });
    cfg.secrets.insert("atlassian".into(), SecretRef::File(token_file.display().to_string()));
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap()
}

fn puts(m: &Mock) -> Vec<Value> {
    m.data.lock().requests.iter().filter(|r| r.0 == "PUT").filter_map(|r| r.2.clone()).collect()
}

#[tokio::test]
async fn page_update_checks_versions_and_markers() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();

    let page = confluence::get_page(&state, &api, "2001", None, None).await.unwrap();
    assert_eq!(page.version.number, 2);
    assert_eq!(page.inline_marker_refs, vec!["m-1"]);
    assert_eq!(page.ancestors.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Dev Home"]);
    assert_eq!(page.space_key.as_deref(), Some("DEV"));
    assert_eq!(page.labels, ["design", "backend"]);
    assert!(page.html.contains(r#"src="/api/confluence/attachments/2001/att7001?v=1""#), "{}", page.html);
    assert!(page.html.contains(r#"data-wb-page="2002""#));
    assert!(page.web_url.starts_with(&format!("{base}/wiki/spaces/DEV/pages/2001")));

    // No base version → refused before anything is read or written (it could overwrite a newer version).
    let before = mock.data.lock().requests.len();
    let blind = confluence::UpdateIn { storage: Some(page.storage.clone() + "<p>x</p>"), ..Default::default() };
    let e = confluence::update_page(&state, &api, "2001", blind).await.unwrap_err();
    assert_eq!((e.status, e.code), (StatusCode::BAD_REQUEST, "bad_request"));
    assert!(e.message.contains("version is required"), "{}", e.message);
    assert_eq!(mock.data.lock().requests.len(), before);

    // Stale base version → conflict, nothing written.
    let stale = confluence::UpdateIn { storage: Some(page.storage.clone() + "<p>x</p>"), version: Some(1), ..Default::default() };
    let e = confluence::update_page(&state, &api, "2001", stale).await.unwrap_err();
    assert_eq!((e.status, e.code), (StatusCode::CONFLICT, "conflict"));
    assert!(e.message.contains("version 2"), "{}", e.message);

    // Dropping the inline-comment marker → refused unless forced.
    let dropped = page.storage.replace(r#"<ac:inline-comment-marker ac:ref="m-1">"#, "").replace("</ac:inline-comment-marker>", "");
    let e = confluence::update_page(&state, &api, "2001", confluence::UpdateIn { storage: Some(dropped.clone()), version: Some(2), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(e.code, "inline_comments");
    assert!(puts(&mock).is_empty());

    // Malformed storage is refused before any request.
    let e = confluence::update_page(&state, &api, "2001", confluence::UpdateIn { storage: Some("<p>open".into()), version: Some(2), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(e.code, "bad_request");

    // Unchanged content → no new version.
    let same = confluence::update_page(&state, &api, "2001", confluence::UpdateIn { storage: Some(page.storage.clone()), version: Some(2), ..Default::default() })
        .await
        .unwrap();
    assert!(same.unchanged);
    assert!(puts(&mock).is_empty());

    // A marker-preserving edit goes through as version 3 with the message.
    let edited = page.storage.replace("written in Rust", "written in Rust 2024");
    let mut events = state.events.subscribe();
    let out = confluence::update_page(
        &state,
        &api,
        "2001",
        confluence::UpdateIn { storage: Some(edited.clone()), version: Some(2), message: Some("tweak".into()), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!((out.version, out.unchanged), (3, false));
    let put = puts(&mock).pop().unwrap();
    assert_eq!(put["version"]["number"], 3);
    assert_eq!(put["version"]["message"], "tweak");
    assert_eq!(put["body"], json!({ "representation": "storage", "value": edited }));
    assert_eq!(put["title"], "Architecture");
    let ev = events.recv().await.unwrap();
    assert_eq!(ev.kind, "confluence.page");
    assert_eq!(ev.data["version"], 3);

    // Forcing drops the marker.
    let forced = confluence::update_page(
        &state,
        &api,
        "2001",
        confluence::UpdateIn { storage: Some(dropped), version: Some(3), force: true, title: Some("Architecture v2".into()), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(forced.version, 4);
    assert_eq!(forced.title, "Architecture v2");

    // History: versions newest first, and an old version's storage.
    let vs = confluence::versions(&state, &api, "2001", None, 2).await.unwrap();
    assert_eq!(vs.results.iter().map(|v| v.number).collect::<Vec<_>>(), [4, 3]);
    assert_eq!(vs.results[0].author_name.as_deref(), Some("Mock User"));
    let more = confluence::versions(&state, &api, "2001", vs.next_cursor.as_deref(), 2).await.unwrap();
    assert_eq!(more.results.iter().map(|v| v.number).collect::<Vec<_>>(), [2, 1]);
    assert!(more.next_cursor.is_none());
    let v1 = confluence::version_storage(&state, &api, "2001", 1).await.unwrap();
    assert!(v1.storage.contains("First draft"));
}

#[tokio::test]
async fn create_comment_tree_and_search() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();

    let created = confluence::create_page(
        &state,
        &api,
        confluence::CreateIn { space_key: Some("DEV".into()), parent_id: Some("2002".into()), title: " New page ".into(), markdown: Some("# Hi\n\n```sh\nls\n```".into()), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(created.title, "New page");
    let post = mock.data.lock().requests.iter().rev().find(|r| r.0 == "POST").and_then(|r| r.2.clone()).unwrap();
    assert_eq!(post["spaceId"], "1000");
    assert_eq!(post["parentId"], "2002");
    assert!(post["body"]["value"].as_str().unwrap().contains(r#"<ac:parameter ac:name="language">bash</ac:parameter>"#));

    // Threads without replies (highlight states) cost no per-thread requests.
    let before = mock.data.lock().requests.len();
    let light = confluence::comments_with(&state, &api, "2001", None, false).await.unwrap();
    assert!(light.footer[0].replies.is_empty() && light.inline.len() == 1);
    assert!(mock.data.lock().requests[before..].iter().all(|r| !r.1.contains("/children")));

    let comments = confluence::comments_with(&state, &api, "2001", None, true).await.unwrap();
    assert_eq!(comments.footer.len(), 1);
    assert_eq!(comments.footer[0].replies.len(), 1);
    assert_eq!(comments.inline[0].selection.as_deref(), Some("This sentence has a comment."));
    assert_eq!(comments.inline[0].resolution_status.as_deref(), Some("open"));
    assert_eq!(comments.footer[0].author_name.as_deref(), Some("Mock User"));

    confluence::add_comment(&state, &api, "2001", confluence::AddCommentIn { markdown: Some("**Nice** `x`".into()), ..Default::default() })
        .await
        .unwrap();
    let post = mock.data.lock().requests.last().and_then(|r| r.2.clone()).unwrap();
    assert_eq!(post, json!({ "pageId": "2001", "body": { "representation": "storage", "value": "<p><strong>Nice</strong> <code>x</code></p>" } }));
    confluence::add_comment(
        &state,
        &api,
        "2001",
        confluence::AddCommentIn { markdown: Some("reply".into()), parent_comment_id: Some("3003".into()), parent_kind: Some("inline".into()), ..Default::default() },
    )
    .await
    .unwrap();
    let last = mock.data.lock().requests.last().cloned().unwrap();
    assert!(last.1.ends_with("/wiki/api/v2/inline-comments"));
    assert_eq!(last.2.unwrap()["parentCommentId"], "3003");

    // Tree: children with hasChildren from one descendants call; folders included.
    let kids = confluence::children(&api, "2000", "page", false).await.unwrap();
    let titles: Vec<(&str, Option<bool>)> = kids.children.iter().map(|k| (k.title.as_str(), k.has_children)).collect();
    assert_eq!(titles, [("Architecture", Some(true)), ("Runbook", Some(true)), ("Drafts", Some(false))]);
    assert_eq!(kids.children[2].kind, "folder");
    let roots = confluence::space_root_pages(&api, "1000", "current").await.unwrap();
    assert_eq!(roots.children.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Dev Home"]);
    let archived = confluence::space_root_pages(&api, "1000", "archived").await.unwrap();
    assert_eq!(archived.children.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Old notes"]);

    // Cursor pagination is followed (tiny pages from the mock).
    let (all, truncated) = api
        .v2_all::<Value>(api.wiki("/api/v2/pages/2000/descendants?depth=2&limit=2"), 100)
        .await
        .unwrap();
    assert_eq!((all.len(), truncated), (5, false));
    let (some, truncated) = api.v2_all::<Value>(api.wiki("/api/v2/pages/2000/descendants?depth=2&limit=2"), 3).await.unwrap();
    assert_eq!((some.len(), truncated), (4, true));

    // Search: markers stripped, entities decoded, archived filtered unless asked.
    let s = confluence::search(&api, &confluence::SearchIn { q: Some("notes".into()), ..Default::default() }).await.unwrap();
    assert!(s.results.is_empty(), "archived pages are hidden by default");
    let s = confluence::search(&api, &confluence::SearchIn { q: Some("notes".into()), archived: Some(true), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(s.results[0].title, "Old notes");
    assert_eq!(s.results[0].status, "archived");
    assert_eq!(s.results[0].excerpt, "…mock & excerpt for Old notes…");
    assert_eq!(s.results[0].space_key.as_deref(), Some("DEV"));
}

#[tokio::test]
async fn client_retries_redacts_and_stays_on_site() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();

    // 429 with Retry-After: 0 is retried.
    let v: Value = api.get(&api.url("/flaky")).await.unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(mock.data.lock().requests.iter().filter(|r| r.1 == "/flaky").count(), 2);

    // Upstream messages never echo the token.
    let e = api.get::<Value>(&api.url("/leak")).await.unwrap_err();
    assert!(!e.message.contains(MOCK_TOKEN), "{}", e.message);
    assert!(e.message.contains("••••"));

    // Pagination links to other hosts are refused.
    assert!(api.follow_link("https://evil.example/wiki/api/v2/pages?cursor=x", "").is_err());
    assert!(api.follow_link("/wiki/api/v2/pages?cursor=x", "").unwrap().starts_with(&base));

    // Bad credentials: v2 reports 404, v1 401 → not_configured.
    let tf = dir.path().join("bad_token");
    std::fs::write(&tf, "wrong").unwrap();
    crate::util::fs::set_mode(&tf, 0o600);
    state.config.write().secrets.insert("atlassian".into(), SecretRef::File(tf.display().to_string()));
    state.secrets.clear_cache();
    let st = super::status::check(&state, None, true).await.unwrap();
    assert!(st.auth_failed && !st.confluence, "{st:?}");
    let api = api_for(&state, None, Product::Confluence).unwrap();
    let e = confluence::get_page(&state, &api, "2001", None, None).await.unwrap_err();
    assert_eq!(e.code, "not_configured", "{e}");

    // Attachment proxy follows the media redirect and caches the bytes.
    state.config.write().secrets.insert("atlassian".into(), SecretRef::File(dir.path().join("atlassian_token").display().to_string()));
    state.secrets.clear_cache();
    let api = api_for(&state, None, Product::Confluence).unwrap();
    let url = api.wiki("/rest/api/content/2001/child/attachment/att7001/download");
    for _ in 0..2 {
        let resp = super::attachments::proxy(&api, &state.atlassian.attachments, &url, "k".into(), None, false, true).await.unwrap();
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert!(resp.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("sandbox"));
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("mock diagram"));
    }
    let downloads = mock.data.lock().requests.iter().filter(|r| r.1.ends_with("/download")).count();
    assert_eq!(downloads, 1, "second response came from the cache");
}

#[tokio::test]
async fn status_follows_credential_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let checks = || mock.data.lock().requests.iter().filter(|r| r.1 == "/wiki/rest/api/user/current").count();
    let use_token_file = |name: &str, value: &str| {
        let f = dir.path().join(name);
        std::fs::write(&f, value).unwrap();
        crate::util::fs::set_mode(&f, 0o600);
        state.config.write().secrets.insert("atlassian".into(), SecretRef::File(f.display().to_string()));
        // What applying a settings change does.
        state.secrets.clear_cache();
    };

    use_token_file("bad", "wrong");
    let st = super::status::check(&state, None, false).await.unwrap();
    assert!(st.auth_failed && !st.confluence, "{st:?}");
    // Cached: asking again does not hit the site.
    let n = checks();
    assert!(super::status::check(&state, None, false).await.unwrap().auth_failed);
    assert_eq!(checks(), n);

    // The token is fixed: the next plain (non-refresh) check sees it at once.
    use_token_file("good", MOCK_TOKEN);
    let st = super::status::check(&state, None, false).await.unwrap();
    assert!(st.confluence && !st.auth_failed, "{st:?}");
    assert_eq!(checks(), n + 1);
    let api = api_for(&state, None, Product::Confluence).unwrap();
    assert!(!api.auth_known_bad, "the old token's failure does not taint the new one");
}

#[tokio::test]
async fn status_endpoint_answers_not_configured_as_data() {
    use tower::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let (base, _mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let app = super::router().with_state(state.clone());
    let get = |app: Router, uri: &str| {
        let req = axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap();
        async move {
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<Value>(&body).unwrap())
        }
    };

    let (code, v) = get(app.clone(), "/api/atlassian/status?refresh=1").await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!((v["configured"].as_bool(), v["confluence"].as_bool()), (Some(true), Some(true)), "{v}");

    // No [atlassian] at all: 200 with configured=false and the setup help, not a 412.
    state.config.write().atlassian = None;
    let (code, v) = get(app.clone(), "/api/atlassian/status").await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["configured"], false);
    assert_eq!((v["confluence"].as_bool(), v["jira"].as_bool(), v["authFailed"].as_bool()), (Some(false), Some(false), Some(false)));
    assert!(v["error"].as_str().unwrap().contains("[atlassian]"), "{v}");

    // Other routes still answer 412 not_configured (the UI shows setup help from it).
    let (code, v) = get(app, "/api/confluence/spaces").await;
    assert_eq!(code, StatusCode::PRECONDITION_FAILED);
    assert_eq!(v["error"]["code"], "not_configured");
}

#[tokio::test]
async fn jira_flows_and_detection() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let st = super::status::check(&state, None, true).await.unwrap();
    assert!(st.confluence && st.jira, "{st:?}");
    assert_eq!(st.jira_title.as_deref(), Some("Mock Jira"));
    let api = jira_api(&state, None).await.unwrap();

    let first = jira::search(&api, &jira::SearchIn { jql: "project = WB".into(), max_results: Some(2), ..Default::default() }).await.unwrap();
    assert_eq!(first.issues.len(), 2);
    assert!(!first.is_last);
    let second = jira::search(&api, &jira::SearchIn { jql: "project = WB".into(), max_results: Some(2), next_page_token: first.next_page_token.clone(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(second.issues.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["WB-3"]);
    assert!(second.is_last);
    let e = jira::search(&api, &jira::SearchIn { jql: "BROKEN".into(), ..Default::default() }).await.unwrap_err();
    assert!(e.message.starts_with("Invalid JQL"), "{}", e.message);

    let issue = jira::get_issue(&api, "wb-1", None).await.unwrap();
    assert_eq!(issue.summary.key, "WB-1");
    assert_eq!(issue.transitions.len(), 3);
    assert!(issue.editable.summary && issue.editable.description);
    assert_eq!(issue.editable.priorities.len(), 3);
    assert!(issue.description_markdown.contains("Tables should use **tokens**."));
    assert!(!issue.description_lossy);
    assert!(issue.description_html.contains(r#"src="/api/jira/attachments/10001""#), "{}", issue.description_html);
    assert_eq!(issue.comments.len(), 1);

    jira::update_issue(&state, &api, "WB-1", jira::UpdateIn { summary: Some("Render tables".into()), description: Some("New **text**".into()), ..Default::default() })
        .await
        .unwrap();
    let put = mock.data.lock().requests.iter().rev().find(|r| r.0 == "PUT").and_then(|r| r.2.clone()).unwrap();
    assert_eq!(put["fields"]["summary"], "Render tables");
    assert_eq!(put["fields"]["description"]["content"][0]["content"][1], json!({ "type": "text", "text": "text", "marks": [{ "type": "strong" }] }));

    jira::transition(&state, &api, "WB-1", "21", Some("Starting")).await.unwrap();
    let issue = jira::get_issue(&api, "WB-1", None).await.unwrap();
    assert_eq!(issue.summary.status.unwrap().name, "In Progress");
    assert_eq!(issue.comments.len(), 2);

    jira::add_comment(&state, &api, "WB-1", "Done with `x`").await.unwrap();
    let post = mock.data.lock().requests.last().and_then(|r| r.2.clone()).unwrap();
    assert_eq!(post["body"]["type"], "doc");

    let created = jira::create_issue(&state, &api, jira::CreateIn { project_key: "wb".into(), summary: "From test".into(), issue_type: Some("bug".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(created["key"], "WB-4");
    let post = mock.data.lock().requests.iter().rev().find(|r| r.1 == "/rest/api/3/issue").and_then(|r| r.2.clone()).unwrap();
    assert_eq!(post["fields"]["issuetype"]["id"], "10002");
    assert_eq!(post["fields"]["project"]["key"], "WB");

    // A site without Jira: detection turns it off and Jira calls are not_configured.
    mock.data.lock().jira = false;
    let st = super::status::check(&state, None, true).await.unwrap();
    assert!(st.confluence && !st.jira);
    let e = jira_api(&state, None).await.err().unwrap();
    assert_eq!(e.code, "not_configured");
}

#[tokio::test]
async fn mcp_tools_against_the_mock() {
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let find = |name: &str| super::tools::all().into_iter().find(|t| t.name == name).unwrap();
    let ctx = crate::mcp::McpCtx::default();
    let text = |o: crate::mcp::ToolOutput| match o {
        crate::mcp::ToolOutput::Text(t) => t,
        crate::mcp::ToolOutput::Json(v) => v.to_string(),
    };

    let t = text((find("confluence_get_page").handler)(state.clone(), ctx.clone(), json!({ "pageId": format!("{base}/wiki/spaces/DEV/pages/2001/Architecture") })).await.unwrap());
    assert!(t.contains("Title: Architecture") && t.contains("baseVersion=2") && t.contains("Inline comments: 1 marker"), "{t}");
    assert!(t.contains("| Name | Language |"), "{t}");
    let tree = text((find("confluence_page_tree").handler)(state.clone(), ctx.clone(), json!({ "spaceKey": "DEV", "depth": 2 })).await.unwrap());
    assert!(tree.contains("- Dev Home [2000]\n  - Architecture [2001]\n    - API [2004]"), "{tree}");
    let e = (find("confluence_update_page").handler)(state.clone(), ctx.clone(), json!({ "pageId": "2001", "markdown": "# Replaced", "baseVersion": 2 }))
        .await
        .unwrap_err();
    assert_eq!(e.code, "inline_comments", "markdown rewrites cannot keep markers");
    // Without baseVersion the tool refuses (and says how to get it); nothing is written.
    let upd = find("confluence_update_page");
    assert!(upd.input_schema["required"].as_array().unwrap().contains(&json!("baseVersion")));
    for args in [json!({ "pageId": "2002", "markdown": "# Blind" }), json!({ "pageId": "2002", "markdown": "# Blind", "baseVersion": 0 })] {
        let e = (upd.handler)(state.clone(), ctx.clone(), args).await.unwrap_err();
        assert_eq!(e.code, "bad_request");
        assert!(e.message.contains("confluence_get_page"), "{}", e.message);
    }
    assert!(puts(&mock).is_empty());
    let ok =text((find("confluence_update_page").handler)(state.clone(), ctx.clone(), json!({ "pageId": "2002", "markdown": "# Runbook\n\n1. Build", "baseVersion": 1 })).await.unwrap());
    assert!(ok.starts_with("Updated “Runbook” to version 2"), "{ok}");
    let s = text((find("jira_search").handler)(state.clone(), ctx.clone(), json!({ "jql": "project = WB AND statusCategory != Done" })).await.unwrap());
    assert!(s.contains("WB-1 [To Do]") && !s.contains("WB-3"), "{s}");
    let tr = text((find("jira_transition").handler)(state.clone(), ctx.clone(), json!({ "key": "WB-2", "transition": "done" })).await.unwrap());
    assert!(tr.contains("WB-2: Done done."), "{tr}");
}

fn last_request(m: &Mock, method: &str, path_end: &str) -> Option<(String, Option<Value>)> {
    m.data.lock().requests.iter().rev().find(|r| r.0 == method && r.1.split('?').next().unwrap_or("").ends_with(path_end)).map(|r| (r.1.clone(), r.2.clone()))
}

fn count_requests(m: &Mock, method: &str) -> usize {
    m.data.lock().requests.iter().filter(|r| r.0 == method).count()
}

#[tokio::test]
async fn inline_comments_resolve_edit_and_delete() {
    use super::comments::{self, CreateInlineIn, UpdateCommentIn};
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();
    let mut events = state.events.subscribe();

    // A selection that occurs once; the UI's count agrees with the page.
    let input = CreateInlineIn { markdown: Some("Which **edition**?".into()), selection: "written in Rust".into(), match_index: Some(0), match_count: Some(1), ..Default::default() };
    let created = comments::create_inline(&state, &api, "2001", input, None).await.unwrap();
    assert_eq!((created.match_count, created.match_index), (1, 0));
    let (_, post) = last_request(&mock, "POST", "/wiki/api/v2/inline-comments").unwrap();
    let post = post.unwrap();
    assert_eq!(post["pageId"], "2001");
    assert_eq!(post["inlineCommentProperties"], json!({ "textSelection": "written in Rust", "textSelectionMatchCount": 1, "textSelectionMatchIndex": 0 }));
    assert_eq!(post["body"], json!({ "representation": "storage", "value": "<p>Which <strong>edition</strong>?</p>" }));
    assert_eq!(events.recv().await.unwrap().data["action"], "commented");
    // Confluence anchored it: the page now carries a second marker.
    let page = confluence::get_page(&state, &api, "2001", None, None).await.unwrap();
    assert_eq!(page.inline_marker_refs.len(), 2, "{:?}", page.inline_marker_refs);
    assert!(page.html.contains(&format!(r#"data-ref="{}""#, created.marker_ref.clone().unwrap())));

    // "Rust" occurs twice (text and table): no index → refused; a stale count → 409; nothing posted.
    let posts = count_requests(&mock, "POST");
    let ambiguous = CreateInlineIn { markdown: Some("x".into()), selection: "Rust".into(), ..Default::default() };
    let e = comments::create_inline(&state, &api, "2001", ambiguous, None).await.unwrap_err();
    assert!(e.message.contains("occurs 2 times"), "{}", e.message);
    let stale = CreateInlineIn { markdown: Some("x".into()), selection: "Rust".into(), match_index: Some(0), match_count: Some(3), ..Default::default() };
    assert_eq!(comments::create_inline(&state, &api, "2001", stale, None).await.unwrap_err().code, "conflict");
    let multi = CreateInlineIn { markdown: Some("x".into()), selection: "two\nlines".into(), ..Default::default() };
    assert_eq!(comments::create_inline(&state, &api, "2001", multi, None).await.unwrap_err().code, "bad_request");
    assert_eq!(count_requests(&mock, "POST"), posts);
    // The second occurrence is anchored when asked for.
    let second = CreateInlineIn { markdown: Some("table cell".into()), selection: "Rust".into(), match_index: Some(1), match_count: Some(2), ..Default::default() };
    comments::create_inline(&state, &api, "2001", second, None).await.unwrap();
    let st = mock.data.lock().pages["2001"].versions.last().unwrap().1.clone();
    assert!(st.contains(r#"<td><p><ac:inline-comment-marker ac:ref="mk-"#), "{st}");

    // Resolve, then reopen: body unchanged, version counted up.
    let id = created.id.clone();
    let out = comments::update_comment(&state, &api, "inline", &id, UpdateCommentIn { resolved: Some(true), ..Default::default() }).await.unwrap();
    assert_eq!((out.version, out.resolution_status.as_deref(), out.page_id.as_deref()), (2, Some("resolved"), Some("2001")));
    let (_, put) = last_request(&mock, "PUT", &format!("/inline-comments/{id}")).unwrap();
    let put = put.unwrap();
    assert_eq!(put["resolved"], true);
    assert_eq!(put["version"]["number"], 2);
    assert_eq!(put["body"]["value"], "<p>Which <strong>edition</strong>?</p>");
    let list = confluence::comments_with(&state, &api, "2001", None, true).await.unwrap();
    let c = list.inline.iter().find(|c| c.id == id).unwrap();
    assert_eq!((c.resolution_status.as_deref(), c.resolved_by.as_deref()), (Some("resolved"), Some("Mock User")));
    assert_eq!((c.markdown.as_str(), c.edit_lossy), ("Which **edition**?", false));
    let out = comments::update_comment(&state, &api, "inline", &id, UpdateCommentIn { resolved: Some(false), ..Default::default() }).await.unwrap();
    assert_eq!(out.resolution_status.as_deref(), Some("reopened"));

    // Edit a footer comment from the version it was read at; a stale version is a 409 without a PUT.
    let puts = count_requests(&mock, "PUT");
    let e = comments::update_comment(&state, &api, "footer", "3001", UpdateCommentIn { markdown: Some("new".into()), version: Some(7), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(e.code, "conflict");
    assert_eq!(count_requests(&mock, "PUT"), puts);
    let e = comments::update_comment(&state, &api, "footer", "3001", UpdateCommentIn { resolved: Some(true), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.code, "bad_request", "footer comments are not resolved");
    let out = comments::update_comment(&state, &api, "footer", "3001", UpdateCommentIn { markdown: Some("**Edited** by [@Ann Lee](mention:u2)".into()), version: Some(1), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(out.version, 2);
    let (_, put) = last_request(&mock, "PUT", "/footer-comments/3001").unwrap();
    assert_eq!(put.unwrap()["body"]["value"], r#"<p><strong>Edited</strong> by <ac:link><ri:user ri:account-id="u2" /></ac:link></p>"#);
    let list = confluence::comments_with(&state, &api, "2001", None, true).await.unwrap();
    assert_eq!(list.footer[0].markdown, "**Edited** by [@Ann Lee](mention:u2)");
    assert!(list.footer[0].html.contains("@Ann Lee"), "{}", list.footer[0].html);

    // Delete the reply, then the whole thread.
    comments::delete_comment(&state, &api, "footer", "3002").await.unwrap();
    assert!(last_request(&mock, "DELETE", "/footer-comments/3002").is_some());
    let list = confluence::comments_with(&state, &api, "2001", None, true).await.unwrap();
    assert!(list.footer[0].replies.is_empty());
    assert_eq!(comments::delete_comment(&state, &api, "footer", "3002").await.unwrap_err().code, "not_found");
    assert_eq!(comments::delete_comment(&state, &api, "blog", "3001").await.unwrap_err().code, "bad_request");
}

#[tokio::test]
async fn attachments_upload_list_download_and_trash() {
    use tower::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();

    let list = super::files::list(&state, &api, "2001", Some("proj")).await.unwrap();
    assert_eq!(list.attachments.len(), 1);
    let a = &list.attachments[0];
    assert_eq!((a.id.as_str(), a.title.as_str(), a.is_image, a.author_name.as_deref()), ("att7001", "diagram.svg", true, Some("Mock User")));
    assert_eq!(a.download_url, "/api/confluence/attachments/2001/att7001?v=1&projectId=proj");

    // Upload through the REST route: streamed, with the XSRF bypass header and minorEdit.
    let app = super::router().with_state(state.clone());
    let upload = |name: &str, body: &'static str, extra: &str, len: Option<usize>| {
        let mut req = axum::http::Request::post(format!("/api/confluence/pages/2001/attachments?name={}{extra}", urlencoding::encode(name)))
            .header(header::CONTENT_TYPE, "text/plain");
        if let Some(n) = len {
            req = req.header(header::CONTENT_LENGTH, n);
        }
        let req = req.body(axum::body::Body::from(body)).unwrap();
        let app = app.clone();
        async move {
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
        }
    };
    let (code, v) = upload("notes v1.txt", "hello notes", "&comment=first", Some(11)).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!((v["title"].as_str(), v["version"].as_u64(), v["fileSize"].as_u64(), v["mediaType"].as_str()), (Some("notes v1.txt"), Some(1), Some(11), Some("text/plain")));
    let (_, rec) = last_request(&mock, "POST", "/child/attachment").unwrap();
    let rec = rec.unwrap();
    assert_eq!(rec["xsrf"], "no-check");
    let parts = rec["parts"].as_array().unwrap();
    assert!(parts.iter().any(|p| p["name"] == "file" && p["filename"] == "notes v1.txt" && p["text"] == "hello notes" && p["contentType"] == "text/plain"), "{rec}");
    assert!(parts.iter().any(|p| p["name"] == "minorEdit" && p["text"] == "true"));
    assert!(parts.iter().any(|p| p["name"] == "comment" && p["text"] == "first"));

    // Same name again: 409 "exists" unless replacing (a new version through PUT).
    let (code, v) = upload("notes v1.txt", "again", "", Some(5)).await;
    assert_eq!((code, v["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("exists")), "{v}");
    let (code, v) = upload("notes v1.txt", "again", "&replace=true", Some(5)).await;
    assert_eq!((code, v["version"].as_u64()), (StatusCode::OK, Some(2)), "{v}");
    assert!(last_request(&mock, "PUT", "/child/attachment").is_some());

    // No length, a lie about the length, a bad name, or too large: refused before anything is sent.
    let before = mock.data.lock().requests.len();
    assert_eq!(upload("x.txt", "abc", "", None).await.0, StatusCode::LENGTH_REQUIRED);
    assert_eq!(upload("a/b.txt", "abc", "", Some(3)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(upload("big.bin", "abc", "", Some(200 * 1024 * 1024)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(mock.data.lock().requests.len(), before);
    let (code, _) = upload("short.txt", "longer than announced", "", Some(4)).await;
    assert_ne!(code, StatusCode::OK, "a body longer than its Content-Length is not forwarded whole");
    assert!(!mock.data.lock().attachments.iter().any(|a| a.title == "short.txt"));

    // The new file downloads through the proxy.
    let att = mock.data.lock().attachments.iter().find(|a| a.title == "notes v1.txt").map(|a| a.id.clone()).unwrap();
    let url = api.wiki(&format!("/rest/api/content/2001/child/attachment/{att}/download"));
    let resp = super::attachments::proxy(&api, &state.atlassian.attachments, &url, format!("k-{att}"), None, true, false).await.unwrap();
    assert!(resp.headers()[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment;"));
    assert_eq!(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()[..], b"again");

    // Delete moves it to the trash; the list no longer shows it.
    let out = super::files::delete(&state, &api, &att).await.unwrap();
    assert_eq!(out["pageId"], "2001");
    assert!(last_request(&mock, "DELETE", &format!("/wiki/api/v2/attachments/{att}")).is_some());
    let list = super::files::list(&state, &api, "2001", None).await.unwrap();
    assert_eq!(list.attachments.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["diagram.svg"]);
    assert_eq!(super::files::delete(&state, &api, "../x").await.unwrap_err().code, "bad_request");
}

#[tokio::test]
async fn labels_move_copy_trash_watch_and_people() {
    use super::pages;
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = api_for(&state, None, Product::Confluence).unwrap();

    // Labels: lowercased, validated before sending, removed by query parameter.
    let now = pages::add_labels(&state, &api, "2001", &["Review".into(), "v2".into()]).await.unwrap();
    assert_eq!(now, ["design", "backend", "review", "v2"]);
    let (_, post) = last_request(&mock, "POST", "/content/2001/label").unwrap();
    assert_eq!(post.unwrap(), json!([{ "prefix": "global", "name": "review" }, { "prefix": "global", "name": "v2" }]));
    let posts = count_requests(&mock, "POST");
    assert_eq!(pages::add_labels(&state, &api, "2001", &["two words".into()]).await.unwrap_err().code, "bad_request");
    assert_eq!(count_requests(&mock, "POST"), posts);
    pages::remove_label(&state, &api, "2001", "design").await.unwrap();
    assert!(last_request(&mock, "DELETE", "/content/2001/label").unwrap().0.ends_with("label?name=design"));
    assert_eq!(pages::labels(&api, "2001").await.unwrap(), ["backend", "review", "v2"]);

    // Move: into another page; before a top-level page is refused without a request; after a sibling.
    let mv = |pos: &str, target: &str| pages::MoveIn { position: pos.into(), target_id: target.into() };
    pages::move_page(&state, &api, "2004", &mv("append", "2002")).await.unwrap();
    assert_eq!(mock.data.lock().pages["2004"].parent_id.as_deref(), Some("2002"));
    let puts = count_requests(&mock, "PUT");
    assert_eq!(pages::move_page(&state, &api, "2004", &mv("before", "2000")).await.unwrap_err().code, "bad_request");
    assert_eq!(pages::move_page(&state, &api, "2004", &mv("sideways", "2002")).await.unwrap_err().code, "bad_request");
    assert_eq!(count_requests(&mock, "PUT"), puts);
    pages::move_page(&state, &api, "2004", &mv("before", "2002")).await.unwrap();
    let kids = confluence::children(&api, "2000", "page", false).await.unwrap();
    assert_eq!(kids.children.iter().map(|k| k.title.as_str()).collect::<Vec<_>>(), ["Architecture", "API", "Runbook", "Drafts"]);
    assert_eq!(pages::move_page(&state, &api, "2000", &mv("append", "2001")).await.unwrap_err().code, "bad_request", "not under its own child");

    // Copy next to the original, with labels; the same title twice is refused by Confluence.
    let copy = pages::copy_page(&state, &api, "2001", pages::CopyIn { copy_labels: true, copy_attachments: true, ..Default::default() }).await.unwrap();
    assert_eq!(copy.title, "Copy of Architecture");
    let (_, post) = last_request(&mock, "POST", "/content/2001/copy").unwrap();
    let post = post.unwrap();
    assert_eq!(post["destination"], json!({ "type": "parent_page", "value": "2000" }));
    assert_eq!((post["copyLabels"].as_bool(), post["copyAttachments"].as_bool(), post["copyPermissions"].as_bool()), (Some(true), Some(true), Some(false)));
    assert_eq!(mock.data.lock().pages[&copy.id].labels, ["backend", "review", "v2"]);
    assert!(mock.data.lock().attachments.iter().any(|a| a.page_id == copy.id && a.title == "diagram.svg"));
    let e = pages::copy_page(&state, &api, "2001", pages::CopyIn::default()).await.unwrap_err();
    assert_eq!(e.code, "bad_request");
    assert!(e.message.contains("already exists"), "{}", e.message);

    // Trash and restore.
    let mut events = state.events.subscribe();
    let out = pages::trash_page(&state, &api, "2004").await.unwrap();
    assert_eq!(out["title"], "API");
    assert_eq!(events.recv().await.unwrap().data["action"], "deleted");
    assert_eq!(confluence::get_page(&state, &api, "2004", None, None).await.unwrap_err().code, "not_found");
    pages::restore_page(&state, &api, "2004").await.unwrap();
    let (_, put) = last_request(&mock, "PUT", "/wiki/api/v2/pages/2004").unwrap();
    let put = put.unwrap();
    assert_eq!((put["status"].as_str(), put["version"]["number"].as_u64()), (Some("current"), Some(2)));
    let p = confluence::get_page(&state, &api, "2004", None, None).await.unwrap();
    assert_eq!((p.status.as_str(), p.version.number), ("current", 2));
    assert!(p.storage.contains("REST routes"), "restoring keeps the content");
    assert_eq!(pages::restore_page(&state, &api, "2004").await.unwrap_err().code, "bad_request", "not in the trash");

    // Watching.
    assert!(!pages::watching(&api, "2001").await.unwrap());
    pages::set_watching(&api, "2001", true).await.unwrap();
    assert!(pages::watching(&api, "2001").await.unwrap());
    pages::set_watching(&api, "2001", false).await.unwrap();
    assert!(!pages::watching(&api, "2001").await.unwrap());

    // People search for @mentions, and their names are cached for rendering.
    let hits = pages::search_users(&state, &api, "an", 5).await.unwrap();
    assert_eq!(hits.iter().map(|h| h.display_name.as_str()).collect::<Vec<_>>(), ["Ann Lee"]);
    let (url, _) = last_request(&mock, "GET", "/search/user").unwrap();
    assert!(url.contains("cql=user.fullname%20~%20%22an%22"), "{url}");
    assert!(pages::search_users(&state, &api, "  ", 5).await.unwrap().is_empty());
    let gets = count_requests(&mock, "GET");
    let names = confluence::user_names(&state, &api, &["u2".to_string()]).await;
    assert_eq!(names["u2"], "Ann Lee");
    assert_eq!(count_requests(&mock, "GET"), gets, "names come from the cache");
}

#[tokio::test]
async fn jira_boards_sprints_backlog_and_moves() {
    use super::agile::{self, Scope};
    let dir = tempfile::tempdir().unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_for(&base, &dir).await;
    let api = jira_api(&state, None).await.unwrap();

    let all = agile::boards(&api, None, None).await.unwrap();
    assert_eq!(all.boards.iter().map(|b| (b.id, b.kind.as_str())).collect::<Vec<_>>(), [(1, "scrum"), (2, "kanban")]);
    assert_eq!(all.boards[0].project_key.as_deref(), Some("WB"));
    assert_eq!(agile::boards(&api, Some("kanban"), None).await.unwrap().boards.len(), 1);
    assert_eq!(agile::boards(&api, None, Some("wb")).await.unwrap().boards.len(), 2);
    assert!(agile::boards(&api, None, Some("W&B")).await.is_err());

    let b = agile::board(&api, 1).await.unwrap();
    assert_eq!(b.columns.iter().map(|c| (c.name.as_str(), c.status_ids.clone())).collect::<Vec<_>>(), [
        ("To Do", vec!["10000".to_string()]),
        ("In Progress", vec!["3".to_string()]),
        ("Done", vec!["10001".to_string()])
    ]);
    assert!(b.has_sprints);
    assert_eq!(b.quick_filters[0].jql, "assignee = currentUser()");
    assert!(b.web_url.ends_with("/jira/software/projects/WB/boards/1"));
    assert!(!agile::board(&api, 2).await.unwrap().has_sprints);
    assert_eq!(agile::board(&api, 9).await.unwrap_err().code, "not_found");

    let active = agile::sprints(&api, 1, "active").await.unwrap();
    assert_eq!(active.iter().map(|s| (s.id, s.state.as_str())).collect::<Vec<_>>(), [(11, "active")]);
    assert_eq!(active[0].goal.as_deref(), Some("Ship the board"));
    assert_eq!(agile::sprints(&api, 1, "active,future,closed").await.unwrap().len(), 3);
    assert!(agile::sprints(&api, 2, "active").await.unwrap().is_empty(), "kanban boards have no sprints");

    let keys = |o: agile::BoardIssuesOut| o.issues.into_iter().map(|i| i.key).collect::<Vec<_>>();
    assert_eq!(keys(agile::board_issues(&api, 1, Scope::Sprint(11), None).await.unwrap()), ["WB-1", "WB-2"]);
    assert_eq!(keys(agile::board_issues(&api, 1, Scope::Backlog, None).await.unwrap()), ["WB-3"]);
    assert_eq!(keys(agile::board_issues(&api, 2, Scope::Board, Some("assignee = currentUser()")).await.unwrap()), ["WB-1", "WB-2"]);
    let e = agile::board_issues(&api, 1, Scope::Board, Some("BROKEN")).await.unwrap_err();
    assert!(e.message.contains("refused the board query"), "{}", e.message);
    let issues = agile::board_issues(&api, 1, Scope::Sprint(11), None).await.unwrap();
    assert_eq!(issues.issues[0].status.as_ref().and_then(|s| s.id.as_deref()), Some("10000"));

    // Dragging WB-1 to "In Progress": the transition whose target is in that column.
    let ts = agile::issue_transitions(&api, "wb-1").await.unwrap();
    let col = &b.columns[1];
    let t = ts.iter().find(|t| t.to.as_ref().and_then(|s| s.id.as_ref()).is_some_and(|id| col.status_ids.contains(id))).unwrap();
    assert_eq!(t.id, "21");
    jira::transition(&state, &api, "WB-1", &t.id, None).await.unwrap();
    let issues = agile::board_issues(&api, 1, Scope::Sprint(11), None).await.unwrap();
    assert_eq!(issues.issues[0].status.as_ref().and_then(|s| s.id.as_deref()), Some("3"));
    assert!(mock.data.lock().requests.iter().all(|r| r.0 == "GET" || r.1.ends_with("/transitions")), "only the transition writes");
}

/// A state with one project (a scratch folder) for tools that read project files.
async fn state_with_project(site: &str, dir: &tempfile::TempDir, project: &std::path::Path) -> AppState {
    let token_file = dir.path().join("atlassian_token");
    std::fs::write(&token_file, MOCK_TOKEN).unwrap();
    crate::util::fs::set_mode(&token_file, 0o600);
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![project.display().to_string()];
    cfg.atlassian = Some(AtlassianConfig { site: site.into(), email: "me@example.com".into(), token: "atlassian".into() });
    cfg.secrets.insert("atlassian".into(), SecretRef::File(token_file.display().to_string()));
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap()
}

#[tokio::test]
async fn new_mcp_tools_against_the_mock() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(project.join("docs/chart.png"), b"\x89PNG fake").unwrap();
    std::fs::write(project.join(".env"), "SECRET=1").unwrap();
    std::fs::write(dir.path().join("outside.txt"), "not in the project").unwrap();
    // Links to refused files, and a nested folder the project marks sensitive.
    std::fs::write(project.join(".workbench.toml"), "[project]\nsensitive = [\"secrets/\"]\n").unwrap();
    std::fs::create_dir_all(project.join("config/secrets")).unwrap();
    std::fs::write(project.join("config/secrets/db.txt"), "pw").unwrap();
    crate::util::os::fs::symlink("../.env", project.join("docs/notes.txt")).unwrap();
    crate::util::os::fs::symlink("../config/secrets", project.join("docs/cfg")).unwrap();
    let (base, mock) = start_mock("127.0.0.1:0").await;
    let state = state_with_project(&base, &dir, &project).await;
    assert!(state.projects.get("proj").is_some());
    let find = |name: &str| super::tools::all().into_iter().find(|t| t.name == name).unwrap();
    let session = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some("proj".into()) };
    let text = |o: crate::mcp::ToolOutput| match o {
        crate::mcp::ToolOutput::Text(t) => t,
        crate::mcp::ToolOutput::Json(v) => v.to_string(),
    };
    let call = |name: &str, args: Value| {
        let t = find(name);
        let s = state.clone();
        let c = session.clone();
        async move { (t.handler)(s, c, args).await }
    };

    // Inline comment by selection and occurrence.
    let t = text(call("confluence_add_inline_comment", json!({ "pageId": "2001", "selection": "Rust", "occurrence": 2, "markdown": "Why Rust here?" })).await.unwrap());
    assert!(t.contains("occurrence 2 of 2"), "{t}");
    let e = call("confluence_add_inline_comment", json!({ "pageId": "2001", "selection": "Rust", "markdown": "?" })).await.unwrap_err();
    assert!(e.message.contains("occurs"), "{}", e.message);

    // Upload a project file; secrets, escapes and other projects are refused before any upload.
    let t = text(call("confluence_upload_attachment", json!({ "pageId": "2002", "path": "docs/chart.png" })).await.unwrap());
    assert!(t.contains("Attached “chart.png”") && t.contains(r#"<ac:image><ri:attachment ri:filename="chart.png" /></ac:image>"#), "{t}");
    let (_, rec) = last_request(&mock, "POST", "/content/2002/child/attachment").unwrap();
    assert!(rec.unwrap()["parts"].as_array().unwrap().iter().any(|p| p["filename"] == "chart.png" && p["contentType"] == "image/png"));
    let uploads = count_requests(&mock, "POST");
    for (args, code) in [
        (json!({ "pageId": "2002", "path": ".env" }), "forbidden"),
        (json!({ "pageId": "2002", "path": "docs/notes.txt" }), "forbidden"),
        (json!({ "pageId": "2002", "path": "docs/cfg/db.txt" }), "forbidden"),
        (json!({ "pageId": "2002", "path": "config/secrets/db.txt" }), "forbidden"),
        (json!({ "pageId": "2002", "path": "../outside.txt" }), "forbidden"),
        (json!({ "pageId": "2002", "path": dir.path().join("outside.txt").display().to_string() }), "forbidden"),
        (json!({ "pageId": "2002", "path": "docs/missing.png" }), "not_found"),
        (json!({ "pageId": "2002", "path": "docs" }), "bad_request"),
        (json!({ "pageId": "2002", "path": "docs/chart.png", "projectId": "other" }), "forbidden"),
    ] {
        let e = call("confluence_upload_attachment", args.clone()).await.unwrap_err();
        assert_eq!(e.code, code, "{args}: {}", e.message);
    }
    assert_eq!(count_requests(&mock, "POST"), uploads);

    // Labels.
    let t = text(call("confluence_labels", json!({ "pageId": "2001", "add": ["Draft"], "remove": ["backend"] })).await.unwrap());
    assert_eq!(t, "Page 2001: added Draft; removed backend. Labels now: design, draft");
    let t = text(call("confluence_labels", json!({ "pageId": "2001" })).await.unwrap());
    assert_eq!(t, "Labels of page 2001: design, draft");

    // Boards.
    let t = text(call("jira_boards", json!({})).await.unwrap());
    assert!(t.contains("- [1] WB board (scrum, project WB)") && t.contains("- [2] WB kanban (kanban"), "{t}");
    let t = text(call("jira_board_issues", json!({ "boardId": 1 })).await.unwrap());
    assert!(t.contains("WB Sprint 2 (active)") && t.contains("## To Do (1)\n- WB-1") && t.contains("## In Progress (1)\n- WB-2") && t.contains("## Done (0)"), "{t}");
    let t = text(call("jira_board_issues", json!({ "boardId": 1, "sprint": "backlog" })).await.unwrap());
    assert!(t.contains("## Done (1)\n- WB-3"), "{t}");
    let t = text(call("jira_board_issues", json!({ "boardId": 2 })).await.unwrap());
    assert!(t.contains("All board issues") && t.contains("WB-3"), "{t}");
}

/// More content for UI work with the standalone mock (tests use the small seed):
/// a busier sprint, mentions and page links, a resolved inline comment, attachments.
fn enrich_for_ui(d: &mut MockData) {
    let t = "2026-09-24T09:30:00.000Z".to_string();
    let issue = |key: &str, summary: &str, status: (&str, &str), assignee: Option<&str>, sprint: Option<u64>| Issue {
        key: key.into(),
        summary: summary.into(),
        description: markdown::to_adf(&format!("{summary}.\n\n- acceptance: works against the mock")),
        status: (status.0.into(), status.1.into()),
        labels: vec!["workbench".into()],
        assignee: assignee.map(str::to_string),
        comments: vec![],
        sprint,
    };
    for i in [
        issue("WB-5", "Board columns follow the board configuration", ("In Progress", "indeterminate"), Some("u2"), Some(11)),
        issue("WB-6", "Drag a card to transition it", ("To Do", "new"), Some("u3"), Some(11)),
        issue("WB-7", "Quick filters as chips", ("Done", "done"), Some("u1"), Some(11)),
        issue("WB-8", "Sprint picker with future and closed sprints", ("To Do", "new"), None, Some(11)),
        issue("WB-9", "Waiting for design review", ("In Review", "indeterminate"), Some("u2"), Some(11)),
        issue("WB-10", "Backlog grooming view", ("To Do", "new"), None, None),
        issue("WB-11", "Estimate display on cards", ("To Do", "new"), None, Some(12)),
    ] {
        d.issues.insert(i.key.clone(), i);
    }
    let guide = r#"<h2>Authoring guide</h2><p>Ping <ac:link><ri:user ri:account-id="u2" /></ac:link> about the <strong>review</strong>, and read <ac:link><ri:page ri:content-title="Runbook" /></ac:link> before deploying.</p><p>Inline comments anchor to <ac:inline-comment-marker ac:ref="m-2">a passage like this one</ac:inline-comment-marker>; resolved ones stop being highlighted.</p><ul><li><p>Paste or drop an image in the editor to attach it.</p></li><li><p>Type [[ to link a page, @ to mention someone.</p></li></ul><p><ac:image ac:alt="chart"><ri:attachment ri:filename="chart.svg" /></ac:image></p>"#;
    if let Some(p) = d.pages.get_mut("2002") {
        p.versions.push((2, guide.into(), "Guide".into(), t.clone()));
        p.labels = vec!["runbook".into(), "ops".into()];
    }
    d.comments.push(Comment {
        id: "3010".into(),
        page_id: "2002".into(),
        parent_id: None,
        inline: true,
        storage: "<p>Done: reworded.</p>".into(),
        created: t.clone(),
        selection: Some("a passage like this one".into()),
        marker_ref: Some("m-2".into()),
        resolution: Some("resolved".into()),
        version: 2,
        resolved_by: Some("u2".into()),
    });
    d.comments.push(Comment {
        id: "3011".into(),
        page_id: "2002".into(),
        parent_id: None,
        inline: false,
        storage: r#"<p>Thanks <ac:link><ri:user ri:account-id="u3" /></ac:link>, this is much clearer.</p>"#.into(),
        created: t.clone(),
        selection: None,
        marker_ref: None,
        resolution: None,
        version: 1,
        resolved_by: None,
    });
    let chart = r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="140"><rect width="320" height="140" rx="8" fill="#2b2d30"/><rect x="30" y="70" width="40" height="50" fill="#548af7"/><rect x="90" y="40" width="40" height="80" fill="#5fb865"/><rect x="150" y="55" width="40" height="65" fill="#e0b050"/><rect x="210" y="25" width="40" height="95" fill="#e55765"/></svg>"##;
    for (id, page, title, ty, data) in [
        ("att7101", "2002", "chart.svg", "image/svg+xml", chart.as_bytes().to_vec()),
        ("att7102", "2002", "release-notes.txt", "text/plain", b"Release 1.4\n- boards\n- inline comments\n".to_vec()),
        ("att7103", "2002", "runbook-export.pdf", "application/pdf", b"%PDF-1.4 mock".to_vec()),
    ] {
        d.attachments.push(Attachment {
            id: id.into(),
            page_id: page.into(),
            title: title.into(),
            media_type: ty.into(),
            data,
            version: 1,
            comment: String::new(),
            status: "current".into(),
        });
    }
}

/// Serve the mock until killed (for UI verification). See the module docs.
#[tokio::test]
#[ignore]
async fn serve_mock_forever() {
    let port = std::env::var("WB_ATLASSIAN_MOCK_PORT").unwrap_or_else(|_| "4010".into());
    let jira = std::env::var("WB_ATLASSIAN_MOCK_JIRA").map(|v| v != "0").unwrap_or(true);
    let (base, mock) = start_mock(&format!("127.0.0.1:{port}")).await;
    mock.data.lock().jira = jira;
    enrich_for_ui(&mut mock.data.lock());
    println!("mock Atlassian at {base} (token {MOCK_TOKEN}, email me@example.com, jira={jira})");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}
