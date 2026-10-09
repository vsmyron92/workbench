//! Integration tests against throwaway repositories in temp directories.

use std::path::Path;

use super::diff::{DiffMode, DiffQuery, HunkOp, HunkRequest, apply_hunks, file_diff};
use super::repo::Repo;
use super::{blame, conflicts, log, ops, refs, stash, status};

/// Run git synchronously in `dir` for test setup (panics on failure).
pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A fresh repository with local config that overrides anything global.
pub(super) fn init_repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    init_repo_at(d.path());
    d
}

/// [`init_repo`] in an existing folder (a repository inside another one's folder).
pub(super) fn init_repo_at(p: &Path) {
    git(p, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.autocrlf", "false"),
        ("merge.conflictstyle", "merge"),
        ("pull.rebase", "false"),
    ] {
        git(p, &["config", k, v]);
    }
    no_hooks(p);
}

/// Run no hooks in `repo` (global ones included): `core.hooksPath` names a folder that does
/// not exist. Not `/dev/null`, which Git for Windows reads as a folder of its installation.
pub(super) fn no_hooks(repo: &Path) {
    git(repo, &["config", "core.hooksPath", &crate::util::os::path::to_slash(&repo.join(".git").join("no-hooks"))]);
}

pub(super) fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, text).unwrap();
}

pub(super) fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

pub(super) async fn repo(dir: &Path) -> Repo {
    Repo::discover_root("test", dir).await.unwrap()
}

pub(super) fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[tokio::test]
async fn status_reports_renames_untracked_and_conflicts() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "one\ntwo\n");
    write(p, "old name.txt", &lines(20));
    commit_all(p, "init");
    git(p, &["mv", "old name.txt", "new name.txt"]);
    write(p, "dir/untracked.txt", "u\n");
    write(p, "a.txt", "one\nTWO\n");
    let r = repo(p).await;
    let st = status::status(&r, false).await.unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.state, "clean");
    let ren = st.files.iter().find(|f| f.path == "new name.txt").unwrap();
    assert_eq!((ren.index, ren.orig_path.as_deref()), ('R', Some("old name.txt")));
    let un = st.files.iter().find(|f| f.path == "dir/untracked.txt").unwrap();
    assert_eq!((un.index, un.worktree), ('?', '?'));
    let m = st.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert_eq!((m.index, m.worktree), (' ', 'M'));

    // A merge conflict.
    commit_all(p, "rename");
    git(p, &["checkout", "-q", "-b", "feature"]);
    write(p, "a.txt", "one\nfeature\n");
    commit_all(p, "feature change");
    git(p, &["checkout", "-q", "main"]);
    write(p, "a.txt", "one\nmain\n");
    commit_all(p, "main change");
    let out = ops::merge(&r, &ops::MergeRequest { rev: "feature".into(), no_ff: false, ff_only: false, squash: false, message: None })
        .await
        .unwrap();
    assert!(!out.ok && out.conflicts, "{out:?}");
    let st = status::status(&r, false).await.unwrap();
    assert_eq!(st.state, "merging");
    assert_eq!(st.state_detail.branch.as_deref(), Some("feature"));
    let c = st.files.iter().find(|f| f.path == "a.txt").unwrap();
    assert!(c.conflict);
    assert_eq!((c.index, c.worktree), ('U', 'U'));

    // Conflict versions and resolution.
    let v = conflicts::versions(&r, "a.txt").await.unwrap();
    assert_eq!(v.ours.as_deref(), Some("one\nmain\n"));
    assert_eq!(v.theirs.as_deref(), Some("one\nfeature\n"));
    assert_eq!(v.base.as_deref(), Some("one\nTWO\n"));
    assert!(v.merged.contains("<<<<<<<"));
    assert!(v.theirs_label.contains("feature"));
    conflicts::resolve(&r, &conflicts::ResolveRequest { path: "a.txt".into(), content: None, side: Some("theirs".into()) })
        .await
        .unwrap();
    let st = status::status(&r, false).await.unwrap();
    assert!(!st.files.iter().any(|f| f.conflict));
    let done = ops::sequencer(&r, "continue", &[], &[]).await.unwrap();
    assert!(done.ok, "{done:?}");
    assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "one\nfeature\n");
    assert_eq!(status::status(&r, false).await.unwrap().state, "clean");
}

fn edit_three_places(p: &Path) {
    let mut v: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
    v[1] = "CHANGED 2".into();
    v[19] = "CHANGED 20".into();
    v.insert(37, "INSERTED after 37".into());
    std::fs::write(p.join("f.txt"), v.join("\n") + "\n").unwrap();
}

