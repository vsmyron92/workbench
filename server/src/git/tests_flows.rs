//! Integration tests of the change and history workflows (line staging, partial commit, undo commit,
//! interactive rebase, shelf, bisect, line history) against throwaway repositories.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use super::diff::{DiffMode, DiffQuery, file_diff};
use super::lines::{LineKind, LineOp, LineRef, LinesRequest, apply_lines};
use super::tests::{app_with, commit_all, git, init_repo, op_done, repo, write};
use super::{bisect, log, ops, rebase_i, shelf, status};

fn read(p: &Path, rel: &str) -> String {
    std::fs::read_to_string(p.join(rel)).unwrap()
}

fn index_of(p: &Path, rel: &str) -> String {
    git(p, &["show", &format!(":{rel}")])
}

fn add(line: u32) -> LineRef {
    LineRef { kind: LineKind::Add, line }
}
fn del(line: u32) -> LineRef {
    LineRef { kind: LineKind::Del, line }
}

async fn lines_op(p: &Path, op: LineOp, path: &str, sel: &[LineRef]) -> Result<(), crate::error::ApiError> {
    let r = repo(p).await;
    let mode = if op == LineOp::Unstage { DiffMode::Staged } else { DiffMode::Working };
    let (d, _) = file_diff(&r, &DiffQuery { path: path.into(), mode: Some(mode), ..Default::default() }).await.unwrap();
    let trash = tempfile::tempdir().unwrap();
    apply_lines(&r, op, &LinesRequest { path: path.into(), fingerprint: d.fingerprint, lines: sel.to_vec() }, trash.path()).await
}

// ---------------------------------------------------------------- line staging

#[tokio::test]
async fn stages_unstages_and_rolls_back_single_lines() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", "a\nb\nc\nd\ne\nf\ng\n");
    commit_all(p, "init");
    // One hunk: b → B (del 2, add 2), d removed (del 4), X inserted after f (add 6).
    write(p, "f.txt", "a\nB\nc\ne\nf\nX\ng\n");
    let r = repo(p).await;
    let (diff, _) = file_diff(&r, &DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap();
    assert!(diff.can_select_lines);
    let kinds: HashSet<(LineKind, u32)> = diff.lines.iter().map(|l| (l.kind, l.line)).collect();
    assert_eq!(kinds, [(LineKind::Del, 2), (LineKind::Add, 2), (LineKind::Del, 4), (LineKind::Add, 6)].into_iter().collect());

    // Stage the removal of d and the new X only.
    lines_op(p, LineOp::Stage, "f.txt", &[del(4), add(6)]).await.unwrap();
    assert_eq!(index_of(p, "f.txt"), "a\nb\nc\ne\nf\nX\ng\n");
    assert_eq!(read(p, "f.txt"), "a\nB\nc\ne\nf\nX\ng\n", "the working tree is untouched");
    // Unstage X again (lines of the staged diff: X is line 6 of the index).
    lines_op(p, LineOp::Unstage, "f.txt", &[add(6)]).await.unwrap();
    assert_eq!(index_of(p, "f.txt"), "a\nb\nc\ne\nf\ng\n");
    // Roll back B (and bring b back) in the working tree; X stays.
    lines_op(p, LineOp::Discard, "f.txt", &[del(2), add(2)]).await.unwrap();
    assert_eq!(read(p, "f.txt"), "a\nb\nc\ne\nf\nX\ng\n");
}

