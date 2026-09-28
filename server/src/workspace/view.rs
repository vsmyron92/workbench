//! Sandboxed card content: `GET /view/{grant}/{*path}`.
//!
//! Reports are untrusted HTML (agents and repositories write them) and run scripts,
//! so they must never execute with Workbench's origin: that origin can spawn shells.
//! Every response here carries `Content-Security-Policy: sandbox …` without
//! `allow-same-origin`, so a report (in our iframe, or opened in a tab) gets an opaque
//! origin: no Workbench cookies, no readable `/api` responses, no access to the parent.
//!
//! The path is public (the guard lets non-`/api` paths through); the capability is
//! the grant: a random token minted by an authenticated call, scoped to one card
//! folder (plus the scope's `_shared/` assets, so `../_shared/report.css` resolves),
//! valid for 12 hours, kept in memory. Dotfiles and credential names are never
//! served, and paths are resolved with symlink containment.
//!
//! Popups a report opens stay in the sandbox (no `allow-popups-to-escape-sandbox`):
//! an escaped popup would be a normal page that keeps `opener`, and one opened on
//! Workbench's own origin would run the app there (`/#wbk=…` replaces the stored
//! device key). External links still open normally: the prelude hands them to the
//! framing Workbench page, which opens them with `noopener` (`HtmlView`), and a
//! report shown in a tab of its own follows them in that tab.
//!
//! HTML gets a small prelude (dark scrollbars, external links as above), adapted
//! from Mr. Mak Workspace (MIT), `desktop/service/report-chrome.mjs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::store;
use crate::app::AppState;
use crate::util;

pub const GRANT_TTL_MS: i64 = 12 * 3600 * 1000;
/// A grant with less than this left is not handed out again.
const REUSE_MIN_LEFT_MS: i64 = 3600 * 1000;
const MAX_GRANTS: usize = 4096;
/// How much of an HTML file is searched for `<head>`.
const SNIFF_BYTES: usize = 64 * 1024;

/// Sent with every response, including errors.
pub const CSP: &str = "sandbox allow-scripts allow-popups allow-downloads; default-src 'self' 'unsafe-inline' data: blob:";

pub const PRELUDE: &str = concat!(
    "<meta name=\"color-scheme\" content=\"dark\"><style data-workbench-chrome>",
    "html{color-scheme:dark;background-color:#1e1f22;color:#dfe1e5}",
    "html,body,body *{scrollbar-color:#4e5157 #1e1f22;scrollbar-width:thin}",
    "::-webkit-scrollbar{width:8px;height:8px}::-webkit-scrollbar-track,::-webkit-scrollbar-corner{background:#1e1f22}",
    "::-webkit-scrollbar-thumb{background:#4e5157;border:2px solid #1e1f22;border-radius:6px}",
    "</style><script data-workbench-links>(()=>{const framed=window.parent!==window;const route=e=>{",
    "const a=e.target.closest&&e.target.closest('a[href]');",
    "if(!a||a.hasAttribute('download')||e.defaultPrevented)return;let u;try{u=new URL(a.href,location.href)}catch{return}",
    "if(!['http:','https:'].includes(u.protocol)||u.origin===location.origin)return;",
    "if(framed){e.preventDefault();parent.postMessage({type:'workbench:open-link',href:u.href},'*')}",
    "else{a.target='_self'}};document.addEventListener('click',route,true);",
    "document.addEventListener('auxclick',route,true)})();</script>"
);

#[derive(Clone)]
struct Grant {
    /// The directory holding card folders (a scope dir, or a repository's `workspace/`).
    root: PathBuf,
    folder: String,
    expires_ms: i64,
}

#[derive(Default)]
pub struct Grants {
    map: Mutex<HashMap<String, Grant>>,
}

impl Grants {
    /// A grant for one card folder under `root`: an existing one with time left, or a
    /// new one. Returns the token and its expiry (ms).
    pub fn mint(&self, root: &Path, folder: &str) -> (String, i64) {
        let now = util::now_ms();
        let mut map = self.map.lock();
        map.retain(|_, g| g.expires_ms > now);
        if let Some((t, g)) = map.iter().find(|(_, g)| g.root == root && g.folder == folder && g.expires_ms - now > REUSE_MIN_LEFT_MS) {
            return (t.clone(), g.expires_ms);
        }
        if map.len() >= MAX_GRANTS {
            if let Some(oldest) = map.iter().min_by_key(|(_, g)| g.expires_ms).map(|(t, _)| t.clone()) {
                map.remove(&oldest);
            }
        }
        let token = util::random_token(24);
        let expires_ms = now + GRANT_TTL_MS;
        map.insert(token.clone(), Grant { root: root.to_path_buf(), folder: folder.to_string(), expires_ms });
        (token, expires_ms)
    }