#[tokio::test]
async fn stages_unstages_and_discards_single_hunks() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", &lines(40));
    commit_all(p, "init");
    edit_three_places(p);
    let r = repo(p).await;

    let q = DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Working), ..Default::default() };
    let (diff, _) = file_diff(&r, &q).await.unwrap();
    assert_eq!(diff.hunks.len(), 3);
    assert!(diff.can_stage_hunks);
    assert_eq!(diff.original, lines(40));
    assert!(diff.modified.contains("CHANGED 20"));

    // Stage only the middle hunk.
    apply_hunks(&r, HunkOp::Stage, &HunkRequest { path: "f.txt".into(), hunk_indexes: vec![1], fingerprint: diff.fingerprint.clone() })
        .await
        .unwrap();
    let cached = git(p, &["diff", "--cached"]);
    assert!(cached.contains("+CHANGED 20"), "{cached}");
    assert!(!cached.contains("CHANGED 2\n") && !cached.contains("INSERTED"), "{cached}");
    let (working, _) = file_diff(&r, &q).await.unwrap();
    assert_eq!(working.hunks.len(), 2);

    // Stage the last hunk too (its position shifted by nothing; the first is still unstaged).
    apply_hunks(&r, HunkOp::Stage, &HunkRequest { path: "f.txt".into(), hunk_indexes: vec![1], fingerprint: working.fingerprint })
        .await
        .unwrap();
    let cached = git(p, &["diff", "--cached"]);
    assert!(cached.contains("+INSERTED after 37") && cached.contains("+CHANGED 20"));

    // Unstage the middle hunk again from the staged diff.
    let sq = DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Staged), ..Default::default() };
    let (staged, _) = file_diff(&r, &sq).await.unwrap();
    assert_eq!(staged.hunks.len(), 2);
    assert!(staged.can_stage_hunks);
    apply_hunks(&r, HunkOp::Unstage, &HunkRequest { path: "f.txt".into(), hunk_indexes: vec![0], fingerprint: staged.fingerprint })
        .await
        .unwrap();
    let cached = git(p, &["diff", "--cached"]);
    assert!(!cached.contains("CHANGED 20") && cached.contains("INSERTED"), "{cached}");

    // Discard the first hunk from the working tree; the others survive.
    let (working, _) = file_diff(&r, &q).await.unwrap();
    assert_eq!(working.hunks.len(), 2);
    apply_hunks(&r, HunkOp::Discard, &HunkRequest { path: "f.txt".into(), hunk_indexes: vec![0], fingerprint: working.fingerprint })
        .await
        .unwrap();
    let text = std::fs::read_to_string(p.join("f.txt")).unwrap();
    assert!(text.contains("line 2\n") && !text.contains("CHANGED 2\n"));
    assert!(text.contains("CHANGED 20") && text.contains("INSERTED after 37"));
}

#[tokio::test]
async fn hunk_operations_refuse_a_stale_fingerprint() {
    let d = init_repo();
    let p = d.path();
    write(p, "f.txt", &lines(40));
    commit_all(p, "init");
    edit_three_places(p);
    let r = repo(p).await;
    let q = DiffQuery { path: "f.txt".into(), mode: Some(DiffMode::Working), ..Default::default() };
    let (diff, _) = file_diff(&r, &q).await.unwrap();
    // Someone else edits the file after we looked at it.
    let text = std::fs::read_to_string(p.join("f.txt")).unwrap().replace("line 30", "agent edit");
    std::fs::write(p.join("f.txt"), text).unwrap();
    let err = apply_hunks(&r, HunkOp::Stage, &HunkRequest { path: "f.txt".into(), hunk_indexes: vec![0], fingerprint: diff.fingerprint })
        .await
        .unwrap_err();
    assert_eq!(err.status.as_u16(), 409);
    assert_eq!(git(p, &["diff", "--cached"]), "");
}