#[tokio::test]
async fn line_staging_keeps_the_last_line_without_newline_valid() {
    let d = init_repo();
    let p = d.path();
    write(p, "n.txt", "a\nb\nc\nd\ne");
    commit_all(p, "init");
    write(p, "n.txt", "a\nB\nc\nD\ne\nf");
    // Adjacent changes in one hunk; stage only the new last line f.
    let r = repo(p).await;
    let (diff, _) = file_diff(&r, &DiffQuery { path: "n.txt".into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap();
    assert_eq!(diff.hunks.len(), 1);
    let f_line = diff.lines.iter().find(|l| l.kind == LineKind::Add && l.line == 6).expect("f is added line 6");
    lines_op(p, LineOp::Stage, "n.txt", &[add(f_line.line)]).await.unwrap();
    assert_eq!(index_of(p, "n.txt"), "a\nb\nc\nd\ne\nf");
    // Roll back D only: d comes back, the rest of the working tree stays.
    lines_op(p, LineOp::Discard, "n.txt", &[del(4), add(4)]).await.unwrap();
    assert_eq!(read(p, "n.txt"), "a\nB\nc\nd\ne\nf");
}

#[tokio::test]
async fn line_staging_round_trips_crlf_files() {
    // CRLF committed as is.
    let d = init_repo();
    let p = d.path();
    write(p, "w.txt", "a\r\nb\r\nc\r\n");
    commit_all(p, "init");
    write(p, "w.txt", "a\r\nNEW\r\nb\r\nc\r\nZ\r\n");
    lines_op(p, LineOp::Stage, "w.txt", &[add(5)]).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\r\nb\r\nc\r\nZ\r\n");
    lines_op(p, LineOp::Discard, "w.txt", &[add(2)]).await.unwrap();
    assert_eq!(read(p, "w.txt"), "a\r\nb\r\nc\r\nZ\r\n");

    // autocrlf: LF in the index, CRLF in the working tree.
    let d = init_repo();
    let p = d.path();
    git(p, &["config", "core.autocrlf", "true"]);
    write(p, "w.txt", "a\r\nb\r\n");
    commit_all(p, "init");
    assert_eq!(index_of(p, "w.txt"), "a\nb\n");
    write(p, "w.txt", "a\r\nNEW\r\nb\r\nZ\r\n");
    lines_op(p, LineOp::Stage, "w.txt", &[add(4)]).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\nb\nZ\n");
    lines_op(p, LineOp::Discard, "w.txt", &[add(2)]).await.unwrap();
    assert_eq!(read(p, "w.txt"), "a\r\nb\r\nZ\r\n");
}

async fn working_diff(p: &Path, path: &str) -> super::diff::GitFileDiff {
    file_diff(&repo(p).await, &DiffQuery { path: path.into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap().0
}

#[tokio::test]
async fn crlf_working_trees_over_lf_indexes_diff_and_stage_as_git_reads_them() {
    // core.autocrlf=true (Git for Windows' default): LF in the index, CRLF on disk.
    let d = init_repo();
    let p = d.path();
    git(p, &["config", "core.autocrlf", "true"]);
    write(p, "w.txt", "a\r\nb\r\nc\r\n");
    commit_all(p, "init");
    write(p, "w.txt", "a\r\nNEW\r\nb\r\nc\r\nZ\r\n");
    // Both sides of the diff read like the hunks: LF.
    let diff = working_diff(p, "w.txt").await;
    assert_eq!((diff.original.as_str(), diff.modified.as_str()), ("a\nb\nc\n", "a\nNEW\nb\nc\nZ\n"));
    assert_eq!(diff.lines.iter().map(|l| (l.kind, l.line)).collect::<Vec<_>>(), vec![(LineKind::Add, 2), (LineKind::Add, 5)]);
    let r = repo(p).await;
    let q = DiffQuery { path: "w.txt".into(), mode: Some(DiffMode::Compare), base: Some("HEAD".into()), ..Default::default() };
    assert_eq!(file_diff(&r, &q).await.unwrap().0.modified, "a\nNEW\nb\nc\nZ\n");
    // Staging a line puts LF into the index; rolling one back keeps the file CRLF.
    lines_op(p, LineOp::Stage, "w.txt", &[add(5)]).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\nb\nc\nZ\n");
    lines_op(p, LineOp::Discard, "w.txt", &[add(2)]).await.unwrap();
    assert_eq!(read(p, "w.txt"), "a\r\nb\r\nc\r\nZ\r\n");
    // Nothing unstaged is left, and both sides say so (the UI's "No changes").
    let diff = working_diff(p, "w.txt").await;
    assert!(diff.hunks.is_empty() && diff.original == diff.modified, "{diff:?}");
    // Hunks, the same way.
    write(p, "w.txt", "a\r\nb\r\nH\r\nc\r\nZ\r\n");
    let diff = working_diff(p, "w.txt").await;
    let req = |fingerprint: String| super::diff::HunkRequest { path: "w.txt".into(), hunk_indexes: vec![0], fingerprint };
    super::diff::apply_hunks(&r, super::diff::HunkOp::Stage, &req(diff.fingerprint)).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\nb\nH\nc\nZ\n");
    // HEAD → index is one hunk (H and Z): unstaging it leaves HEAD's version.
    let staged = file_diff(&r, &DiffQuery { path: "w.txt".into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap().0;
    super::diff::apply_hunks(&r, super::diff::HunkOp::Unstage, &req(staged.fingerprint)).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\nb\nc\n");
    let diff = working_diff(p, "w.txt").await;
    assert_eq!(diff.modified, "a\nb\nH\nc\nZ\n");
    super::diff::apply_hunks(&r, super::diff::HunkOp::Discard, &req(diff.fingerprint)).await.unwrap();
    assert_eq!(read(p, "w.txt"), "a\r\nb\r\nc\r\n");

    // Part of an untracked CRLF file goes into the index with LF, as `git add` would.
    write(p, "u.txt", "x\r\ny\r\nz\r\n");
    let diff = working_diff(p, "u.txt").await;
    assert!(diff.untracked && diff.modified == "x\ny\nz\n", "{diff:?}");
    lines_op(p, LineOp::Stage, "u.txt", &[add(1), add(3)]).await.unwrap();
    assert_eq!(index_of(p, "u.txt"), "x\nz\n");
    assert_eq!(working_diff(p, "u.txt").await.modified, "x\ny\nz\n");
}

#[tokio::test]
async fn crlf_by_attribute_and_unconverted_crlf() {
    // `text eol=crlf` in .gitattributes, core.autocrlf off.
    let d = init_repo();
    let p = d.path();
    write(p, ".gitattributes", "*.txt text eol=crlf\n");
    write(p, "w.txt", "a\r\nb\r\n");
    commit_all(p, "init");
    assert_eq!(index_of(p, "w.txt"), "a\nb\n");
    write(p, "w.txt", "a\r\nNEW\r\nb\r\nZ\r\n");
    assert_eq!(working_diff(p, "w.txt").await.modified, "a\nNEW\nb\nZ\n");
    lines_op(p, LineOp::Stage, "w.txt", &[add(4)]).await.unwrap();
    assert_eq!(index_of(p, "w.txt"), "a\nb\nZ\n");
    lines_op(p, LineOp::Discard, "w.txt", &[add(2)]).await.unwrap();
    assert_eq!(read(p, "w.txt"), "a\r\nb\r\nZ\r\n");
    // `-text`: bytes as they are.
    write(p, ".gitattributes", "*.txt text eol=crlf\nraw.txt -text\n");
    write(p, "raw.txt", "r\n");
    commit_all(p, "raw");
    write(p, "raw.txt", "r\r\n");
    assert_eq!(working_diff(p, "raw.txt").await.modified, "r\r\n");

    // No conversion: a file whose line endings the user changed shows its CRs (every line
    // is a change for git too).
    let d = init_repo();
    let p = d.path();
    write(p, "w.txt", "a\nb\n");
    commit_all(p, "init");
    write(p, "w.txt", "a\r\nb\r\n");
    let diff = working_diff(p, "w.txt").await;
    assert_eq!((diff.original.as_str(), diff.modified.as_str()), ("a\nb\n", "a\r\nb\r\n"));
    assert_eq!(diff.lines.len(), 4);
}

#[tokio::test]
async fn conflicts_in_crlf_working_trees_are_shown_with_lf_and_resolved_with_crlf() {
    let d = init_repo();
    let p = d.path();
    git(p, &["config", "core.autocrlf", "true"]);
    write(p, "a.txt", "one\r\ntwo\r\n");
    commit_all(p, "init");
    git(p, &["checkout", "-q", "-b", "feature"]);
    write(p, "a.txt", "one\r\nfeature\r\n");
    commit_all(p, "feature");
    git(p, &["checkout", "-q", "main"]);
    write(p, "a.txt", "one\r\nmain\r\n");
    commit_all(p, "main");
    let r = repo(p).await;
    let out = ops::merge(&r, &ops::MergeRequest { rev: "feature".into(), no_ff: false, ff_only: false, squash: false, message: None }).await.unwrap();
    assert!(out.conflicts, "{out:?}");
    assert!(read(p, "a.txt").contains("<<<<<<< HEAD\r\n"), "git writes the conflict with CRLF");
    let v = super::conflicts::versions(&r, "a.txt").await.unwrap();
    assert_eq!(v.ours.as_deref(), Some("one\nmain\n"));
    assert!(v.merged.starts_with("one\n<<<<<<< HEAD\nmain\n=======\n") && !v.merged.contains('\r'), "{}", v.merged);
    let req = super::conflicts::ResolveRequest { path: "a.txt".into(), content: Some("one\nmain and feature\n".into()), side: None };
    super::conflicts::resolve(&r, &req).await.unwrap();
    assert_eq!(read(p, "a.txt"), "one\r\nmain and feature\r\n");
    assert_eq!(index_of(p, "a.txt"), "one\nmain and feature\n");
    assert!(status::status(&r, false).await.unwrap().files.iter().all(|f| f.path != "a.txt" || f.worktree == ' '));
}

#[tokio::test]
async fn line_staging_of_new_deleted_and_renamed_files() {
    let d = init_repo();
    let p = d.path();
    write(p, "keep.txt", "p\nq\nr\n");
    write(p, "old name.txt", "1\n2\n3\n4\n5\n6\n7\n8\n");
    commit_all(p, "init");

    // Part of an untracked file: the index gets a new file with those lines.
    write(p, "new.txt", "x\ny\nz\n");
    lines_op(p, LineOp::Stage, "new.txt", &[add(1), add(3)]).await.unwrap();
    assert_eq!(index_of(p, "new.txt"), "x\nz\n");
    // Unstage z from the staged new file; roll back y from the working tree.
    lines_op(p, LineOp::Unstage, "new.txt", &[add(2)]).await.unwrap();
    assert_eq!(index_of(p, "new.txt"), "x\n");
    lines_op(p, LineOp::Discard, "new.txt", &[add(2)]).await.unwrap();
    assert_eq!(read(p, "new.txt"), "x\nz\n");

    // A deleted file: stage the removal of q only; the whole file is a file-level op.
    std::fs::remove_file(p.join("keep.txt")).unwrap();
    lines_op(p, LineOp::Stage, "keep.txt", &[del(2)]).await.unwrap();
    assert_eq!(index_of(p, "keep.txt"), "p\nr\n");
    let e = lines_op(p, LineOp::Discard, "keep.txt", &[del(1)]).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 400, "{}", e.message);
    lines_op(p, LineOp::Stage, "keep.txt", &[del(1), del(2)]).await.unwrap();
    let st = status::status(&repo(p).await, false).await.unwrap();
    assert_eq!(st.files.iter().find(|f| f.path == "keep.txt").map(|f| f.index), Some('D'));

    // A staged rename with edits: unstage one edited line of the new name.
    git(p, &["mv", "old name.txt", "new name.txt"]);
    write(p, "new name.txt", "1\n2\nTHREE\n4\n5\n6\n7\nEIGHT\n");
    git(p, &["add", "new name.txt"]);
    let r = repo(p).await;
    let (sd, _) = file_diff(&r, &DiffQuery { path: "new name.txt".into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap();
    assert_eq!(sd.old_path.as_deref(), Some("old name.txt"));
    assert!(sd.can_select_lines);
    lines_op(p, LineOp::Unstage, "new name.txt", &[del(8), add(8)]).await.unwrap();
    assert_eq!(index_of(p, "new name.txt"), "1\n2\nTHREE\n4\n5\n6\n7\n8\n");
    let st = status::status(&repo(p).await, false).await.unwrap();
    let ren = st.files.iter().find(|f| f.path == "new name.txt").unwrap();
    assert_eq!((ren.index, ren.orig_path.as_deref()), ('R', Some("old name.txt")));
}

#[tokio::test]
async fn line_staging_into_an_intent_to_add_file() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    write(p, "ita.txt", "one\ntwo\nthree\n");
    git(p, &["add", "-N", "ita.txt"]);
    lines_op(p, LineOp::Stage, "ita.txt", &[add(2)]).await.unwrap();
    assert_eq!(index_of(p, "ita.txt"), "two\n");
}

#[tokio::test]
async fn line_operations_refuse_stale_diffs_and_foreign_lines() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", "a\nb\nc\n");
    commit_all(p, "init");
    write(p, "f.txt", "a\nB\nc\n");
    let r = repo(p).await;
    let (diff, _) = file_diff(&r, &DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap();
    write(p, "f.txt", "a\nB\nc\nagent\n");
    let trash = tempfile::tempdir().unwrap();
    let e = apply_lines(&r, LineOp::Stage, &LinesRequest { path: "f.txt".into(), fingerprint: diff.fingerprint, lines: vec![add(2)] }, trash.path())
        .await
        .unwrap_err();
    assert_eq!(e.status.as_u16(), 409);
    let e = lines_op(p, LineOp::Stage, "f.txt", &[add(3)]).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 409, "line 3 is context: {}", e.message);
    assert_eq!(index_of(p, "f.txt"), "a\nb\nc\n", "nothing was staged");
}

// ---------------------------------------------------------------- partial commit, undo

#[tokio::test]
async fn commits_selected_lines_and_leaves_the_rest() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", &super::tests::lines(30));
    write(p, "other.txt", "o\n");
    commit_all(p, "init");
    let text = read(p, "f.txt").replace("line 3\n", "THREE\n").replace("line 25\n", "TWENTY-FIVE\n");
    write(p, "f.txt", &text);
    write(p, "other.txt", "o2\n");
    git(p, &["add", "other.txt"]);
    write(p, "whole.txt", "w\n");
    let r = repo(p).await;
    let (cd, _) = file_diff(&r, &DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Compare), base: Some("HEAD".into()), ..Default::default() })
        .await
        .unwrap();
    assert!(cd.can_select_lines);
    let req = ops::CommitRequest {
        message: "only line 3".into(),
        amend: false,
        signoff: false,
        paths: Some(vec!["whole.txt".into()]),
        no_verify: false,
        partial: vec![ops::PartialFile { path: "f.txt".into(), fingerprint: cd.fingerprint.clone(), lines: vec![del(3), add(3)] }],
    };
    ops::commit(&r, &req).await.unwrap();
    let committed = git(p, &["show", "HEAD:f.txt"]);
    assert!(committed.contains("THREE\n") && committed.contains("line 25\n"), "{committed}");
    assert_eq!(git(p, &["show", "HEAD:whole.txt"]), "w\n");
    assert_eq!(git(p, &["show", "HEAD:other.txt"]), "o\n", "the staged other.txt stays out of the commit");
    assert_eq!(index_of(p, "other.txt"), "o2\n", "…and staged");
    assert_eq!(index_of(p, "f.txt"), committed, "the index has the committed version");
    assert!(read(p, "f.txt").contains("TWENTY-FIVE"), "the unselected line stays in the working tree");
    // A stale selection is refused.
    let mut stale = req.clone();
    stale.paths = None;
    stale.partial[0].lines = vec![add(25)];
    assert_eq!(ops::commit(&r, &stale).await.unwrap_err().status.as_u16(), 409);
}

#[tokio::test]
async fn commits_selected_lines_of_a_renamed_file() {
    let d = init_repo();
    let p = d.path();
    write(p, "b.txt", &super::tests::lines(10));
    commit_all(p, "init");
    git(p, &["mv", "b.txt", "b2.txt"]);
    write(p, "b2.txt", &super::tests::lines(10).replace("line 2\n", "TWO\n").replace("line 9\n", "NINE\n"));
    let r = repo(p).await;
    let q = DiffQuery { path: "b2.txt".into(), mode: Some(DiffMode::Compare), base: Some("HEAD".into()), old_path: Some("b.txt".into()), ..Default::default() };
    let (cd, _) = file_diff(&r, &q).await.unwrap();
    assert!(cd.can_select_lines);
    assert_eq!(cd.old_path.as_deref(), Some("b.txt"));
    let req = ops::CommitRequest {
        message: "rename with line 2".into(),
        amend: false,
        signoff: false,
        paths: None,
        no_verify: false,
        partial: vec![ops::PartialFile { path: "b2.txt".into(), fingerprint: cd.fingerprint.clone(), lines: vec![del(2), add(2)] }],
    };
    ops::commit(&r, &req).await.unwrap();
    let committed = git(p, &["show", "HEAD:b2.txt"]);
    assert_eq!(committed, super::tests::lines(10).replace("line 2\n", "TWO\n"), "the rename and the selected line");
    assert!(git(p, &["ls-tree", "--name-only", "HEAD"]).lines().all(|l| l != "b.txt"), "the old name is gone from HEAD");
    assert_eq!(git(p, &["diff", "--name-status", "-M", "HEAD~1", "HEAD"]).split_whitespace().next(), Some("R090"));
    assert_eq!(index_of(p, "b2.txt"), committed, "the index has the committed version");
    assert!(read(p, "b2.txt").contains("NINE"), "the unselected line stays in the working tree");
    let st = status::status(&r, false).await.unwrap();
    let files: Vec<(&str, char, char)> = st.files.iter().map(|f| (f.path.as_str(), f.index, f.worktree)).collect();
    assert_eq!(files, [("b2.txt", ' ', 'M')]);
}

#[tokio::test]
async fn undo_commit_keeps_changes_and_refuses_published_commits() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    let r = repo(p).await;
    let root = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(ops::undo_commit(&r, &ops::UndoCommitRequest { sha: root.clone() }).await.unwrap_err().status.as_u16(), 400);
    write(p, "a.txt", "2\n");
    commit_all(p, "second\n\nbody");
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(ops::undo_commit(&r, &ops::UndoCommitRequest { sha: root.clone() }).await.unwrap_err().status.as_u16(), 409, "not HEAD");
    // Published: a remote-tracking branch contains HEAD.
    git(p, &["update-ref", "refs/remotes/origin/main", &head]);
    assert_eq!(ops::undo_commit(&r, &ops::UndoCommitRequest { sha: head.clone() }).await.unwrap_err().status.as_u16(), 409);
    git(p, &["update-ref", "-d", "refs/remotes/origin/main"]);
    let msg = ops::undo_commit(&r, &ops::UndoCommitRequest { sha: head[..10].into() }).await.unwrap();
    assert_eq!(msg, "second\n\nbody");
    assert_eq!(git(p, &["rev-parse", "HEAD"]).trim(), root);
    assert_eq!(index_of(p, "a.txt"), "2\n", "the changes stay staged");
}

// ---------------------------------------------------------------- interactive rebase

/// Not a test: git runs this test binary as its sequence/message editor in the
/// rebase tests (see `shim`), and then this runs the real helper.
#[test]
fn git_editor_shim() {
    if std::env::var_os("WB_GIT_EDITOR_SHIM").is_none() {
        return;
    }
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--").expect("-- separator");
    rebase_i::cli_git_editor(&args[i + 1], &args[i + 2], &args[i + 3]).unwrap();
}

fn shim() -> String {
    let exe = std::env::current_exe().unwrap();
    format!(
        "WB_GIT_EDITOR_SHIM=1 {} --exact --quiet --test-threads=1 git::tests_flows::git_editor_shim --",
        rebase_i::sh_path(&exe)
    )
}

fn subjects(p: &Path) -> Vec<String> {
    git(p, &["log", "--format=%s", "--reverse"]).lines().map(str::to_string).collect()
}

fn entry(c: &rebase_i::PlanCommit, action: rebase_i::Action, message: Option<&str>) -> rebase_i::Entry {
    rebase_i::Entry { sha: c.sha.clone(), action, message: message.map(str::to_string) }
}

async fn run_rebase(state: &crate::app::AppState, r: &std::sync::Arc<super::Repo>, req: rebase_i::RunRequest) -> super::remote::OpInfo {
    let id = rebase_i::start(state, r.clone(), &req).await.unwrap();
    op_done(state, &id, Duration::from_secs(60)).await
}

fn run_req(plan: &rebase_i::RebasePlan, from: Option<&str>, onto: Option<&str>, entries: Vec<rebase_i::Entry>) -> rebase_i::RunRequest {
    rebase_i::RunRequest {
        op_id: None,
        from: from.map(str::to_string),
        onto: onto.map(str::to_string),
        head: plan.head.clone(),
        entries,
        autostash: false,
        confirm_pushed: false,
    }
}

#[tokio::test]
async fn interactive_rebase_rewords_reorders_squashes_and_drops() {
    use rebase_i::Action::*;
    let d = init_repo();
    let p = d.path();
    for i in 1..=5 {
        write(p, &format!("f{i}.txt"), &format!("{i}\n"));
        commit_all(p, &format!("c{i}"));
    }
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    state.git.editor.set(shim()).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let r = state.git.repo(&state.projects.require(&pid).unwrap()).await.unwrap();
    let c2 = git(p, &["rev-parse", "HEAD~3"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(c2.clone()), onto: None }).await.unwrap();
    let names: Vec<&str> = plan.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(names, ["c2", "c3", "c4", "c5"]);
    assert!(!plan.merges && plan.dirty == 0 && !plan.root);
    let c = &plan.commits;
    let entries = vec![
        entry(&c[0], Drop, None),
        entry(&c[1], Reword, Some("Reworded c3\n\n#123 keeps its hash line")),
        entry(&c[3], Pick, None),
        entry(&c[2], Squash, Some("c5 and c4 together")),
    ];
    let op = run_rebase(&state, &r, run_req(&plan, Some(&c2), None, entries)).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    assert_eq!(subjects(p), ["c1", "Reworded c3", "c5 and c4 together"]);
    assert_eq!(git(p, &["log", "-1", "--format=%B", "HEAD~1"]).trim_end(), "Reworded c3\n\n#123 keeps its hash line");
    assert!(!p.join("f2.txt").exists(), "c2 was dropped");
    assert!(p.join("f4.txt").exists() && p.join("f5.txt").exists());
    assert_eq!(status::status(&r, false).await.unwrap().state, "clean");
    let staging = tmp.path().join("data/git/rebase");
    assert_eq!(std::fs::read_dir(&staging).map(|d| d.count()).unwrap_or(0), 0, "the staging folder is removed");
}

#[tokio::test]
async fn interactive_rebase_stops_for_edit_and_for_conflicts() {
    use rebase_i::Action::*;
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "base\n");
    commit_all(p, "base");
    git(p, &["checkout", "-q", "-b", "other"]);
    write(p, "a.txt", "other\n");
    commit_all(p, "other change");
    git(p, &["checkout", "-q", "main"]);
    write(p, "a.txt", "mine\n");
    commit_all(p, "my change");
    write(p, "b.txt", "b\n");
    commit_all(p, "second");
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    state.git.editor.set(shim()).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let r = state.git.repo(&state.projects.require(&pid).unwrap()).await.unwrap();

    // Edit the last commit: the rebase stops, an amend is allowed, Continue finishes.
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(head.clone()), onto: None }).await.unwrap();
    let op = run_rebase(&state, &r, run_req(&plan, Some(&head), None, vec![entry(&plan.commits[0], Edit, None)])).await;
    assert!(op.stopped && !op.conflicts, "{op:?}");
    let (st, det) = status::detect_state(&r.git_dir);
    assert!(st == "rebasing" && det.edit, "{st} {det:?}");
    write(p, "b.txt", "b amended\n");
    git(p, &["add", "b.txt"]);
    ops::commit(&r, &ops::CommitRequest { message: "second, amended".into(), amend: true, signoff: false, paths: None, no_verify: false, partial: vec![] })
        .await
        .unwrap();
    let (pre, env) = rebase_i::continue_env(&r.git_dir, state.git.editor.get().map(String::as_str));
    let o = ops::sequencer(&r, "continue", &pre, &env).await.unwrap();
    assert!(o.ok && o.state == "clean", "{o:?}");
    assert_eq!(subjects(p).last().unwrap(), "second, amended");

    // Onto `other` with a reword of the conflicting commit: stops on the conflict; after
    // resolving, Continue still writes the prepared message.
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: None, onto: Some("other".into()) }).await.unwrap();
    assert!(plan.rewrites_all);
    let names: Vec<&str> = plan.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(names, ["my change", "second, amended"]);
    let entries = vec![entry(&plan.commits[0], Reword, Some("My change, reworded")), entry(&plan.commits[1], Pick, None)];
    let op = run_rebase(&state, &r, run_req(&plan, None, Some("other"), entries)).await;
    assert!(op.stopped && op.conflicts, "{op:?}");
    write(p, "a.txt", "resolved\n");
    git(p, &["add", "a.txt"]);
    let (pre, env) = rebase_i::continue_env(&r.git_dir, state.git.editor.get().map(String::as_str));
    assert!(!env.is_empty(), "the message editor is used for continue");
    let o = ops::sequencer(&r, "continue", &pre, &env).await.unwrap();
    assert!(o.ok && o.state == "clean", "{o:?}");
    assert_eq!(subjects(p), ["base", "other change", "My change, reworded", "second, amended"]);
    assert_eq!(read(p, "a.txt"), "resolved\n");
}

