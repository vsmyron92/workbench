//! Confluence Cloud: REST handlers under `/api/confluence/**` and the core functions
//! the MCP tools reuse. Reads use the v2 API (v1 for CQL search, the current user
//! and attachment downloads).

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use futures::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{Api, Product, V2List, cursor_of, q};
use super::html::{Rewrite, clean_search_text};
use super::{api_for, attachments, markdown, storage};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Caps on how much of a listing is fetched in one request.
const MAX_TREE_ITEMS: usize = 1000;
const MAX_COMMENTS: usize = 250;
const MAX_REPLY_FETCHES: usize = 100;
const MAX_STORAGE_BYTES: usize = 5 * 1024 * 1024;

// ---------------------------------------------------------------- upstream shapes

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct V2Version {
    pub number: u32,
    pub message: String,
    pub minor_edit: bool,
    pub author_id: String,
    pub created_at: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Repr {
    pub value: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct V2Body {
    pub storage: Option<Repr>,
    pub view: Option<Repr>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct V2Links {
    pub webui: Option<String>,
    pub editui: Option<String>,
    pub edituiv2: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Label {
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct V2Page {
    pub id: String,
    pub title: String,
    pub status: String,
    pub space_id: String,
    pub parent_id: Option<String>,
    pub parent_type: Option<String>,
    pub version: V2Version,
    pub body: V2Body,
    pub labels: Option<V2List<Label>>,
    #[serde(rename = "_links")]
    pub links: V2Links,
}

impl V2Page {
    fn storage(&self) -> &str {
        self.body.storage.as_ref().map(|r| r.value.as_str()).unwrap_or("")
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Descendant {
    id: String,
    status: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
    parent_id: String,
    depth: u32,
    child_position: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V2Space {
    id: String,
    key: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    status: String,
    homepage_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AncestorRef {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

// ---------------------------------------------------------------- outputs

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceOut {
    pub id: String,
    pub key: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub homepage_id: Option<String>,
}

impl From<V2Space> for SpaceOut {
    fn from(s: V2Space) -> Self {
        SpaceOut { id: s.id, key: s.key, name: s.name, kind: s.kind, status: s.status, homepage_id: s.homepage_id }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub id: String,
    pub title: String,
    /// page | folder | whiteboard | database | embed
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    /// `None` when unknown (the UI shows an expander and finds out on expand).
    pub has_children: Option<bool>,
    pub position: Option<i64>,
    pub parent_id: Option<String>,
    pub space_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildrenOut {
    pub children: Vec<TreeNode>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionOut {
    pub number: u32,
    pub message: String,
    pub minor_edit: bool,
    pub created_at: String,
    pub author_id: String,
    pub author_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageOut {
    pub id: String,
    pub title: String,
    pub status: String,
    pub space_id: String,
    pub space_key: Option<String>,
    pub space_name: Option<String>,
    pub parent_id: Option<String>,
    pub version: VersionOut,
    pub web_url: String,
    pub edit_url: Option<String>,
    pub ancestors: Vec<Crumb>,
    pub labels: Vec<String>,
    /// Sanitized view HTML (images proxied, internal links marked with `data-wb-page`).
    pub html: String,
    /// Raw storage XHTML (for editing).
    pub storage: String,
    pub has_inline_comment_markers: bool,
    pub inline_marker_refs: Vec<String>,
    /// Display names of the accounts the storage mentions (`ri:user`), for the editor.
    pub users: HashMap<String, String>,
    /// A historical version was requested.
    pub historical: bool,
}

pub(crate) fn web(api: &Api, path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty()).map(|p| format!("{}/wiki{p}", api.site.base))
}

/// Confluence ids are numeric; anything else never reaches an upstream URL.
pub(crate) fn check_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > 24 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::bad_request(format!("invalid Confluence id {id:?}")));
    }
    Ok(())
}

pub(crate) fn rewrite<'a>(api: &'a Api, project_id: Option<&str>) -> Rewrite<'a> {
    Rewrite { site: &api.site.base, product: Product::Confluence, project_id: project_id.map(str::to_string) }
}

// ---------------------------------------------------------------- lookups with caches

/// Display names for account ids (cached per site; failures are skipped).
pub(crate) async fn user_names(state: &AppState, api: &Api, ids: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut missing: Vec<String> = vec![];
    for id in ids.iter().filter(|i| !i.is_empty()) {
        let key = format!("{}|{id}", api.site.base);
        match state.atlassian.names.get(&key) {
            Some(n) => {
                out.insert(id.clone(), n.clone());
            }
            None if !missing.contains(id) && missing.len() < 40 => missing.push(id.clone()),
            None => {}
        }
    }
    let fetched: Vec<(String, Option<String>)> = futures::stream::iter(missing)
        .map(|id| async move {
            let url = api.wiki(&format!("/rest/api/user?accountId={}", q(&id)));
            let name = api.get::<Value>(&url).await.ok().and_then(|v| {
                v.get("displayName").or_else(|| v.get("publicName")).and_then(Value::as_str).map(str::to_string)
            });
            (id, name)
        })
        .buffer_unordered(4)
        .collect()
        .await;
    for (id, name) in fetched {
        if let Some(n) = name {
            state.atlassian.remember_name(format!("{}|{id}", api.site.base), n.clone());
            out.insert(id, n);
        }
    }
    out
}

pub(crate) async fn space_info(state: &AppState, api: &Api, space_id: &str) -> Result<SpaceOut, ApiError> {
    check_id(space_id)?;
    let key = format!("{}|{space_id}", api.site.base);
    if let Some(s) = state.atlassian.spaces.get(&key) {
        return Ok(s.clone());
    }
    let s: V2Space = api.get(&api.wiki(&format!("/api/v2/spaces/{space_id}"))).await?;
    let out = SpaceOut::from(s);
    state.atlassian.spaces.insert(key, out.clone());
    Ok(out)
}

/// Resolve a space key (`DESIGN`) to its id.
pub(crate) async fn space_by_key(state: &AppState, api: &Api, key: &str) -> Result<SpaceOut, ApiError> {
    if key.is_empty() || key.len() > 255 || key.contains(['&', '?', '#', '/', ' ']) {
        return Err(ApiError::bad_request(format!("invalid space key {key:?}")));
    }
    if let Some(s) = state.atlassian.spaces.iter().find(|e| e.key().starts_with(&api.site.base) && e.value().key == key) {
        return Ok(s.value().clone());
    }
    let list: V2List<V2Space> = api.get(&api.wiki(&format!("/api/v2/spaces?keys={}", q(key)))).await?;
    let s = list.results.into_iter().next().ok_or_else(|| ApiError::not_found(format!("no Confluence space with key {key}")))?;
    let out = SpaceOut::from(s);
    state.atlassian.spaces.insert(format!("{}|{}", api.site.base, out.id), out.clone());
    Ok(out)
}

// ---------------------------------------------------------------- spaces and trees

pub async fn list_spaces(state: &AppState, api: &Api) -> Result<Vec<SpaceOut>, ApiError> {
    let url = api.wiki("/api/v2/spaces?limit=250&status=current&sort=name");
    let (spaces, _) = api.v2_all::<V2Space>(url, 1000).await?;
    let out: Vec<SpaceOut> = spaces.into_iter().map(SpaceOut::from).collect();
    for s in &out {
        state.atlassian.spaces.insert(format!("{}|{}", api.site.base, s.id), s.clone());
    }
    Ok(out)
}

fn sort_nodes(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| {
        a.position
            .unwrap_or(i64::MAX)
            .cmp(&b.position.unwrap_or(i64::MAX))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// Root pages of a space. `status`: current | archived | all.
pub async fn space_root_pages(api: &Api, space_id: &str, status: &str) -> Result<ChildrenOut, ApiError> {
    check_id(space_id)?;
    let status_q = match status {
        "archived" => "&status=archived",
        "all" => "",
        _ => "&status=current",
    };
    let url = api.wiki(&format!("/api/v2/spaces/{space_id}/pages?depth=root&limit=250{status_q}"));
    let (pages, truncated) = api.v2_all::<V2Page>(url, MAX_TREE_ITEMS).await?;
    let mut children: Vec<TreeNode> = pages
        .into_iter()
        .map(|p| TreeNode {
            id: p.id,
            title: p.title,
            kind: "page".into(),
            status: p.status,
            has_children: None,
            position: None,
            parent_id: None,
            space_id: Some(p.space_id),
        })
        .collect();
    sort_nodes(&mut children);
    Ok(ChildrenOut { children, truncated })
}

/// Direct children of a page or folder, with `hasChildren` computed from one
/// `descendants?depth=2` listing (falls back to `direct-children` when that listing
/// had to be cut short).
pub async fn children(api: &Api, id: &str, kind: &str, include_archived: bool) -> Result<ChildrenOut, ApiError> {
    check_id(id)?;
    let base = match kind {
        "folder" => "folders",
        "whiteboard" => "whiteboards",
        "database" => "databases",
        "embed" => "embeds",
        _ => "pages",
    };
    let url = api.wiki(&format!("/api/v2/{base}/{id}/descendants?depth=2&limit=250"));
    let (items, truncated) = api.v2_all::<Descendant>(url, MAX_TREE_ITEMS).await?;
    let parents: HashSet<&str> = items.iter().filter(|d| d.depth >= 2).map(|d| d.parent_id.as_str()).collect();
    let node = |d: &Descendant, known: bool| TreeNode {
        id: d.id.clone(),
        title: d.title.clone(),
        kind: if d.kind.is_empty() { "page".into() } else { d.kind.clone() },
        status: d.status.clone(),
        has_children: if parents.contains(d.id.as_str()) { Some(true) } else if known { Some(false) } else { None },
        position: d.child_position,
        parent_id: Some(id.to_string()),
        space_id: None,
    };
    let mut out: Vec<TreeNode> = if truncated {
        let url = api.wiki(&format!("/api/v2/{base}/{id}/direct-children?limit=250"));
        let (direct, _) = api.v2_all::<Descendant>(url, MAX_TREE_ITEMS).await?;
        direct.iter().map(|d| node(d, false)).collect()
    } else {
        items.iter().filter(|d| d.depth == 1 || (d.depth == 0 && d.parent_id == id)).map(|d| node(d, true)).collect()
    };
    if !include_archived {
        out.retain(|n| n.status != "archived");
    }
    sort_nodes(&mut out);
    Ok(ChildrenOut { children: out, truncated })
}

/// Pages by id (titles for pinned pages and project root pages).
pub async fn pages_by_ids(api: &Api, ids: &[String]) -> Result<Vec<TreeNode>, ApiError> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    for id in ids {
        check_id(id)?;
    }
    let ids: Vec<&str> = ids.iter().take(250).map(String::as_str).collect();
    let url = api.wiki(&format!("/api/v2/pages?id={}&limit=250", ids.join(",")));
    let (pages, _) = api.v2_all::<V2Page>(url, 250).await?;
    let mut by_id: HashMap<String, TreeNode> = pages
        .into_iter()
        .map(|p| {
            (
                p.id.clone(),
                TreeNode {
                    id: p.id,
                    title: p.title,
                    kind: "page".into(),
                    status: p.status,
                    has_children: None,
                    position: None,
                    parent_id: p.parent_id,
                    space_id: Some(p.space_id),
                },
            )
        })
        .collect();
    // Keep the caller's order.
    Ok(ids.iter().filter_map(|id| by_id.remove(*id)).collect())
}

async fn crumbs(api: &Api, refs: Vec<AncestorRef>) -> Vec<Crumb> {
    let page_ids: Vec<String> = refs.iter().filter(|r| r.kind != "folder").map(|r| r.id.clone()).collect();
    let mut titles: HashMap<String, String> = HashMap::new();
    if let Ok(pages) = pages_by_ids(api, &page_ids).await {
        titles.extend(pages.into_iter().map(|p| (p.id, p.title)));
    }
    // Collected first: a borrowing iterator inside the stream makes the future !Send.
    let folder_ids: Vec<String> = refs.iter().filter(|r| r.kind == "folder").map(|r| r.id.clone()).collect();
    let folders: Vec<(String, Option<String>)> = futures::stream::iter(folder_ids)
        .map(|id| async move {
            let title = if check_id(&id).is_ok() {
                api.get::<Value>(&api.wiki(&format!("/api/v2/folders/{id}")))
                    .await
                    .ok()
                    .and_then(|v| v.get("title").and_then(Value::as_str).map(str::to_string))
            } else {
                None
            };
            (id, title)
        })
        .buffer_unordered(4)
        .collect()
        .await;
    titles.extend(folders.into_iter().filter_map(|(id, t)| t.map(|t| (id, t))));
    refs.into_iter()
        .map(|r| Crumb { title: titles.get(&r.id).cloned().unwrap_or_else(|| format!("#{}", r.id)), id: r.id, kind: r.kind })
        .collect()
}

// ---------------------------------------------------------------- pages

pub async fn get_page(
    state: &AppState,
    api: &Api,
    id: &str,
    version: Option<u32>,
    project_id: Option<&str>,
) -> Result<PageOut, ApiError> {
    check_id(id)?;
    let vq = version.map(|v| format!("&version={v}")).unwrap_or_default();
    let storage_url = api.wiki(&format!("/api/v2/pages/{id}?body-format=storage&include-labels=true{vq}"));
    let view_url = api.wiki(&format!("/api/v2/pages/{id}?body-format=view{vq}"));
    let anc_url = api.wiki(&format!("/api/v2/pages/{id}/ancestors?limit=50"));
    // Storage and view come from two requests; retry once if an edit landed in between.
    let mut attempt = 0;
    let (s, v, anc) = loop {
        let (s, v, a) = tokio::join!(
            api.get::<V2Page>(&storage_url),
            api.get::<V2Page>(&view_url),
            api.get::<V2List<AncestorRef>>(&anc_url)
        );
        let (s, v) = (s?, v?);
        if s.version.number == v.version.number || attempt > 0 || version.is_some() {
            break (s, v, a);
        }
        attempt += 1;
    };
    let ancestors = match anc {
        Ok(a) => crumbs(api, a.results).await,
        Err(_) => vec![],
    };
    let space = space_info(state, api, &s.space_id).await.ok();
    let mut mentioned = storage::mentioned_accounts(s.storage());
    mentioned.sort();
    mentioned.dedup();
    mentioned.truncate(39);
    let mut lookup = mentioned.clone();
    lookup.push(s.version.author_id.clone());
    let names = user_names(state, api, &lookup).await;
    let users: HashMap<String, String> = mentioned.iter().filter_map(|id| names.get(id).map(|n| (id.clone(), n.clone()))).collect();
    let view_html = v.body.view.as_ref().map(|r| r.value.as_str()).unwrap_or("");
    let html = rewrite(api, project_id).render(view_html);
    let storage_text = s.storage().to_string();
    let refs = storage::inline_marker_refs(&storage_text);
    Ok(PageOut {
        web_url: web(api, s.links.webui.as_deref()).unwrap_or_else(|| format!("{}/wiki/pages/viewpage.action?pageId={id}", api.site.base)),
        edit_url: web(api, s.links.edituiv2.as_deref().or(s.links.editui.as_deref())),
        labels: s.labels.as_ref().map(|l| l.results.iter().map(|x| x.name.clone()).collect()).unwrap_or_default(),
        version: VersionOut {
            number: s.version.number,
            message: s.version.message.clone(),
            minor_edit: s.version.minor_edit,
            created_at: s.version.created_at.clone(),
            author_name: names.get(&s.version.author_id).cloned(),
            author_id: s.version.author_id.clone(),
        },
        historical: version.is_some(),
        id: s.id,
        title: s.title,
        status: s.status,
        space_key: space.as_ref().map(|x| x.key.clone()),
        space_name: space.map(|x| x.name),
        space_id: s.space_id,
        parent_id: s.parent_id,
        ancestors,
        html,
        has_inline_comment_markers: !refs.is_empty(),
        inline_marker_refs: refs,
        users,
        storage: storage_text,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionsOut {
    pub results: Vec<VersionOut>,
    pub next_cursor: Option<String>,
}

pub async fn versions(state: &AppState, api: &Api, id: &str, cursor: Option<&str>, limit: u32) -> Result<VersionsOut, ApiError> {
    check_id(id)?;
    let cq = cursor.map(|c| format!("&cursor={}", q(c))).unwrap_or_default();
    let url = api.wiki(&format!("/api/v2/pages/{id}/versions?limit={}{cq}", limit.clamp(1, 250)));
    let list: V2List<V2Version> = api.get(&url).await?;
    let ids: Vec<String> = list.results.iter().map(|v| v.author_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    let names = user_names(state, api, &ids).await;
    Ok(VersionsOut {
        next_cursor: list.links.and_then(|l| l.next).and_then(|n| cursor_of(&n)),
        results: list
            .results
            .into_iter()
            .map(|v| VersionOut {
                author_name: names.get(&v.author_id).cloned(),
                number: v.number,
                message: v.message,
                minor_edit: v.minor_edit,
                created_at: v.created_at,
                author_id: v.author_id,
            })
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionBodyOut {
    pub number: u32,
    pub title: String,
    pub storage: String,
    pub created_at: String,
    pub message: String,
    pub author_name: Option<String>,
}

pub async fn version_storage(state: &AppState, api: &Api, id: &str, n: u32) -> Result<VersionBodyOut, ApiError> {
    check_id(id)?;
    let p: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{id}?version={n}&body-format=storage"))).await?;
    let names = user_names(state, api, std::slice::from_ref(&p.version.author_id)).await;
    Ok(VersionBodyOut {
        storage: p.storage().to_string(),
        number: p.version.number,
        title: p.title,
        created_at: p.version.created_at,
        message: p.version.message,
        author_name: names.get(&p.version.author_id).cloned(),
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateIn {
    pub title: Option<String>,
    pub storage: Option<String>,
    pub markdown: Option<String>,
    /// The version the edit started from (optimistic concurrency). Required (a missing
    /// one is a 400): an update without it could overwrite someone's newer version unseen.
    pub version: Option<u32>,
    pub message: Option<String>,
    /// Save even though inline-comment markers would be removed.
    pub force: bool,
    pub minor_edit: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOut {
    pub id: String,
    pub title: String,
    pub version: u32,
    pub web_url: String,
    /// Nothing changed, so no version was created.
    pub unchanged: bool,
}

pub(crate) fn body_of(storage: Option<String>, md: Option<String>) -> Result<String, ApiError> {
    let s = match (storage, md) {
        (Some(s), _) => s,
        (None, Some(md)) => markdown::to_storage(&md),
        (None, None) => return Err(ApiError::bad_request("storage or markdown is required")),
    };
    if s.len() > MAX_STORAGE_BYTES {
        return Err(ApiError::bad_request("the page body is too large"));
    }
    storage::validate(&s).map_err(ApiError::bad_request)?;
    Ok(s)
}

pub(crate) fn check_title(t: &str) -> Result<(), ApiError> {
    if t.trim().is_empty() {
        return Err(ApiError::bad_request("the title is empty"));
    }
    if t.chars().count() > 255 {
        return Err(ApiError::bad_request("titles are limited to 255 characters"));
    }
    Ok(())
}

/// Update a page: optimistic concurrency on `version` (mandatory), refusal to silently
/// drop inline-comment markers (unless `force`), then `PUT` version + 1.
pub async fn update_page(state: &AppState, api: &Api, id: &str, input: UpdateIn) -> Result<UpdateOut, ApiError> {
    check_id(id)?;
    let base = input.version.ok_or_else(|| {
        ApiError::bad_request(
            "version is required: pass the version the edit is based on (read the page first), \
             so a newer version saved by someone else is never overwritten",
        )
    })?;
    let new_storage = body_of(input.storage, input.markdown)?;
    let url = api.wiki(&format!("/api/v2/pages/{id}"));
    let cur: V2Page = api.get(&format!("{url}?body-format=storage")).await?;
    if cur.status == "archived" {
        return Err(ApiError::bad_request("This page is archived; restore it in Confluence before editing it"));
    }
    if base != cur.version.number {
        let names = user_names(state, api, std::slice::from_ref(&cur.version.author_id)).await;
        let who = names.get(&cur.version.author_id).map(|n| format!(" by {n}")).unwrap_or_default();
        return Err(ApiError::conflict(format!(
            "The page changed while you were editing: it is now version {}{who} ({}), and your edit started from version {base}.",
            cur.version.number, cur.version.created_at
        )));
    }
    let dropped = storage::dropped_markers(cur.storage(), &new_storage);
    if !dropped.is_empty() && !input.force {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "inline_comments",
            format!(
                "This edit removes {} inline comment marker{} ({}). The comments anchored there would lose their place. \
                 Keep the highlighted text, or save again with force to drop them.",
                dropped.len(),
                if dropped.len() == 1 { "" } else { "s" },
                dropped.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    let title = input.title.filter(|t| !t.trim().is_empty()).map(|t| t.trim().to_string()).unwrap_or_else(|| cur.title.clone());
    check_title(&title)?;
    let web_url = web(api, cur.links.webui.as_deref()).unwrap_or_default();
    if new_storage == cur.storage() && title == cur.title {
        return Ok(UpdateOut { id: id.to_string(), title, version: cur.version.number, web_url, unchanged: true });
    }
    let body = json!({
        "id": id,
        "status": "current",
        "title": title,
        "body": { "representation": "storage", "value": new_storage },
        "version": {
            "number": cur.version.number + 1,
            "message": input.message.unwrap_or_default().chars().take(500).collect::<String>(),
            "minorEdit": input.minor_edit,
        },
    });
    let saved: V2Page = api.send_json(Method::PUT, &url, &body).await.map_err(|e| {
        if e.code == "conflict" {
            ApiError::conflict(format!("Someone saved this page at the same moment; reload and try again ({})", e.message))
        } else {
            e
        }
    })?;
    let version = if saved.version.number > 0 { saved.version.number } else { cur.version.number + 1 };
    let title = if saved.title.is_empty() { title } else { saved.title };
    state.events.emit("confluence.page", None, json!({ "pageId": id, "version": version, "title": title, "action": "updated" }));
    Ok(UpdateOut { id: id.to_string(), title, version, web_url, unchanged: false })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateIn {
    pub space_id: Option<String>,
    pub space_key: Option<String>,
    pub parent_id: Option<String>,
    pub title: String,
    pub storage: Option<String>,
    pub markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedOut {
    pub id: String,
    pub title: String,
    pub space_id: String,
    pub version: u32,
    pub web_url: String,
}

pub async fn create_page(state: &AppState, api: &Api, input: CreateIn) -> Result<CreatedOut, ApiError> {
    let title = input.title.trim().to_string();
    check_title(&title)?;
    let storage_body = body_of(input.storage.or_else(|| input.markdown.is_none().then(String::new)), input.markdown)?;
    if let Some(p) = &input.parent_id {
        check_id(p)?;
    }
    let space_id = match (input.space_id.filter(|s| !s.is_empty()), input.space_key.filter(|s| !s.is_empty()), &input.parent_id) {
        (Some(id), _, _) => {
            check_id(&id)?;
            id
        }
        (None, Some(key), _) => space_by_key(state, api, &key).await?.id,
        (None, None, Some(parent)) => {
            let p: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{parent}"))).await?;
            p.space_id
        }
        _ => return Err(ApiError::bad_request("spaceId, spaceKey or parentId is required")),
    };
    let mut body = json!({
        "spaceId": space_id,
        "status": "current",
        "title": title,
        "body": { "representation": "storage", "value": storage_body },
    });
    if let Some(p) = &input.parent_id {
        body["parentId"] = Value::String(p.clone());
    }
    let created: V2Page = api.send_json(Method::POST, &api.wiki("/api/v2/pages"), &body).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": created.id, "version": created.version.number, "title": created.title, "action": "created" }));
    Ok(CreatedOut {
        web_url: web(api, created.links.webui.as_deref()).unwrap_or_default(),
        version: created.version.number,
        space_id: if created.space_id.is_empty() { space_id } else { created.space_id },
        title: created.title,
        id: created.id,
    })
}

// ---------------------------------------------------------------- comments

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct V2CommentProps {
    pub inline_marker_ref: Option<String>,
    pub inline_original_selection: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct V2Comment {
    pub id: String,
    pub status: String,
    pub page_id: Option<String>,
    pub version: V2Version,
    pub body: V2Body,
    pub resolution_status: Option<String>,
    pub resolution_last_modifier_id: Option<String>,
    pub resolution_last_modified_at: Option<String>,
    pub properties: Option<V2CommentProps>,
    #[serde(rename = "_links")]
    pub links: V2Links,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentOut {
    pub id: String,
    /// footer | inline
    pub kind: String,
    pub author_id: String,
    pub author_name: Option<String>,
    pub created_at: String,
    pub version: u32,
    pub html: String,
    pub web_url: Option<String>,
    /// Inline comments: the text the comment was made on.
    pub selection: Option<String>,
    pub marker_ref: Option<String>,
    /// Inline comments: open | reopened | resolved | dangling.
    pub resolution_status: Option<String>,
    /// Who resolved or reopened it last, and when.
    pub resolved_by: Option<String>,
    pub resolved_at: Option<String>,
    /// The body as markdown, the starting point of an edit (mentions are
    /// `[@Name](mention:<accountId>)` links).
    pub markdown: String,
    /// Saving `markdown` back would lose formatting markdown cannot express.
    pub edit_lossy: bool,
    pub replies: Vec<CommentOut>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentsOut {
    pub footer: Vec<CommentOut>,
    pub inline: Vec<CommentOut>,
    pub truncated: bool,
}

fn comment_out(api: &Api, rw: &Rewrite, names: &HashMap<String, String>, c: &V2Comment, kind: &str) -> CommentOut {
    let props = c.properties.as_ref();
    let storage_body = c.body.storage.as_ref().map(|r| r.value.as_str()).unwrap_or("");
    let (markdown, edit_lossy) = super::comment_md::to_markdown(storage_body, names);
    CommentOut {
        id: c.id.clone(),
        kind: kind.to_string(),
        author_id: c.version.author_id.clone(),
        author_name: names.get(&c.version.author_id).cloned(),
        created_at: c.version.created_at.clone(),
        version: c.version.number,
        // v2 comments come only as storage/ADF: render storage, then sanitize.
        html: rw.render(&storage::to_html(storage_body, names)),
        web_url: web(api, c.links.webui.as_deref()),
        selection: props.and_then(|p| p.inline_original_selection.clone()),
        marker_ref: props.and_then(|p| p.inline_marker_ref.clone()),
        resolution_status: c.resolution_status.clone(),
        resolved_by: c.resolution_last_modifier_id.as_ref().map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone())),
        resolved_at: c.resolution_last_modified_at.clone(),
        markdown,
        edit_lossy,
        replies: vec![],
    }
}

/// Comments of a page; without `with_replies` the threads' replies are not fetched
/// (one request per thread), for callers that only need the threads' states.
pub async fn comments_with(state: &AppState, api: &Api, id: &str, project_id: Option<&str>, with_replies: bool) -> Result<CommentsOut, ApiError> {
    check_id(id)?;
    let footer_url = api.wiki(&format!("/api/v2/pages/{id}/footer-comments?body-format=storage&limit=100"));
    let inline_url = api.wiki(&format!("/api/v2/pages/{id}/inline-comments?body-format=storage&limit=100"));
    let (footer, inline) = tokio::join!(
        api.v2_all::<V2Comment>(footer_url, MAX_COMMENTS),
        api.v2_all::<V2Comment>(inline_url, MAX_COMMENTS)
    );
    let ((footer, t1), (inline, t2)) = (footer?, inline?);
    let footer: Vec<V2Comment> = footer.into_iter().filter(|c| c.status != "deleted").collect();
    let inline: Vec<V2Comment> = inline.into_iter().filter(|c| c.status != "deleted").collect();

    // One level of replies for each top-level comment (bounded concurrency).
    let targets: Vec<(String, String)> = footer
        .iter()
        .map(|c| ("footer".to_string(), c.id.clone()))
        .chain(inline.iter().map(|c| ("inline".to_string(), c.id.clone())))
        .take(if with_replies { MAX_REPLY_FETCHES } else { 0 })
        .collect();
    let replies: HashMap<String, Vec<V2Comment>> = futures::stream::iter(targets)
        .map(|(kind, cid)| async move {
            let url = api.wiki(&format!("/api/v2/{kind}-comments/{cid}/children?body-format=storage&limit=100"));
            let list = api.get::<V2List<V2Comment>>(&url).await.map(|l| l.results).unwrap_or_default();
            (cid, list.into_iter().filter(|r| r.status != "deleted").collect::<Vec<_>>())
        })
        .buffer_unordered(6)
        .collect()
        .await;

    // Names for authors and @mentions, all at once.
    let mut ids: Vec<String> = vec![];
    for c in footer.iter().chain(inline.iter()).chain(replies.values().flatten()) {
        ids.push(c.version.author_id.clone());
        ids.extend(c.resolution_last_modifier_id.clone());
        if let Some(b) = &c.body.storage {
            ids.extend(storage::mentioned_accounts(&b.value));
        }
    }
    ids.sort();
    ids.dedup();
    let names = user_names(state, api, &ids).await;

    let rw = rewrite(api, project_id);
    let build = |list: &[V2Comment], kind: &str| -> Vec<CommentOut> {
        list.iter()
            .map(|c| {
                let mut out = comment_out(api, &rw, &names, c, kind);
                if let Some(rs) = replies.get(&c.id) {
                    out.replies = rs.iter().map(|r| comment_out(api, &rw, &names, r, kind)).collect();
                }
                out
            })
            .collect()
    };
    Ok(CommentsOut { footer: build(&footer, "footer"), inline: build(&inline, "inline"), truncated: t1 || t2 })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddCommentIn {
    pub markdown: Option<String>,
    pub storage: Option<String>,
    /// Reply to this comment instead of starting a new footer comment.
    pub parent_comment_id: Option<String>,
    /// Kind of the parent comment: footer (default) | inline.
    pub parent_kind: Option<String>,
}

pub async fn add_comment(state: &AppState, api: &Api, page_id: &str, input: AddCommentIn) -> Result<Value, ApiError> {
    check_id(page_id)?;
    let value = body_of(input.storage, input.markdown)?;
    if value.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    let body = json!({ "representation": "storage", "value": value });
    let (url, payload) = match input.parent_comment_id.filter(|p| !p.is_empty()) {
        Some(parent) => {
            check_id(&parent)?;
            let kind = if input.parent_kind.as_deref() == Some("inline") { "inline" } else { "footer" };
            (api.wiki(&format!("/api/v2/{kind}-comments")), json!({ "parentCommentId": parent, "body": body }))
        }
        None => (api.wiki("/api/v2/footer-comments"), json!({ "pageId": page_id, "body": body })),
    };
    let created: Value = api.send_json(Method::POST, &url, &payload).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "commented" }));
    Ok(json!({ "id": created.get("id").cloned().unwrap_or(Value::Null) }))
}

// ---------------------------------------------------------------- search

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchIn {
    /// Free text (searched with `text ~`).
    pub q: Option<String>,
    /// Raw CQL (overrides `q`).
    pub cql: Option<String>,
    /// Space key to restrict to.
    pub space: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Include archived pages.
    pub archived: Option<bool>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    pub space_key: Option<String>,
    pub space_name: Option<String>,
    pub status: String,
    pub last_modified: Option<String>,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOut {
    pub cql: String,
    pub results: Vec<SearchHit>,
    pub next_cursor: Option<String>,
    pub total_size: Option<u64>,
}

/// Escape a string for a double-quoted CQL literal.
pub fn cql_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn build_cql(input: &SearchIn) -> Result<String, ApiError> {
    if let Some(c) = input.cql.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        return Ok(c.to_string());
    }
    let text = input.q.as_deref().map(str::trim).unwrap_or("");
    if text.is_empty() {
        return Err(ApiError::bad_request("a search needs q or cql"));
    }
    let mut parts = vec!["type = page".to_string(), format!("text ~ {}", cql_quote(text))];
    if let Some(s) = input.space.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("space = {}", cql_quote(s)));
    }
    Ok(parts.join(" AND "))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V1SearchContent {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    status: String,
    title: String,
    #[serde(rename = "_links")]
    links: V2Links,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V1Container {
    title: String,
    display_url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V1SearchResult {
    content: Option<V1SearchContent>,
    title: String,
    excerpt: String,
    url: String,
    result_global_container: Option<V1Container>,
    last_modified: Option<String>,
    entity_type: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V1SearchPage {
    results: Vec<V1SearchResult>,
    total_size: Option<u64>,
    #[serde(rename = "_links")]
    links: super::client::NextLinks,
}

/// Turn v1 search results into hits: strip highlight markers, decode entities,
/// take the status from the expanded content (the unexpanded one is always "current").
fn search_hits(api: &Api, page: V1SearchPage, include_archived: bool) -> Vec<SearchHit> {
    page.results
        .into_iter()
        .filter(|r| r.entity_type == "content" || r.content.is_some())
        .filter_map(|r| {
            let c = r.content?;
            if !include_archived && c.status == "archived" {
                return None;
            }
            let space_key = r
                .result_global_container
                .as_ref()
                .and_then(|g| g.display_url.strip_prefix("/spaces/"))
                .map(|k| k.trim_end_matches('/').to_string());
            let title = clean_search_text(if r.title.is_empty() { &c.title } else { &r.title });
            Some(SearchHit {
                web_url: web(api, c.links.webui.as_deref().or(Some(r.url.as_str()))),
                id: c.id,
                kind: c.kind,
                title,
                excerpt: clean_search_text(&r.excerpt),
                space_name: r.result_global_container.map(|g| g.title),
                space_key,
                status: c.status,
                last_modified: r.last_modified,
            })
        })
        .collect()
}

pub async fn search(api: &Api, input: &SearchIn) -> Result<SearchOut, ApiError> {
    let cql = build_cql(input)?;
    let limit = input.limit.unwrap_or(25).clamp(1, 100);
    let mut url = api.wiki(&format!("/rest/api/search?cql={}&limit={limit}&excerpt=highlight&expand=content.status", q(&cql)));
    if let Some(c) = input.cursor.as_deref().filter(|c| !c.is_empty()) {
        url.push_str(&format!("&cursor={}&next=true", q(c)));
    }
    let page: V1SearchPage = api.get(&url).await.map_err(|e| {
        if e.code == "bad_request" { ApiError::bad_request(format!("Invalid CQL `{cql}`: {}", e.message)) } else { e }
    })?;
    let next_cursor = page.links.next.as_deref().and_then(cursor_of);
    let total_size = page.total_size;
    let results = search_hits(api, page, input.archived.unwrap_or(false));
    Ok(SearchOut { cql, results, next_cursor, total_size })
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectQuery {
    pub project_id: Option<String>,
}

pub async fn spaces_handler(State(state): State<AppState>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Vec<SpaceOut>>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(list_spaces(&state, &api).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpacePagesQuery {
    project_id: Option<String>,
    status: Option<String>,
}

pub async fn space_pages_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(sq): Query<SpacePagesQuery>,
) -> ApiResult<Json<ChildrenOut>> {
    let api = api_for(&state, sq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(space_root_pages(&api, &id, sq.status.as_deref().unwrap_or("current")).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IdsQuery {
    project_id: Option<String>,
    ids: Option<String>,
}

pub async fn pages_by_ids_handler(State(state): State<AppState>, Query(iq): Query<IdsQuery>) -> ApiResult<Json<Vec<TreeNode>>> {
    let api = api_for(&state, iq.project_id.as_deref(), Product::Confluence)?;
    let ids: Vec<String> =
        iq.ids.unwrap_or_default().split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    Ok(Json(pages_by_ids(&api, &ids).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChildrenQuery {
    project_id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    archived: Option<bool>,
}

pub async fn children_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(cq): Query<ChildrenQuery>,
) -> ApiResult<Json<ChildrenOut>> {
    let api = api_for(&state, cq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(children(&api, &id, cq.kind.as_deref().unwrap_or("page"), cq.archived.unwrap_or(false)).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PageQuery {
    project_id: Option<String>,
    version: Option<u32>,
}

pub async fn page_handler(State(state): State<AppState>, Path(id): Path<String>, Query(pq): Query<PageQuery>) -> ApiResult<Json<PageOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(get_page(&state, &api, &id, pq.version, pq.project_id.as_deref()).await?))
}

pub async fn update_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<UpdateIn>,
) -> ApiResult<Json<UpdateOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(update_page(&state, &api, &id, body).await?))
}

pub async fn create_handler(
    State(state): State<AppState>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<CreateIn>,
) -> ApiResult<Json<CreatedOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(create_page(&state, &api, body).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionsQuery {
    project_id: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
}

pub async fn versions_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(vq): Query<VersionsQuery>,
) -> ApiResult<Json<VersionsOut>> {
    let api = api_for(&state, vq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(versions(&state, &api, &id, vq.cursor.as_deref(), vq.limit.unwrap_or(50)).await?))
}

pub async fn version_handler(
    State(state): State<AppState>,
    Path((id, n)): Path<(String, u32)>,
    Query(pq): Query<ProjectQuery>,
) -> ApiResult<Json<VersionBodyOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(version_storage(&state, &api, &id, n).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentsQuery {
    project_id: Option<String>,
    /// `false`: threads without their replies (cheaper; for highlight states).
    replies: Option<bool>,
}

pub async fn comments_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(cq): Query<CommentsQuery>,
) -> ApiResult<Json<CommentsOut>> {
    let api = api_for(&state, cq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(comments_with(&state, &api, &id, cq.project_id.as_deref(), cq.replies.unwrap_or(true)).await?))
}

pub async fn add_comment_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<AddCommentIn>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(add_comment(&state, &api, &id, body).await?))
}

pub async fn search_handler(State(state): State<AppState>, Query(sq): Query<SearchIn>) -> ApiResult<Json<SearchOut>> {
    let api = api_for(&state, sq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(search(&api, &sq).await?))
}

#[derive(Debug, Deserialize)]
pub struct MarkdownIn {
    markdown: String,
}

/// Markdown → storage (the page editor's "insert markdown" and previews).
pub async fn markdown_handler(Json(body): Json<MarkdownIn>) -> ApiResult<Json<Value>> {
    if body.markdown.len() > MAX_STORAGE_BYTES {
        return Err(ApiError::bad_request("markdown is too large"));
    }
    Ok(Json(json!({ "storage": markdown::to_storage(&body.markdown) })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttachmentQuery {
    project_id: Option<String>,
    v: Option<u32>,
    download: Option<String>,
}

pub async fn attachment_handler(
    State(state): State<AppState>,
    Path((page, att)): Path<(String, String)>,
    Query(aq): Query<AttachmentQuery>,
) -> ApiResult<Response> {
    check_id(&page)?;
    let att = attachments::normalize_att_id(&att)?;
    let api = api_for(&state, aq.project_id.as_deref(), Product::Confluence)?;
    let vq = aq.v.map(|v| format!("?version={v}")).unwrap_or_default();
    let url = api.wiki(&format!("/rest/api/content/{page}/child/attachment/{att}/download{vq}"));
    let key = format!("{}|{att}|{}", api.site.base, aq.v.map(|v| v.to_string()).unwrap_or_else(|| "latest".into()));
    let download = aq.download.as_deref().is_some_and(|d| d == "1" || d == "true");
    attachments::proxy(&api, &state.atlassian.attachments, &url, key, None, download, aq.v.is_some()).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V2Attachment {
    id: String,
    title: String,
    version: V2Version,
}

pub async fn attachment_by_name_handler(
    State(state): State<AppState>,
    Path((page, name)): Path<(String, String)>,
    Query(aq): Query<AttachmentQuery>,
) -> ApiResult<Response> {
    check_id(&page)?;
    if name.is_empty() || name.len() > 255 || name.contains(['/', '\\']) {
        return Err(ApiError::bad_request("invalid attachment name"));
    }
    let api = api_for(&state, aq.project_id.as_deref(), Product::Confluence)?;
    let download = aq.download.as_deref().is_some_and(|d| d == "1" || d == "true");
    let name_key = format!("{}|{page}|name:{name}", api.site.base);
    let list: V2List<V2Attachment> =
        api.get(&api.wiki(&format!("/api/v2/pages/{page}/attachments?filename={}&limit=5", q(&name)))).await?;
    let Some(a) = list.results.into_iter().find(|a| a.title == name) else {
        return Ok(attachments::not_found_response());
    };
    let att = attachments::normalize_att_id(&a.id)?;
    let url = api.wiki(&format!("/rest/api/content/{page}/child/attachment/{att}/download"));
    let key = format!("{name_key}|{}", a.version.number);
    attachments::proxy(&api, &state.atlassian.attachments, &url, key, Some(&a.title), download, false).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_cql() {
        let s = SearchIn { q: Some("army \"cap\"".into()), space: Some("DESIGN".into()), ..Default::default() };
        assert_eq!(build_cql(&s).unwrap(), r#"type = page AND text ~ "army \"cap\"" AND space = "DESIGN""#);
        let raw = SearchIn { cql: Some(" ancestor = 458753 ".into()), q: Some("ignored".into()), ..Default::default() };
        assert_eq!(build_cql(&raw).unwrap(), "ancestor = 458753");
        assert!(build_cql(&SearchIn::default()).is_err());
        assert_eq!(cql_quote(r"a\b"), r#""a\\b""#);
    }

    #[test]
    fn rejects_non_numeric_ids() {
        assert!(check_id("458753").is_ok());
        for bad in ["", "12a", "../1", "1/children", "1?x=2", &"9".repeat(30)] {
            assert!(check_id(bad).is_err(), "{bad}");
        }
    }
}