#[tokio::test]
async fn diff_modes_handle_untracked_new_binary_and_root_commits() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    commit_all(p, "root");
    let root = git(p, &["rev-parse", "HEAD"]).trim().to_string();
    let r = repo(p).await;
    let (c, _) = file_diff(&r, &DiffQuery { path: "a.txt".into(), mode: Some(DiffMode::Commit), sha: Some(root.clone()), ..Default::default() })
        .await
        .unwrap();
    assert!(c.original_missing && c.original.is_empty() && c.modified == "a\n");
    assert_eq!(c.hunks.len(), 1);
    assert!(!c.can_stage_hunks);

    write(p, "new.txt", "x\ny\n");
    let (u, _) = file_diff(&r, &DiffQuery { path: "new.txt".into(), ..Default::default() }).await.unwrap();
    assert!(u.untracked && !u.can_stage_hunks && u.modified == "x\ny\n" && u.hunks.len() == 1);

    std::fs::write(p.join("bin.dat"), [0u8, 1, 2, 3, 0, 5]).unwrap();
    git(p, &["add", "bin.dat"]);
    let (b, _) = file_diff(&r, &DiffQuery { path: "bin.dat".into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap();
    assert!(b.binary && b.modified.is_empty() && b.hunks.is_empty());

    // A rename shows old → new in commit mode.
    git(p, &["mv", "a.txt", "b.txt"]);
    commit_all(p, "rename");
    let (rn, _) = file_diff(&r, &DiffQuery { path: "b.txt".into(), mode: Some(DiffMode::Commit), sha: Some("HEAD".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(rn.old_path.as_deref(), Some("a.txt"));
    assert_eq!(rn.original, "a\n");

    // Compare two refs.
    let (cmp, _) = file_diff(
        &r,
        &DiffQuery { path: "b.txt".into(), mode: Some(DiffMode::Compare), base: Some(root.clone()), head: Some("HEAD".into()), ..Default::default() },
    )
    .await
    .unwrap();
    assert_eq!(cmp.old_path.as_deref(), Some("a.txt"));
    assert!(file_diff(&r, &DiffQuery { path: "../x".into(), ..Default::default() }).await.is_err());
}

#[tokio::test]
async fn log_graph_refs_and_commit_details() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "first");
    git(p, &["tag", "-a", "v1", "-m", "release 1"]);
    git(p, &["checkout", "-q", "-b", "feature"]);
    write(p, "b.txt", "b\n");
    commit_all(p, "feature work");
    git(p, &["checkout", "-q", "main"]);
    write(p, "a.txt", "2\n");
    commit_all(p, "main work");
    git(p, &["merge", "-q", "--no-ff", "--no-edit", "feature"]);
    let r = repo(p).await;

    let page = log::log(&r, &log::LogQuery { all: Some(true), ..Default::default() }).await.unwrap();
    assert_eq!(page.commits.len(), 4);
    assert!(!page.has_more);
    let merge = &page.commits[0];
    assert_eq!(merge.parents.len(), 2);
    assert!(merge.refs.iter().any(|x| x.name == "main" && x.kind == "head"));
    // Topological order: every commit comes before its parents.
    for (i, c) in page.commits.iter().enumerate() {
        for par in &c.parents {
            let j = page.commits.iter().position(|x| &x.sha == par).unwrap();
            assert!(j > i);
        }
    }
    let first = page.commits.iter().find(|c| c.subject == "first").unwrap();
    assert!(first.refs.iter().any(|x| x.name == "v1" && x.kind == "tag"));
    let feat = page.commits.iter().find(|c| c.subject == "feature work").unwrap();
    assert!(feat.refs.iter().any(|x| x.name == "feature" && x.kind == "branch"));
    assert!(first.time > 0);

    let paged = log::log(&r, &log::LogQuery { all: Some(true), limit: Some(2), ..Default::default() }).await.unwrap();
    assert!(paged.has_more && paged.commits.len() == 2);
    let rest = log::log(&r, &log::LogQuery { all: Some(true), limit: Some(2), skip: Some(2), ..Default::default() }).await.unwrap();
    assert_eq!(rest.commits.len(), 2);
    assert!(!rest.has_more);

    let grep = log::log(&r, &log::LogQuery { grep: Some("FEATURE WORK".into()), ..Default::default() }).await.unwrap();
    assert_eq!(grep.commits.len(), 1);
    let file = log::log(&r, &log::LogQuery { path: Some("b.txt".into()), ..Default::default() }).await.unwrap();
    assert_eq!(file.commits.len(), 1);
    assert!(log::log(&r, &log::LogQuery { rev: Some("--output=/tmp/x".into()), ..Default::default() }).await.is_err());

    let det = log::commit_details(&r, &feat.sha).await.unwrap();
    assert_eq!(det.message, "feature work");
    assert_eq!(det.files.len(), 1);
    assert_eq!((det.files[0].path.as_str(), det.files[0].status, det.files[0].additions), ("b.txt", 'A', 1));

    let cmp = log::compare(&r, "feature", "main").await.unwrap();
    assert_eq!(cmp.base_only.len(), 0);
    assert_eq!(cmp.head_only.len(), 2);

    let br = refs::branches(&r).await.unwrap();
    assert_eq!(br.current.as_deref(), Some("main"));
    assert!(br.local.iter().any(|b| b.name == "feature" && !b.current));
    assert_eq!(br.tags[0].name, "v1");
    assert!(br.recent.contains(&"feature".to_string()));

    // Deleting an unmerged branch answers "not merged" instead of failing; force works.
    git(p, &["branch", "unmerged", "feature"]);
    git(p, &["checkout", "-q", "unmerged"]);
    write(p, "c.txt", "c\n");
    commit_all(p, "unmerged work");
    git(p, &["checkout", "-q", "main"]);
    let del = |force| ops::DeleteBranchRequest { name: "unmerged".into(), force };
    let not_merged = ops::delete_branch(&r, &del(false)).await.unwrap();
    assert!(not_merged.is_some_and(|m| m.contains("not fully merged")));
    assert_eq!(ops::delete_branch(&r, &del(true)).await.unwrap(), None);

    let bl = blame::blame(&r, "a.txt", None).await.unwrap();
    assert_eq!(bl.lines.len(), 1);
    assert_eq!(bl.lines[0].summary, "main work");
    assert_eq!(bl.lines[0].line, 1);
}

#[tokio::test]
async fn commit_amend_stash_and_checkout() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    let r = repo(p).await;
    ops::stage(&r, &ops::PathsRequest { paths: vec!["a.txt".into()], all: false }).await.unwrap();
    // Unstage before the first commit works too.
    ops::unstage(&r, &ops::PathsRequest { paths: vec!["a.txt".into()], all: false }).await.unwrap();
    ops::stage(&r, &ops::PathsRequest { paths: vec![], all: true }).await.unwrap();
    let c = ops::commit(&r, &ops::CommitRequest { message: "first".into(), amend: false, signoff: false, paths: None, no_verify: false, partial: vec![] })
        .await
        .unwrap();
    assert_eq!(c.sha.len(), 40);
    let c2 = ops::commit(&r, &ops::CommitRequest { message: "first, amended".into(), amend: true, signoff: true, paths: None, no_verify: false, partial: vec![] })
        .await
        .unwrap();
    assert_ne!(c.sha, c2.sha);
    let msg = ops::last_commit_message(&r).await.unwrap();
    assert!(msg.starts_with("first, amended") && msg.contains("Signed-off-by: Test User"));
    let err = ops::commit(&r, &ops::CommitRequest { message: "nothing".into(), amend: false, signoff: false, paths: None, no_verify: false, partial: vec![] })
        .await
        .unwrap_err();
    assert_eq!(err.status.as_u16(), 400);

    // Partial commit of one file with --only.
    write(p, "a.txt", "2\n");
    write(p, "b.txt", "b\n");
    ops::commit(&r, &ops::CommitRequest { message: "only b".into(), amend: false, signoff: false, paths: Some(vec!["b.txt".into()]), no_verify: false, partial: vec![] })
        .await
        .unwrap();
    let st = status::status(&r, false).await.unwrap();
    assert!(st.files.iter().any(|f| f.path == "a.txt" && f.worktree == 'M'));
    assert!(!st.files.iter().any(|f| f.path == "b.txt"));

    // Stash and restore.
    ops::stash_push(&r, &ops::StashPushRequest { message: Some("wip a".into()), include_untracked: false, keep_index: false, paths: None, staged: false })
        .await
        .unwrap();
    let list = stash::list(&r).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "wip a");
    let det = stash::show(&r, 0).await.unwrap();
    assert_eq!(det.files[0].path, "a.txt");
    let stale = ops::stash_apply(&r, &ops::StashRefRequest { index: 0, sha: Some("deadbeef".into()), reinstate_index: false }, true).await;
    assert_eq!(stale.unwrap_err().status.as_u16(), 409);
    let o = ops::stash_apply(&r, &ops::StashRefRequest { index: 0, sha: Some(list[0].sha.clone()), reinstate_index: false }, true)
        .await
        .unwrap();
    assert!(o.ok);
    assert!(stash::list(&r).await.unwrap().is_empty());

    // Checkout with a dirty tree: a clear error, then a smart checkout.
    git(p, &["checkout", "-q", "-b", "other"]);
    write(p, "a.txt", "other\n");
    commit_all(p, "other");
    git(p, &["checkout", "-q", "main"]);
    write(p, "a.txt", "local edit\n");
    let req = ops::CheckoutRequest { rev: Some("other".into()), create: None, start_point: None, track: None, detach: false, force: false, smart: false };
    let refused = ops::checkout(&r, &req).await.unwrap();
    assert!(!refused.ok && refused.dirty);
    assert!(refused.message.contains("a.txt"));
    assert_eq!(crate::util::git::current_branch(p).await.as_deref(), Some("main"));
    let o = ops::checkout(&r, &ops::CheckoutRequest { smart: true, ..req }).await.unwrap();
    assert!(o.conflicts, "{o:?}");
    assert_eq!(crate::util::git::current_branch(p).await.as_deref(), Some("other"));

    // Discard (rollback) restores and trashes untracked files.
    git(p, &["checkout", "-q", "--force", "other"]);
    git(p, &["stash", "drop", "-q"]);
    write(p, "a.txt", "changed\n");
    write(p, "junk.txt", "junk\n");
    let trash = tempfile::tempdir().unwrap();
    let res = ops::discard(
        &r,
        &ops::DiscardRequest { paths: vec!["a.txt".into(), "junk.txt".into()], scope: None, delete_added: false },
        trash.path(),
    )
    .await
    .unwrap();
    assert_eq!(res.trashed, vec!["junk.txt".to_string()]);
    assert!(!p.join("junk.txt").exists());
    assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "other\n");
}