#[tokio::test]
async fn interactive_rebase_guards_pushed_dirty_moved_and_merged_histories() {
    use rebase_i::Action::*;
    let d = init_repo();
    let p = d.path();
    for i in 1..=3 {
        write(p, "f.txt", &format!("{i}\n"));
        commit_all(p, &format!("c{i}"));
    }
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    state.git.editor.set(shim()).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let r = state.git.repo(&state.projects.require(&pid).unwrap()).await.unwrap();
    // c1..c2 are "pushed".
    git(p, &["remote", "add", "origin", "https://example.invalid/x.git"]);
    let c2 = git(p, &["rev-parse", "HEAD~1"]).trim().to_string();
    git(p, &["update-ref", "refs/remotes/origin/main", &c2]);
    git(p, &["branch", "-q", "--set-upstream-to=origin/main"]);
    let c1 = git(p, &["rev-parse", "HEAD~2"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(c1.clone()), onto: None }).await.unwrap();
    assert_eq!(plan.pushed_ref.as_deref(), Some("origin/main"));
    assert_eq!(plan.commits.iter().map(|c| c.pushed).collect::<Vec<_>>(), [true, true, false]);
    assert!(plan.root);
    let reword_c1 = || vec![entry(&plan.commits[0], Reword, Some("C1")), entry(&plan.commits[1], Pick, None), entry(&plan.commits[2], Pick, None)];
    let e = rebase_i::start(&state, r.clone(), &run_req(&plan, Some(&c1), None, reword_c1())).await.unwrap_err();
    assert_eq!((e.status.as_u16(), e.code), (409, "pushed"), "{}", e.message);
    // A dirty tree needs autostash.
    write(p, "f.txt", "dirty\n");
    let e = rebase_i::start(&state, r.clone(), &run_req(&plan, Some(&c1), None, reword_c1())).await.unwrap_err();
    assert_eq!(e.code, "dirty_tree");
    let mut req = run_req(&plan, Some(&c1), None, reword_c1());
    req.confirm_pushed = true;
    req.autostash = true;
    let id = rebase_i::start(&state, r.clone(), &req).await.unwrap();
    let op = op_done(&state, &id, Duration::from_secs(60)).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    assert_eq!(subjects(p), ["C1", "c2", "c3"]);
    assert_eq!(read(p, "f.txt"), "dirty\n", "the local change came back");
    git(p, &["checkout", "--", "f.txt"]);
    // HEAD moved since the plan.
    let e = rebase_i::start(&state, r.clone(), &run_req(&plan, Some(&c1), None, reword_c1())).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 409);
    // A merge in the range is refused.
    git(p, &["checkout", "-q", "-b", "side", "HEAD~1"]);
    write(p, "s.txt", "s\n");
    commit_all(p, "side");
    git(p, &["checkout", "-q", "main"]);
    git(p, &["merge", "-q", "--no-edit", "side"]);
    let first = git(p, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(first.clone()), onto: None }).await.unwrap();
    assert!(plan.merges);
}

