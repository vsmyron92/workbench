//! Line-level staging: stage, unstage or roll back a chosen subset of a diff's
//! changed lines (CLion's "partial" staging and commit).
//!
//! The client names changed lines by side and number (`add` = line of the modified
//! side, `del` = line of the original side). The server re-runs the diff, checks the
//! fingerprint the client saw (409 when the file moved on), and builds a *minimal
//! patch* from the parsed hunks:
//!
//! * forward (stage; applied to the old side with `git apply --cached`): a selected
//!   change is kept, an unselected deletion becomes context (the line stays), an
//!   unselected addition is left out;
//! * reverse (unstage / roll back; applied to the new side with `-R`): a selected
//!   change is kept, an unselected addition becomes context, an unselected deletion
//!   is left out.
//!
//! Positions are exact on the side the patch is applied to and shifted by the net
//! delta of the hunks already emitted on the other side. A line without a final
//! newline (`\ No newline at end of file`) must stay the last line of its side: when
//! turning a change into context would put lines after it, the change is kept and a
//! copy of the line *with* a newline is emitted on the other side (the smallest patch
//! that is still valid). Lines are raw bytes, so CRLF files and any encoding
//! round-trip.

use std::collections::HashSet;
use std::path::Path;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::diff::{DiffMode, DiffQuery, ParsedDiff, ParsedHunk, file_diff};
use super::ops::{self, DiscardRequest, PathsRequest};
use super::repo::Repo;
use super::status::narrow_entries;
use crate::error::ApiError;

/// Above this many changed lines a diff is not offered for line selection.
pub const MAX_SELECTABLE_LINES: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Add,
    Del,
}

/// A changed line named by the client: an added line of the modified side or a
/// deleted line of the original side (1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct LineRef {
    pub kind: LineKind,
    pub line: u32,
}

/// One changed line of a diff, as the UI needs it to draw checkboxes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedLine {
    /// Index of its hunk.
    pub hunk: usize,
    pub kind: LineKind,
    /// Line number on its own side (modified for `add`, original for `del`).
    pub line: u32,
    /// Line of the modified side at which it shows: itself for an addition; for a
    /// deletion the line just below the gap (one past the end at the end of the file).
    pub at: u32,
}

/// A body line of a hunk with its numbers.
#[derive(Debug, Clone, PartialEq)]
struct BodyLine {
    /// b' ', b'-' or b'+'.
    op: u8,
    /// Content without the prefix and without '\n' (a '\r' stays).
    text: Vec<u8>,
    /// Followed by '\n' in its file (false: `\ No newline at end of file`).
    eol: bool,
    old: u32,
    new: u32,
}

fn body_lines(h: &ParsedHunk) -> Vec<BodyLine> {
    let mut out: Vec<BodyLine> = Vec::with_capacity(h.body.len());
    // A side with 0 lines has its start one line *before* the change.
    let mut o = h.old_start.max(1);
    let mut n = h.new_start.max(1);
    if h.old_lines == 0 {
        o = h.old_start + 1;
    }
    if h.new_lines == 0 {
        n = h.new_start + 1;
    }
    for raw in &h.body {
        match raw.first() {
            Some(b'\\') => {
                if let Some(last) = out.last_mut() {
                    last.eol = false;
                }
            }
            Some(&op @ (b' ' | b'-' | b'+')) => {
                let text = raw[1..].to_vec();
                let (lo, ln) = (o, n);
                match op {
                    b' ' => {
                        o += 1;
                        n += 1;
                    }
                    b'-' => o += 1,
                    _ => n += 1,
                }
                out.push(BodyLine { op, text, eol: true, old: lo, new: ln });
            }
            _ => {}
        }
    }
    out
}

/// Every changed line of a diff (empty when the diff has more than
/// [`MAX_SELECTABLE_LINES`]).
pub fn changed_lines(d: &ParsedDiff) -> Vec<ChangedLine> {
    let mut v = vec![];
    for (i, h) in d.hunks.iter().enumerate() {
        for b in body_lines(h) {
            match b.op {
                b'+' => v.push(ChangedLine { hunk: i, kind: LineKind::Add, line: b.new, at: b.new }),
                b'-' => v.push(ChangedLine { hunk: i, kind: LineKind::Del, line: b.old, at: b.new }),
                _ => {}
            }
            if v.len() > MAX_SELECTABLE_LINES {
                return vec![];
            }
        }
    }
    v
}