#[tokio::test]
async fn subdirectory_projects_see_project_relative_paths() {
    let d = init_repo();
    let p = d.path();
    write(p, "app/web/src/x.ts", "x\n");
    write(p, "README.md", "r\n");
    commit_all(p, "init");
    write(p, "app/web/src/x.ts", "y\n");
    write(p, "README.md", "changed\n");
    let r = Repo::discover_root("sub", &p.join("app/web")).await.unwrap();
    assert_eq!(r.prefix, "app/web/");
    let st = status::status(&r, false).await.unwrap();
    assert_eq!(st.files.len(), 1);
    assert_eq!(st.files[0].path, "src/x.ts");
    let (df, _) = file_diff(&r, &DiffQuery { path: "src/x.ts".into(), ..Default::default() }).await.unwrap();
    assert_eq!((df.original.as_str(), df.modified.as_str()), ("x\n", "y\n"));
}

#[tokio::test]
async fn linked_worktrees_have_their_own_state() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    git(p, &["worktree", "add", "-q", "-b", "side", wt_path.to_str().unwrap()]);
    let r = Repo::discover_root("wt", &wt_path).await.unwrap();
    // A linked worktree has a `.git` file; its git dir is under the common dir.
    assert!(wt_path.join(".git").is_file());
    assert!(r.git_dir.starts_with(&r.common_dir) && r.git_dir != r.common_dir);
    // A merge marker in the main worktree does not leak into the linked one.
    std::fs::write(p.join(".git/MERGE_HEAD"), "0000000000000000000000000000000000000000\n").unwrap();
    assert_eq!(status::status(&r, false).await.unwrap().state, "clean");
    assert_eq!(status::status(&r, false).await.unwrap().branch.as_deref(), Some("side"));
    std::fs::remove_file(p.join(".git/MERGE_HEAD")).unwrap();
    let list = refs::worktrees(&r).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|w| w.current && w.branch.as_deref() == Some("side")));
}