#[tokio::test]
async fn interactive_rebase_onto_a_branch_skips_commits_already_there() {
    use rebase_i::Action::*;
    let d = init_repo();
    let p = d.path();
    write(p, "base.txt", "base\n");
    commit_all(p, "base");
    git(p, &["checkout", "-q", "-b", "feature"]);
    for (f, s) in [("a.txt", "feat A"), ("b.txt", "feat B"), ("c.txt", "feat C")] {
        write(p, f, &format!("{s}\n"));
        commit_all(p, s);
    }
    git(p, &["commit", "-q", "--allow-empty", "-m", "empty on feature"]);
    let feat_b = git(p, &["rev-parse", "HEAD~2"]).trim().to_string();
    // main got a hotfix: feat B cherry-picked, plus an empty commit (patch-equivalent to
    // the feature's empty one, which git still keeps).
    git(p, &["checkout", "-q", "main"]);
    git(p, &["cherry-pick", &feat_b]);
    write(p, "m.txt", "m\n");
    commit_all(p, "main work");
    git(p, &["commit", "-q", "--allow-empty", "-m", "empty on main"]);
    git(p, &["checkout", "-q", "feature"]);
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    state.git.editor.set(shim()).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let r = state.git.repo(&state.projects.require(&pid).unwrap()).await.unwrap();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: None, onto: Some("main".into()) }).await.unwrap();
    let names: Vec<&str> = plan.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(names, ["feat A", "feat C", "empty on feature"]);
    assert_eq!(plan.skipped.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["feat B"]);
    assert!(plan.rewrites_all);
    let entries = vec![entry(&plan.commits[0], Pick, None), entry(&plan.commits[1], Reword, Some("feat C, reworded")), entry(&plan.commits[2], Pick, None)];
    let op = run_rebase(&state, &r, run_req(&plan, None, Some("main"), entries)).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    assert_eq!(subjects(p), ["base", "feat B", "main work", "empty on main", "feat A", "feat C, reworded", "empty on feature"]);
}

