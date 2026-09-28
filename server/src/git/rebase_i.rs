//! Interactive rebase (CLion's "Interactively Rebase from Here…" and rebasing onto a
//! branch with a prepared todo).
//!
//! The client gets a plan (`GET …/rebase/plan`: the commits oldest first, which are
//! already on a remote branch, which git will skip as already on the target, whether
//! the tree is dirty), edits it (reorder, pick / reword / edit / squash / fixup /
//! drop, messages) and posts it back. The server
//! checks it against a fresh plan (same HEAD, same commits), writes the todo and the
//! messages to a staging folder and runs `git rebase -i` as a git op (progress as
//! `git.op` events). Git never opens an interactive editor:
//!
//! * `GIT_SEQUENCE_EDITOR` is `workbench git-editor todo <staging>`: it checks that
//!   git's generated todo lists exactly the planned commits (else the rebase is
//!   refused before anything happens), copies the messages into
//!   `<git dir>/rebase-merge/workbench/` (so they live exactly as long as the rebase)
//!   and replaces the todo with ours.
//! * `GIT_EDITOR` is `workbench git-editor message <git dir>`: git opens it for a
//!   `reword` and at the end of a chain containing a `squash`; the helper finds the
//!   commit in `rebase-merge/done` and writes the prepared message (or leaves git's).
//!   `continue` uses it too while such a rebase runs, so a reword that stopped on a
//!   conflict still gets its message.
//!
//! Messages keep lines starting with `#`: the rebase runs with a `core.commentChar`
//! that starts no line of any prepared message. Stops (`edit`, conflicts) use the
//! regular continue / skip / abort flow and the conflict panel.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use super::cmd::check_rev;
use super::diff::resolve_commit;
use super::remote::{self, RemoteOpSpec};
use super::repo::Repo;
use super::status::detect_state;
use crate::app::AppState;
use crate::error::ApiError;

/// Plans longer than this are refused (use the command line for such rewrites).
pub const MAX_REBASE_COMMITS: usize = 1000;
const COMMENT_CHARS: [char; 10] = ['#', ';', '@', '!', '$', '%', '^', '&', '|', ':'];
/// Folder inside `rebase-merge/` with the prepared messages.
pub const MSG_DIR: &str = "workbench";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Pick,
    Reword,
    Edit,
    Squash,
    Fixup,
    Drop,
}

impl Action {
    fn word(self) -> &'static str {
        match self {
            Action::Pick => "pick",
            Action::Reword => "reword",
            Action::Edit => "edit",
            Action::Squash => "squash",
            Action::Fixup => "fixup",
            Action::Drop => "drop",
        }
    }
}

// ---------------------------------------------------------------- plan

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanQuery {
    /// Rebase from this commit (inclusive) up to HEAD.
    pub from: Option<String>,
    /// Or: replay the commits of HEAD that are not on this branch onto it.
    pub onto: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanCommit {
    pub sha: String,
    pub subject: String,
    /// The full message.
    pub message: String,
    pub author: String,
    pub email: String,
    /// Author time, Unix ms.
    pub time: i64,
    /// Already on a remote branch (the upstream or any other remote-tracking branch).
    pub pushed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebasePlan {
    pub head: String,
    pub branch: Option<String>,
    /// The `<upstream>` argument: the parent of `from`, or the branch's tip
    /// (`onto` mode). None with `root` (the range starts at a root commit).
    pub base: Option<String>,
    pub root: bool,
    /// The branch given as `onto`, as named.
    pub onto: Option<String>,
    /// Oldest first.
    pub commits: Vec<PlanCommit>,
    /// `onto` mode: commits of the branch whose change is already on the target
    /// (cherry-picked, patch-equivalent). Git leaves them out of the rebase, so they
    /// are not in `commits` and disappear from the branch. Oldest first.
    pub skipped: Vec<PlanCommit>,
    /// Tracked files with uncommitted changes (the rebase needs a clean tree or autostash).
    pub dirty: usize,
    /// The range has merge commits: an interactive rebase would flatten them, so it is refused.
    pub merges: bool,
    /// Where the pushed commits are: the remote branch holding the newest of them
    /// (the upstream when it does); without pushed commits the upstream, or "remote
    /// branches". None without any remote-tracking branch.
    pub pushed_ref: Option<String>,
    /// Rebasing onto a branch replays every commit (they all get new ids), even unchanged ones.
    pub rewrites_all: bool,
    /// clean | merging | rebasing | … (anything but clean refuses the run).
    pub state: &'static str,
}

const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

/// Records of `PLAN_FORMAT`: (`%m` mark, commit). The mark is `=` for a commit
/// `--cherry-mark` found patch-equivalent to one on the other side.
fn parse_plan_log(text: &str) -> Vec<(char, PlanCommit)> {
    text.split(RS)
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.splitn(7, FS).collect();
            if f.len() < 7 || f[1].len() < 7 {
                return None;
            }
            Some((
                f[0].chars().next().unwrap_or(' '),
                PlanCommit {
                    sha: f[1].to_string(),
                    author: f[2].to_string(),
                    email: f[3].to_string(),
                    time: f[4].trim().parse::<i64>().unwrap_or(0) * 1000,
                    subject: f[5].to_string(),
                    message: f[6].trim_end().to_string(),
                    pushed: false,
                },
            ))
        })
        .collect()
}

