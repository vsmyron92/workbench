//! File contents: read (text + metadata), stat, raw bytes with Range support, and
//! saves with optimistic concurrency.
//!
//! The revision of a file is the sha256 of its bytes (`etag`). A save names the
//! revision it was based on; if the disk moved on, the save is refused with 409
//! `conflict` and the editor offers reload / keep mine / compare. Saves are atomic
//! (temp file + fsync + rename, permissions kept) and the hash is re-checked right
//! before the rename, so an agent writing the same file at the same time is never
//! silently overwritten.

use std::fs::{OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{MAX_TEXT_BYTES, Sensitive, blocking, in_git_dir, mtime_ms, resolve, sha256_hex};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub allow_sensitive: bool,
    /// raw only: `Content-Disposition: attachment`.
    #[serde(default)]
    pub download: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// Project-relative (or absolute for `/api/fs/read`).
    pub path: String,
    /// UTF-8 text without a BOM; `null` for binary, too large or withheld sensitive files.
    pub content: Option<String>,
    pub binary: bool,
    pub size: u64,
    pub mtime: i64,
    /// sha256 (hex) of the bytes on disk; `null` when the content was not read.
    pub etag: Option<String>,
    /// Larger than 5 MB: view it through the raw endpoint instead.
    pub too_large: bool,
    /// Matches a sensitive pattern. Content is withheld unless `allowSensitive=true`.
    pub sensitive: bool,
    /// `utf-8`, `utf-8-bom`, or `unknown` (not valid UTF-8; shown lossily, read-only).
    pub encoding: Option<&'static str>,
    pub mime: String,
    /// Saving is not offered (outside a project, inside `.git`, no write permission, lossy decode).
    pub read_only: bool,
}

pub async fn read(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<ReadQuery>,
) -> ApiResult<Json<FileContent>> {
    let r = resolve(&state, &pid, &q.path)?;
    let sensitive = Sensitive::new(&r.project.config.project.sensitive).matches(&r.rel);
    let read_only = in_git_dir(&r.rel);
    let (rel, project) = (r.rel.clone(), r.project.clone());
    let out = blocking(move || read_file(&r.abs, rel, sensitive, q.allow_sensitive, read_only)).await?;
    // Local History keeps the version the editor starts from (if it is new to it).
    if let (false, Some(_), Some(etag)) = (sensitive, &out.content, &out.etag) {
        super::history::opened(&state, project, out.path.clone(), etag.clone());
    }
    Ok(Json(out))
}

pub fn read_file(abs: &Path, display: String, sensitive: bool, allow_sensitive: bool, read_only: bool) -> ApiResult<FileContent> {
    let md = std::fs::metadata(abs)?;
    if md.is_dir() {
        return Err(ApiError::bad_request(format!("{display} is a directory")));
    }
    let mut out = FileContent {
        path: display,
        content: None,
        binary: false,
        size: md.len(),
        mtime: mtime_ms(&md),
        etag: None,
        too_large: false,
        sensitive,
        encoding: None,
        mime: mime_for(abs).to_string(),
        read_only: read_only || md.permissions().readonly(),
    };
    if sensitive && !allow_sensitive {
        return Ok(out);
    }
    if md.len() > MAX_TEXT_BYTES {
        out.too_large = true;
        let mut head = Vec::with_capacity(8192);
        std::fs::File::open(abs)?.take(8192).read_to_end(&mut head)?;
        out.binary = looks_binary(&head);
        return Ok(out);
    }
    let bytes = std::fs::read(abs)?;
    out.size = bytes.len() as u64;
    out.etag = Some(sha256_hex(&bytes));
    match decode_text(&bytes) {
        Decoded::Binary => out.binary = true,
        Decoded::Text { text, encoding, lossy } => {
            out.content = Some(text);
            out.encoding = Some(encoding);
            out.read_only |= lossy;
            if out.mime == "application/octet-stream" {
                out.mime = "text/plain".into();
            }
        }
    }
    Ok(out)
}