    fn get(&self, token: &str) -> Option<Grant> {
        let g = self.map.lock().get(token).cloned()?;
        (g.expires_ms > util::now_ms()).then_some(g)
    }

    #[cfg(test)]
    fn expire_all(&self) {
        for g in self.map.lock().values_mut() {
            g.expires_ms = 0;
        }
    }
}

/// Security headers on every `/view` response.
fn harden(h: &mut HeaderMap) {
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A sandboxed report has an opaque origin, so its own module scripts, fonts and
    // fetches of files in its folder are cross-origin requests. The grant is the
    // capability; no credentials are ever honoured here.
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    let mut r = (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], msg).into_response();
    harden(r.headers_mut());
    r
}

/// Why a `/view` path is refused.
#[derive(Debug, PartialEq)]
enum Refusal {
    NotFound,
    Forbidden,
}

/// Validate the path under a grant: its first segment must be the granted folder
/// or `_shared`; no `..`, dotfiles or credential names anywhere.
fn check_view_path(grant_folder: &str, path: &str) -> Result<String, Refusal> {
    if path.contains(['\0', '\\']) {
        return Err(Refusal::Forbidden);
    }
    let mut parts = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(Refusal::Forbidden),
            s if store::is_private_name(s) => return Err(Refusal::Forbidden),
            s => parts.push(s),
        }
    }
    match parts.first() {
        Some(first) if (*first == grant_folder || *first == store::SHARED_DIR) && parts.len() > 1 => Ok(parts.join("/")),
        _ => Err(Refusal::NotFound),
    }
}

pub async fn view_root() -> Response {
    plain(StatusCode::NOT_FOUND, "not found")
}

pub async fn view(State(state): State<AppState>, UrlPath((token, path)): UrlPath<(String, String)>, headers: HeaderMap) -> Response {
    let Some(grant) = state.workspace.grants.get(&token) else {
        return plain(StatusCode::NOT_FOUND, "This link has expired. Reopen the card in Workbench.");
    };
    let rel = match check_view_path(&grant.folder, &path) {
        Ok(r) => r,
        Err(Refusal::NotFound) => return plain(StatusCode::NOT_FOUND, "not found"),
        Err(Refusal::Forbidden) => return plain(StatusCode::FORBIDDEN, "Private files are not served."),
    };
    let abs = match util::paths::resolve_in_root(&grant.root, &rel) {
        Ok(p) => p,
        Err(_) => return plain(StatusCode::FORBIDDEN, "Links that leave the card folder are not served."),
    };
    let mut resp = match serve(&abs, &headers).await {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return plain(StatusCode::NOT_FOUND, "not found"),
        Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => return plain(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::debug!("workspace view: {e}");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "cannot read this file");
        }
    };
    harden(resp.headers_mut());
    resp
}

/// Content type by extension, with a charset for text.
pub fn content_type(path: &Path) -> String {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mime = match ext.as_str() {
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "js" | "mjs" => "text/javascript",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "jsonl" | "log" | "txt" => "text/plain",
        _ => return mime_guess::from_path(path).first_raw().map(|m| if m.starts_with("text/") { format!("{m}; charset=utf-8") } else { m.to_string() }).unwrap_or_else(|| "application/octet-stream".into()),
    };
    if mime.starts_with("text/") || mime == "application/json" { format!("{mime}; charset=utf-8") } else { mime.to_string() }
}

fn is_html(path: &Path) -> bool {
    matches!(path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref(), Some("html" | "htm"))
}