#[tokio::test]
async fn pushed_means_on_any_remote_branch_not_only_the_upstream() {
    use rebase_i::Action::*;
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", "0\n");
    commit_all(p, "base");
    git(p, &["remote", "add", "origin", "https://example.invalid/x.git"]);
    let base = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    git(p, &["update-ref", "refs/remotes/origin/main", &base]);
    // topic tracks origin/main, and was pushed as origin/topic.
    git(p, &["checkout", "-q", "-b", "topic", "--track", "origin/main"]);
    for i in 1..=2 {
        write(p, "f.txt", &format!("{i}\n"));
        commit_all(p, &format!("t{i}"));
    }
    let head = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    git(p, &["update-ref", "refs/remotes/origin/topic", &head]);
    git(p, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    state.git.editor.set(shim()).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let r = state.git.repo(&state.projects.require(&pid).unwrap()).await.unwrap();
    let t1 = git(p, &["rev-parse", "HEAD~1"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(t1.clone()), onto: None }).await.unwrap();
    assert_eq!(plan.commits.iter().map(|c| c.pushed).collect::<Vec<_>>(), [true, true]);
    assert_eq!(plan.pushed_ref.as_deref(), Some("origin/topic"));
    let e = rebase_i::start(&state, r.clone(), &run_req(&plan, Some(&t1), None, vec![entry(&plan.commits[0], Pick, None), entry(&plan.commits[1], Drop, None)]))
        .await
        .unwrap_err();
    assert_eq!((e.status.as_u16(), e.code), (409, "pushed"), "{}", e.message);
    assert!(e.message.contains("origin/topic"), "{}", e.message);
    // Fixing a new local commit up into the pushed t2 amends t2: confirmation needed too.
    write(p, "f.txt", "local\n");
    commit_all(p, "local fix");
    let t2 = git(p, &["rev-parse", "HEAD~1"]).trim().to_string();
    let plan = rebase_i::plan(&r, &rebase_i::PlanQuery { from: Some(t2.clone()), onto: None }).await.unwrap();
    assert_eq!(plan.commits.iter().map(|c| c.pushed).collect::<Vec<_>>(), [true, false]);
    let fixup = || vec![entry(&plan.commits[0], Pick, None), entry(&plan.commits[1], Fixup, None)];
    let e = rebase_i::start(&state, r.clone(), &run_req(&plan, Some(&t2), None, fixup())).await.unwrap_err();
    assert_eq!((e.status.as_u16(), e.code), (409, "pushed"), "{}", e.message);
    let mut req = run_req(&plan, Some(&t2), None, fixup());
    req.confirm_pushed = true;
    let op = run_rebase(&state, &r, req).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    assert_eq!(subjects(p), ["base", "t1", "t2"]);
    assert_eq!(read(p, "f.txt"), "local\n");
}

// ---------------------------------------------------------------- shelf

#[tokio::test]
async fn shelf_round_trips_text_binary_new_deleted_and_renamed_files() {
    let d = init_repo();
    let p = d.path();
    let bin_old: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
    write(p, "text.txt", &super::tests::lines(20));
    write(p, "gone.txt", "bye\n");
    write(p, "old.txt", &super::tests::lines(12));
    std::fs::write(p.join("img.bin"), &bin_old).unwrap();
    commit_all(p, "init");
    write(p, "text.txt", &super::tests::lines(20).replace("line 7\n", "SEVEN\n"));
    std::fs::remove_file(p.join("gone.txt")).unwrap();
    git(p, &["mv", "old.txt", "renamed.txt"]);
    write(p, "fresh.txt", "brand new\n");
    let bin_new: Vec<u8> = bin_old.iter().rev().copied().chain([0u8, 1, 2]).collect();
    std::fs::write(p.join("img.bin"), &bin_new).unwrap();
    let new_bin: Vec<u8> = vec![0, 159, 146, 150, 0, 7];
    std::fs::write(p.join("new.bin"), &new_bin).unwrap();
    let r = repo(p).await;
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("shelf");
    let paths: Vec<String> = ["text.txt", "gone.txt", "renamed.txt", "fresh.txt", "img.bin", "new.bin"].iter().map(|s| s.to_string()).collect();
    let meta = shelf::shelve(&r, &root, "Everything", &paths, false).await.unwrap();
    assert_eq!(meta.files.len(), 6, "{:?}", meta.files);
    assert!(meta.files.iter().any(|f| f.status == 'R' && f.old_path.as_deref() == Some("old.txt")));
    assert!(meta.files.iter().filter(|f| f.binary).count() == 2);
    let st = status::status(&r, false).await.unwrap();
    assert!(st.files.is_empty(), "the working tree is clean: {:?}", st.files);
    assert_eq!(read(p, "text.txt"), super::tests::lines(20));
    assert!(p.join("gone.txt").exists() && !p.join("fresh.txt").exists() && !p.join("new.bin").exists());

    // Viewable as a commit on top of the base.
    let shown = shelf::view_commit(&r, &root, &meta.id).await.unwrap();
    let vc = shown.view_commit.clone().unwrap();
    let changed = git(p, &["diff", "--name-status", "-M", &format!("{vc}^"), &vc]);
    assert!(changed.contains("fresh.txt") && changed.contains("R100\told.txt\trenamed.txt"), "{changed}");

    // Unshelve everything and remove the shelf.
    let res = shelf::unshelve(&r, &root, &meta.id, &shelf::UnshelveRequest { paths: None, remove: true, changelist: None }).await.unwrap();
    assert!(res.ok && res.removed, "{res:?}");
    assert!(shelf::list(&root).is_empty());
    assert!(read(p, "text.txt").contains("SEVEN"));
    assert!(!p.join("gone.txt").exists());
    assert_eq!(read(p, "fresh.txt"), "brand new\n");
    assert_eq!(std::fs::read(p.join("img.bin")).unwrap(), bin_new);
    assert_eq!(std::fs::read(p.join("new.bin")).unwrap(), new_bin);
    assert_eq!(read(p, "renamed.txt"), super::tests::lines(12));
    let st = status::status(&r, false).await.unwrap();
    let by = |path: &str| st.files.iter().find(|f| f.path == path).map(|f| (f.index, f.worktree));
    assert_eq!(by("text.txt"), Some((' ', 'M')), "modifications come back unstaged");
    assert_eq!(by("fresh.txt"), Some(('A', ' ')), "new files come back staged");
    let ren = st.files.iter().find(|f| f.path == "renamed.txt").unwrap();
    assert_eq!((ren.index, ren.orig_path.as_deref()), ('R', Some("old.txt")), "a rename stays a staged rename");
}

#[tokio::test]
async fn shelf_partial_unshelve_keep_and_conflicts() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    write(p, "b.txt", "b\n");
    commit_all(p, "init");
    let r = repo(p).await;
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("shelf");

    // "Save to shelf": a copy, the working tree keeps the changes.
    write(p, "a.txt", "a2\n");
    write(p, "b.txt", "b2\n");
    let kept = shelf::shelve(&r, &root, "Copy", &["a.txt".into(), "b.txt".into()], true).await.unwrap();
    assert_eq!(read(p, "a.txt"), "a2\n");
    shelf::delete(&root, &kept.id).unwrap();

    let meta = shelf::shelve(&r, &root, "Two files", &["a.txt".into(), "b.txt".into()], false).await.unwrap();
    // Unshelve one file with remove: the shelf keeps the other.
    let res = shelf::unshelve(&r, &root, &meta.id, &shelf::UnshelveRequest { paths: Some(vec!["a.txt".into()]), remove: true, changelist: None }).await.unwrap();
    assert!(res.ok && !res.removed);
    assert_eq!(read(p, "a.txt"), "a2\n");
    assert_eq!(read(p, "b.txt"), "b\n");
    let left = shelf::load(&root, &meta.id).unwrap();
    assert_eq!(left.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);

    // A conflicting commit meanwhile: the 3-way apply leaves a conflict and keeps the shelf.
    write(p, "b.txt", "b-committed\n");
    commit_all(p, "b changed");
    let res = shelf::unshelve(&r, &root, &meta.id, &shelf::UnshelveRequest { paths: None, remove: true, changelist: None }).await.unwrap();
    assert!(res.conflicts && !res.removed, "{res:?}");
    assert_eq!(res.conflicted, vec!["b.txt"]);
    assert!(status::status(&r, false).await.unwrap().files.iter().any(|f| f.path == "b.txt" && f.conflict));
    assert!(shelf::load(&root, &meta.id).is_ok());
    // Local changes in the way are reported, not merged over.
    git(p, &["checkout", "-q", "--theirs", "b.txt"]);
    git(p, &["reset", "-q", "--hard"]);
    write(p, "a.txt", "local\n");
    let meta2 = shelf::shelve(&r, &root, "A again", &["a.txt".into()], false).await.unwrap();
    write(p, "a.txt", "in the way\n");
    let e = shelf::unshelve(&r, &root, &meta2.id, &shelf::UnshelveRequest::default()).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 409, "{}", e.message);
}