const PLAN_FORMAT: &str = "--format=%m%x1f%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%B%x1e";

/// A commit that changes nothing (git keeps those even when "already applied").
async fn is_empty_commit(repo: &Repo, sha: &str) -> Result<bool, ApiError> {
    let out = repo.git().args(["rev-parse", &format!("{sha}^{{tree}}"), &format!("{sha}^1^{{tree}}")]).run().await?;
    let t = out.text();
    let trees: Vec<&str> = t.split_whitespace().collect();
    Ok(out.ok() && trees.len() == 2 && trees[0] == trees[1])
}

/// The remote-tracking branch to name for a pushed commit: the upstream when it
/// holds it, else the first other one (symbolic refs like `origin/HEAD` left out).
async fn remote_holding(repo: &Repo, sha: &str, upstream: Option<&str>) -> Result<Option<String>, ApiError> {
    let out = repo
        .git()
        .args(["for-each-ref", "--format=%(refname:short)%09%(symref)", "--contains", sha, "refs/remotes"])
        .run_ok()
        .await?;
    let names: Vec<String> = out
        .text()
        .lines()
        .filter_map(|l| {
            let (name, symref) = l.split_once('\t').unwrap_or((l, ""));
            (symref.is_empty() && !name.is_empty()).then(|| name.to_string())
        })
        .collect();
    Ok(match upstream {
        Some(u) if names.iter().any(|n| n == u) => Some(u.to_string()),
        _ => names.into_iter().next(),
    })
}

async fn rev_count(repo: &Repo, args: &[&str]) -> Result<usize, ApiError> {
    let out = repo.git().args(["rev-list", "--count"]).args(args.iter().copied()).run_ok().await?;
    Ok(out.text().trim().parse().unwrap_or(0))
}