#[derive(Debug, PartialEq)]
pub enum Decoded {
    Binary,
    Text { text: String, encoding: &'static str, lossy: bool },
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// UTF-8 (optionally with a BOM, which is stripped) → text. A NUL byte near the
/// start means binary. Other invalid UTF-8 is decoded lossily (read-only) unless
/// it is mostly garbage.
pub fn decode_text(bytes: &[u8]) -> Decoded {
    let (body, bom) = match bytes.strip_prefix(BOM) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    if looks_binary(body) {
        return Decoded::Binary;
    }
    match std::str::from_utf8(body) {
        Ok(s) => Decoded::Text { text: s.to_string(), encoding: if bom { "utf-8-bom" } else { "utf-8" }, lossy: false },
        Err(_) => {
            let s = String::from_utf8_lossy(body);
            let bad = s.chars().filter(|&c| c == '\u{FFFD}').count();
            let total = s.chars().count().max(1);
            if bad * 10 > total {
                Decoded::Binary
            } else {
                Decoded::Text { text: s.into_owned(), encoding: "unknown", lossy: true }
            }
        }
    }
}

/// MIME type by extension, with the few corrections an IDE needs.
pub fn mime_for(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match ext.as_deref() {
        // mime_guess says video/mp2t; in a code base it is TypeScript.
        Some("ts" | "tsx" | "mts" | "cts") => "text/plain",
        Some("rs" | "toml" | "lock" | "cs" | "go" | "kt" | "py" | "sh" | "yml" | "yaml" | "sql") => "text/plain",
        _ => mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream"),
    }
}

// ---------------------------------------------------------------- stat

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub exists: bool,
    /// `file` | `dir` | `other`
    pub kind: Option<&'static str>,
    pub size: u64,
    pub mtime: i64,
    /// For files up to 5 MB (and not withheld as sensitive).
    pub etag: Option<String>,
}

pub async fn stat(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<ReadQuery>,
) -> ApiResult<Json<FileStat>> {
    let r = resolve(&state, &pid, &q.path)?;
    let hide_etag = !q.allow_sensitive && Sensitive::new(&r.project.config.project.sensitive).matches(&r.rel);
    let rel = r.rel.clone();
    Ok(Json(blocking(move || stat_file(&r.abs, rel, !hide_etag)).await?))
}

pub fn stat_file(abs: &Path, display: String, with_etag: bool) -> ApiResult<FileStat> {
    let md = match std::fs::metadata(abs) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileStat { path: display, exists: false, kind: None, size: 0, mtime: 0, etag: None });
        }
        Err(e) => return Err(e.into()),
    };
    let kind = if md.is_file() {
        "file"
    } else if md.is_dir() {
        "dir"
    } else {
        "other"
    };
    let etag = if with_etag && md.is_file() && md.len() <= MAX_TEXT_BYTES {
        Some(sha256_hex(&std::fs::read(abs)?))
    } else {
        None
    };
    Ok(FileStat { path: display, exists: true, kind: Some(kind), size: md.len(), mtime: mtime_ms(&md), etag })
}

// ---------------------------------------------------------------- raw

pub async fn raw(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<ReadQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let r = resolve(&state, &pid, &q.path)?;
    if !q.allow_sensitive && Sensitive::new(&r.project.config.project.sensitive).matches(&r.rel) {
        return Err(ApiError::forbidden(format!("{} is marked sensitive", r.rel)));
    }
    serve_file(r.abs, &headers, q.download).await
}

#[derive(Debug, PartialEq)]
pub enum RangeSpec {
    /// No usable Range header: send everything.
    Full,
    /// Inclusive byte range.
    Partial(u64, u64),
    Unsatisfiable,
}

/// Parse a single `Range: bytes=…` header (multi-range requests get the full body,
/// which RFC 9110 allows).
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

/// Stream a file with the right content type, conditional GET and Range support.
///
/// Everything except PDFs gets `Content-Security-Policy: sandbox`, so an HTML or
/// SVG file from a repository opened directly cannot run script in Workbench's origin.
pub async fn serve_file(abs: PathBuf, headers: &HeaderMap, download: bool) -> ApiResult<Response> {
    let mut file = tokio::fs::File::open(&abs).await?;
    let md = file.metadata().await?;
    if md.is_dir() {
        return Err(ApiError::bad_request("is a directory"));
    }
    let len = md.len();
    let mtime_ns = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let etag = format!("W/\"{len:x}-{mtime_ns:x}\"");
    let if_none_match = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    if if_none_match.is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*")) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    let mut range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|r| parse_range(r, len))
        .unwrap_or(RangeSpec::Full);
    if let Some(ir) = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok()) {
        if ir.trim() != etag {
            range = RangeSpec::Full;
        }
    }
    let (status, start, end) = match range {
        RangeSpec::Full => (StatusCode::OK, 0, len),
        RangeSpec::Partial(s, e) => (StatusCode::PARTIAL_CONTENT, s, e + 1),
        RangeSpec::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{len}")), (header::ACCEPT_RANGES, "bytes".into())],
            )
                .into_response());
        }
    };
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(end - start), 64 * 1024);
    let mime = mime_for(&abs);
    let content_type = if mime.starts_with("text/") { format!("{mime}; charset=utf-8") } else { mime.to_string() };
    let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let disposition = format!(
        "{}; filename*=UTF-8''{}",
        if download { "attachment" } else { "inline" },
        percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC)
    );
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    let hv = |s: &str| HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_TYPE, hv(&content_type));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::ETAG, hv(&etag));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_DISPOSITION, hv(&disposition));
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(header::CONTENT_RANGE, hv(&format!("bytes {start}-{}/{len}", end - 1)));
    }
    if mime != "application/pdf" {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; font-src 'self' data:",
            ),
        );
    }
    Ok(resp)
}

