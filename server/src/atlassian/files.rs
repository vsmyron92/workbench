//! Page attachments: list (v2), upload (v1 multipart, `X-Atlassian-Token: no-check`),
//! delete to the trash (v2). Downloads and previews use the proxy in `attachments`.
//!
//! Uploads stream from the browser through Workbench to Confluence: the request must
//! say how long it is (`Content-Length`), may not exceed `MAX_UPLOAD_BYTES`, and is not
//! retried on `429` (a streamed body cannot be replayed).

use std::collections::HashSet;
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use futures::StreamExt;
use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::attachments::normalize_att_id;
use super::client::{Api, Product, q};
use super::confluence::{V2Version, check_id, user_names};
use super::api_for;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Largest attachment Workbench uploads (Confluence Cloud's own default limit is 100 MB).
pub const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;
const MAX_LIST: usize = 500;
/// Uploads may take far longer than the client's default 60 s.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct V2Attachment {
    id: String,
    title: String,
    status: String,
    media_type: Option<String>,
    file_size: Option<u64>,
    comment: Option<String>,
    created_at: Option<String>,
    page_id: Option<String>,
    version: V2Version,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentOut {
    /// `att123`.
    pub id: String,
    pub title: String,
    pub media_type: String,
    pub file_size: Option<u64>,
    pub comment: String,
    pub created_at: Option<String>,
    pub version: u32,
    pub author_id: String,
    pub author_name: Option<String>,
    /// Workbench's proxy for the bytes (inline for images, PDFs, text, media).
    pub download_url: String,
    pub is_image: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentsOut {
    pub attachments: Vec<AttachmentOut>,
    pub truncated: bool,
}

/// A file name Confluence can store and Workbench can put in a URL.
pub fn check_name(name: &str) -> Result<String, ApiError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 255 || n.contains(['/', '\\']) || n.chars().any(char::is_control) || n == "." || n == ".." {
        return Err(ApiError::bad_request(format!("invalid attachment name {name:?}")));
    }
    Ok(n.to_string())
}

fn proxy_url(page_id: &str, att: &str, version: u32, project_id: Option<&str>) -> String {
    let mut u = format!("/api/confluence/attachments/{page_id}/{att}?v={version}");
    if let Some(p) = project_id {
        u.push_str(&format!("&projectId={}", q(p)));
    }
    u
}

fn is_image(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

pub async fn list(state: &AppState, api: &Api, page_id: &str, project_id: Option<&str>) -> Result<AttachmentsOut, ApiError> {
    check_id(page_id)?;
    let url = api.wiki(&format!("/api/v2/pages/{page_id}/attachments?limit=100&sort=-created-date"));
    let (items, truncated) = api.v2_all::<V2Attachment>(url, MAX_LIST).await?;
    let authors: Vec<String> = items.iter().map(|a| a.version.author_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    let names = user_names(state, api, &authors).await;
    let attachments = items
        .into_iter()
        .filter(|a| a.status != "trashed")
        .filter_map(|a| {
            let id = normalize_att_id(&a.id).ok()?;
            let media_type = a.media_type.unwrap_or_else(|| "application/octet-stream".into());
            Some(AttachmentOut {
                download_url: proxy_url(page_id, &id, a.version.number, project_id),
                is_image: is_image(&media_type),
                id,
                title: a.title,
                media_type,
                file_size: a.file_size,
                comment: a.comment.unwrap_or_default(),
                created_at: a.created_at.or(Some(a.version.created_at.clone())).filter(|c| !c.is_empty()),
                version: a.version.number,
                author_name: names.get(&a.version.author_id).cloned(),
                author_id: a.version.author_id,
            })
        })
        .collect();
    Ok(AttachmentsOut { attachments, truncated })
}

/// What to upload: a name, its media type, its length and the bytes.
pub struct Upload {
    pub name: String,
    pub media_type: String,
    pub len: u64,
    pub body: reqwest::Body,
    pub comment: Option<String>,
    /// Add a new version when an attachment of that name exists (else that is a 409).
    pub replace: bool,
}

/// The v1 answer to an upload: a content array with the attachment.
fn uploaded(v: &Value, page_id: &str, project_id: Option<&str>) -> Option<AttachmentOut> {
    let a = v.get("results").and_then(Value::as_array).and_then(|r| r.first()).unwrap_or(v);
    let id = normalize_att_id(a.get("id")?.as_str()?).ok()?;
    let media_type = a
        .pointer("/extensions/mediaType")
        .or_else(|| a.pointer("/metadata/mediaType"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .to_string();
    let version = a.pointer("/version/number").and_then(Value::as_u64).unwrap_or(1) as u32;
    Some(AttachmentOut {
        download_url: proxy_url(page_id, &id, version, project_id),
        is_image: is_image(&media_type),
        title: a.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
        file_size: a.pointer("/extensions/fileSize").and_then(Value::as_u64),
        comment: a.pointer("/extensions/comment").and_then(Value::as_str).unwrap_or_default().to_string(),
        created_at: a.pointer("/version/when").and_then(Value::as_str).map(str::to_string),
        author_id: a.pointer("/version/by/accountId").and_then(Value::as_str).unwrap_or_default().to_string(),
        author_name: a.pointer("/version/by/displayName").and_then(Value::as_str).map(str::to_string),
        media_type,
        version,
        id,
    })
}

pub async fn upload(state: &AppState, api: &Api, page_id: &str, up: Upload, project_id: Option<&str>) -> Result<AttachmentOut, ApiError> {
    check_id(page_id)?;
    let name = check_name(&up.name)?;
    if up.len > MAX_UPLOAD_BYTES {
        return Err(too_large());
    }
    let part = Part::stream_with_length(up.body, up.len)
        .file_name(name.clone())
        .mime_str(&up.media_type)
        .map_err(|_| ApiError::bad_request("invalid content type"))?;
    let mut form = Form::new().part("file", part).text("minorEdit", "true");
    if let Some(c) = up.comment.filter(|c| !c.trim().is_empty()) {
        let c: String = c.chars().take(255).collect();
        let part = Part::text(c).mime_str("text/plain; charset=utf-8").map_err(|_| ApiError::bad_request("invalid comment"))?;
        form = form.part("comment", part);
    }
    // PUT creates or adds a version; POST refuses a name that exists.
    let method = if up.replace { Method::PUT } else { Method::POST };
    let url = api.wiki(&format!("/rest/api/content/{page_id}/child/attachment"));
    let resp = api.send_form(method, &url, form, UPLOAD_TIMEOUT).await?;
    if !resp.status().is_success() {
        let e = api.error_from(resp).await;
        let m = e.message.to_ascii_lowercase();
        if e.code == "bad_request" && (m.contains("same file name") || m.contains("already exists")) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "exists",
                format!("An attachment named “{name}” already exists on this page: replace it (a new version) or rename the file"),
            ));
        }
        return Err(e);
    }
    let v: Value = api.json(resp).await?;
    let out = uploaded(&v, page_id, project_id).ok_or_else(|| ApiError::upstream("Confluence did not describe the uploaded attachment"))?;
    state.events.emit("confluence.page", None, json!({ "pageId": page_id, "action": "attachment-added", "title": out.title }));
    Ok(out)
}

/// Move an attachment to the trash (it can be restored in Confluence).
pub async fn delete(state: &AppState, api: &Api, att: &str) -> Result<Value, ApiError> {
    let att = normalize_att_id(att)?;
    let cur: V2Attachment = api.get(&api.wiki(&format!("/api/v2/attachments/{att}"))).await?;
    api.ok(Method::DELETE, &api.wiki(&format!("/api/v2/attachments/{att}")), None).await?;
    if let Some(p) = &cur.page_id {
        state.events.emit("confluence.page", None, json!({ "pageId": p, "action": "attachment-deleted", "title": cur.title }));
    }
    Ok(json!({ "ok": true, "pageId": cur.page_id, "title": cur.title }))
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too_large",
        format!("attachments are limited to {} MB", MAX_UPLOAD_BYTES / 1024 / 1024),
    )
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UploadQuery {
    project_id: Option<String>,
    name: String,
    comment: Option<String>,
    replace: Option<bool>,
}

pub async fn list_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pq): Query<super::confluence::ProjectQuery>,
) -> ApiResult<Json<AttachmentsOut>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(list(&state, &api, &id, pq.project_id.as_deref()).await?))
}

