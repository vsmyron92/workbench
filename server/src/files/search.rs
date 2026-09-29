//! Find and replace in files with ripgrep's libraries.
//!
//! The walk honours .gitignore (also outside git repos), includes dotfiles such as
//! `.gitlab-ci.yml` but never `.git`, skips binary and sensitive files, and stops
//! at a result cap or deadline. Results are sorted by path, line and column.
//!
//! Replace is two-step: a dry run returns per-line previews and each file's etag;
//! the confirmed request names the files and their expected etags, and a file that
//! changed in between is reported as a conflict instead of being rewritten.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use grep_matcher::{LineTerminator, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::{WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};

use super::content::write_file;
use super::{MAX_TEXT_BYTES, Sensitive, blocking, in_git_dir, resolve, sha256_hex};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

const DEFAULT_MAX: usize = 2000;
const HARD_MAX: usize = 20_000;
const MAX_FILESIZE: u64 = 4 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(15);
/// Lines longer than this get a window around the match as preview.
const PREVIEW_FULL_LINE: usize = 400;
const PREVIEW_WINDOW: usize = 300;
const PREVIEW_LEAD: usize = 80;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub q: String,
    #[serde(default)]
    pub regex: bool,
    /// Case-sensitive.
    #[serde(default)]
    pub case: bool,
    /// Whole words.
    #[serde(default)]
    pub word: bool,
    /// Comma-separated globs; `!glob` excludes (`*.rs, !tests/**`).
    #[serde(default)]
    pub glob: String,
    #[serde(default)]
    pub max: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// 1-based column in UTF-16 code units (Monaco's unit), in the full line.
    pub column: u32,
    pub end_column: u32,
    /// The line, or a window of it for very long lines.
    pub preview: String,
    /// UTF-16 units of the line cut before `preview` (0 = preview starts the line).
    pub preview_offset: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<Hit>,
    /// Stopped at the result cap or the deadline; refine the query.
    pub truncated: bool,
    pub timed_out: bool,
    pub files_searched: usize,
    pub files_matched: usize,
    /// Files skipped because they are marked sensitive.
    pub sensitive_skipped: usize,
    pub elapsed_ms: u64,
}

/// The final regex source: literal text escaped, whole-word boundaries added.
pub fn build_pattern(q: &str, regex: bool, word: bool) -> String {
    let body = if regex { q.to_string() } else { regex::escape(q) };
    if !word {
        return body;
    }
    if regex {
        return format!(r"\b(?:{body})\b");
    }
    // `\b` next to a non-word character would demand a word character there.
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let lead = if is_word(q.chars().next()) { r"\b" } else { "" };
    let trail = if is_word(q.chars().last()) { r"\b" } else { "" };
    format!("{lead}(?:{body}){trail}")
}

/// Set up like `rg --crlf`: `^` and `$` are line anchors (multi-line mode) that
/// also accept `\r\n` endings, and a match never contains `\r` or `\n`. The line
/// terminator must equal the searcher's ([`searcher`]); otherwise every search
/// fails with a config error.
fn build_matcher(p: &SearchParams) -> ApiResult<RegexMatcher> {
    if p.q.is_empty() {
        return Err(ApiError::bad_request("empty query"));
    }
    RegexMatcherBuilder::new()
        .case_insensitive(!p.case)
        .multi_line(true)
        .line_terminator(Some(b'\n'))
        // After `line_terminator`, which would otherwise reset the terminator to `\n` alone.
        .crlf(true)
        .build(&build_pattern(&p.q, p.regex, p.word))
        .map_err(|e| ApiError::bad_request(format!("invalid pattern: {e}")))
}

fn searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(true)
        .line_terminator(LineTerminator::crlf())
        .binary_detection(BinaryDetection::quit(0))
        .build()
}

