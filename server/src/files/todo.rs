//! The TODO tool window (CLion's TODO view): `// TODO`, `# FIXME`, `/* XXX */`,
//! `<!-- HACK -->` comments across a project. Runs Find in Files' walker (so
//! `.gitignore`, binary and sensitive files are skipped) and keeps the matches that
//! sit in a comment of the file's language.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use serde::Serialize;

use super::search::{Hit, SearchParams, run_search};
use super::{Sensitive, blocking};
use crate::app::AppState;
use crate::error::ApiResult;

/// `todo` and `fixme` in any case (CLion's defaults); `XXX` and `HACK` only as
/// capitals, so prose such as "a hack" is not listed.
const PATTERN: &str = r"\b(?i:todo|fixme)\b|\b(?:XXX|HACK)\b";
const MAX_ITEMS: usize = 5000;
const MAX_TEXT: usize = 240;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub path: String,
    /// 1-based.
    pub line: u64,
    /// 1-based, UTF-16 units (Monaco's), at the keyword.
    pub column: u32,
    pub end_column: u32,
    /// `TODO`, `FIXME`, `XXX` or `HACK`.
    pub kind: String,
    /// The comment from the keyword on, without a closing `*/` or `-->`.
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoResult {
    pub items: Vec<TodoItem>,
    /// Stopped at the item cap or the search deadline.
    pub truncated: bool,
    pub files_searched: usize,
    pub elapsed_ms: u64,
}

