//! Attachment proxy. Confluence images in page HTML point at `/wiki/download/…`,
//! which API tokens cannot load, so the sanitized HTML points at these routes instead
//! and Workbench fetches the bytes with the site's credentials:
//! v1 `/rest/api/content/{page}/child/attachment/{att}/download` answers with a
//! redirect to the media service, which reqwest follows (dropping the Authorization
//! header on the cross-host hop). Jira attachments work the same way.
//!
//! Small files are kept in a byte-bounded LRU keyed by site, attachment id and
//! version; the redirect URLs themselves are short-lived and never cached.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::StreamExt;
use reqwest::Method;

use super::client::Api;
use crate::error::ApiError;

const MAX_ITEM: u64 = 8 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Cached {
    pub data: Bytes,
    pub content_type: String,
    pub filename: Option<String>,
}

#[derive(Default)]
pub struct Lru {
    map: HashMap<String, (u64, Cached)>,
    bytes: usize,
    tick: u64,
}

impl Lru {
    pub fn get(&mut self, key: &str) -> Option<Cached> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|(t, c)| {
            *t = tick;
            c.clone()
        })
    }

    pub fn put(&mut self, key: String, value: Cached) {
        if value.data.len() as u64 > MAX_ITEM {
            return;
        }
        self.tick += 1;
        if let Some((_, old)) = self.map.insert(key, (self.tick, value.clone())) {
            self.bytes -= old.data.len();
        }
        self.bytes += value.data.len();
        while self.bytes > MAX_TOTAL {
            let Some(oldest) = self.map.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone()) else { break };
            if let Some((_, c)) = self.map.remove(&oldest) {
                self.bytes -= c.data.len();
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.bytes
    }

}

/// Types the browser may render inline. Everything else is served as a download.
fn inline_ok(ct: &str) -> bool {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    ct.starts_with("image/") || ct == "application/pdf" || ct == "text/plain" || ct.starts_with("video/") || ct.starts_with("audio/")
}

/// A filename safe for a quoted `Content-Disposition` value.
fn safe_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\' | '/' | ';'))
        .take(180)
        .collect::<String>()
}

fn disposition(filename: Option<&str>, content_type: &str, download: bool) -> String {
    let kind = if download || !inline_ok(content_type) { "attachment" } else { "inline" };
    match filename.map(safe_filename).filter(|f| !f.is_empty()) {
        Some(f) => {
            let ascii: String = f.chars().map(|c| if c.is_ascii() { c } else { '_' }).collect();
            format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{}", urlencoding::encode(&f))
        }
        None => kind.to_string(),
    }
}

/// Filename from an upstream `Content-Disposition` (either form).
fn upstream_filename(h: &reqwest::header::HeaderMap) -> Option<String> {
    let v = h.get(header::CONTENT_DISPOSITION)?.to_str().ok()?;
    if let Some(i) = v.find("filename*=") {
        let raw = v[i + 10..].split(';').next()?.trim().trim_matches('"');
        let enc = raw.rsplit("''").next()?;
        return urlencoding::decode(enc).ok().map(|s| s.into_owned());
    }
    let i = v.find("filename=")?;
    Some(v[i + 9..].split(';').next()?.trim().trim_matches('"').to_string())
}

fn respond(data: Body, content_type: &str, filename: Option<&str>, download: bool, pinned: bool, len: Option<u64>) -> Response {
    let mut resp = data.into_response();
    let h = resp.headers_mut();
    let ct = HeaderValue::from_str(content_type).unwrap_or(HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_TYPE, ct);
    if let Ok(v) = HeaderValue::from_str(&disposition(filename, content_type, download)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Some(n) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(n));
    }
    // Served from Workbench's origin: never let an attachment run script here.
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; img-src data:; style-src 'unsafe-inline'; media-src 'self'; sandbox"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if pinned { "private, max-age=86400, immutable" } else { "private, max-age=300" }),
    );
    resp
}