fn build_overrides(root: &Path, globs: &str) -> ApiResult<ignore::overrides::Override> {
    let mut b = ignore::overrides::OverrideBuilder::new(root);
    for g in globs.split([',', ';', '\n']).map(str::trim).filter(|g| !g.is_empty()) {
        b.add(g).map_err(|e| ApiError::bad_request(format!("invalid glob {g:?}: {e}")))?;
    }
    b.build().map_err(|e| ApiError::bad_request(format!("invalid globs: {e}")))
}

/// Number of UTF-16 code units in UTF-8 `bytes` (invalid bytes count as one each).
fn utf16_len(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .filter(|&&b| (b & 0xC0) != 0x80)
        .map(|&b| if b >= 0xF0 { 2 } else { 1 })
        .sum()
}

fn floor_boundary(line: &[u8], mut i: usize) -> usize {
    while i > 0 && i < line.len() && (line[i] & 0xC0) == 0x80 {
        i -= 1;
    }
    i
}

fn trim_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Build a hit for the match `[start, end)` (byte offsets) in `line`.
pub fn make_hit(path: &str, line_no: u64, line: &[u8], start: usize, end: usize) -> Hit {
    let line = trim_eol(line);
    let end = end.min(line.len());
    let start = start.min(end);
    let column = utf16_len(&line[..start]) + 1;
    let end_column = column + utf16_len(&line[start..end]);
    let (preview, preview_offset) = if line.len() <= PREVIEW_FULL_LINE {
        (String::from_utf8_lossy(line).into_owned(), 0)
    } else {
        let s = floor_boundary(line, start.saturating_sub(PREVIEW_LEAD));
        let e = floor_boundary(line, (s + PREVIEW_WINDOW).max(end).min(line.len()));
        let e = if e < end { line.len().min(end) } else { e };
        (String::from_utf8_lossy(&line[s..e]).into_owned(), utf16_len(&line[..s]))
    };
    Hit { path: path.to_string(), line: line_no, column, end_column, preview, preview_offset }
}

struct Collector {
    hits: parking_lot::Mutex<Vec<Hit>>,
    count: AtomicUsize,
    files: AtomicUsize,
    files_matched: AtomicUsize,
    sensitive_skipped: AtomicUsize,
    max: usize,
    stop: AtomicBool,
}

struct HitSink<'a> {
    matcher: &'a RegexMatcher,
    rel: &'a str,
    out: &'a Collector,
    local: Vec<Hit>,
}

impl Sink for HitSink<'_> {
    type Error = std::io::Error;
    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // Without its terminator, so that `$` and `\z` match at the end of the line.
        let line = trim_eol(m.bytes());
        let line_no = m.line_number().unwrap_or(0);
        let mut keep = true;
        let _ = self.matcher.find_iter(line, |mm| {
            if mm.start() == mm.end() {
                return true; // empty matches (`^`, `a*`) are not useful results
            }
            if self.out.count.fetch_add(1, Ordering::Relaxed) >= self.out.max {
                keep = false;
                return false;
            }
            self.local.push(make_hit(self.rel, line_no, line, mm.start(), mm.end()));
            true
        });
        Ok(keep && !self.out.stop.load(Ordering::Relaxed))
    }
}

fn walker(root: &Path, overrides: ignore::overrides::Override) -> WalkBuilder {
    let mut b = super::gitignore::walk(root);
    b.parents(true).max_filesize(Some(MAX_FILESIZE)).overrides(overrides);
    b
}

