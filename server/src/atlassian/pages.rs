//! Page operations beyond editing: labels (v1), move and copy (v1), delete to the
//! trash and restore (v2), watching (v1), and the user search behind @mentions (v1).

use axum::Json;
use axum::extract::{Path, Query, State};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{Api, Product, q};
use super::confluence::{CreatedOut, ProjectQuery, V2Page, check_id, check_title, cql_quote, web};
use super::api_for;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

// ---------------------------------------------------------------- labels

/// Characters Confluence refuses in labels.
const LABEL_FORBIDDEN: &[char] = &['!', '#', '&', '(', ')', '*', ',', '.', ':', ';', '<', '>', '?', '@', '[', ']', '^'];

/// Confluence labels are lowercase words: no spaces, none of `LABEL_FORBIDDEN`.
pub fn check_label(name: &str) -> Result<String, ApiError> {
    let l = name.trim().to_lowercase();
    if l.is_empty() || l.chars().count() > 255 || l.contains(char::is_whitespace) || l.contains(LABEL_FORBIDDEN) || l.contains(char::is_control) {
        return Err(ApiError::bad_request(format!(
            "invalid label {name:?}: labels are single words without spaces or any of {}",
            LABEL_FORBIDDEN.iter().collect::<String>()
        )));
    }
    Ok(l)
}

fn label_names(v: &Value) -> Vec<String> {
    v.get("results")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default()
}

pub async fn labels(api: &Api, page_id: &str) -> Result<Vec<String>, ApiError> {
    check_id(page_id)?;
    let v: Value = api.get(&api.wiki(&format!("/rest/api/content/{page_id}/label?limit=200"))).await?;
    Ok(label_names(&v))
}

/// Add labels; returns the page's labels afterwards.
pub async fn add_labels(state: &AppState, api: &Api, page_id: &str, names: &[String]) -> Result<Vec<String>, ApiError> {
    check_id(page_id)?;
    let mut list: Vec<String> = names.iter().map(|n| check_label(n)).collect::<Result<_, _>>()?;
    list.dedup();
    if list.is_empty() {
        return Err(ApiError::bad_request("no labels to add"));
    }
    if list.len() > 50 {
        return Err(ApiError::bad_request("add at most 50 labels at a time"));
    }
    let body: Vec<Value> = list.iter().map(|n| json!({ "prefix": "global", "name": n })).collect();
    let v: Value = api.send_json(Method::POST, &api.wiki(&format!("/rest/api/content/{page_id}/label")), &Value::Array(body)).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "labels" }));
    Ok(label_names(&v))
}

pub async fn remove_label(state: &AppState, api: &Api, page_id: &str, name: &str) -> Result<(), ApiError> {
    check_id(page_id)?;
    let l = check_label(name)?;
    // The query form also takes names the path form would refuse.
    api.ok(Method::DELETE, &api.wiki(&format!("/rest/api/content/{page_id}/label?name={}", q(&l))), None).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "labels" }));
    Ok(())
}

// ---------------------------------------------------------------- move and copy

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoveIn {
    /// before | after (a sibling of `target_id`) | append (a child of `target_id`).
    pub position: String,
    pub target_id: String,
}