async fn serve(abs: &Path, headers: &HeaderMap) -> std::io::Result<Response> {
    let mut file = tokio::fs::File::open(abs).await?;
    let md = file.metadata().await?;
    if md.is_dir() {
        return Err(std::io::Error::from(std::io::ErrorKind::IsADirectory));
    }
    let len = md.len();
    if is_html(abs) {
        return serve_html(file, len).await;
    }
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).map(|r| parse_range(r, len)).unwrap_or(RangeSpec::Full);
    let (status, start, end) = match range {
        RangeSpec::Full => (StatusCode::OK, 0, len),
        RangeSpec::Partial(s, e) => (StatusCode::PARTIAL_CONTENT, s, e + 1),
        RangeSpec::Unsatisfiable => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            r.headers_mut().insert(header::CONTENT_RANGE, hv(&format!("bytes */{len}")));
            r.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(r);
        }
    };
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(end - start), 64 * 1024);
    let mut r = Response::new(Body::from_stream(stream));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, hv(&content_type(abs)));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(header::CONTENT_RANGE, hv(&format!("bytes {start}-{}/{len}", end - 1)));
    }
    Ok(r)
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"))
}

/// HTML: the whole document (ranges are ignored), with the prelude inserted.
async fn serve_html(mut file: tokio::fs::File, len: u64) -> std::io::Result<Response> {
    let mut head = Vec::with_capacity(SNIFF_BYTES.min(len as usize));
    (&mut file).take(SNIFF_BYTES as u64).read_to_end(&mut head).await?;
    let offset = prelude_offset(&head);
    let (prelude, content_type): (&'static [u8], &str) = match offset {
        Some(_) => (PRELUDE.as_bytes(), "text/html; charset=utf-8"),
        // UTF-16: leave the bytes alone and let the BOM speak.
        None => (b"", "text/html"),
    };
    let offset = offset.unwrap_or(0);
    let rest_len = len.saturating_sub(head.len() as u64);
    let head = Bytes::from(head);
    let parts: Vec<std::io::Result<Bytes>> = vec![Ok(head.slice(..offset)), Ok(Bytes::from_static(prelude)), Ok(head.slice(offset..))];
    let rest = tokio_util::io::ReaderStream::with_capacity(file.take(rest_len), 64 * 1024);
    let body = Body::from_stream(futures::stream::iter(parts).chain(rest));
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(if content_type.contains("utf-8") { "text/html; charset=utf-8" } else { "text/html" }));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(head.len() as u64 + rest_len + prelude.len() as u64));
    Ok(r)
}

fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Where the prelude goes in the first bytes of an HTML file: right after `<head …>`,
/// else after the doctype, else after a UTF-8 BOM, else at the start. `None` for
/// UTF-16 (no injection). Byte-exact, so the BOM, doctype and charset stay intact.
pub fn prelude_offset(head: &[u8]) -> Option<usize> {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return None;
    }
    let mut from = 0;
    while let Some(i) = find_ci(head, b"<head", from) {
        match head.get(i + 5) {
            Some(b'>') => return Some(i + 6),
            Some(c) if c.is_ascii_whitespace() || *c == b'/' => {
                if let Some(end) = head[i..].iter().position(|b| *b == b'>') {
                    return Some(i + end + 1);
                }
                break;
            }
            // `<header>`, `<heading>`… keep looking.
            _ => from = i + 5,
        }
    }
    let bom = if head.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let mut i = bom;
    while head.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
        i += 1;
    }
    if head.len() >= i + 9 && head[i..i + 9].eq_ignore_ascii_case(b"<!doctype") {
        if let Some(end) = head[i..].iter().position(|b| *b == b'>') {
            return Some(i + end + 1);
        }
    }
    Some(bom)
}

#[derive(Debug, PartialEq)]
pub enum RangeSpec {
    Full,
    Partial(u64, u64),
    Unsatisfiable,
}

