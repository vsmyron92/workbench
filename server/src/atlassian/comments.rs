//! Confluence comment writes (v2): inline comments on a text selection, edits,
//! resolve / reopen and deletes of inline and footer comments. Replies and new footer
//! comments go through `confluence::add_comment`.
//!
//! An inline comment is anchored by `inlineCommentProperties {textSelection,
//! textSelectionMatchCount, textSelectionMatchIndex}`: the selected text, how often it
//! occurs in the page and which occurrence is meant (zero-based). Confluence matches
//! the selection against the rendered page, so the count is taken here from the same
//! sanitized view HTML the UI shows; a UI whose count differs is looking at an older
//! version and gets a 409.

use axum::Json;
use axum::extract::{Path, Query, State};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{Api, Product};
use super::confluence::{self, ProjectQuery, V2Comment, V2Page, body_of, check_id};
use super::html::{occurrences, text_content};
use super::api_for;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Longest selection an inline comment may anchor to.
const MAX_SELECTION_CHARS: usize = 1000;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateInlineIn {
    pub markdown: Option<String>,
    pub storage: Option<String>,
    /// The exact text the comment is about (one paragraph at most).
    pub selection: String,
    /// Which occurrence of `selection` in the page (zero-based). Required when the text
    /// occurs more than once.
    pub match_index: Option<u32>,
    /// How often the UI saw `selection` in the page; must equal the server's count.
    pub match_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineCreated {
    pub id: String,
    pub marker_ref: Option<String>,
    pub match_count: u32,
    pub match_index: u32,
}

fn check_selection(sel: &str) -> Result<(), ApiError> {
    if sel.trim().is_empty() {
        return Err(ApiError::bad_request("select the text the comment is about"));
    }
    if sel.contains(['\n', '\r']) {
        return Err(ApiError::bad_request("an inline comment can only be anchored to text within one paragraph"));
    }
    if sel.chars().count() > MAX_SELECTION_CHARS {
        return Err(ApiError::bad_request(format!("select at most {MAX_SELECTION_CHARS} characters for an inline comment")));
    }
    Ok(())
}

/// The page's text as the page view shows it (the sanitized `view` rendering).
async fn page_text(api: &Api, page_id: &str, project_id: Option<&str>) -> Result<String, ApiError> {
    let p: V2Page = api.get(&api.wiki(&format!("/api/v2/pages/{page_id}?body-format=view"))).await?;
    if p.status == "archived" {
        return Err(ApiError::bad_request("This page is archived; restore it in Confluence before commenting on it"));
    }
    let view = p.body.view.as_ref().map(|r| r.value.as_str()).unwrap_or("");
    Ok(text_content(&confluence::rewrite(api, project_id).render(view)))
}

/// Which occurrence to anchor to, checked against the page's current text.
pub fn resolve_occurrence(text: &str, sel: &str, index: Option<u32>, seen: Option<u32>) -> Result<(u32, u32), ApiError> {
    let count = occurrences(text, sel).len() as u32;
    let short: String = sel.chars().take(60).collect();
    if count == 0 {
        return Err(ApiError::bad_request(format!(
            "“{short}” is not in the current version of the page (the page may have changed: reload it and select the text again)"
        )));
    }
    if let Some(seen) = seen {
        if seen != count {
            return Err(ApiError::conflict(format!(
                "“{short}” occurs {count} time(s) in the current page but {seen} time(s) in the version you are looking at; reload the page and select the text again"
            )));
        }
    }
    let index = match index {
        Some(i) if i < count => i,
        Some(i) => return Err(ApiError::bad_request(format!("occurrence {} does not exist: “{short}” occurs {count} time(s)", i + 1))),
        None if count == 1 => 0,
        None => {
            return Err(ApiError::bad_request(format!(
                "“{short}” occurs {count} times in the page: say which occurrence (or select a longer, unique passage)"
            )));
        }
    };
    Ok((count, index))
}

pub async fn create_inline(
    state: &AppState,
    api: &Api,
    page_id: &str,
    input: CreateInlineIn,
    project_id: Option<&str>,
) -> Result<InlineCreated, ApiError> {
    check_id(page_id)?;
    check_selection(&input.selection)?;
    let value = body_of(input.storage, input.markdown)?;
    if value.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    let text = page_text(api, page_id, project_id).await?;
    let (count, index) = resolve_occurrence(&text, &input.selection, input.match_index, input.match_count)?;
    let payload = json!({
        "pageId": page_id,
        "body": { "representation": "storage", "value": value },
        "inlineCommentProperties": {
            "textSelection": input.selection,
            "textSelectionMatchCount": count,
            "textSelectionMatchIndex": index,
        },
    });
    let created: Value = api.send_json(Method::POST, &api.wiki("/api/v2/inline-comments"), &payload).await?;
    let id = created.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let marker_ref = created.pointer("/properties/inlineMarkerRef").and_then(Value::as_str).map(str::to_string);
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "commented" }));
    Ok(InlineCreated { id, marker_ref, match_count: count, match_index: index })
}

fn kind_of(kind: &str) -> Result<&'static str, ApiError> {
    match kind {
        "inline" => Ok("inline"),
        "footer" => Ok("footer"),
        _ => Err(ApiError::bad_request("comment kind must be inline or footer")),
    }
}