/// An `AppState` whose only project is `repo` (isolated config and data dirs).
pub(super) async fn app_with(repo: &Path, tmp: &Path) -> crate::app::AppState {
    let paths = crate::config::Paths { config_dir: tmp.join("config"), data_dir: tmp.join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![repo.display().to_string()];
    crate::app::AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap()
}

#[tokio::test]
async fn mcp_tools_drive_the_ui() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    write(p, "a.txt", "2\n");
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    let pid = state.projects.list()[0].id.clone();
    let mut rx = state.events.subscribe();
    let tools = super::mcp_tools();
    let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap().handler.clone();
    let ctx = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };

    (tool("workbench_set_commit_message"))(state.clone(), ctx.clone(), serde_json::json!({ "message": "Fix the thing" })).await.unwrap();
    let ev = loop {
        let e = rx.recv().await.unwrap();
        if e.kind == "git.commitMessage" {
            break e;
        }
    };
    assert_eq!(ev.project_id.as_deref(), Some(pid.as_str()));
    assert_eq!(ev.data["message"], "Fix the thing");

    (tool("workbench_show_diff"))(state.clone(), ctx.clone(), serde_json::json!({ "path": "a.txt" })).await.unwrap();
    let ev = loop {
        let e = rx.recv().await.unwrap();
        if e.kind == "ui.open" {
            break e;
        }
    };
    assert_eq!(ev.data["panel"], "diff");
    assert_eq!(ev.data["id"], format!("diff:{pid}:working::a.txt"));
    assert_eq!(ev.data["params"]["mode"], "working");

    (tool("workbench_show_diff"))(state.clone(), ctx.clone(), serde_json::json!({ "sha": "HEAD" })).await.unwrap();
    let ev = loop {
        let e = rx.recv().await.unwrap();
        if e.kind == "ui.open" {
            break e;
        }
    };
    assert_eq!(ev.data["panel"], "commit");
    assert!(ev.data["id"].as_str().unwrap().starts_with(&format!("commit:{pid}:")));

    let err = (tool("workbench_show_diff"))(state.clone(), crate::mcp::McpCtx::default(), serde_json::json!({})).await;
    assert!(err.is_err());

    // Outside a session's project, `projectId` (like every other Workbench tool)
    // selects the project; the older `project` still works.
    for key in ["projectId", "project"] {
        (tool("workbench_show_diff"))(state.clone(), crate::mcp::McpCtx::default(), serde_json::json!({ key: pid, "path": "a.txt" }))
            .await
            .unwrap_or_else(|e| panic!("{key}: {}", e.message));
    }
    let schema = tools.iter().find(|t| t.name == "workbench_set_commit_message").unwrap().input_schema.clone();
    assert!(schema["properties"]["projectId"].is_object() && schema["properties"].get("project").is_none());
}

fn st_of<'a>(st: &'a status::GitStatus, path: &str) -> Option<&'a status::StatusFile> {
    st.files.iter().find(|f| f.path == path)
}

#[tokio::test]
async fn stash_include_untracked_and_keep_index_really_do_it() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    write(p, "b.txt", "b\n");
    commit_all(p, "init");
    let r = repo(p).await;
    let push = |include_untracked, keep_index| ops::StashPushRequest { message: None, include_untracked, keep_index, paths: None, staged: false };

    // Include unversioned files: they leave the working tree, and come back on pop.
    write(p, "a.txt", "a2\n");
    write(p, "u.txt", "untracked\n");
    ops::stash_push(&r, &push(true, false)).await.unwrap();
    assert!(!p.join("u.txt").exists(), "the untracked file must be stashed away");
    assert!(status::status(&r, false).await.unwrap().files.is_empty());
    let o = ops::stash_apply(&r, &ops::StashRefRequest { index: 0, sha: None, reinstate_index: false }, true).await.unwrap();
    assert!(o.ok, "{o:?}");
    assert_eq!(std::fs::read_to_string(p.join("u.txt")).unwrap(), "untracked\n");
    assert!(stash::list(&r).await.unwrap().is_empty());
    std::fs::remove_file(p.join("u.txt")).unwrap();
    git(p, &["checkout", "-q", "--", "."]);

    // Keep index: succeeds, and the staged change stays in the working tree.
    write(p, "a.txt", "staged\n");
    git(p, &["add", "a.txt"]);
    write(p, "b.txt", "unstaged\n");
    ops::stash_push(&r, &push(false, true)).await.unwrap();
    assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "staged\n");
    assert_eq!(std::fs::read_to_string(p.join("b.txt")).unwrap(), "b\n");
    assert_eq!(stash::list(&r).await.unwrap().len(), 1);
}