/// `POST /api/confluence/pages/{id}/attachments?name=&comment=&replace=` with the raw
/// file as the body.
pub async fn upload_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(uq): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<AttachmentOut>> {
    check_id(&id)?;
    let name = check_name(&uq.name)?;
    let len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| ApiError::new(StatusCode::LENGTH_REQUIRED, "length_required", "the upload must say its length (Content-Length)"))?;
    if len > MAX_UPLOAD_BYTES {
        return Err(too_large());
    }
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty() && v != "application/x-www-form-urlencoded" && v.contains('/'))
        .unwrap_or_else(|| mime_guess::from_path(&name).first_or_octet_stream().essence_str().to_string());
    let api = api_for(&state, uq.project_id.as_deref(), Product::Confluence)?;
    // Count what arrives: never forward more than was announced.
    let mut seen = 0u64;
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        seen += chunk.len() as u64;
        if seen > len {
            return Err(std::io::Error::other("the upload is longer than its Content-Length"));
        }
        Ok::<_, std::io::Error>(chunk)
    });
    let up = Upload {
        name,
        media_type,
        len,
        body: reqwest::Body::wrap_stream(stream),
        comment: uq.comment,
        replace: uq.replace.unwrap_or(false),
    };
    Ok(Json(upload(&state, &api, &id, up, uq.project_id.as_deref()).await?))
}

pub async fn delete_handler(
    State(state): State<AppState>,
    Path(att): Path<String>,
    Query(pq): Query<super::confluence::ProjectQuery>,
) -> ApiResult<Json<Value>> {
    let api = api_for(&state, pq.project_id.as_deref(), Product::Confluence)?;
    Ok(Json(delete(&state, &api, &att).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert_eq!(check_name(" diagram v2.png ").unwrap(), "diagram v2.png");
        for bad in ["", "a/b.png", "..", "a\\b", "x\u{7}.png", &"x".repeat(256)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn describes_v1_upload_answers() {
        let v = json!({ "results": [{ "id": "att9001", "type": "attachment", "title": "a.png",
            "version": { "number": 2, "when": "2026-09-27T10:00:00.000Z", "by": { "accountId": "u1", "displayName": "Ann" } },
            "extensions": { "mediaType": "image/png", "fileSize": 42, "comment": "c" } }], "size": 1 });
        let a = uploaded(&v, "2001", Some("proj")).unwrap();
        assert_eq!((a.id.as_str(), a.version, a.is_image, a.file_size), ("att9001", 2, true, Some(42)));
        assert_eq!(a.download_url, "/api/confluence/attachments/2001/att9001?v=2&projectId=proj");
        assert_eq!(a.author_name.as_deref(), Some("Ann"));
        assert!(uploaded(&json!({ "results": [] }), "1", None).is_none());
    }
}