async fn current(api: &Api, kind: &str, id: &str) -> Result<V2Comment, ApiError> {
    check_id(id)?;
    api.get(&api.wiki(&format!("/api/v2/{kind}-comments/{id}?body-format=storage"))).await
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateCommentIn {
    pub markdown: Option<String>,
    pub storage: Option<String>,
    /// The comment version the edit started from; the edit is refused if it changed.
    pub version: Option<u32>,
    /// Inline comments: resolve (true) or reopen (false).
    pub resolved: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentWritten {
    pub id: String,
    pub page_id: Option<String>,
    pub version: u32,
    pub resolution_status: Option<String>,
}

/// Edit a comment's body and/or (inline) resolve or reopen it: `PUT` with version + 1.
pub async fn update_comment(state: &AppState, api: &Api, kind: &str, id: &str, input: UpdateCommentIn) -> Result<CommentWritten, ApiError> {
    let kind = kind_of(kind)?;
    let edits_body = input.markdown.is_some() || input.storage.is_some();
    if !edits_body && input.resolved.is_none() {
        return Err(ApiError::bad_request("nothing to change: pass a new body or resolved"));
    }
    if input.resolved.is_some() && kind != "inline" {
        return Err(ApiError::bad_request("only inline comments can be resolved or reopened"));
    }
    let cur = current(api, kind, id).await?;
    if edits_body {
        if let Some(v) = input.version {
            if v != cur.version.number {
                return Err(ApiError::conflict(format!(
                    "The comment changed while you were editing it (it is now version {}); reload and edit it again",
                    cur.version.number
                )));
            }
        }
    }
    if input.resolved.is_some() && cur.resolution_status.as_deref() == Some("dangling") {
        return Err(ApiError::bad_request(
            "This comment's highlighted text is gone from the page (dangling); Confluence cannot resolve or reopen it",
        ));
    }
    let value = if edits_body {
        let v = body_of(input.storage, input.markdown)?;
        if v.trim().is_empty() {
            return Err(ApiError::bad_request("the comment is empty"));
        }
        v
    } else {
        cur.body.storage.as_ref().map(|r| r.value.clone()).unwrap_or_default()
    };
    let mut payload = json!({
        "version": { "number": cur.version.number + 1, "message": "" },
        "body": { "representation": "storage", "value": value },
    });
    if let Some(r) = input.resolved {
        payload["resolved"] = json!(r);
    }
    let saved: V2Comment = api.send_json(Method::PUT, &api.wiki(&format!("/api/v2/{kind}-comments/{id}")), &payload).await.map_err(|e| {
        if e.code == "conflict" {
            ApiError::conflict(format!("Someone changed this comment at the same moment; reload and try again ({})", e.message))
        } else {
            e
        }
    })?;
    let page_id = saved.page_id.clone().or(cur.page_id.clone());
    let action = match input.resolved {
        Some(true) => "comment-resolved",
        Some(false) => "comment-reopened",
        None => "comment-updated",
    };
    if let Some(p) = &page_id {
        state.events.emit("confluence.page", None, json!({ "pageId": p, "action": action }));
    }
    Ok(CommentWritten {
        id: id.to_string(),
        page_id,
        version: if saved.version.number > 0 { saved.version.number } else { cur.version.number + 1 },
        resolution_status: saved.resolution_status.or(match input.resolved {
            Some(true) => Some("resolved".into()),
            Some(false) => Some("reopened".into()),
            None => cur.resolution_status,
        }),
    })
}

/// Delete a comment (and with it its replies).
pub async fn delete_comment(state: &AppState, api: &Api, kind: &str, id: &str) -> Result<Value, ApiError> {
    let kind = kind_of(kind)?;
    let cur = current(api, kind, id).await?;
    api.ok(Method::DELETE, &api.wiki(&format!("/api/v2/{kind}-comments/{id}")), None).await?;
    if let Some(p) = &cur.page_id {
        state.events.emit("confluence.page", None, json!({ "pageId": p, "action": "comment-deleted" }));
    }
    Ok(json!({ "ok": true, "pageId": cur.page_id }))
}

// ---------------------------------------------------------------- handlers

pub async fn create_inline_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<CreateInlineIn>,
) -> ApiResult<Json<InlineCreated>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(create_inline(&state, &api, &id, body, pq.project_id.as_deref()).await?))
}

pub async fn update_handler(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, String)>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<UpdateCommentIn>,
) -> ApiResult<Json<CommentWritten>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(update_comment(&state, &api, &kind, &id, body).await?))
}

pub async fn delete_handler(
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, String)>,
    Query(pq): Query<ProjectQuery>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(delete_comment(&state, &api, &kind, &id).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrences_are_resolved_against_the_page() {
        let text = "Army cap is 30. The army cap grows. Army cap!";
        assert_eq!(resolve_occurrence(text, "Army cap", Some(1), Some(2)).unwrap(), (2, 1));
        assert_eq!(resolve_occurrence(text, "grows", None, None).unwrap(), (1, 0));
        let e = resolve_occurrence(text, "Army cap", None, None).unwrap_err();
        assert!(e.message.contains("occurs 2 times"), "{}", e.message);
        assert_eq!(resolve_occurrence(text, "Army cap", Some(0), Some(3)).unwrap_err().code, "conflict");
        assert_eq!(resolve_occurrence(text, "Army cap", Some(2), None).unwrap_err().code, "bad_request");
        assert_eq!(resolve_occurrence(text, "navy", Some(0), None).unwrap_err().code, "bad_request");
    }

    #[test]
    fn selections_are_checked() {
        assert!(check_selection("one line").is_ok());
        assert!(check_selection("  ").is_err());
        assert!(check_selection("two\nlines").is_err());
        assert!(check_selection(&"x".repeat(1001)).is_err());
        assert!(kind_of("inline").is_ok() && kind_of("footer").is_ok() && kind_of("blog").is_err());
    }
}