#[tokio::test]
async fn paths_with_glob_characters_are_literal() {
    // A file called `*.txt` (Windows allows no `*` in names: `[a].txt`, a glob matching
    // a.txt too).
    let star = if cfg!(windows) { "[a].txt" } else { "*.txt" };
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    write(p, star, "star\n");
    write(p, "app/[id]/page.tsx", "x\n");
    write(p, "app/i/page.tsx", "i\n");
    commit_all(p, "init");
    let r = repo(p).await;
    write(p, "a.txt", "a2\n");
    write(p, star, "star2\n");
    write(p, "app/[id]/page.tsx", "x2\n");
    write(p, "app/i/page.tsx", "i2\n");
    ops::stage(&r, &ops::PathsRequest { paths: vec![star.into(), "app/[id]/page.tsx".into()], all: false }).await.unwrap();
    let staged = git(p, &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, format!("{star}\napp/[id]/page.tsx\n"));
    let (df, _) = file_diff(&r, &DiffQuery { path: star.into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap();
    assert_eq!(df.hunks.len(), 1);
    // Stash only the file called `*.txt` (not a.txt).
    ops::stash_push(&r, &ops::StashPushRequest { message: None, include_untracked: false, keep_index: false, paths: Some(vec![star.into()]), staged: false })
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "a2\n");
    assert_eq!(std::fs::read_to_string(p.join(star)).unwrap(), "star\n");
    let hist = log::log(&r, &log::LogQuery { path: Some("app/[id]/page.tsx".into()), ..Default::default() }).await.unwrap();
    assert_eq!(hist.commits.len(), 1);
}

#[tokio::test]
async fn unstage_rollback_and_commit_of_a_staged_rename_cover_both_sides() {
    let d = init_repo();
    let p = d.path();
    write(p, "torename.txt", &lines(20));
    write(p, "other.txt", "o\n");
    commit_all(p, "init");
    let r = repo(p).await;
    let staged_changes = |st: &status::GitStatus| st.files.iter().filter(|f| f.index != ' ' && f.index != '?').count();

    // Unstage (the UI sends only the new name).
    git(p, &["mv", "torename.txt", "renamed.txt"]);
    ops::unstage(&r, &ops::PathsRequest { paths: vec!["renamed.txt".into()], all: false }).await.unwrap();
    let st = status::status(&r, false).await.unwrap();
    assert_eq!(staged_changes(&st), 0, "{:?}", st.files);
    assert_eq!(st_of(&st, "renamed.txt").map(|f| f.index), Some('?'));

    // The staged diff finds the rename source by itself.
    git(p, &["add", "-A"]);
    let (df, _) = file_diff(&r, &DiffQuery { path: "renamed.txt".into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap();
    assert_eq!(df.old_path.as_deref(), Some("torename.txt"));
    assert!(!df.original_missing && df.original == lines(20));

    // Rollback: the old name comes back, the new one goes (to the trash).
    let trash = tempfile::tempdir().unwrap();
    let res = ops::discard(&r, &ops::DiscardRequest { paths: vec!["renamed.txt".into()], scope: None, delete_added: false }, trash.path())
        .await
        .unwrap();
    assert_eq!(res.trashed, vec!["renamed.txt".to_string()]);
    assert_eq!(std::fs::read_to_string(p.join("torename.txt")).unwrap(), lines(20));
    assert!(!p.join("renamed.txt").exists());
    assert!(status::status(&r, false).await.unwrap().files.is_empty());

    // Rolling back by the old name does the same.
    git(p, &["mv", "torename.txt", "renamed.txt"]);
    ops::discard(&r, &ops::DiscardRequest { paths: vec!["torename.txt".into()], scope: None, delete_added: false }, trash.path())
        .await
        .unwrap();
    assert!(status::status(&r, false).await.unwrap().files.is_empty());

    // Committing only the new name commits the whole rename.
    git(p, &["mv", "torename.txt", "renamed.txt"]);
    write(p, "other.txt", "changed\n");
    ops::commit(&r, &ops::CommitRequest { message: "rename".into(), amend: false, signoff: false, paths: Some(vec!["renamed.txt".into()]), no_verify: false, partial: vec![] })
        .await
        .unwrap();
    let st = status::status(&r, false).await.unwrap();
    assert_eq!(staged_changes(&st), 0, "{:?}", st.files);
    assert_eq!(st.files.len(), 1);
    assert_eq!(st.files[0].path, "other.txt");
}

#[tokio::test]
async fn rebase_with_autostash_reports_conflicting_local_changes() {
    let d = init_repo();
    let p = d.path();
    write(p, "m.txt", "1\n2\n3\n4\n5\n");
    commit_all(p, "base");
    git(p, &["checkout", "-q", "-b", "up"]);
    write(p, "m.txt", "1\n2\nupstream\n4\n5\n");
    commit_all(p, "upstream change");
    git(p, &["checkout", "-q", "main"]);
    write(p, "m.txt", "1\n2\nlocal\n4\n5\n");
    let r = repo(p).await;
    let o = ops::rebase(&r, &ops::RebaseRequest { onto: "up".into(), autostash: true }).await.unwrap();
    assert!(!o.ok && o.conflicts, "{o:?}");
    assert!(o.message.contains("stash"), "{}", o.message);
    assert_eq!(stash::list(&r).await.unwrap().len(), 1);
    // Clean runs still succeed.
    git(p, &["reset", "-q", "--hard", "main"]);
    git(p, &["stash", "clear"]);
    let o = ops::rebase(&r, &ops::RebaseRequest { onto: "up".into(), autostash: true }).await.unwrap();
    assert!(o.ok && !o.conflicts, "{o:?}");
}

/// Wait for a remote op to finish.
pub(super) async fn op_done(state: &crate::app::AppState, id: &str, within: std::time::Duration) -> super::remote::OpInfo {
    let t0 = std::time::Instant::now();
    loop {
        let op = state.git.ops.get(id).unwrap();
        if op.done {
            return op;
        }
        assert!(t0.elapsed() < within, "op {id} still running after {within:?}: {:?}", op.lines);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn pull_with_autostash_conflict_is_not_reported_as_success() {
    let up = init_repo();
    write(up.path(), "m.txt", "1\n2\n3\n4\n5\n");
    commit_all(up.path(), "base");
    let tmp = tempfile::tempdir().unwrap();
    let down = tmp.path().join("down");
    git(tmp.path(), &["clone", "-q", up.path().to_str().unwrap(), down.to_str().unwrap()]);
    for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("core.autocrlf", "false")] {
        git(&down, &["config", k, v]);
    }
    no_hooks(&down);
    write(up.path(), "m.txt", "1\n2\nremote\n4\n5\n");
    commit_all(up.path(), "remote change");
    write(&down, "m.txt", "1\n2\nlocal\n4\n5\n");
    let state = app_with(&down, tmp.path()).await;
    let pid = state.projects.list()[0].id.clone();
    let project = state.projects.require(&pid).unwrap();
    let r = state.git.repo(&project).await.unwrap();
    let spec = super::remote::RemoteOpSpec::new(
        "pull",
        "Update main (merge)".into(),
        ["pull", "--progress", "--no-rebase", "--autostash"].map(String::from).to_vec(),
        true,
    );
    let id = super::remote::start(&state, r.clone(), spec, None).unwrap();
    let op = op_done(&state, &id, std::time::Duration::from_secs(60)).await;
    assert_eq!(op.ok, Some(false), "{op:?}");
    assert!(op.conflicts, "{op:?}");
    assert!(op.message.as_deref().unwrap_or_default().contains("stash"), "{op:?}");
    assert!(status::status(&r, false).await.unwrap().files.iter().any(|f| f.path == "m.txt" && f.conflict));
}

#[tokio::test]
async fn cancelling_a_stalled_fetch_is_immediate() {
    // A "server" that accepts connections and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = vec![];
        for s in listener.incoming().flatten() {
            held.push(s);
        }
    });
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    git(p, &["remote", "add", "slow", &format!("http://127.0.0.1:{port}/x.git")]);
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    let pid = state.projects.list()[0].id.clone();
    let project = state.projects.require(&pid).unwrap();
    let r = state.git.repo(&project).await.unwrap();
    let spec = super::remote::RemoteOpSpec::new("fetch", "Fetch slow".into(), ["fetch", "--progress", "slow"].map(String::from).to_vec(), false);
    let id = super::remote::start(&state, r, spec, None).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    assert!(!state.git.ops.get(&id).unwrap().done, "the fetch should be stalled");
    assert!(state.git.ops.cancel(&id));
    let op = op_done(&state, &id, std::time::Duration::from_secs(8)).await;
    assert_eq!(op.ok, Some(false));
    assert!(op.message.as_deref().unwrap_or_default().contains("cancelled"), "{op:?}");
}

#[tokio::test]
async fn local_operations_do_not_wait_forever_for_the_repository_lock() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "1\n");
    commit_all(p, "init");
    let tmp = tempfile::tempdir().unwrap();
    let state = app_with(p, tmp.path()).await;
    let pid = state.projects.list()[0].id.clone();
    let project = state.projects.require(&pid).unwrap();
    let r = state.git.repo(&project).await.unwrap();
    let held = state.git.lock(&r.top).lock_owned().await;
    let err = super::routes::mutate_waiting(&state, &pid, &super::routes::RepoSel::default(), std::time::Duration::from_millis(200), |_| async { Ok(()) }).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (409, "busy"));
    drop(held);
    super::routes::mutate_waiting(&state, &pid, &super::routes::RepoSel::default(), std::time::Duration::from_millis(200), |_| async { Ok(()) }).await.unwrap();
}