pub async fn plan(repo: &Repo, q: &PlanQuery) -> Result<RebasePlan, ApiError> {
    let head_out = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?;
    let head = head_out.text().trim().to_string();
    if !head_out.ok() || head.is_empty() {
        return Err(ApiError::bad_request("there are no commits yet"));
    }
    let branch = crate::util::git::current_branch(&repo.top).await;
    let (base, root, onto, rewrites_all) = match (q.from.as_deref().filter(|s| !s.is_empty()), q.onto.as_deref().filter(|s| !s.is_empty())) {
        (Some(from), None) => {
            let from = resolve_commit(repo, from).await?;
            let anc = repo.git().args(["merge-base", "--is-ancestor", &from, &head]).run().await?;
            if !anc.ok() {
                return Err(ApiError::bad_request(format!("{} is not on the current branch (not an ancestor of HEAD)", &from[..10.min(from.len())])));
            }
            let p = repo.git().args(["rev-parse", "--verify", "--quiet", &format!("{from}^1")]).run().await?;
            let parent = p.text().trim().to_string();
            if p.ok() && !parent.is_empty() { (Some(parent), false, None, false) } else { (None, true, None, false) }
        }
        (None, Some(onto)) => {
            let name = check_rev(onto)?.to_string();
            let sha = resolve_commit(repo, &name).await?;
            let mb = repo.git().args(["merge-base", &sha, &head]).run().await?;
            let same_base = mb.ok() && mb.text().trim() == sha;
            (Some(sha), false, Some(name), !same_base)
        }
        _ => return Err(ApiError::bad_request("give either from (a commit) or onto (a branch)")),
    };
    let range: Vec<String> = match &base {
        Some(b) => vec![format!("{b}..{head}")],
        None => vec![head.clone()],
    };
    let range_args: Vec<&str> = range.iter().map(String::as_str).collect();
    let n = rev_count(repo, &range_args).await?;
    if n > MAX_REBASE_COMMITS {
        return Err(ApiError::bad_request(format!(
            "{n} commits would be rebased; Workbench plans at most {MAX_REBASE_COMMITS} (use the command line for this one)"
        )));
    }
    let merges = rev_count(repo, &[&["--min-parents=2"], range_args.as_slice()].concat()).await? > 0;
    // Onto a branch, git leaves out the commits whose change is already there
    // (cherry-picked: `--cherry-mark` finds them like git's own todo generator), except
    // commits that change nothing. The plan lists exactly what git's todo will.
    let mut g = repo.git().args(["log", "--reverse", "--topo-order", PLAN_FORMAT]);
    g = match (&onto, &base) {
        (Some(_), Some(b)) => g.args(["--right-only".to_string(), "--cherry-mark".into(), format!("{b}...{head}")]),
        _ => g.args(range_args.iter().copied()),
    };
    let out = g.arg("--").run_ok().await?;
    let mut commits = vec![];
    let mut skipped = vec![];
    for (mark, c) in parse_plan_log(&out.text()) {
        if mark == '=' && !is_empty_commit(repo, &c.sha).await? {
            skipped.push(c);
        } else {
            commits.push(c);
        }
    }

    // Which commits are already published: on any remote-tracking branch (the upstream
    // or another one, e.g. a branch that tracks origin/main but was pushed as its own).
    let up = repo.git().args(["rev-parse", "--symbolic-full-name", "@{upstream}"]).run().await?;
    let upstream = up
        .ok()
        .then(|| up.text().trim().to_string())
        .and_then(|s| s.strip_prefix("refs/remotes/").map(str::to_string))
        .filter(|s| !s.is_empty());
    let any_remote = !repo.git().args(["for-each-ref", "--count=1", "refs/remotes"]).run_ok().await?.stdout.is_empty();
    let mut pushed_ref = None;
    if any_remote {
        let mut g = repo.git().args(["rev-list", &head, "--not", "--remotes"]);
        if let Some(b) = &base {
            g = g.arg(b.clone());
        }
        let unpushed = g.run().await?;
        if unpushed.ok() {
            let set: HashSet<&str> = unpushed.stdout.split(|b| *b == b'\n').filter_map(|l| std::str::from_utf8(l).ok()).collect();
            for c in commits.iter_mut().chain(skipped.iter_mut()) {
                c.pushed = !set.contains(c.sha.as_str());
            }
        }
        // Name where they are: the branch holding the newest pushed commit (it holds the older ones too).
        let newest = commits.iter().rev().chain(skipped.iter().rev()).find(|c| c.pushed).map(|c| c.sha.clone());
        if let Some(sha) = newest {
            pushed_ref = remote_holding(repo, &sha, upstream.as_deref()).await?;
        }
        if pushed_ref.is_none() {
            pushed_ref = Some(upstream.clone().unwrap_or_else(|| "remote branches".to_string()));
        }
    }
    let dirty = repo
        .git()
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=no", "--ignore-submodules=dirty"])
        .run_ok()
        .await?
        .stdout
        .split(|b| *b == 0)
        .filter(|r| r.first().is_some_and(|c| matches!(c, b'1' | b'2' | b'u')))
        .count();
    Ok(RebasePlan {
        head,
        branch,
        base,
        root,
        onto,
        commits,
        skipped,
        dirty,
        merges,
        pushed_ref,
        rewrites_all,
        state: detect_state(&repo.git_dir).0,
    })
}

// ---------------------------------------------------------------- the todo

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub sha: String,
    pub action: Action,
    /// reword: the new message; squash: the message of the combined commit.
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub todo: String,
    /// sha → message the editor helper writes when git asks for that commit.
    pub messages: Vec<(String, String)>,
    pub comment_char: char,
    /// Planned commits that get new ids (or disappear) and are already pushed.
    pub rewrites_pushed: usize,
    pub kept: usize,
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// The first planned position the rebase rewrites: the first entry that is not an
/// in-place `pick`, or earlier, the kept commit a squash/fixup melds into (that
/// commit is amended although its own entry is an unchanged `pick`). Positions
/// before `first_changed` are the same in the entries and the plan.
fn rewrite_start(entries: &[Entry], first_changed: usize) -> usize {
    let mut from = first_changed;
    let mut last_kept: Option<usize> = None;
    for (i, e) in entries.iter().enumerate() {
        match e.action {
            Action::Drop => {}
            Action::Squash | Action::Fixup => {
                if let Some(h) = last_kept {
                    from = from.min(h);
                }
            }
            _ => last_kept = Some(i),
        }
    }
    from
}