/// A single `bytes=` range (multi-range requests get the full body, as RFC 9110 allows).
pub fn parse_range(h: &str, len: u64) -> RangeSpec {
    let Some(spec) = h.trim().strip_prefix("bytes=") else { return RangeSpec::Full };
    if spec.contains(',') {
        return RangeSpec::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else { return RangeSpec::Full };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let Ok(n) = b.parse::<u64>() else { return RangeSpec::Full };
        if n == 0 || len == 0 {
            return RangeSpec::Unsatisfiable;
        }
        return RangeSpec::Partial(len - n.min(len), len - 1);
    }
    let Ok(start) = a.parse::<u64>() else { return RangeSpec::Full };
    if start >= len {
        return RangeSpec::Unsatisfiable;
    }
    let end = if b.is_empty() {
        len - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) if e >= start => e.min(len - 1),
            _ => return RangeSpec::Full,
        }
    };
    RangeSpec::Partial(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prelude_goes_after_head_doctype_or_bom() {
        assert_eq!(prelude_offset(b"<!DOCTYPE html><html><head><title>x"), Some(27));
        assert_eq!(prelude_offset(b"<html><HEAD lang=\"en\">x"), Some(22));
        // `<header>` is not a head.
        assert_eq!(prelude_offset(b"<!doctype html><body><header>x</header>"), Some(15));
        assert_eq!(prelude_offset(b"\xEF\xBB\xBF<!doctype html><p>"), Some(18));
        assert_eq!(prelude_offset(b"\xEF\xBB\xBF<p>hi"), Some(3));
        assert_eq!(prelude_offset(b"  \n<!DOCTYPE html>\n<p>"), Some(18));
        assert_eq!(prelude_offset(b"<p>fragment"), Some(0));
        assert_eq!(prelude_offset(b"\xFF\xFE<\0h\0"), None);
        assert_eq!(prelude_offset(b""), Some(0));
    }

    #[test]
    fn view_paths_stay_in_the_grant() {
        let f = "2026-09-15_x";
        assert_eq!(check_view_path(f, "2026-09-15_x/report.html"), Ok("2026-09-15_x/report.html".into()));
        assert_eq!(check_view_path(f, "2026-09-15_x//img/./a.png"), Ok("2026-09-15_x/img/a.png".into()));
        assert_eq!(check_view_path(f, "_shared/report.css"), Ok("_shared/report.css".into()));
        assert_eq!(check_view_path(f, "other-card/report.html"), Err(Refusal::NotFound));
        assert_eq!(check_view_path(f, "workspace.json"), Err(Refusal::NotFound));
        assert_eq!(check_view_path(f, "2026-09-15_x"), Err(Refusal::NotFound));
        assert_eq!(check_view_path(f, "2026-09-15_x/../other/x"), Err(Refusal::Forbidden));
        assert_eq!(check_view_path(f, "2026-09-15_x/.env"), Err(Refusal::Forbidden));
        assert_eq!(check_view_path(f, "2026-09-15_x/keys/id_rsa"), Err(Refusal::Forbidden));
        assert_eq!(check_view_path(f, "2026-09-15_x/a\\b"), Err(Refusal::Forbidden));
    }

    #[test]
    fn grants_are_reused_and_expire() {
        let g = Grants::default();
        let (a, exp) = g.mint(Path::new("/r"), "f");
        assert!(exp > util::now_ms());
        assert_eq!(g.mint(Path::new("/r"), "f").0, a);
        assert_ne!(g.mint(Path::new("/r"), "g").0, a);
        assert_ne!(g.mint(Path::new("/other"), "f").0, a);
        assert!(g.get(&a).is_some());
        g.expire_all();
        assert!(g.get(&a).is_none());
        assert_ne!(g.mint(Path::new("/r"), "f").0, a);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), RangeSpec::Partial(0, 9));
        assert_eq!(parse_range("bytes=90-", 100), RangeSpec::Partial(90, 99));
        assert_eq!(parse_range("bytes=-10", 100), RangeSpec::Partial(90, 99));
        assert_eq!(parse_range("bytes=50-500", 100), RangeSpec::Partial(50, 99));
        assert_eq!(parse_range("bytes=100-", 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), RangeSpec::Full);
        assert_eq!(parse_range("items=0-1", 100), RangeSpec::Full);
    }

    #[test]
    fn popups_stay_sandboxed_and_links_go_through_the_frame() {
        assert!(CSP.starts_with("sandbox allow-scripts allow-popups allow-downloads;"));
        assert!(!CSP.contains("allow-same-origin") && !CSP.contains("escape-sandbox") && !CSP.contains("allow-top-navigation"));
        // The message web/src/features/workspace/viewers/basic.tsx (HtmlView) listens for.
        assert!(PRELUDE.contains("parent.postMessage({type:'workbench:open-link',href:u.href},'*')"));
        assert!(!PRELUDE.contains("_blank"), "a popup from the report would stay sandboxed");
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a.HTML")), "text/html; charset=utf-8");
        assert_eq!(content_type(Path::new("m.glb")), "model/gltf-binary");
        assert_eq!(content_type(Path::new("x.png")), "image/png");
        assert_eq!(content_type(Path::new("x.css")), "text/css; charset=utf-8");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }
}