// ---------------------------------------------------------------- write

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteBody {
    pub path: String,
    pub content: String,
    /// The revision the edit is based on; `null` = the file must not exist yet.
    #[serde(default)]
    pub etag: Option<String>,
    /// Overwrite whatever is on disk (the user chose "keep mine").
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub path: String,
    pub etag: String,
    pub mtime: i64,
    pub size: u64,
}

pub async fn write(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Json(body): Json<WriteBody>,
) -> ApiResult<Json<WriteResult>> {
    let r = resolve(&state, &pid, &body.path)?;
    if r.rel.is_empty() {
        return Err(ApiError::bad_request("cannot write the project root"));
    }
    if in_git_dir(&r.rel) {
        return Err(ApiError::forbidden("files inside .git are read-only in Workbench"));
    }
    let WriteBody { content, etag, force, .. } = body;
    let _guard = state.files.write_lock.lock().await;
    let rel = r.rel.clone();
    let abs = r.abs;
    let w = blocking(move || write_file(&abs, content.into_bytes(), etag.as_deref(), force)).await?;
    super::history::saved(&state, r.project, rel.clone(), w.prev, w.data, None);
    Ok(Json(WriteResult { path: rel, etag: w.etag, mtime: w.mtime, size: w.size }))
}

fn read_existing(path: &Path) -> ApiResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Err(ApiError::bad_request("path is a directory")),
        Err(e) => Err(e.into()),
    }
}

/// Atomically replace `path` with `data` if its current revision is `expected`
/// (`None` = must not exist). Returns `(etag, mtime, size)` of the new content.
#[cfg(test)]
pub fn write_checked(path: &Path, data: Vec<u8>, expected: Option<&str>, force: bool) -> ApiResult<(String, i64, u64)> {
    write_file(path, data, expected, force).map(|w| (w.etag, w.mtime, w.size))
}

/// A save that happened.
pub struct Written {
    pub etag: String,
    pub mtime: i64,
    pub size: u64,
    /// What the file held before (`None`: it did not exist).
    pub prev: Option<Vec<u8>>,
    /// The bytes written (with the BOM the file kept).
    pub data: Vec<u8>,
}