/// Validate an edited plan and write the todo. `plan` is oldest first, as planned.
pub fn prepare(plan: &[PlanCommit], entries: &[Entry], rewrites_all: bool) -> Result<Prepared, ApiError> {
    if entries.len() != plan.len() {
        return Err(ApiError::conflict("the plan changed (a different number of commits); reopen the rebase dialog"));
    }
    let planned: HashMap<&str, &PlanCommit> = plan.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut seen = HashSet::new();
    for e in entries {
        if !planned.contains_key(e.sha.as_str()) {
            return Err(ApiError::conflict(format!("{} is not in the planned range; reopen the rebase dialog", &e.sha[..10.min(e.sha.len())])));
        }
        if !seen.insert(e.sha.as_str()) {
            return Err(ApiError::bad_request(format!("{} is listed twice", &e.sha[..10.min(e.sha.len())])));
        }
    }
    if let Some(first) = entries.iter().find(|e| e.action != Action::Drop) {
        if matches!(first.action, Action::Squash | Action::Fixup) {
            return Err(ApiError::bad_request(
                "the first kept commit cannot be squashed or fixed up: there is no earlier commit in the rebase to join it to",
            ));
        }
    }
    let mut messages: Vec<(String, String)> = vec![];
    // Walk the chains: a pick/reword/edit followed by squash/fixup entries.
    let kept: Vec<&Entry> = entries.iter().filter(|e| e.action != Action::Drop).collect();
    let mut i = 0;
    while i < kept.len() {
        let head = kept[i];
        if head.action == Action::Reword {
            let m = head.message.as_deref().map(str::trim_end).unwrap_or("");
            if m.trim().is_empty() {
                return Err(ApiError::bad_request(format!("the new message of {} is empty", &head.sha[..10.min(head.sha.len())])));
            }
            messages.push((head.sha.clone(), m.to_string()));
        }
        let mut j = i + 1;
        let mut squash_msg: Option<&str> = None;
        let mut has_squash = false;
        while j < kept.len() && matches!(kept[j].action, Action::Squash | Action::Fixup) {
            if kept[j].action == Action::Squash {
                has_squash = true;
                if let Some(m) = kept[j].message.as_deref().map(str::trim_end).filter(|m| !m.trim().is_empty()) {
                    squash_msg = Some(m);
                }
            }
            j += 1;
        }
        if has_squash {
            if let Some(m) = squash_msg {
                messages.push((kept[j - 1].sha.clone(), m.to_string()));
            }
        }
        i = j;
    }
    let first_changed = entries
        .iter()
        .zip(plan)
        .position(|(e, p)| e.sha != p.sha || e.action != Action::Pick)
        .unwrap_or(entries.len());
    if first_changed == entries.len() && !rewrites_all {
        return Err(ApiError::bad_request("nothing to change: every commit is picked in its place"));
    }
    let from = if rewrites_all { 0 } else { rewrite_start(entries, first_changed) };
    let rewrites_pushed = plan[from..].iter().filter(|c| c.pushed).count();
    let comment_char = COMMENT_CHARS
        .iter()
        .copied()
        .find(|ch| !messages.iter().any(|(_, m)| m.lines().any(|l| l.starts_with(*ch))))
        .ok_or_else(|| ApiError::bad_request("the messages start lines with every comment character git could use"))?;
    let mut todo = String::new();
    for e in entries {
        let subject = planned.get(e.sha.as_str()).map(|c| one_line(&c.subject)).unwrap_or_default();
        todo.push_str(&format!("{} {} {}\n", e.action.word(), e.sha, subject));
    }
    if kept.is_empty() {
        // Every commit dropped: git refuses an empty todo, `noop` resets to the base.
        todo.push_str("noop\n");
    }
    Ok(Prepared { todo, messages, comment_char, rewrites_pushed, kept: kept.len() })
}

/// Pick a comment character no line of `messages` starts with.
#[cfg(test)]
fn comment_char_for(messages: &[&str]) -> Option<char> {
    COMMENT_CHARS.iter().copied().find(|ch| !messages.iter().any(|m| m.lines().any(|l| l.starts_with(*ch))))
}

/// Write the staging folder for the editor helper: `todo`, `expected` (the planned
/// shas, one per line), `gitdir`, `comment-char` and `messages/<sha>`.
pub fn write_staging(dir: &Path, git_dir: &Path, plan: &[PlanCommit], p: &Prepared) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir.join("messages"))?;
    crate::util::fs::set_mode(dir, 0o700);
    let w = |name: &str, data: &[u8]| crate::util::fs::write_atomic(&dir.join(name), data, 0o600);
    w("todo", p.todo.as_bytes())?;
    let expected: String = plan.iter().map(|c| format!("{}\n", c.sha)).collect();
    w("expected", expected.as_bytes())?;
    w("gitdir", git_dir.to_string_lossy().as_bytes())?;
    w("comment-char", p.comment_char.to_string().as_bytes())?;
    for (sha, msg) in &p.messages {
        crate::util::fs::write_atomic(&dir.join("messages").join(sha), format!("{msg}\n").as_bytes(), 0o600)?;
    }
    Ok(())
}

/// Shell-quote one word for the `GIT_*EDITOR` command lines (git runs them with sh).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `GIT_SEQUENCE_EDITOR` / `GIT_EDITOR` for a run, given the helper program prefix.
pub fn editor_env(program: &str, staging: &Path, git_dir: &Path) -> Vec<(String, String)> {
    vec![
        ("GIT_SEQUENCE_EDITOR".into(), format!("{program} todo {}", sh_quote(&staging.to_string_lossy()))),
        ("GIT_EDITOR".into(), format!("{program} message {}", sh_quote(&git_dir.to_string_lossy()))),
    ]
}