/// Comment openers by file type; `None` for unknown types (any common one counts).
fn markers(path: &str) -> Option<&'static [&'static str]> {
    const C: &[&str] = &["//", "/*"];
    const HASH: &[&str] = &["#"];
    const DASH: &[&str] = &["--"];
    const VHDL: &[&str] = &["--", "/*"];
    const WEB: &[&str] = &["<!--", "//", "/*"];
    const MARKUP: &[&str] = &["<!--"];
    const SEMI: &[&str] = &[";"];
    const PERCENT: &[&str] = &["%"];
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    match name.as_str() {
        "dockerfile" | "makefile" | "gnumakefile" | "cmakelists.txt" | "justfile" | "procfile" | "gemfile" | "rakefile" | ".gitignore"
        | ".dockerignore" | ".editorconfig" | "caddyfile" => return Some(HASH),
        _ => {}
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    Some(match ext {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "c" | "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh"
        | "hxx" | "h++" | "ipp" | "tpp" | "txx" | "inl" | "ixx" | "cppm" | "ino" | "cu" | "cuh" | "v" | "vh" | "sv" | "svh" | "cs" | "java" | "kt" | "kts" | "go" | "swift" | "scala" | "dart" | "css" | "scss" | "less" | "proto" | "groovy" | "gradle"
        | "jsonc" | "json5" | "glsl" | "hlsl" | "wgsl" | "shader" | "zig" | "m" | "mm" | "sol" | "fs" | "fsx" => C,
        "php" | "vue" | "svelte" | "html" | "htm" | "astro" => WEB,
        "py" | "pyi" | "sh" | "bash" | "zsh" | "fish" | "rb" | "pl" | "pm" | "yaml" | "yml" | "toml" | "conf" | "cfg" | "cmake" | "r"
        | "nix" | "tf" | "hcl" | "ps1" | "psm1" | "properties" | "mk" | "ex" | "exs" | "jl" | "coffee" | "cr" | "nim" | "tcl" => HASH,
        "sql" | "lua" | "hs" | "elm" | "ada" | "adb" | "ads" | "purs" => DASH,
        "vhd" | "vhdl" | "vho" | "vht" => VHDL,
        "md" | "markdown" | "xml" | "svg" | "xaml" | "csproj" | "plist" => MARKUP,
        "clj" | "cljs" | "cljc" | "edn" | "lisp" | "el" | "scm" | "asm" | "s" | "ini" => SEMI,
        "tex" | "sty" | "erl" | "hrl" | "matlab" => PERCENT,
        _ => return None,
    })
}

const ANY: &[&str] = &["//", "/*", "#", "--", "<!--", ";"];

/// Is the text before a keyword (on the same line) inside a comment?
fn in_comment(path: &str, before: &str) -> bool {
    let set = markers(path).unwrap_or(ANY);
    if set.iter().any(|m| before.contains(m)) {
        return true;
    }
    // The middle lines of a `/* … */` block usually start with `*`.
    set.contains(&"/*") && before.trim_start().starts_with('*')
}

/// Byte offset of UTF-16 offset `units` in `s` (clamped to `s`).
fn byte_at_utf16(s: &str, units: u32) -> usize {
    let mut seen = 0u32;
    for (i, c) in s.char_indices() {
        if seen >= units {
            return i;
        }
        seen += c.len_utf16() as u32;
    }
    s.len()
}

/// The TODO item for a Find in Files hit, or `None` when the keyword is not in a comment.
fn todo_of(hit: &Hit) -> Option<TodoItem> {
    let start_units = hit.column.saturating_sub(1).saturating_sub(hit.preview_offset);
    let len_units = hit.end_column.saturating_sub(hit.column);
    let start = byte_at_utf16(&hit.preview, start_units);
    let end = start + byte_at_utf16(&hit.preview[start..], len_units);
    if !in_comment(&hit.path, &hit.preview[..start]) {
        return None;
    }
    let kind = hit.preview[start..end].to_ascii_uppercase();
    let mut text = hit.preview[start..].trim_end();
    for close in ["*/", "-->"] {
        text = text.strip_suffix(close).unwrap_or(text).trim_end();
    }
    let mut text = text.to_string();
    if text.chars().count() > MAX_TEXT {
        text = text.chars().take(MAX_TEXT).collect::<String>() + "…";
    }
    Some(TodoItem { path: hit.path.clone(), line: hit.line, column: hit.column, end_column: hit.end_column, kind, text })
}

/// Scan `root`. Blocking; `cancel` stops the walk early.
pub fn scan(root: &Path, nested: &[PathBuf], sensitive: &Sensitive, cancel: &AtomicBool) -> ApiResult<TodoResult> {
    let started = Instant::now();
    // Case-sensitive at the top level: the pattern opts into insensitivity per keyword.
    let params = SearchParams { q: PATTERN.into(), regex: true, case: true, word: false, glob: String::new(), max: Some(MAX_ITEMS * 2) };
    let found = run_search(root, nested, &params, sensitive, cancel)?;
    let mut items: Vec<TodoItem> = found.matches.iter().filter_map(todo_of).collect();
    // One item per line: `TODO(fixme)` style lines list once, at the first keyword.
    items.dedup_by(|b, a| a.path == b.path && a.line == b.line);
    let truncated = found.truncated || items.len() > MAX_ITEMS;
    items.truncate(MAX_ITEMS);
    Ok(TodoResult { items, truncated, files_searched: found.files_searched, elapsed_ms: started.elapsed().as_millis() as u64 })
}

/// Sets the flag when dropped: a cancelled request stops its background scan.
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// `GET /api/projects/{pid}/files/todos`
pub async fn todos(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<TodoResult>> {
    let project = state.projects.require(&pid)?;
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());
    let root = project.root.clone();
    let nested = project.nested_repo_dirs();
    Ok(Json(blocking(move || scan(&root, &nested, &sensitive, &cancel)).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, line: &str, word: &str) -> Hit {
        let at = line.find(word).unwrap();
        super::super::search::make_hit(path, 3, line.as_bytes(), at, at + word.len())
    }

    #[test]
    fn keeps_keywords_in_comments_only() {
        let t = todo_of(&hit("src/a.rs", "    let x = 1; // TODO: remove this */", "TODO")).unwrap();
        assert_eq!(t.kind, "TODO");
        assert_eq!(t.text, "TODO: remove this");
        assert_eq!(t.column, 19);
        assert!(todo_of(&hit("src/a.rs", "let todo = vec![];", "todo")).is_none());
        assert!(todo_of(&hit("src/a.ts", "const s = 'TODO: not a comment'", "TODO")).is_none());
        assert_eq!(todo_of(&hit("src/a.ts", " * fixme later", "fixme")).unwrap().kind, "FIXME");
        assert_eq!(todo_of(&hit("app.py", "x = 1  # XXX: temporary", "XXX")).unwrap().text, "XXX: temporary");
        assert!(todo_of(&hit("app.py", "// TODO wrong marker for python", "TODO")).is_none());
        assert_eq!(todo_of(&hit("q.sql", "select 1 -- HACK until v2", "HACK")).unwrap().kind, "HACK");
        assert_eq!(todo_of(&hit("README.md", "<!-- TODO: screenshots -->", "TODO")).unwrap().text, "TODO: screenshots");
        assert!(todo_of(&hit("README.md", "- [ ] TODO write docs", "TODO")).is_none());
        assert!(todo_of(&hit("Dockerfile", "# todo: slim image", "todo")).is_some());
        assert!(todo_of(&hit("notes.unknown", "; TODO ini style", "TODO")).is_some());
        assert_eq!(todo_of(&hit("rtl/top.sv", "  assign y = a; // TODO: reset", "TODO")).unwrap().text, "TODO: reset");
        assert!(todo_of(&hit("rtl/top.v", "  $display(\"TODO\");", "TODO")).is_none());
        assert_eq!(todo_of(&hit("rtl/top.vhd", "  y <= a; -- FIXME: glitch", "FIXME")).unwrap().kind, "FIXME");
        assert_eq!(todo_of(&hit("rtl/top.vhdl", "  /* todo: 2008 block comment */", "todo")).unwrap().text, "todo: 2008 block comment");
        assert!(todo_of(&hit("rtl/top.vhd", "  report \"TODO\";", "TODO")).is_none());
        assert!(todo_of(&hit("src/k.cu", "__global__ void k() {} // XXX", "XXX")).is_some());
    }

    #[test]
    fn columns_are_utf16_and_long_text_is_capped() {
        let line = format!("// é🙂 TODO {}", "x".repeat(400));
        let t = todo_of(&hit("a.rs", &line, "TODO")).unwrap();
        assert_eq!(t.column, 1 + "// é🙂 ".encode_utf16().count() as u32);
        assert_eq!(t.text.chars().count(), MAX_TEXT + 1);
        assert!(t.text.ends_with('…'));
    }

    #[test]
    fn scans_a_tree_respecting_gitignore_and_sensitive_files() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::create_dir_all(r.join("target")).unwrap();
        std::fs::write(r.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main() {\n    // TODO: parse args\n    let todo_list = 1; // fixme(me): name\n}\n").unwrap();
        std::fs::write(r.join("target/gen.rs"), "// TODO generated\n").unwrap();
        std::fs::write(r.join(".env"), "# TODO rotate SECRET=1\n").unwrap();
        std::fs::write(r.join("run.py"), "print('TODO')  # HACK: quick\n").unwrap();
        let sensitive = Sensitive::new(&[]);
        let out = scan(r, &[], &sensitive, &AtomicBool::new(false)).unwrap();
        let got: Vec<(String, u64, String)> = out.items.iter().map(|i| (i.path.clone(), i.line, i.kind.clone())).collect();
        assert_eq!(
            got,
            vec![("run.py".into(), 1, "HACK".into()), ("src/main.rs".into(), 2, "TODO".into()), ("src/main.rs".into(), 3, "FIXME".into())]
        );
        assert!(!out.truncated);
    }
}