pub async fn move_page(state: &AppState, api: &Api, page_id: &str, input: &MoveIn) -> Result<Value, ApiError> {
    check_id(page_id)?;
    check_id(&input.target_id)?;
    let position = match input.position.as_str() {
        p @ ("before" | "after" | "append") => p,
        _ => return Err(ApiError::bad_request("position must be before, after or append")),
    };
    if page_id == input.target_id {
        return Err(ApiError::bad_request("a page cannot be moved relative to itself"));
    }
    if position != "append" {
        // Before/after a top-level page would make this page top-level too, and
        // Confluence's page tree does not show top-level pages.
        let target: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{}", input.target_id))).await?;
        if target.parent_id.is_none() {
            return Err(ApiError::bad_request(
                "that would make the page a top-level page, which Confluence's page tree hides; move it into a page instead",
            ));
        }
    }
    let url = api.wiki(&format!("/rest/api/content/{page_id}/move/{position}/{}", input.target_id));
    api.ok(Method::PUT, &url, None).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "moved" }));
    Ok(json!({ "ok": true, "pageId": page_id }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CopyIn {
    /// Title of the copy (default "Copy of …").
    pub title: Option<String>,
    /// New parent page; else `space_key` (a root page there); else the original's parent.
    pub parent_id: Option<String>,
    pub space_key: Option<String>,
    pub copy_attachments: bool,
    pub copy_labels: bool,
}

pub async fn copy_page(state: &AppState, api: &Api, page_id: &str, input: CopyIn) -> Result<CreatedOut, ApiError> {
    check_id(page_id)?;
    let orig: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{page_id}"))).await?;
    let title = input.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("Copy of {}", orig.title));
    check_title(&title)?;
    let destination = match (input.parent_id.filter(|p| !p.is_empty()), input.space_key.filter(|s| !s.is_empty())) {
        (Some(p), _) => {
            check_id(&p)?;
            json!({ "type": "parent_page", "value": p })
        }
        (None, Some(key)) => {
            if key.len() > 255 || key.contains(['&', '?', '#', '/', ' ']) {
                return Err(ApiError::bad_request(format!("invalid space key {key:?}")));
            }
            json!({ "type": "space", "value": key })
        }
        (None, None) => match orig.parent_id.as_deref() {
            Some(p) => json!({ "type": "parent_page", "value": p }),
            None => {
                return Err(ApiError::bad_request(format!(
                    "“{}” is a top-level page: choose a page to put the copy under (parentId), or spaceKey for a top-level copy",
                    orig.title
                )));
            }
        },
    };
    let body = json!({
        "destination": destination,
        "pageTitle": title,
        "copyAttachments": input.copy_attachments,
        "copyLabels": input.copy_labels,
        "copyPermissions": false,
        "copyProperties": false,
        "copyCustomContents": false,
    });
    let v: Value = api.send_json(Method::POST, &api.wiki(&format!("/rest/api/content/{page_id}/copy")), &body).await?;
    let id = v.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| ApiError::upstream("Confluence did not say which page it created"))?;
    let title = v.get("title").and_then(Value::as_str).unwrap_or(&title).to_string();
    state.events.emit("confluence.page", None, json!({ "pageId": id, "title": title, "action": "created" }));
    Ok(CreatedOut {
        web_url: web(api, v.pointer("/_links/webui").and_then(Value::as_str)).unwrap_or_default(),
        version: v.pointer("/version/number").and_then(Value::as_u64).unwrap_or(1) as u32,
        space_id: orig.space_id,
        title,
        id,
    })
}

// ---------------------------------------------------------------- trash

/// Move a page to the trash (Confluence keeps it there until an admin purges it).
pub async fn trash_page(state: &AppState, api: &Api, page_id: &str) -> Result<Value, ApiError> {
    check_id(page_id)?;
    let cur: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{page_id}"))).await?;
    api.ok(Method::DELETE, &api.wiki(&format!("/api/v2/pages/{page_id}")), None).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "title": cur.title, "action": "deleted" }));
    Ok(json!({ "ok": true, "title": cur.title, "spaceId": cur.space_id, "parentId": cur.parent_id }))
}

/// Restore a trashed page: v2 `PUT` with status `current` (Confluence changes only the status).
pub async fn restore_page(state: &AppState, api: &Api, page_id: &str) -> Result<Value, ApiError> {
    check_id(page_id)?;
    let cur: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{page_id}?status=trashed&body-format=storage"))).await?;
    if cur.status != "trashed" {
        return Err(ApiError::bad_request(format!("the page is {}, not in the trash", if cur.status.is_empty() { "unknown" } else { &cur.status })));
    }
    let body = json!({
        "id": page_id,
        "status": "current",
        "title": cur.title,
        "body": { "representation": "storage", "value": cur.body.storage.as_ref().map(|r| r.value.as_str()).unwrap_or("") },
        "version": { "number": cur.version.number + 1, "message": "Restored from the trash" },
    });
    api.ok(Method::PUT, &api.wiki(&format!("/api/v2/pages/{page_id}")), Some(&body)).await?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "title": cur.title, "action": "restored" }));
    Ok(json!({ "ok": true, "title": cur.title }))
}

// ---------------------------------------------------------------- watching