/// Search `root`. Blocking; `cancel` stops the walk early (the client went away).
pub fn run_search(root: &Path, p: &SearchParams, sensitive: &Sensitive, cancel: &AtomicBool) -> ApiResult<SearchResult> {
    let started = Instant::now();
    let matcher = build_matcher(p)?;
    let overrides = build_overrides(root, &p.glob)?;
    let max = p.max.unwrap_or(DEFAULT_MAX).clamp(1, HARD_MAX);
    let out = Collector {
        hits: parking_lot::Mutex::new(vec![]),
        count: AtomicUsize::new(0),
        files: AtomicUsize::new(0),
        files_matched: AtomicUsize::new(0),
        sensitive_skipped: AtomicUsize::new(0),
        max,
        stop: AtomicBool::new(false),
    };
    let timed_out = AtomicBool::new(false);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    walker(root, overrides).threads(threads).build_parallel().run(|| {
        let matcher = &matcher;
        let out = &out;
        let timed_out = &timed_out;
        let mut searcher = searcher();
        Box::new(move |res| {
            if out.stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            if started.elapsed() > DEADLINE {
                timed_out.store(true, Ordering::Relaxed);
                out.stop.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            if out.count.load(Ordering::Relaxed) >= out.max {
                out.stop.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            let Ok(ent) = res else { return WalkState::Continue };
            if !ent.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let rel = ent.path().strip_prefix(root).unwrap_or(ent.path()).to_string_lossy().replace('\\', "/");
            if sensitive.matches(&rel) {
                out.sensitive_skipped.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
            out.files.fetch_add(1, Ordering::Relaxed);
            let mut sink = HitSink { matcher, rel: &rel, out, local: vec![] };
            let _ = searcher.search_path(matcher, ent.path(), &mut sink);
            if !sink.local.is_empty() {
                out.files_matched.fetch_add(1, Ordering::Relaxed);
                out.hits.lock().extend(sink.local);
            }
            WalkState::Continue
        })
    });
    let mut matches = std::mem::take(&mut *out.hits.lock());
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)).then(a.column.cmp(&b.column)));
    matches.truncate(max);
    let truncated = out.count.load(Ordering::Relaxed) > matches.len() || timed_out.load(Ordering::Relaxed);
    Ok(SearchResult {
        matches,
        truncated,
        timed_out: timed_out.load(Ordering::Relaxed),
        files_searched: out.files.load(Ordering::Relaxed),
        files_matched: out.files_matched.load(Ordering::Relaxed),
        sensitive_skipped: out.sensitive_skipped.load(Ordering::Relaxed),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Sets the flag when dropped: a cancelled request stops its background search.
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub async fn search(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(p): Query<SearchParams>,
) -> ApiResult<Json<SearchResult>> {
    let project = state.projects.require(&pid)?;
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    let root = project.root.clone();
    let result = blocking(move || run_search(&root, &p, &sensitive, &cancel)).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------- replace

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceBody {
    pub q: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case: bool,
    #[serde(default)]
    pub word: bool,
    /// Kept for symmetry with search (the file set is `paths`).
    #[serde(default)]
    #[allow(dead_code)]
    pub glob: String,
    pub replacement: String,
    /// The files to change (the confirmed subset of the search results).
    pub paths: Vec<String>,
    /// path → etag from the dry run. Required unless `dryRun`.
    #[serde(default)]
    pub expected: HashMap<String, String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinePreview {
    pub line: u64,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    pub path: String,
    pub etag: String,
    pub count: usize,
    /// Changed lines (capped at 200 per file).
    pub lines: Vec<LinePreview>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replaced {
    pub path: String,
    pub count: usize,
    pub etag: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceConflict {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceResult {
    /// Dry run only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FilePreview>,
    pub replaced: Vec<Replaced>,
    pub conflicts: Vec<ReplaceConflict>,
    /// Occurrences replaced (or that would be).
    pub total: usize,
}

const MAX_REPLACE_FILES: usize = 2000;
const MAX_PREVIEW_LINES: usize = 200;

pub fn build_regex(q: &str, regex: bool, case: bool, word: bool) -> ApiResult<regex::bytes::Regex> {
    if q.is_empty() {
        return Err(ApiError::bad_request("empty query"));
    }
    let re = regex::bytes::RegexBuilder::new(&build_pattern(q, regex, word))
        .case_insensitive(!case)
        .build()
        .map_err(|e| ApiError::bad_request(format!("invalid pattern: {e}")))?;
    // `x*` would insert the replacement between every character.
    if re.is_match(b"") {
        return Err(ApiError::bad_request("the pattern matches empty text; make it more specific"));
    }
    Ok(re)
}

/// Apply the replacement line by line (matching never crosses a line, as in the
/// search). Returns the new content, the number of replacements and line previews.
pub fn replace_in(content: &[u8], re: &regex::bytes::Regex, replacement: &str, expand: bool) -> (Vec<u8>, usize, Vec<LinePreview>) {
    let mut out = Vec::with_capacity(content.len());
    let mut count = 0;
    let mut previews = vec![];
    for (i, raw) in content.split_inclusive(|&b| b == b'\n').enumerate() {
        let body = trim_eol(raw);
        let eol = &raw[body.len()..];
        let n = re.find_iter(body).filter(|m| m.start() != m.end()).count();
        if n == 0 {
            out.extend_from_slice(raw);
            continue;
        }
        let new: Vec<u8> = if expand {
            re.replace_all(body, replacement.as_bytes()).into_owned()
        } else {
            re.replace_all(body, regex::bytes::NoExpand(replacement.as_bytes())).into_owned()
        };
        if new == body {
            out.extend_from_slice(raw);
            continue;
        }
        count += n;
        if previews.len() < MAX_PREVIEW_LINES {
            previews.push(LinePreview {
                line: i as u64 + 1,
                before: String::from_utf8_lossy(body).chars().take(PREVIEW_WINDOW).collect(),
                after: String::from_utf8_lossy(&new).chars().take(PREVIEW_WINDOW).collect(),
            });
        }
        out.extend_from_slice(&new);
        out.extend_from_slice(eol);
    }
    (out, count, previews)
}

pub async fn replace(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Json(body): Json<ReplaceBody>,
) -> ApiResult<Json<ReplaceResult>> {
    let project = state.projects.require(&pid)?;
    let re = build_regex(&body.q, body.regex, body.case, body.word)?;
    if body.paths.len() > MAX_REPLACE_FILES {
        return Err(ApiError::bad_request(format!("at most {MAX_REPLACE_FILES} files per replace")));
    }
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    let mut targets: Vec<(String, PathBuf)> = vec![];
    let mut result = ReplaceResult::default();
    for p in &body.paths {
        let r = resolve(&state, &pid, p)?;
        if r.rel.is_empty() || in_git_dir(&r.rel) || sensitive.matches(&r.rel) {
            result.conflicts.push(ReplaceConflict { path: r.rel, message: "not replaceable here".into() });
            continue;
        }
        targets.push((r.rel, r.abs));
    }
    // Writes are serialized with editor saves.
    let _guard = if body.dry_run { None } else { Some(state.files.write_lock.lock().await) };
    let st = state.clone();
    let result = blocking(move || {
        let expand = body.regex;
        for (rel, abs) in targets {
            let bytes = match std::fs::read(&abs) {
                Ok(b) => b,
                Err(e) => {
                    result.conflicts.push(ReplaceConflict { path: rel, message: e.to_string() });
                    continue;
                }
            };
            if bytes.len() as u64 > MAX_TEXT_BYTES || bytes[..bytes.len().min(8000)].contains(&0) {
                result.conflicts.push(ReplaceConflict { path: rel, message: "binary or too large".into() });
                continue;
            }
            let etag = sha256_hex(&bytes);
            if !body.dry_run {
                match body.expected.get(&rel) {
                    Some(e) if e.eq_ignore_ascii_case(&etag) => {}
                    Some(_) => {
                        result.conflicts.push(ReplaceConflict { path: rel, message: "changed on disk since the preview".into() });
                        continue;
                    }
                    None => {
                        result.conflicts.push(ReplaceConflict { path: rel, message: "not in the preview".into() });
                        continue;
                    }
                }
            }
            let (new, count, lines) = replace_in(&bytes, &re, &body.replacement, expand);
            if count == 0 {
                continue;
            }
            result.total += count;
            if body.dry_run {
                result.files.push(FilePreview { path: rel, etag, count, lines });
                continue;
            }
            match write_file(&abs, new, Some(&etag), false) {
                Ok(w) => {
                    super::history::saved(&st, project.clone(), rel.clone(), w.prev, w.data, Some("Replace in Files"));
                    result.replaced.push(Replaced { path: rel, count, etag: w.etag })
                }
                Err(e) => result.conflicts.push(ReplaceConflict { path: rel, message: e.message }),
            }
        }
        if !body.dry_run {
            result.total = result.replaced.iter().map(|r| r.count).sum();
        }
        Ok(result)
    })
    .await?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(q: &str) -> SearchParams {
        SearchParams { q: q.into(), ..Default::default() }
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::create_dir_all(r.join("ignored")).unwrap();
        std::fs::write(r.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main() {\n    let foo = foo_bar(foo);\n}\n").unwrap();
        std::fs::write(r.join("src/lib.rs"), "pub fn Foo() {}\r\n").unwrap();
        std::fs::write(r.join(".gitlab-ci.yml"), "stages: [foo]\n").unwrap();
        std::fs::write(r.join(".git/config"), "foo\n").unwrap();
        std::fs::write(r.join("target/out.rs"), "foo\n").unwrap();
        std::fs::write(r.join("ignored/x.rs"), "foo\n").unwrap();
        std::fs::write(r.join(".env"), "FOO=secret\n").unwrap();
        std::fs::write(r.join("bin.dat"), b"foo\0\0binary").unwrap();
        dir
    }

    #[test]
    fn searches_with_ignores_dotfiles_and_sorting() {
        let dir = fixture();
        let res = run_search(dir.path(), &params("foo"), &Sensitive::defaults(), &AtomicBool::new(false)).unwrap();
        let files: Vec<_> = res.matches.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(files, vec![".gitlab-ci.yml", "src/lib.rs", "src/main.rs", "src/main.rs", "src/main.rs"]);
        assert_eq!(res.sensitive_skipped, 1);
        assert!(!res.truncated);
        let h = &res.matches[2];
        assert_eq!((h.line, h.column, h.end_column), (2, 9, 12));
        assert_eq!(h.preview, "    let foo = foo_bar(foo);");
        // CRLF is not part of the preview.
        assert_eq!(res.matches[1].preview, "pub fn Foo() {}");
    }

    #[test]
    fn case_word_regex_and_globs() {
        let dir = fixture();
        let s = Sensitive::defaults();
        let no = AtomicBool::new(false);
        let r = run_search(dir.path(), &SearchParams { case: true, ..params("Foo") }, &s, &no).unwrap();
        assert_eq!(r.matches.len(), 1);
        let r = run_search(dir.path(), &SearchParams { word: true, glob: "*.rs".into(), ..params("foo") }, &s, &no).unwrap();
        // `foo_bar` is not the word `foo`.
        assert_eq!(r.matches.iter().filter(|h| h.path == "src/main.rs").count(), 2);
        assert!(r.matches.iter().all(|h| h.path.ends_with(".rs")));
        let r = run_search(dir.path(), &SearchParams { regex: true, glob: "!src/lib.rs".into(), ..params(r"foo_\w+") }, &s, &no).unwrap();
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].end_column - r.matches[0].column, 7);
        assert!(run_search(dir.path(), &SearchParams { regex: true, ..params("(") }, &s, &no).is_err());
    }

    #[test]
    fn line_anchors_in_lf_and_crlf_files() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        std::fs::write(r.join("lf.txt"), "line4\nline5\nline55 x\nline5").unwrap();
        std::fs::write(r.join("crlf.txt"), "A\r\nB\r\nBB\r\nfoo \r\nB").unwrap();
        let s = Sensitive::defaults();
        let no = AtomicBool::new(false);
        let find = |q: &str| -> Vec<(String, u64, u32, u32)> {
            let p = SearchParams { regex: true, case: true, ..params(q) };
            run_search(r, &p, &s, &no).unwrap().matches.into_iter().map(|h| (h.path, h.line, h.column, h.end_column)).collect()
        };
        let hit = |p: &str, line: u64, col: u32, end: u32| (p.to_string(), line, col, end);
        // `$` at the end of a line, including the last one without a terminator.
        assert_eq!(find("^line5$"), vec![hit("lf.txt", 2, 1, 6), hit("lf.txt", 4, 1, 6)]);
        assert_eq!(find("5$"), vec![hit("lf.txt", 2, 5, 6), hit("lf.txt", 4, 5, 6)]);
        // CRLF: `$` matches before the `\r`, which is never part of a match.
        assert_eq!(find("^B$"), vec![hit("crlf.txt", 2, 1, 2), hit("crlf.txt", 5, 1, 2)]);
        assert_eq!(find(r"foo\s*$"), vec![hit("crlf.txt", 4, 1, 5)]);
        assert_eq!(find(r"^B"), vec![hit("crlf.txt", 2, 1, 2), hit("crlf.txt", 3, 1, 2), hit("crlf.txt", 5, 1, 2)]);
        // A match never spans lines.
        assert!(find(r"A\s+B").is_empty());
        // Search and replace agree on what `$` means.
        let re = build_regex("^B$", true, true, false).unwrap();
        let (_, n, _) = replace_in(&std::fs::read(r.join("crlf.txt")).unwrap(), &re, "b", true);
        assert_eq!(n, 2);
    }

    #[test]
    fn caps_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x x x x\n".repeat(10)).unwrap();
        }
        let r = run_search(dir.path(), &SearchParams { max: Some(25), ..params("x") }, &Sensitive::defaults(), &AtomicBool::new(false)).unwrap();
        assert_eq!(r.matches.len(), 25);
        assert!(r.truncated);
        let r = run_search(dir.path(), &params("x"), &Sensitive::defaults(), &AtomicBool::new(true)).unwrap();
        assert!(r.matches.is_empty());
    }

    #[test]
    fn hits_count_utf16_and_window_long_lines() {
        let line = "é😀 foo".as_bytes();
        let start = line.len() - 3;
        let h = make_hit("a", 1, line, start, line.len());
        assert_eq!((h.column, h.end_column), (5, 8));
        let long = format!("{}needle{}", "a".repeat(1000), "b".repeat(1000));
        let h = make_hit("a", 1, long.as_bytes(), 1000, 1006);
        assert_eq!(h.column, 1001);
        assert!(h.preview.len() <= PREVIEW_WINDOW + 10);
        let off = (h.column - 1 - h.preview_offset) as usize;
        assert_eq!(&h.preview[off..off + 6], "needle");
    }

    #[test]
    fn word_patterns() {
        assert_eq!(build_pattern("foo", false, true), r"\b(?:foo)\b");
        assert_eq!(build_pattern("-x", false, true), r"(?:\-x)\b");
        assert_eq!(build_pattern("a.b", false, false), r"a\.b");
        assert_eq!(build_pattern("a|b", true, true), r"\b(?:a|b)\b");
    }

    #[test]
    fn replace_lines_keeps_eols_and_expands_groups() {
        let re = build_regex(r"(\w+)=(\d+)", true, true, false).unwrap();
        let (out, n, prev) = replace_in(b"a=1\r\nb=x\nc=3", &re, "$2=$1", true);
        assert_eq!(out, b"1=a\r\nb=x\n3=c");
        assert_eq!(n, 2);
        assert_eq!(prev[0], LinePreview { line: 1, before: "a=1".into(), after: "1=a".into() });
        let re = build_regex("$x", false, true, false).unwrap();
        let (out, n, _) = replace_in(b"cost $x\n", &re, "$1", false);
        assert_eq!((out.as_slice(), n), (&b"cost $1\n"[..], 1));
    }
}