/// Extra `git -c` arguments and environment for `rebase --continue` while a rebase
/// started by Workbench (with prepared messages) is stopped.
pub fn continue_env(git_dir: &Path, program: Option<&str>) -> (Vec<String>, Vec<(String, String)>) {
    let dir = git_dir.join("rebase-merge").join(MSG_DIR);
    let (Some(program), true) = (program, dir.is_dir()) else { return (vec![], vec![]) };
    let ch = std::fs::read_to_string(dir.join("comment-char")).ok().and_then(|s| s.trim().chars().next()).unwrap_or('#');
    (
        vec!["-c".into(), format!("core.commentChar={ch}")],
        vec![("GIT_EDITOR".into(), format!("{program} message {}", sh_quote(&git_dir.to_string_lossy())))],
    )
}

// ---------------------------------------------------------------- running it

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub op_id: Option<String>,
    pub from: Option<String>,
    pub onto: Option<String>,
    /// HEAD when the plan was made.
    pub head: String,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub autostash: bool,
    /// The user confirmed rewriting commits that are already pushed.
    #[serde(default)]
    pub confirm_pushed: bool,
}

/// Check the edited plan against a fresh one and start `git rebase -i` as a git op.
/// Returns the op id (progress and the end come as `git.op` events).
pub async fn start(state: &AppState, repo: Arc<Repo>, b: &RunRequest) -> Result<String, ApiError> {
    let program = state
        .git
        .editor
        .get()
        .cloned()
        .ok_or_else(|| ApiError::internal("the git editor helper is not available (see the server log)"))?;
    let head = repo.git().args(["rev-parse", "--verify", "--quiet", "HEAD"]).run().await?.text().trim().to_string();
    if head != b.head {
        return Err(ApiError::conflict("HEAD moved since the rebase was planned; reopen the dialog"));
    }
    let plan = plan(&repo, &PlanQuery { from: b.from.clone(), onto: b.onto.clone() }).await?;
    if plan.state != "clean" {
        return Err(ApiError::conflict(format!("finish the {} first (continue or abort it)", plan.state)));
    }
    if plan.head != b.head {
        return Err(ApiError::conflict("HEAD moved since the rebase was planned; reopen the dialog"));
    }
    if plan.merges {
        return Err(ApiError::bad_request(
            "the range contains merge commits: an interactive rebase here would flatten them. Rebase from a commit after the last merge.",
        ));
    }
    if plan.dirty > 0 && !b.autostash {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "dirty_tree",
            format!(
                "{} file{} uncommitted changes: commit, stash or shelve them first, or choose Autostash",
                plan.dirty,
                if plan.dirty == 1 { " has" } else { "s have" }
            ),
        ));
    }
    let prepared = prepare(&plan.commits, &b.entries, plan.rewrites_all)?;
    // Skipped (already applied) commits leave the branch: pushed ones are rewritten history too.
    let rewrites_pushed = prepared.rewrites_pushed + plan.skipped.iter().filter(|c| c.pushed).count();
    if rewrites_pushed > 0 && !b.confirm_pushed {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "pushed",
            format!(
                "{} of these commits {} already on {}: rewriting them needs a force push. Confirm to go on.",
                rewrites_pushed,
                if rewrites_pushed == 1 { "is" } else { "are" },
                plan.pushed_ref.as_deref().unwrap_or("a remote")
            ),
        ));
    }
    let staging = state
        .paths
        .data("git")
        .join("rebase")
        .join(format!("{}-{}", repo.project_id, crate::util::random_token(6).replace(['-', '_'], "x")));
    let (st2, gd, commits, prep) = (staging.clone(), repo.git_dir.clone(), plan.commits.clone(), prepared.clone());
    tokio::task::spawn_blocking(move || write_staging(&st2, &gd, &commits, &prep))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::internal(format!("cannot prepare the rebase: {e:#}")))?;
    let mut args: Vec<String> = vec![
        "-c".into(),
        format!("core.commentChar={}", prepared.comment_char),
        "rebase".into(),
        "-i".into(),
        "--no-update-refs".into(),
        "--no-autosquash".into(),
        if b.autostash { "--autostash".into() } else { "--no-autostash".into() },
    ];
    match (&plan.base, plan.root) {
        (_, true) => args.push("--root".into()),
        (Some(base), false) => args.push(base.clone()),
        (None, false) => return Err(ApiError::internal("the plan has no base")),
    }
    let what = plan.branch.clone().unwrap_or_else(|| "HEAD".into());
    let title = match &plan.onto {
        Some(o) => format!("Rebase {what} onto {o} (interactive)"),
        None => format!("Rebase {what} (interactive)"),
    };
    let mut spec = RemoteOpSpec::new("rebase", title, args, true);
    spec.env = editor_env(&program, &staging, &repo.git_dir);
    spec.cleanup = Some(staging.clone());
    match remote::start(state, repo, spec, b.op_id.as_deref()) {
        Ok(id) => Ok(id),
        Err(e) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            Err(e)
        }
    }
}