pub async fn watching(api: &Api, page_id: &str) -> Result<bool, ApiError> {
    check_id(page_id)?;
    let v: Value = api.get(&api.wiki(&format!("/rest/api/user/watch/content/{page_id}"))).await?;
    Ok(v.get("watching").and_then(Value::as_bool).unwrap_or(false))
}

pub async fn set_watching(api: &Api, page_id: &str, watch: bool) -> Result<bool, ApiError> {
    check_id(page_id)?;
    let url = api.wiki(&format!("/rest/api/user/watch/content/{page_id}"));
    api.ok(if watch { Method::POST } else { Method::DELETE }, &url, None).await?;
    Ok(watch)
}

// ---------------------------------------------------------------- people

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserHit {
    pub account_id: String,
    pub display_name: String,
    pub email: Option<String>,
}

/// People whose name starts with `query` (`user.fullname ~ "…"`), for @mentions.
pub async fn search_users(state: &AppState, api: &Api, query: &str, limit: u32) -> Result<Vec<UserHit>, ApiError> {
    let text = query.trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    if text.chars().count() > 100 {
        return Err(ApiError::bad_request("the name to search for is too long"));
    }
    let cql = format!("user.fullname ~ {}", cql_quote(text));
    let url = api.wiki(&format!("/rest/api/search/user?cql={}&limit={}", q(&cql), limit.clamp(1, 25)));
    let v: Value = api.get(&url).await?;
    let hits: Vec<UserHit> = v
        .get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let u = r.get("user")?;
                    if u.get("type").and_then(Value::as_str) == Some("app") {
                        return None;
                    }
                    let id = u.get("accountId").and_then(Value::as_str)?.to_string();
                    let name = u
                        .get("displayName")
                        .or_else(|| u.get("publicName"))
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .or_else(|| r.get("title").and_then(Value::as_str))
                        .unwrap_or("Someone")
                        .to_string();
                    Some(UserHit { account_id: id, display_name: name, email: u.get("email").and_then(Value::as_str).filter(|e| !e.is_empty()).map(str::to_string) })
                })
                .collect()
        })
        .unwrap_or_default();
    for h in &hits {
        state.atlassian.remember_name(format!("{}|{}", api.site.base, h.account_id), h.display_name.clone());
    }
    Ok(hits)
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LabelsIn {
    names: Vec<String>,
}

pub async fn labels_handler(State(state): State<AppState>, Path(id): Path<String>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(json!({ "labels": labels(&api, &id).await? })))
}

pub async fn add_labels_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<LabelsIn>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(json!({ "labels": add_labels(&state, &api, &id, &body.names).await? })))
}

pub async fn remove_label_handler(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Query(pq): Query<ProjectQuery>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    remove_label(&state, &api, &id, &name).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn move_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<MoveIn>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(move_page(&state, &api, &id, &body).await?))
}

pub async fn copy_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<CopyIn>,
) -> ApiResult<Json<CreatedOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(copy_page(&state, &api, &id, body).await?))
}

pub async fn trash_handler(State(state): State<AppState>, Path(id): Path<String>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(trash_page(&state, &api, &id).await?))
}

pub async fn restore_handler(State(state): State<AppState>, Path(id): Path<String>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(restore_page(&state, &api, &id).await?))
}

pub async fn watch_get_handler(State(state): State<AppState>, Path(id): Path<String>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(json!({ "watching": watching(&api, &id).await? })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WatchIn {
    watching: bool,
}

pub async fn watch_put_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<WatchIn>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(json!({ "watching": set_watching(&api, &id, body.watching).await? })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsersQuery {
    project_id: Option<String>,
    q: String,
    limit: Option<u32>,
}

pub async fn users_handler(State(state): State<AppState>, Query(uq): Query<UsersQuery>) -> ApiResult<Json<Vec<UserHit>>> {
    let api = api_for(&state, uq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(search_users(&state, &api, &uq.q, uq.limit.unwrap_or(8)).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_checked_and_lowercased() {
        assert_eq!(check_label(" Design-Review ").unwrap(), "design-review");
        assert_eq!(check_label("v2_final").unwrap(), "v2_final");
        for bad in ["", "two words", "a.b", "x:y", "tag#1", "@me", &"x".repeat(256)] {
            assert!(check_label(bad).is_err(), "{bad:?}");
        }
    }
}