/// Fetch `url` with the site's credentials and stream it to the browser, caching
/// small files under `cache_key`.
pub async fn proxy(
    api: &Api,
    cache: &parking_lot::Mutex<Lru>,
    url: &str,
    cache_key: String,
    fallback_name: Option<&str>,
    download: bool,
    pinned: bool,
) -> Result<Response, ApiError> {
    if let Some(c) = cache.lock().get(&cache_key) {
        let len = c.data.len() as u64;
        return Ok(respond(Body::from(c.data), &c.content_type, c.filename.as_deref(), download, pinned, Some(len)));
    }
    let resp = api.ok(Method::GET, url, None).await?;
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let filename = upstream_filename(resp.headers()).or_else(|| fallback_name.map(str::to_string));
    match resp.content_length() {
        Some(n) if n <= MAX_ITEM => {
            let data = resp.bytes().await.map_err(|e| ApiError::upstream(e.without_url().to_string()))?;
            cache.lock().put(
                cache_key,
                Cached { data: data.clone(), content_type: content_type.clone(), filename: filename.clone() },
            );
            let len = data.len() as u64;
            Ok(respond(Body::from(data), &content_type, filename.as_deref(), download, pinned, Some(len)))
        }
        len => {
            let stream = resp.bytes_stream().map(|r| r.map_err(std::io::Error::other));
            Ok(respond(Body::from_stream(stream), &content_type, filename.as_deref(), download, pinned, len))
        }
    }
}

/// Attachment ids arrive as `att123` or `123`; the v1 download path wants `att123`.
pub fn normalize_att_id(id: &str) -> Result<String, ApiError> {
    let digits = id.strip_prefix("att").unwrap_or(id);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 20 {
        return Err(ApiError::bad_request("invalid attachment id"));
    }
    Ok(format!("att{digits}"))
}

pub fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, "attachment not found").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: usize) -> Cached {
        Cached { data: Bytes::from(vec![0u8; n]), content_type: "image/png".into(), filename: None }
    }

    #[test]
    fn lru_evicts_least_recently_used_by_bytes() {
        let mut lru = Lru::default();
        let mb = 1024 * 1024;
        for i in 0..8 {
            lru.put(format!("k{i}"), item(8 * mb));
        }
        assert_eq!(lru.total_bytes(), 64 * mb);
        assert!(lru.get("k0").is_some()); // k0 is now the most recent
        lru.put("k8".into(), item(8 * mb));
        assert!(lru.get("k1").is_none(), "k1 was the least recently used");
        assert!(lru.get("k0").is_some());
        assert!(lru.total_bytes() <= MAX_TOTAL);
        lru.put("huge".into(), item(9 * mb));
        assert!(lru.get("huge").is_none(), "items over the per-item cap are not cached");
        lru.put("k0".into(), item(1));
        assert_eq!(lru.len(), 8);
    }

    #[test]
    fn dispositions_are_safe() {
        assert_eq!(disposition(Some("a.png"), "image/png", false), "inline; filename=\"a.png\"; filename*=UTF-8''a.png");
        assert!(disposition(Some("x.html"), "text/html", false).starts_with("attachment;"));
        assert!(disposition(Some("x.svg"), "image/svg+xml", true).starts_with("attachment;"));
        let d = disposition(Some("evil\"\r\nX: y/../ü.png"), "image/png", false);
        assert!(!d.contains('\r') && !d.contains('\n') && !d.contains("\"\r"));
        assert!(d.contains("filename*=UTF-8''evilX%3A%20y..%C3%BC.png"), "{d}");
    }

    #[test]
    fn parses_upstream_filenames() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline; filename=\"chantz.svg\""));
        assert_eq!(upstream_filename(&h).as_deref(), Some("chantz.svg"));
        h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename*=UTF-8''my%20file.png"));
        assert_eq!(upstream_filename(&h).as_deref(), Some("my file.png"));
    }

    #[test]
    fn attachment_ids() {
        assert_eq!(normalize_att_id("att229517").unwrap(), "att229517");
        assert_eq!(normalize_att_id("229517").unwrap(), "att229517");
        assert!(normalize_att_id("att").is_err());
        assert!(normalize_att_id("../x").is_err());
    }
}