// ---------------------------------------------------------------- bisect, line history

#[tokio::test]
async fn bisect_finds_the_first_bad_commit() {
    let d = init_repo();
    let p = d.path();
    for i in 1..=9 {
        write(p, "v.txt", if i >= 6 { "broken\n" } else { "fine\n" });
        write(p, "n.txt", &format!("{i}\n"));
        commit_all(p, &format!("c{i}"));
    }
    let r = repo(p).await;
    let culprit = git(p, &["rev-parse", "HEAD~3"]).trim().to_string();
    let first = git(p, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
    assert!(!bisect::state(&r).await.unwrap().active);
    let out = bisect::start(&r, &bisect::StartRequest { bad: None, good: vec![first.clone()] }).await.unwrap();
    assert!(out.state.active && out.message.starts_with("Bisecting"), "{out:?}");
    assert_eq!(status::status(&r, false).await.unwrap().state, "bisecting");
    assert!(out.state.remaining.unwrap() >= 1 && !out.state.candidates.is_empty());
    let mut result = None;
    for _ in 0..10 {
        let verdict = if read(p, "v.txt") == "broken\n" { "bad" } else { "good" };
        let o = bisect::mark(&r, &bisect::MarkRequest { verdict: verdict.into(), rev: None }).await.unwrap();
        if let Some(res) = o.state.result.clone() {
            assert!(o.message.contains("is the first") && o.message.contains(&res), "{}", o.message);
            result = Some(res);
            break;
        }
    }
    assert_eq!(result.as_deref(), Some(culprit.as_str()));
    let o = bisect::reset(&r).await.unwrap();
    assert!(!o.state.active);
    assert_eq!(git(p, &["symbolic-ref", "--short", "HEAD"]).trim(), "main");
    assert!(bisect::start(&r, &bisect::StartRequest { bad: None, good: vec![] }).await.is_err());
}

#[tokio::test]
async fn line_history_lists_only_commits_touching_the_range() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", &super::tests::lines(20));
    commit_all(p, "create");
    write(p, "f.txt", &super::tests::lines(20).replace("line 15\n", "fifteen\n"));
    commit_all(p, "touch 15");
    write(p, "f.txt", &super::tests::lines(20).replace("line 15\n", "fifteen\n").replace("line 3\n", "three\n"));
    commit_all(p, "touch 3");
    let r = repo(p).await;
    let page = log::log(&r, &log::LogQuery { path: Some("f.txt".into()), lines: Some("14,16".into()), ..Default::default() }).await.unwrap();
    let s: Vec<&str> = page.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(s, ["touch 15", "create"]);
    assert!(page.linear);
    assert!(log::log(&r, &log::LogQuery { path: Some("f.txt".into()), lines: Some("0,2".into()), ..Default::default() }).await.is_err());

    // An editor selection: working-tree lines, shifted by an uncommitted line above them.
    let committed = git(p, &["show", "HEAD:f.txt"]);
    write(p, "f.txt", &committed.replace("line 5\n", "line 5\nNEW\n"));
    let wt = |lines: &str| log::LogQuery { path: Some("f.txt".into()), lines: Some(lines.into()), worktree_lines: Some(true), ..Default::default() };
    let subjects_of = |page: log::LogPage| page.commits.into_iter().map(|c| c.subject).collect::<Vec<_>>();
    assert_eq!(subjects_of(log::log(&r, &wt("16,16")).await.unwrap()), ["touch 15", "create"], "working line 16 is HEAD line 15");
    assert_eq!(subjects_of(log::log(&r, &wt("3,3")).await.unwrap()), ["touch 3", "create"], "above the new line nothing moves");
    let e = log::log(&r, &wt("6,6")).await.unwrap_err();
    assert!(e.message.contains("not committed yet"), "{}", e.message);
    // Renamed (staged) since HEAD: the history of the old name.
    git(p, &["mv", "f.txt", "g.txt"]);
    let q = log::LogQuery { path: Some("g.txt".into()), ..wt("16,16") };
    assert_eq!(subjects_of(log::log(&r, &q).await.unwrap()), ["touch 15", "create"]);
}