// ---------------------------------------------------------------- the editor helper

/// The shas of `pick`-like lines of a git todo (comments, blank lines, `exec`,
/// `noop`, `break`… are skipped).
fn todo_shas(todo: &str) -> Vec<String> {
    todo.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let cmd = w.next()?;
            let takes_commit = matches!(cmd, "p" | "pick" | "r" | "reword" | "e" | "edit" | "s" | "squash" | "f" | "fixup" | "d" | "drop");
            if !takes_commit {
                return None;
            }
            let mut sha = w.next()?;
            if sha == "-C" || sha == "-c" {
                sha = w.next()?;
            }
            Some(sha.to_string())
        })
        .collect()
}

/// Git's todo must list exactly the planned commits (by abbreviated or full sha).
fn same_commits(git_todo: &str, expected: &[String]) -> bool {
    let got = todo_shas(git_todo);
    if got.len() != expected.len() {
        return false;
    }
    let mut left: Vec<&String> = expected.iter().collect();
    for g in &got {
        match left.iter().position(|e| e.starts_with(g.as_str()) || g.starts_with(e.as_str())) {
            Some(i) => {
                left.swap_remove(i);
            }
            None => return false,
        }
    }
    left.is_empty()
}

/// The commit of the last step git did (`rebase-merge/done`).
fn last_done_sha(done: &str) -> Option<String> {
    let l = done.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with('#'))?;
    todo_shas(l).into_iter().next()
}