#[tokio::test]
async fn blame_at_a_revision() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "one\n");
    commit_all(p, "first");
    write(p, "a.txt", "one\ntwo\n");
    commit_all(p, "second");
    let r = repo(p).await;
    let bl = blame::blame(&r, "a.txt", Some("HEAD~1")).await.unwrap();
    assert_eq!(bl.lines.len(), 1);
    assert_eq!(bl.lines[0].summary, "first");
    assert_eq!(blame::blame(&r, "a.txt", Some("HEAD")).await.unwrap().lines.len(), 2);
}

#[tokio::test]
async fn resolve_acts_only_on_conflicted_files() {
    let d = init_repo();
    let p = d.path();
    write(p, "a.txt", "a\n");
    commit_all(p, "init");
    write(p, "nested/.git", "gitdir: /somewhere\n");
    let r = repo(p).await;
    let req = |path: &str, content: Option<&str>, side: Option<&str>| conflicts::ResolveRequest {
        path: path.into(),
        content: content.map(str::to_string),
        side: side.map(str::to_string),
    };
    // Not in conflict: nothing is written, nothing deleted.
    let e = conflicts::resolve(&r, &req("a.txt", Some("overwritten\n"), None)).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 409);
    assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "a\n");
    assert!(conflicts::resolve(&r, &req("a.txt", None, Some("ours"))).await.is_err());
    assert!(conflicts::resolve(&r, &req("a.txt", None, Some("theirs"))).await.is_err());
    assert!(p.join("a.txt").exists());
    // A nested `.git` is never a target.
    let e = conflicts::resolve(&r, &req("nested/.git", Some("gitdir: /evil\n"), None)).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 403);
    assert_eq!(std::fs::read_to_string(p.join("nested/.git")).unwrap(), "gitdir: /somewhere\n");
}