/// Does the selection cover every change of the diff (then the whole-file operation
/// is the right one)? Errors when it names a line that is not a change.
pub fn check_selection(d: &ParsedDiff, sel: &HashSet<LineRef>) -> Result<bool, ApiError> {
    if sel.is_empty() {
        return Err(ApiError::bad_request("no lines selected"));
    }
    let all = changed_lines(d);
    if all.is_empty() {
        return Err(ApiError::bad_request("this diff has no lines that can be selected"));
    }
    let known: HashSet<LineRef> = all.iter().map(|c| LineRef { kind: c.kind, line: c.line }).collect();
    if let Some(bad) = sel.iter().find(|r| !known.contains(r)) {
        return Err(ApiError::conflict(format!(
            "{} line {} is not a change in the current diff; refresh and select again",
            match bad.kind {
                LineKind::Add => "added",
                LineKind::Del => "deleted",
            },
            bad.line
        )));
    }
    Ok(known.len() == sel.len())
}

/// How the file appears in the patch header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    /// `--- a/P` / `+++ b/P`.
    Modify,
    /// `new file mode M` / `--- /dev/null` / `+++ b/P` (stage part of an untracked file).
    Create,
}

/// Git's C-style quoting of a path in patch headers (`core.quotepath=false`: only
/// control characters, `"` and `\` force quoting).
pub fn quote_path(prefix: &str, path: &[u8]) -> Vec<u8> {
    let needs = path.iter().any(|&b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\');
    let mut out = Vec::with_capacity(path.len() + prefix.len() + 2);
    if !needs {
        out.extend_from_slice(prefix.as_bytes());
        out.extend_from_slice(path);
        return out;
    }
    out.push(b'"');
    out.extend_from_slice(prefix.as_bytes());
    for &b in path {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            0x07 => out.extend_from_slice(b"\\a"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0b => out.extend_from_slice(b"\\v"),
            0x0c => out.extend_from_slice(b"\\f"),
            b if b < 0x20 || b == 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

/// A line of the patch being built: its prefix, content, newline, and the change it
/// came from when it was turned into context (to undo that for a no-newline line).
#[derive(Debug, Clone)]
struct OutLine {
    op: u8,
    text: Vec<u8>,
    eol: bool,
    converted_from: Option<u8>,
}

fn on_old(op: u8) -> bool {
    op == b' ' || op == b'-'
}
fn on_new(op: u8) -> bool {
    op == b' ' || op == b'+'
}

/// A line without a newline must be the last line of its side. Undo the context
/// conversion that broke that: keep the change and emit a copy with a newline on the
/// other side.
fn fix_missing_newlines(lines: &mut Vec<OutLine>) {
    for _ in 0..4 {
        let mut broken: Option<usize> = None;
        for side_old in [true, false] {
            let on = |op: u8| if side_old { on_old(op) } else { on_new(op) };
            let idx: Vec<usize> = (0..lines.len()).filter(|&i| on(lines[i].op)).collect();
            if let Some(&i) = idx.iter().rev().skip(1).find(|&&i| !lines[i].eol && lines[i].converted_from.is_some()) {
                broken = Some(i);
                break;
            }
        }
        let Some(i) = broken else { return };
        let l = lines[i].clone();
        let (Some(orig), text) = (l.converted_from, l.text) else { return };
        let other = if orig == b'-' { b'+' } else { b'-' };
        let kept = OutLine { op: orig, text: text.clone(), eol: false, converted_from: None };
        let copy = OutLine { op: other, text, eol: true, converted_from: None };
        // Old-side lines first so both sides keep their order.
        let pair = if orig == b'-' { [kept, copy] } else { [copy, kept] };
        lines.splice(i..=i, pair);
    }
}

/// A built patch. `zero_context`: some hunk has no context line (a tiny or empty
/// file), which `git apply` accepts only with `--unidiff-zero`.
#[derive(Debug, Clone)]
pub struct LinePatch {
    pub bytes: Vec<u8>,
    pub zero_context: bool,
}

#[cfg(test)]
/// Build the patch for `sel` from a parsed single-file diff. `path` is the
/// repository path (raw bytes) the patch applies to; `mode` the new file's mode for
/// [`PatchKind::Create`]. `reverse`: the patch will be applied with `-R`.
pub fn build_line_patch(
    d: &ParsedDiff,
    sel: &HashSet<LineRef>,
    reverse: bool,
    path: &[u8],
    kind: PatchKind,
    mode: &str,
) -> Result<Vec<u8>, ApiError> {
    Ok(line_patch(d, sel, reverse, path, kind, mode)?.bytes)
}

/// [`build_line_patch`] with the `--unidiff-zero` hint.
pub fn line_patch(d: &ParsedDiff, sel: &HashSet<LineRef>, reverse: bool, path: &[u8], kind: PatchKind, mode: &str) -> Result<LinePatch, ApiError> {
    let mut zero_context = false;
    let mut out = Vec::new();
    out.extend_from_slice(b"diff --git ");
    out.extend_from_slice(&quote_path("a/", path));
    out.push(b' ');
    out.extend_from_slice(&quote_path("b/", path));
    out.push(b'\n');
    match kind {
        PatchKind::Create => {
            out.extend_from_slice(format!("new file mode {mode}\n--- /dev/null\n").as_bytes());
        }
        PatchKind::Modify => {
            out.extend_from_slice(b"--- ");
            out.extend_from_slice(&quote_path("a/", path));
            out.push(b'\n');
        }
    }
    out.extend_from_slice(b"+++ ");
    out.extend_from_slice(&quote_path("b/", path));
    out.push(b'\n');

    let selected = |op: u8, b: &BodyLine| match op {
        b'+' => sel.contains(&LineRef { kind: LineKind::Add, line: b.new }),
        b'-' => sel.contains(&LineRef { kind: LineKind::Del, line: b.old }),
        _ => false,
    };
    // Net line delta (new - old) of the hunks emitted so far.
    let mut delta: i64 = 0;
    let mut emitted = 0;
    for h in &d.hunks {
        let mut lines: Vec<OutLine> = vec![];
        for b in body_lines(h) {
            let line = |op: u8, converted_from: Option<u8>| OutLine { op, text: b.text.clone(), eol: b.eol, converted_from };
            match (b.op, selected(b.op, &b), reverse) {
                (b' ', _, _) => lines.push(line(b' ', None)),
                (op, true, _) => lines.push(line(op, None)),
                (b'-', false, false) => lines.push(line(b' ', Some(b'-'))),
                (b'+', false, true) => lines.push(line(b' ', Some(b'+'))),
                _ => {} // an unselected addition (forward) or deletion (reverse) is left out
            }
        }
        if !lines.iter().any(|l| l.op != b' ') {
            continue;
        }
        fix_missing_newlines(&mut lines);
        let old_n = lines.iter().filter(|l| on_old(l.op)).count() as i64;
        let new_n = lines.iter().filter(|l| on_new(l.op)).count() as i64;
        // The side the patch applies to keeps the hunk's own numbers.
        let (old_start, new_start) = if !reverse {
            let first_old = if h.old_lines == 0 { h.old_start as i64 + 1 } else { h.old_start as i64 };
            let first_new = first_old + delta;
            (if old_n == 0 { first_old - 1 } else { first_old }, if new_n == 0 { first_new - 1 } else { first_new })
        } else {
            let first_new = if h.new_lines == 0 { h.new_start as i64 + 1 } else { h.new_start as i64 };
            let first_old = first_new - delta;
            (if old_n == 0 { first_old - 1 } else { first_old }, if new_n == 0 { first_new - 1 } else { first_new })
        };
        delta += new_n - old_n;
        emitted += 1;
        zero_context |= !lines.iter().any(|l| l.op == b' ');
        out.extend_from_slice(format!("@@ -{},{} +{},{} @@\n", old_start.max(0), old_n, new_start.max(0), new_n).as_bytes());
        for l in &lines {
            out.push(l.op);
            out.extend_from_slice(&l.text);
            out.push(b'\n');
            if !l.eol {
                out.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
    }
    if emitted == 0 {
        return Err(ApiError::bad_request("no selected line is a change"));
    }
    Ok(LinePatch { bytes: out, zero_context })
}

// ---------------------------------------------------------------- operations

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOp {
    Stage,
    Unstage,
    Discard,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinesRequest {
    pub path: String,
    pub fingerprint: String,
    pub lines: Vec<LineRef>,
}

/// Feed a patch to `git apply`; a refusal means the file moved on (409).
pub async fn git_apply(repo: &Repo, patch: LinePatch, args: &[&str], index_file: Option<&Path>) -> Result<(), ApiError> {
    let mut g = repo.git_w().args(["apply", "--recount", "--whitespace=nowarn"]).args(args.iter().copied());
    if patch.zero_context {
        g = g.arg("--unidiff-zero");
    }
    if let Some(ix) = index_file {
        g = g.env("GIT_INDEX_FILE", ix.to_string_lossy().to_string());
    }
    g.arg("-").stdin(patch.bytes).run_ok_retry_lock().await.map_err(|e| {
        if e.status == StatusCode::UNPROCESSABLE_ENTITY {
            ApiError::conflict(format!("git apply refused the selected lines ({}); refresh the diff and try again", e.message))
        } else {
            e
        }
    })?;
    Ok(())
}

/// Stage, unstage or roll back the selected lines. The caller holds the
/// repository write lock.
pub async fn apply_lines(repo: &Repo, op: LineOp, req: &LinesRequest, trash_dir: &Path) -> Result<(), ApiError> {
    if req.lines.len() > MAX_SELECTABLE_LINES {
        return Err(ApiError::bad_request("too many lines selected"));
    }
    let mode = match op {
        LineOp::Stage | LineOp::Discard => DiffMode::Working,
        LineOp::Unstage => DiffMode::Staged,
    };
    let q = DiffQuery { path: req.path.clone(), mode: Some(mode), ..Default::default() };
    let (diff, parsed) = file_diff(repo, &q).await?;
    if diff.fingerprint != req.fingerprint {
        return Err(ApiError::conflict(format!("{} changed since the diff was shown; review the new diff and try again", req.path)));
    }
    if !diff.can_select_lines {
        return Err(ApiError::bad_request(format!(
            "{} has no lines that can be selected (binary, LFS, symlink, conflicted or too large); use the file action instead",
            req.path
        )));
    }
    let sel: HashSet<LineRef> = req.lines.iter().copied().collect();
    let all = check_selection(&parsed, &sel)?;
    let whole_file = diff.untracked || parsed.new_file || parsed.deleted_file || diff.original_missing || diff.modified_missing;
    if all && whole_file {
        // Every line of a new or deleted file: the file-level operation says it exactly.
        let paths = vec![req.path.clone()];
        match op {
            LineOp::Stage => {
                ops::stage(repo, &PathsRequest { paths, all: false }).await?;
            }
            LineOp::Unstage => ops::unstage(repo, &PathsRequest { paths, all: false }).await?,
            LineOp::Discard => {
                ops::discard(repo, &DiscardRequest { paths, scope: Some("worktree".into()), delete_added: false }, trash_dir).await?;
            }
        }
        return Ok(());
    }
    match op {
        LineOp::Unstage if parsed.deleted_file || diff.modified_missing => {
            return Err(ApiError::bad_request(format!("{} is deleted in the index: unstage the whole file", req.path)));
        }
        LineOp::Discard if diff.modified_missing => {
            return Err(ApiError::bad_request(format!("{} is deleted: roll back the whole file", req.path)));
        }
        _ => {}
    }
    let repo_path = repo.to_repo(&req.path)?;
    // An intent-to-add entry (`git add -N`) is in the index, empty: add to it.
    let intent_to_add = op == LineOp::Stage
        && parsed.new_file
        && !diff.untracked
        && narrow_entries(repo, std::slice::from_ref(&repo_path)).await?.iter().any(|f| f.path == repo_path && f.index == ' ' && f.worktree == 'A');
    let (reverse, kind) = match op {
        LineOp::Stage if (diff.untracked || parsed.new_file) && !intent_to_add => (false, PatchKind::Create),
        LineOp::Stage => (false, PatchKind::Modify),
        LineOp::Unstage | LineOp::Discard => (true, PatchKind::Modify),
    };
    let file_mode = parsed.new_mode.clone().filter(|m| m.len() == 6).unwrap_or_else(|| "100644".into());
    let patch = line_patch(&parsed, &sel, reverse, repo_path.as_bytes(), kind, &file_mode)?;
    let args: &[&str] = match op {
        LineOp::Stage => &["--cached"],
        LineOp::Unstage => &["--cached", "-R"],
        LineOp::Discard => &["-R"],
    };
    git_apply(repo, patch, args, None).await
}

#[cfg(test)]
mod tests {
    use super::super::diff::parse_diff;
    use super::*;

    fn sel(v: &[(LineKind, u32)]) -> HashSet<LineRef> {
        v.iter().map(|&(kind, line)| LineRef { kind, line }).collect()
    }
    use LineKind::{Add, Del};

    const TWO_HUNKS: &str = "diff --git a/f b/f
--- a/f
+++ b/f
@@ -1,4 +1,5 @@
 a
-b
+B
+B2
 c
 d
@@ -10,3 +11,3 @@
 j
-k
+K
 l
";

    #[test]
    fn numbers_changed_lines_and_their_anchor() {
        let d = parse_diff(TWO_HUNKS.as_bytes());
        let c = changed_lines(&d);
        assert_eq!(
            c,
            vec![
                ChangedLine { hunk: 0, kind: Del, line: 2, at: 2 },
                ChangedLine { hunk: 0, kind: Add, line: 2, at: 2 },
                ChangedLine { hunk: 0, kind: Add, line: 3, at: 3 },
                ChangedLine { hunk: 1, kind: Del, line: 11, at: 12 },
                ChangedLine { hunk: 1, kind: Add, line: 12, at: 12 },
            ]
        );
        assert!(check_selection(&d, &sel(&[(Add, 2)])).is_ok_and(|all| !all));
        assert!(check_selection(&d, &sel(&[(Del, 2), (Add, 2), (Add, 3), (Del, 11), (Add, 12)])).is_ok_and(|all| all));
        assert_eq!(check_selection(&d, &sel(&[(Add, 5)])).unwrap_err().status.as_u16(), 409);
        assert!(check_selection(&d, &sel(&[])).is_err());
    }

    #[test]
    fn forward_keeps_selected_turns_unselected_deletions_into_context() {
        let d = parse_diff(TWO_HUNKS.as_bytes());
        // Only the added B2: b stays, B is not added.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 3)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert_eq!(p, "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,5 @@\n a\n b\n+B2\n c\n d\n");
        // The second hunk alone: its new start follows the old one (nothing before it emitted).
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 11), (Add, 12)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -10,3 +10,3 @@\n j\n-k\n+K\n l\n"), "{p}");
        // Both: the second hunk's new start moves by the first's delta (+1).
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 3), (Del, 11)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.contains("@@ -10,3 +11,2 @@\n j\n-k\n l\n"), "{p}");
    }

    #[test]
    fn reverse_keeps_new_side_numbers_and_turns_unselected_additions_into_context() {
        let d = parse_diff(TWO_HUNKS.as_bytes());
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 12)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -11,2 +11,3 @@\n j\n+K\n l\n"), "{p}");
        // Deletion b re-added only (B, B2 stay): old' has a, b, B, B2, c, d.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 2)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.contains("@@ -1,6 +1,5 @@\n a\n-b\n B\n B2\n c\n d\n"), "{p}");
        // Then the second hunk shifts by the emitted delta (new 5 - old 6 = -1).
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 2), (Del, 11)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.contains("@@ -12,4 +11,3 @@\n j\n-k\n K\n l\n"), "{p}");
    }

    #[test]
    fn pure_insertions_and_deletions_use_the_zero_count_convention() {
        // Two lines added after line 3; nothing else.
        let d = parse_diff(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3,0 +4,2 @@\n+x\n+y\n");
        let c = changed_lines(&d);
        assert_eq!((c[0].line, c[1].line), (4, 5));
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 5)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -3,0 +4,1 @@\n+y\n"), "{p}");
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 5)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -4,1 +4,2 @@\n x\n+y\n"), "{p}");
        // Two lines deleted after new line 2.
        let d = parse_diff(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -3,2 +2,0 @@\n-x\n-y\n");
        let c = changed_lines(&d);
        assert_eq!((c[0].line, c[0].at, c[1].line), (3, 3, 4));
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 3)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -3,2 +3,1 @@\n-x\n y\n"), "{p}");
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 4), (Del, 3)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -3,2 +2,0 @@\n-x\n-y\n"), "{p}");
    }

    #[test]
    fn a_line_without_newline_stays_last_on_its_side() {
        // old "a\nb" (no newline), new "a\nb\nc\n": b gains a newline, c is added.
        let d = parse_diff(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,3 @@\n a\n-b\n\\ No newline at end of file\n+b\n+c\n");
        // Staging only c needs b's newline too.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 3)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,2 +1,3 @@\n a\n-b\n\\ No newline at end of file\n+b\n+c\n"), "{p}");
        // Rolling back only c keeps b's new newline.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 3)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,2 +1,3 @@\n a\n b\n+c\n"), "{p}");
        // Staging only the newline change (-b/+b) is a plain patch.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 2), (Add, 2)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"), "{p}");

        // old "a\nb\nc\n", new "a\nb" (c removed, b loses its newline); roll back only c:
        // it returns where the block starts, the new last line stays last.
        let d = parse_diff(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,2 @@\n a\n-b\n-c\n+b\n\\ No newline at end of file\n");
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 3)]), true, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,2 @@\n a\n-c\n b\n\\ No newline at end of file\n"), "{p}");
        // Staging only the removal of c: b keeps its newline in the index.
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Del, 3)]), false, b"f", PatchKind::Modify, "").unwrap()).unwrap();
        assert!(p.ends_with("@@ -1,3 +1,2 @@\n a\n b\n-c\n"), "{p}");
    }

    #[test]
    fn crlf_and_binary_safe_content_round_trips() {
        let d = parse_diff(b"diff --git a/w b/w\n--- a/w\n+++ b/w\n@@ -1,2 +1,3 @@\n a\r\n+\xff\xfe\r\n b\r\n");
        let p = build_line_patch(&d, &sel(&[(Add, 2)]), false, b"w", PatchKind::Modify, "").unwrap();
        assert!(p.ends_with(b"@@ -1,2 +1,3 @@\n a\r\n+\xff\xfe\r\n b\r\n"));
    }

    #[test]
    fn creates_part_of_a_new_file() {
        let d = parse_diff(b"diff --git a/n b/n\nnew file mode 100755\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,3 @@\n+x\n+y\n+z\n\\ No newline at end of file\n");
        let p = String::from_utf8(build_line_patch(&d, &sel(&[(Add, 1), (Add, 3)]), false, b"n", PatchKind::Create, "100755").unwrap()).unwrap();
        assert_eq!(p, "diff --git a/n b/n\nnew file mode 100755\n--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+x\n+z\n\\ No newline at end of file\n");
    }

    #[test]
    fn quotes_paths_like_git() {
        assert_eq!(quote_path("a/", b"dir/my file.rs"), b"a/dir/my file.rs");
        assert_eq!(quote_path("b/", b"we\"ird\\\tname"), b"\"b/we\\\"ird\\\\\\tname\"");
        assert_eq!(quote_path("a/", "ünï.txt".as_bytes()), "a/ünï.txt".as_bytes());
    }
}