/// `workbench git-editor <todo|message> <dir> <file>`: never interactive.
pub fn cli_git_editor(mode: &str, dir: &str, file: &str) -> anyhow::Result<()> {
    let dir = PathBuf::from(dir);
    let file = PathBuf::from(file);
    match mode {
        "todo" => {
            let git_todo = std::fs::read_to_string(&file)?;
            let expected: Vec<String> =
                std::fs::read_to_string(dir.join("expected"))?.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
            if !same_commits(&git_todo, &expected) {
                anyhow::bail!("workbench: the commits to rebase changed since the plan was made; nothing was done");
            }
            let git_dir = PathBuf::from(std::fs::read_to_string(dir.join("gitdir"))?.trim());
            let target = git_dir.join("rebase-merge").join(MSG_DIR);
            std::fs::create_dir_all(target.join("messages"))?;
            if let Ok(rd) = std::fs::read_dir(dir.join("messages")) {
                for e in rd.flatten() {
                    std::fs::copy(e.path(), target.join("messages").join(e.file_name()))?;
                }
            }
            std::fs::copy(dir.join("comment-char"), target.join("comment-char"))?;
            std::fs::write(&file, std::fs::read(dir.join("todo"))?)?;
            Ok(())
        }
        "message" => {
            let rm = dir.join("rebase-merge");
            let Some(sha) = std::fs::read_to_string(rm.join("done")).ok().and_then(|d| last_done_sha(&d)) else { return Ok(()) };
            let Ok(rd) = std::fs::read_dir(rm.join(MSG_DIR).join("messages")) else { return Ok(()) };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.len() >= 7 && (name.starts_with(&sha) || sha.starts_with(&name)) {
                    std::fs::write(&file, std::fs::read(e.path())?)?;
                    break;
                }
            }
            // No prepared message: git's own text stays (its comment lines are stripped).
            Ok(())
        }
        _ => anyhow::bail!("workbench git-editor: unknown mode {mode:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(sha: &str, pushed: bool) -> PlanCommit {
        PlanCommit {
            sha: sha.repeat(40 / sha.len()),
            subject: format!("subject {sha}"),
            message: format!("subject {sha}\n\nbody"),
            author: "A".into(),
            email: "a@x".into(),
            time: 0,
            pushed,
        }
    }
    fn e(c: &PlanCommit, action: Action, message: Option<&str>) -> Entry {
        Entry { sha: c.sha.clone(), action, message: message.map(str::to_string) }
    }

    #[test]
    fn parses_log_records_with_multiline_messages() {
        let raw = ">\u{1f}aaaaaaaaaa\u{1f}Ann\u{1f}a@x\u{1f}1700000000\u{1f}first\u{1f}first\n\nbody # x\n\u{1e}\n=\u{1f}bbbbbbbbbb\u{1f}Bob\u{1f}b@x\u{1f}1\u{1f}second\u{1f}second\n\u{1e}\n";
        let v = parse_plan_log(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].0, '>');
        assert_eq!(v[0].1.message, "first\n\nbody # x");
        assert_eq!((v[1].0, v[1].1.subject.as_str(), v[1].1.time), ('=', "second", 1000));
    }

    #[test]
    fn melding_into_a_pushed_commit_rewrites_it() {
        // The fixup's chain head is an unchanged pick before the first changed entry: it is amended.
        let plan = vec![pc("a", true), pc("b", true), pc("c", false), pc("d", false)];
        let entries =
            vec![e(&plan[0], Action::Pick, None), e(&plan[1], Action::Pick, None), e(&plan[2], Action::Fixup, None), e(&plan[3], Action::Pick, None)];
        assert_eq!(prepare(&plan, &entries, false).unwrap().rewrites_pushed, 1);
        // Two entries, [pushed pick, unpushed fixup].
        let two = vec![pc("a", true), pc("b", false)];
        assert_eq!(prepare(&two, &[e(&two[0], Action::Pick, None), e(&two[1], Action::Fixup, None)], false).unwrap().rewrites_pushed, 1);
        // Across a dropped commit and a squash: the head is still the pushed a.
        let entries = vec![
            e(&plan[0], Action::Pick, None),
            e(&plan[1], Action::Drop, None),
            e(&plan[2], Action::Squash, Some("a and c")),
            e(&plan[3], Action::Pick, None),
        ];
        assert_eq!(prepare(&plan, &entries, false).unwrap().rewrites_pushed, 2);
        // Melding d into the unpushed c leaves the pushed commits alone.
        let entries =
            vec![e(&plan[0], Action::Pick, None), e(&plan[1], Action::Pick, None), e(&plan[2], Action::Pick, None), e(&plan[3], Action::Fixup, None)];
        assert_eq!(prepare(&plan, &entries, false).unwrap().rewrites_pushed, 0);
    }

    #[test]
    fn writes_todo_messages_and_counts_pushed_rewrites() {
        let plan = vec![pc("a", true), pc("b", true), pc("c", false), pc("d", false)];
        // Reword c; squash d into it with a combined message. a and b stay: nothing pushed is rewritten.
        let entries = vec![
            e(&plan[0], Action::Pick, None),
            e(&plan[1], Action::Pick, None),
            e(&plan[2], Action::Reword, Some("new c\n\n# keep this line")),
            e(&plan[3], Action::Squash, Some("c and d")),
        ];
        let p = prepare(&plan, &entries, false).unwrap();
        assert_eq!(p.rewrites_pushed, 0);
        assert_eq!(p.messages, vec![(plan[2].sha.clone(), "new c\n\n# keep this line".into()), (plan[3].sha.clone(), "c and d".into())]);
        assert_eq!(p.comment_char, ';');
        let lines: Vec<&str> = p.todo.lines().collect();
        assert_eq!(lines[0], format!("pick {} subject a", plan[0].sha));
        assert_eq!(lines[3], format!("squash {} subject d", plan[3].sha));
        // Moving b below c rewrites b (pushed).
        let entries = vec![e(&plan[0], Action::Pick, None), e(&plan[2], Action::Pick, None), e(&plan[1], Action::Pick, None), e(&plan[3], Action::Pick, None)];
        assert_eq!(prepare(&plan, &entries, false).unwrap().rewrites_pushed, 1);
        // Onto another branch everything is rewritten, even an unchanged order.
        let same: Vec<Entry> = plan.iter().map(|c| e(c, Action::Pick, None)).collect();
        assert_eq!(prepare(&plan, &same, true).unwrap().rewrites_pushed, 2);
        assert!(prepare(&plan, &same, false).is_err(), "nothing to change");
    }

    #[test]
    fn squash_chain_message_goes_to_the_chain_end() {
        let plan = vec![pc("a", false), pc("b", false), pc("c", false)];
        let entries = vec![e(&plan[0], Action::Pick, None), e(&plan[1], Action::Squash, Some("all three")), e(&plan[2], Action::Fixup, None)];
        let p = prepare(&plan, &entries, false).unwrap();
        assert_eq!(p.messages, vec![(plan[2].sha.clone(), "all three".into())]);
        // A fixup-only chain opens no editor: no message.
        let entries = vec![e(&plan[0], Action::Pick, None), e(&plan[1], Action::Fixup, Some("ignored")), e(&plan[2], Action::Fixup, None)];
        assert!(prepare(&plan, &entries, false).unwrap().messages.is_empty());
    }

    #[test]
    fn refuses_invalid_plans() {
        let plan = vec![pc("a", false), pc("b", false)];
        // The first kept commit cannot be squashed.
        let entries = vec![e(&plan[0], Action::Drop, None), e(&plan[1], Action::Fixup, None)];
        assert!(prepare(&plan, &entries, false).is_err());
        // Missing / duplicated / unknown commits.
        assert_eq!(prepare(&plan, &[e(&plan[0], Action::Pick, None)], false).unwrap_err().status.as_u16(), 409);
        assert!(prepare(&plan, &[e(&plan[0], Action::Pick, None), e(&plan[0], Action::Drop, None)], false).is_err());
        let other = pc("c", false);
        assert_eq!(prepare(&plan, &[e(&plan[0], Action::Pick, None), e(&other, Action::Pick, None)], false).unwrap_err().status.as_u16(), 409);
        // An empty reword message.
        assert!(prepare(&plan, &[e(&plan[0], Action::Reword, Some("  \n")), e(&plan[1], Action::Pick, None)], false).is_err());
        // Everything dropped: a noop todo.
        let p = prepare(&plan, &[e(&plan[0], Action::Drop, None), e(&plan[1], Action::Drop, None)], false).unwrap();
        assert!(p.todo.ends_with("noop\n") && p.kept == 0);
    }

    #[test]
    fn chooses_a_comment_char_no_message_line_starts_with() {
        assert_eq!(comment_char_for(&["plain"]), Some('#'));
        assert_eq!(comment_char_for(&["#1 fix\n;x"]), Some('@'));
    }

    #[test]
    fn checks_git_todo_against_the_plan() {
        let expected = vec!["1111111aaaa".to_string(), "2222222bbbb".to_string()];
        let todo = "pick 1111111 one\npick 2222222 two\n\n# Rebase 0000..2222 onto 0000 (2 commands)\n# p, pick <commit> = use commit\n";
        assert!(same_commits(todo, &expected));
        assert!(!same_commits("pick 1111111 one\n", &expected));
        assert!(!same_commits("pick 1111111 one\npick 3333333 three\n", &expected));
        assert_eq!(todo_shas("fixup -C 1234567 x\nexec make\nnoop\n"), vec!["1234567"]);
        assert_eq!(last_done_sha("pick 1111111 one\nreword 2222222bbbb two\n"), Some("2222222bbbb".into()));
    }

    #[test]
    fn editor_helper_replaces_the_todo_and_writes_messages() {
        let d = tempfile::tempdir().unwrap();
        let git_dir = d.path().join("gitdir");
        std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        let staging = d.path().join("staging");
        let plan = vec![pc("a", false), pc("b", false)];
        let p = prepare(&plan, &[e(&plan[0], Action::Pick, None), e(&plan[1], Action::Reword, Some("better b"))], false).unwrap();
        write_staging(&staging, &git_dir, &plan, &p).unwrap();
        let todo_file = git_dir.join("rebase-merge/git-rebase-todo");
        std::fs::write(&todo_file, format!("pick {} a\npick {} b\n# comment\n", &plan[0].sha[..7], &plan[1].sha[..7])).unwrap();
        cli_git_editor("todo", &staging.to_string_lossy(), &todo_file.to_string_lossy()).unwrap();
        assert_eq!(std::fs::read_to_string(&todo_file).unwrap(), p.todo);
        // Git reached the reword: the message helper writes the prepared text.
        std::fs::write(git_dir.join("rebase-merge/done"), format!("pick {} subject a\nreword {} subject b\n", plan[0].sha, plan[1].sha)).unwrap();
        let msg = d.path().join("COMMIT_EDITMSG");
        std::fs::write(&msg, "subject b\n# Please enter the commit message\n").unwrap();
        cli_git_editor("message", &git_dir.to_string_lossy(), &msg.to_string_lossy()).unwrap();
        assert_eq!(std::fs::read_to_string(&msg).unwrap(), "better b\n");
        // A commit without a prepared message keeps git's text.
        std::fs::write(git_dir.join("rebase-merge/done"), format!("pick {} subject a\n", plan[0].sha)).unwrap();
        std::fs::write(&msg, "git's\n").unwrap();
        cli_git_editor("message", &git_dir.to_string_lossy(), &msg.to_string_lossy()).unwrap();
        assert_eq!(std::fs::read_to_string(&msg).unwrap(), "git's\n");
        let (args, env) = continue_env(&git_dir, Some("wb git-editor"));
        assert_eq!(args, vec!["-c".to_string(), "core.commentChar=#".into()]);
        assert!(env[0].1.starts_with("wb git-editor message '"));
        // A changed todo (someone committed meanwhile) is refused.
        std::fs::write(&todo_file, format!("pick {} a\n", &plan[0].sha[..7])).unwrap();
        assert!(cli_git_editor("todo", &staging.to_string_lossy(), &todo_file.to_string_lossy()).is_err());
    }
}