#[tokio::test]
async fn submodule_changes_are_reported_not_faked() {
    let lib = init_repo();
    write(lib.path(), "l.txt", "l\n");
    commit_all(lib.path(), "lib");
    let d = init_repo();
    let p = d.path();
    write(p, "t.txt", "t\n");
    commit_all(p, "top");
    git(p, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", lib.path().to_str().unwrap(), "sub"]);
    commit_all(p, "add sub");
    let r = repo(p).await;
    let trash = tempfile::tempdir().unwrap();

    // Modified content inside the submodule.
    write(p, "sub/l.txt", "changed\n");
    let st = status::status(&r, false).await.unwrap();
    assert!(st_of(&st, "sub").is_some_and(|f| f.submodule && f.worktree == 'M'));
    let e = ops::stage(&r, &ops::PathsRequest { paths: vec!["sub".into()], all: false }).await.unwrap_err();
    assert!(e.message.contains("submodule"), "{}", e.message);
    let e = ops::discard(&r, &ops::DiscardRequest { paths: vec!["sub".into()], scope: Some("worktree".into()), delete_added: false }, trash.path())
        .await
        .unwrap_err();
    assert!(e.message.contains("submodule"), "{}", e.message);
    assert_eq!(std::fs::read_to_string(p.join("sub/l.txt")).unwrap(), "changed\n");
    // Mixed with a normal file: that one is staged, the submodule reported.
    write(p, "t.txt", "t2\n");
    let skipped = ops::stage(&r, &ops::PathsRequest { paths: vec!["sub".into(), "t.txt".into()], all: false }).await.unwrap();
    assert_eq!(skipped, vec!["sub".to_string()]);
    assert_eq!(git(p, &["diff", "--cached", "--name-only"]), "t.txt\n");

    let (df, _) = file_diff(&r, &DiffQuery { path: "sub".into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap();
    assert!(df.submodule && !df.binary && !df.original_missing && !df.can_stage_hunks && !df.submodule_new_commit, "{df:?}");
    assert!(df.submodule_summary.as_deref().unwrap_or_default().contains("modified content"), "{df:?}");

    // A new commit inside the submodule: that can be staged, and the diff says so.
    git(&p.join("sub"), &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "-am", "lib change"]);
    let (df, _) = file_diff(&r, &DiffQuery { path: "sub".into(), mode: Some(DiffMode::Working), ..Default::default() }).await.unwrap();
    assert!(df.submodule && df.submodule_new_commit && df.submodule_summary.as_deref().unwrap_or_default().contains("lib change"), "{df:?}");
    assert!(ops::stage(&r, &ops::PathsRequest { paths: vec!["sub".into()], all: false }).await.unwrap().is_empty());
    let (sd, _) = file_diff(&r, &DiffQuery { path: "sub".into(), mode: Some(DiffMode::Staged), ..Default::default() }).await.unwrap();
    assert!(sd.submodule && sd.submodule_summary.as_deref().unwrap_or_default().contains("lib change"), "{sd:?}");
}

/// On Windows (`os::fs::FOREIGN_OWNERS`); elsewhere git's refusal reads as before.
#[tokio::test]
async fn a_repository_git_refuses_for_its_owner_is_reported_in_gits_words() {
    let d = init_repo();
    let p = d.path();
    // Git's own switch for testing its ownership check, and an empty global config, so no
    // `safe.directory` of this computer lets the folder pass.
    let cfg = tempfile::tempdir().unwrap();
    let empty = cfg.path().join("gitconfig");
    std::fs::write(&empty, "").unwrap();
    let out = super::cmd::Git::read(p)
        .args(super::repo::DISCOVER_ARGS)
        .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
        .env("GIT_CONFIG_GLOBAL", empty.to_string_lossy().to_string())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .run()
        .await
        .unwrap();
    assert!(!out.ok());
    let e = super::repo::discover_error(&out, p);
    if crate::util::os::fs::FOREIGN_OWNERS {
        assert_eq!((e.status.as_u16(), e.code), (403, "unsafe_repository"), "{}", e.message);
        assert_eq!(e.message, out.stderr.trim(), "verbatim");
        assert!(e.message.contains("--add safe.directory"), "{}", e.message);
        assert_eq!(super::cmd::git_error(&out).code, "unsafe_repository");
    } else {
        // Linux: as it always was.
        assert_eq!((e.status.as_u16(), e.code), (404, "not_a_repo"), "{}", e.message);
        assert_ne!(super::cmd::git_error(&out).code, "unsafe_repository");
    }
    // A folder that is no repository still says so.
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(Repo::discover_root("plain", plain.path()).await.unwrap_err().code, "not_a_repo");
}