// ---------------------------------------------------------------- changelists over the REST routes, MCP

#[tokio::test]
async fn changelists_follow_the_status_shelve_by_list_and_are_readable_over_mcp() {
    use axum::http::Method;
    use crate::mcp::{McpCtx, call_api};
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    write(p, "b.txt", "b\n");
    commit_all(p, "init");
    write(p, "a.txt", "a2\n");
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    let _router = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    let user = McpCtx { terminal_id: None, project_id: None };
    let base = format!("/api/projects/{pid}/git");
    let get = |path: &'static str| {
        let (state, user, base) = (state.clone(), user.clone(), base.clone());
        async move { call_api(&state, Method::GET, &format!("{base}/{path}"), None, &user).await.unwrap() }
    };
    let v = get("changelists").await;
    assert_eq!(v["lists"][0]["name"], "Changes");
    assert_eq!(v["lists"][0]["files"], serde_json::json!(["a.txt"]));
    let created = call_api(&state, Method::POST, &format!("{base}/changelists"), Some(serde_json::json!({ "name": "Feature", "active": true })), &user)
        .await
        .unwrap();
    let feature = created["id"].as_str().unwrap().to_string();
    // A new change joins the active list as soon as the status sees it.
    write(p, "b.txt", "b2\n");
    get("status").await;
    write(p, "a.txt", "a3\n");
    let v = get("changelists").await;
    let files = |v: &serde_json::Value, i: usize| v["lists"][i]["files"].clone();
    assert_eq!(files(&v, 0), serde_json::json!(["a.txt"]), "a.txt stays where it was");
    assert_eq!(files(&v, 1), serde_json::json!(["b.txt"]));
    assert_eq!(v["active"], feature.as_str());
    // Stash, status, pop: the changes leave the working tree for a while and come back
    // into their lists (not into the active one).
    let post = |path: &'static str, body: serde_json::Value| {
        let (state, user, base) = (state.clone(), user.clone(), base.clone());
        async move { call_api(&state, Method::POST, &format!("{base}/{path}"), Some(body), &user).await.unwrap() }
    };
    post("stashes", serde_json::json!({})).await;
    assert!(get("changelists").await["lists"][0]["files"].as_array().unwrap().is_empty());
    post("stash/pop", serde_json::json!({ "index": 0 })).await;
    let v = get("changelists").await;
    assert_eq!((files(&v, 0), files(&v, 1)), (serde_json::json!(["a.txt"]), serde_json::json!(["b.txt"])));
    // a.txt committed from a terminal, then changed again: a new change, in the active list.
    git(p, &["commit", "-qm", "a only", "--", "a.txt"]);
    get("changelists").await;
    write(p, "a.txt", "a4\n");
    let v = get("changelists").await;
    assert_eq!((files(&v, 0), files(&v, 1)), (serde_json::json!([]), serde_json::json!(["a.txt", "b.txt"])));
    // Rolled back in Workbench: forgotten.
    post("discard", serde_json::json!({ "paths": ["a.txt"] })).await;
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(tmp.path().join(format!("data/git/changelists/{pid}.json"))).unwrap()).unwrap();
    assert!(stored["files"].get("a.txt").is_none(), "{stored}");
    assert!(stored["files"].get("b.txt").is_some(), "{stored}");
    // Shelve the Feature list by id: named after it, b.txt rolled back.
    let meta = call_api(&state, Method::POST, &format!("{base}/shelf"), Some(serde_json::json!({ "name": "", "changelist": feature })), &user)
        .await
        .unwrap();
    assert_eq!(meta["name"], "Feature");
    assert_eq!(read(p, "b.txt"), "b\n");
    // Agents read lists and shelves; a session of another project is refused.
    let tools = super::mcp_tools();
    let tool = tools.iter().find(|t| t.name == "workbench_changelists").unwrap().handler.clone();
    let session = McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };
    let out = (tool)(state.clone(), session, serde_json::json!({})).await.unwrap();
    let crate::mcp::ToolOutput::Json(j) = out else { panic!("json output") };
    assert_eq!(j["shelves"][0]["name"], "Feature");
    assert_eq!(j["shelves"][0]["files"], serde_json::json!(["b.txt"]));
    let other = McpCtx { terminal_id: Some("t2".into()), project_id: Some("elsewhere".into()) };
    let e = (tool)(state.clone(), other, serde_json::json!({ "projectId": pid })).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 403);
    // The shelf remembers the list; unshelving (no list chosen) puts b.txt back into
    // Feature although Changes is active now and the store forgot b.txt meanwhile.
    assert_eq!(meta["files"][0]["changelist"], feature.as_str());
    call_api(&state, Method::PATCH, &format!("{base}/changelists/default"), Some(serde_json::json!({ "active": true })), &user).await.unwrap();
    let store_file = tmp.path().join(format!("data/git/changelists/{pid}.json"));
    let mut stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&store_file).unwrap()).unwrap();
    stored["files"].as_object_mut().unwrap().remove("b.txt");
    std::fs::write(&store_file, stored.to_string()).unwrap();
    let id = meta["id"].as_str().unwrap();
    call_api(&state, Method::POST, &format!("{base}/shelf/{id}/unshelve"), Some(serde_json::json!({ "remove": true })), &user).await.unwrap();
    let v = get("changelists").await;
    assert_eq!((files(&v, 0), files(&v, 1)), (serde_json::json!([]), serde_json::json!(["b.txt"])));
}