/// Atomically replace `path` with `data` if its current revision is `expected`
/// (`None` = must not exist). Also returns the previous and the written bytes
/// (for Local History).
pub fn write_file(path: &Path, mut data: Vec<u8>, expected: Option<&str>, force: bool) -> ApiResult<Written> {
    // Saving through a symlink updates its target; the link stays a link.
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => crate::util::os::path::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let current = read_existing(&target)?;
    let current_hash = current.as_deref().map(sha256_hex);
    if !force {
        match (expected, &current_hash) {
            (None, Some(_)) => return Err(ApiError::conflict(format!("{name} already exists"))),
            (Some(_), None) => return Err(ApiError::conflict(format!("{name} was deleted on disk"))),
            (Some(e), Some(h)) if !e.eq_ignore_ascii_case(h) => {
                return Err(ApiError::conflict(format!("{name} changed on disk since it was loaded")));
            }
            _ => {}
        }
    }
    // The editor never sees the BOM; keep it if the file had one.
    if current.as_deref().is_some_and(|c| c.starts_with(BOM)) && !data.starts_with(BOM) {
        data.splice(0..0, BOM.iter().copied());
    }
    let dir = target.parent().ok_or_else(|| ApiError::bad_request("path has no parent"))?;
    if current.is_none() {
        std::fs::create_dir_all(dir)?;
    }
    let keep_mode = std::fs::metadata(&target).ok().map(|m| m.permissions().mode() & 0o7777);
    let tmp = dir.join(format!(".{name}.wb-tmp-{}", crate::util::random_token(6)));
    let result = (|| -> ApiResult<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o666).open(&tmp)?;
        if let Some(m) = keep_mode {
            f.set_permissions(Permissions::from_mode(m))?;
        }
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        // Re-check just before the rename: an agent may have written meanwhile.
        if !force {
            let now = read_existing(&target)?;
            if now.as_deref().map(sha256_hex) != current_hash {
                return Err(ApiError::conflict(format!("{name} changed on disk while saving")));
            }
        }
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    let md = std::fs::metadata(&target)?;
    Ok(Written { etag: sha256_hex(&data), mtime: mtime_ms(&md), size: md.len(), prev: current, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_bom_binary_and_lossy() {
        assert_eq!(decode_text(b"fn main() {}\n"), Decoded::Text { text: "fn main() {}\n".into(), encoding: "utf-8", lossy: false });
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi"), Decoded::Text { text: "hi".into(), encoding: "utf-8-bom", lossy: false });
        assert_eq!(decode_text(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Decoded::Binary);
        match decode_text(b"caf\xe9 au lait, tr\xe8s bien") {
            Decoded::Text { encoding, lossy, .. } => assert!(lossy && encoding == "unknown"),
            d => panic!("{d:?}"),
        }
        assert_eq!(decode_text(&[0xff, 0xfe, 0xfd, 0xfc, 0xfb]), Decoded::Binary);
        assert_eq!(decode_text(b""), Decoded::Text { text: String::new(), encoding: "utf-8", lossy: false });
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-99", 1000), RangeSpec::Partial(0, 99));
        assert_eq!(parse_range("bytes=500-", 1000), RangeSpec::Partial(500, 999));
        assert_eq!(parse_range("bytes=-100", 1000), RangeSpec::Partial(900, 999));
        assert_eq!(parse_range("bytes=-5000", 1000), RangeSpec::Partial(0, 999));
        assert_eq!(parse_range("bytes=900-5000", 1000), RangeSpec::Partial(900, 999));
        assert_eq!(parse_range("bytes=1000-", 1000), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range("bytes=5-1", 1000), RangeSpec::Full);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), RangeSpec::Full);
        assert_eq!(parse_range("items=0-1", 1000), RangeSpec::Full);
        assert_eq!(parse_range("bytes=x-", 1000), RangeSpec::Full);
    }

    #[test]
    fn mime_corrections() {
        assert_eq!(mime_for(Path::new("a/b.ts")), "text/plain");
        assert_eq!(mime_for(Path::new("x.PNG")), "image/png");
        assert_eq!(mime_for(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(mime_for(Path::new("Makefile")), "application/octet-stream");
    }

    #[test]
    fn optimistic_concurrency() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/a.txt");
        // Create: parents are made, a second create conflicts.
        let (e1, _, size) = write_checked(&p, b"one".to_vec(), None, false).unwrap();
        assert_eq!(size, 3);
        assert_eq!(e1, sha256_hex(b"one"));
        assert_eq!(write_checked(&p, b"x".to_vec(), None, false).unwrap_err().code, "conflict");
        // Update with the right etag.
        let (e2, _, _) = write_checked(&p, b"two".to_vec(), Some(&e1), false).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // Stale etag → conflict, disk untouched.
        let err = write_checked(&p, b"three".to_vec(), Some(&e1), false).unwrap_err();
        assert_eq!(err.code, "conflict");
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // Force overwrites.
        write_checked(&p, b"three".to_vec(), Some(&e1), true).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"three");
        let _ = e2;
        // Deleted underneath → conflict.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(write_checked(&p, b"4".to_vec(), Some(&e1), false).unwrap_err().code, "conflict");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("sub")).unwrap().flatten().collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn keeps_mode_bom_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("run.sh");
        std::fs::write(&p, b"\xEF\xBB\xBFecho hi\n").unwrap();
        std::fs::set_permissions(&p, Permissions::from_mode(0o755)).unwrap();
        let etag = sha256_hex(&std::fs::read(&p).unwrap());
        write_checked(&p, b"echo bye\n".to_vec(), Some(&etag), false).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read(&p).unwrap(), b"\xEF\xBB\xBFecho bye\n");

        let link = dir.path().join("link.sh");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        let etag = sha256_hex(&std::fs::read(&p).unwrap());
        write_checked(&link, b"echo link\n".to_vec(), Some(&etag), false).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&p).unwrap(), b"\xEF\xBB\xBFecho link\n");
    }

    #[test]
    fn read_withholds_sensitive_and_flags_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        std::fs::write(&env, "TOKEN=abc").unwrap();
        let r = read_file(&env, ".env".into(), true, false, false).unwrap();
        assert!(r.sensitive && r.content.is_none() && r.etag.is_none());
        let r = read_file(&env, ".env".into(), true, true, false).unwrap();
        assert_eq!(r.content.as_deref(), Some("TOKEN=abc"));

        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'a'; (MAX_TEXT_BYTES + 1) as usize]).unwrap();
        let r = read_file(&big, "big.txt".into(), false, false, false).unwrap();
        assert!(r.too_large && r.content.is_none() && !r.binary);
    }
}