// ---------------------------------------------------------------- Local History labels (files ↔ git)

/// Operations that rewrite the working tree put an automatic Local History label first
/// (`files::history::auto_label`), so the versions from before them are easy to find.
#[tokio::test(flavor = "multi_thread")]
async fn working_tree_rewrites_label_the_local_history_first() {
    use crate::mcp::{McpCtx, call_api};
    use axum::http::Method;
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    commit_all(p, "init");
    git(p, &["branch", "topic"]);
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    let _router = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    let user = McpCtx { terminal_id: None, project_id: None };
    let base = format!("/api/projects/{pid}");
    let post = |path: &str, body: serde_json::Value| {
        let (state, user, url) = (state.clone(), user.clone(), format!("{base}/git/{path}"));
        async move { call_api(&state, Method::POST, &url, Some(body), &user).await.unwrap() }
    };
    let labels = || {
        let (state, user, url) = (state.clone(), user.clone(), format!("{base}/files/history/dir?path="));
        async move {
            let v = call_api(&state, Method::GET, &url, None, &user).await.unwrap();
            let mut out: Vec<String> =
                v["entries"].as_array().unwrap().iter().filter(|e| e["kind"] == "auto").map(|e| e["label"].as_str().unwrap().to_string()).collect();
            out.reverse();
            out
        }
    };

    write(p, "a.txt", "a2\n");
    post("stashes", serde_json::json!({})).await;
    post("stash/pop", serde_json::json!({ "index": 0 })).await;
    post("discard", serde_json::json!({ "paths": ["a.txt"] })).await;
    post("checkout", serde_json::json!({ "ref": "topic" })).await;
    post("reset", serde_json::json!({ "ref": "HEAD", "mode": "soft" })).await;
    post("reset", serde_json::json!({ "ref": "HEAD", "mode": "hard" })).await;
    write(p, "a.txt", "a3\n");
    let shelf = post("shelf", serde_json::json!({ "name": "wip", "paths": ["a.txt"] })).await;
    post(&format!("shelf/{}/unshelve", shelf["id"].as_str().unwrap()), serde_json::json!({ "remove": true })).await;
    assert_eq!(
        labels().await,
        [
            "Before git stash",
            "Before git stash pop",
            "Before rollback",
            "Before git checkout topic",
            "Before git reset --hard HEAD",
            "Before shelve wip",
            "Before unshelve",
        ],
        "a soft reset leaves the working tree alone"
    );
}
