//! Long git operations: the network ones (fetch, pull, push, delete a remote branch)
//! and an interactive rebase. They answer `202 {opId}` at once and run in the
//! background, streaming git's progress as `git.op` events `{opId, op, line}` and
//! finishing with `{opId, op, done, ok, message, conflicts?, stopped?}`.
//! The client may choose the `opId` so it can match events that arrive before
//! the HTTP response. Recent ops (with their log) stay queryable for late viewers.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::cmd::{Git, clean_message};
use super::repo::Repo;
use crate::app::AppState;
use crate::error::ApiError;
use crate::secrets::Secret;

const MAX_OP_LINES: usize = 400;
const MAX_LINE_CHARS: usize = 1000;
const KEEP_OPS: usize = 50;
const OP_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Progress lines (`\r`-terminated) are emitted at most this often per op.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpInfo {
    pub op_id: String,
    pub project_id: String,
    pub op: String,
    pub title: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub done: bool,
    pub ok: Option<bool>,
    pub message: Option<String>,
    /// Finished, but files were left in conflict (a pull whose autostash could
    /// not be re-applied cleanly).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub conflicts: bool,
    /// A rebase stopped (an `edit` step, conflicts): continue, skip or abort it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    pub lines: Vec<String>,
}

/// How a remote op ended.
#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    pub ok: bool,
    pub message: String,
    pub conflicts: bool,
    pub stopped: bool,
}

impl OpResult {
    fn ok(message: String) -> Self {
        Self { ok: true, message, conflicts: false, stopped: false }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), conflicts: false, stopped: false }
    }
}

struct OpRecord {
    info: OpInfo,
    lines: VecDeque<String>,
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct OpRegistry {
    ops: Mutex<HashMap<String, OpRecord>>,
    order: Mutex<VecDeque<String>>,
}

impl OpRegistry {
    fn insert(&self, info: OpInfo, cancel: CancellationToken) -> Result<(), ApiError> {
        let mut ops = self.ops.lock();
        if ops.get(&info.op_id).is_some_and(|r| !r.info.done) {
            return Err(ApiError::conflict(format!("operation {} is already running", info.op_id)));
        }
        let id = info.op_id.clone();
        ops.insert(id.clone(), OpRecord { info, lines: VecDeque::new(), cancel });
        let mut order = self.order.lock();
        order.retain(|x| x != &id);
        order.push_back(id);
        while order.len() > KEEP_OPS {
            if let Some(old) = order.pop_front() {
                if ops.get(&old).is_some_and(|r| r.info.done) {
                    ops.remove(&old);
                } else {
                    order.push_back(old);
                    break;
                }
            }
        }
        Ok(())
    }

    fn push_line(&self, id: &str, line: &str) {
        if let Some(r) = self.ops.lock().get_mut(id) {
            r.lines.push_back(line.to_string());
            while r.lines.len() > MAX_OP_LINES {
                r.lines.pop_front();
            }
        }
    }

    fn finish(&self, id: &str, res: &OpResult) {
        if let Some(r) = self.ops.lock().get_mut(id) {
            r.info.done = true;
            r.info.ok = Some(res.ok);
            r.info.message = Some(res.message.clone());
            r.info.conflicts = res.conflicts;
            r.info.stopped = res.stopped;
            r.info.finished_at = Some(crate::util::now_ms());
        }
    }

    pub fn get(&self, id: &str) -> Option<OpInfo> {
        self.ops.lock().get(id).map(|r| OpInfo { lines: r.lines.iter().cloned().collect(), ..r.info.clone() })
    }

    /// Running and recent ops of a project (without their logs).
    pub fn list(&self, project_id: &str) -> Vec<OpInfo> {
        let ops = self.ops.lock();
        let order = self.order.lock();
        order
            .iter()
            .rev()
            .filter_map(|id| ops.get(id))
            .filter(|r| r.info.project_id == project_id)
            .map(|r| r.info.clone())
            .collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        match self.ops.lock().get(id) {
            Some(r) if !r.info.done => {
                r.cancel.cancel();
                true
            }
            _ => false,
        }
    }
}

/// Validate a client-chosen op id, or make one.
pub fn op_id(requested: Option<&str>) -> Result<String, ApiError> {
    match requested.filter(|s| !s.is_empty()) {
        Some(id) if id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') => Ok(id.to_string()),
        Some(_) => Err(ApiError::bad_request("opId must be 1-64 characters of [A-Za-z0-9_-]")),
        None => Ok(format!("op-{}", crate::util::random_token(9))),
    }
}

pub struct RemoteOpSpec {
    pub op: &'static str,
    pub title: String,
    pub args: Vec<String>,
    /// Take the repository write lock (pull and rebase change the working tree).
    pub lock: bool,
    /// Extra environment (the interactive rebase's editors).
    pub env: Vec<(String, String)>,
    /// A folder to delete when the op ends (the rebase's staging folder).
    pub cleanup: Option<PathBuf>,
}

impl RemoteOpSpec {
    pub fn new(op: &'static str, title: String, args: Vec<String>, lock: bool) -> Self {
        Self { op, title, args, lock, env: vec![], cleanup: None }
    }
}

/// Split a byte stream into lines on `\n` and `\r`; `\r` marks transient progress.
struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    fn push(&mut self, data: &[u8], out: &mut Vec<(String, bool)>) {
        for &b in data {
            if b == b'\n' || b == b'\r' {
                let line = String::from_utf8_lossy(&self.buf).trim_end().to_string();
                self.buf.clear();
                if !line.is_empty() {
                    out.push((line, b == b'\r'));
                }
            } else if self.buf.len() < 64 * 1024 {
                self.buf.push(b);
            }
        }
    }
    fn finish(&mut self, out: &mut Vec<(String, bool)>) {
        let line = String::from_utf8_lossy(&self.buf).trim_end().to_string();
        self.buf.clear();
        if !line.is_empty() {
            out.push((line, false));
        }
    }
}

fn truncate_line(s: &str) -> String {
    if s.chars().count() > MAX_LINE_CHARS { format!("{}…", s.chars().take(MAX_LINE_CHARS).collect::<String>()) } else { s.to_string() }
}

/// Known secret values that could appear in git output (defence in depth).
fn secrets_for(state: &AppState, repo: &Repo) -> Vec<Secret> {
    let project = state.projects.get(&repo.project_id);
    let mut names = vec![];
    if let Some(g) = project.as_ref().and_then(|p| p.config.repo.as_ref()).and_then(|r| r.gitlab.as_ref()) {
        if !g.token.is_empty() {
            names.push(g.token.clone());
        }
    }
    if let Some(g) = state.config.read().gitlab.as_ref() {
        names.push(g.token.clone());
    }
    names.iter().filter_map(|n| state.secret(project.as_deref(), n).ok()).collect()
}

/// Start a remote op in the background; returns its id.
pub fn start(state: &AppState, repo: Arc<Repo>, spec: RemoteOpSpec, requested_id: Option<&str>) -> Result<String, ApiError> {
    let id = op_id(requested_id)?;
    let cancel = CancellationToken::new();
    let info = OpInfo {
        op_id: id.clone(),
        project_id: repo.project_id.clone(),
        op: spec.op.to_string(),
        title: spec.title.clone(),
        started_at: crate::util::now_ms(),
        finished_at: None,
        done: false,
        ok: None,
        message: None,
        conflicts: false,
        stopped: false,
        lines: vec![],
    };
    state.git.ops.insert(info, cancel.clone())?;
    let state = state.clone();
    let op_id = id.clone();
    tokio::spawn(async move {
        let res = run(&state, &repo, &spec, &op_id, cancel).await;
        if let Some(dir) = spec.cleanup.clone() {
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
        }
        state.git.ops.finish(&op_id, &res);
        let mut ev = json!({ "opId": op_id, "op": spec.op, "title": spec.title, "done": true, "ok": res.ok, "message": res.message });
        if res.conflicts {
            ev["conflicts"] = json!(true);
        }
        if res.stopped {
            ev["stopped"] = json!(true);
        }
        state.events.emit("git.op", Some(&repo.project_id), ev);
        state.events.emit("git.changed", Some(&repo.project_id), json!({}));
    });
    Ok(id)
}

/// Kill git and everything it started. The child runs in its own session (see
/// `run`), so its process group also holds the transport helper
/// (`git-remote-https`, `ssh`) that inherited our pipes; killing only `git`
/// would leave the op "running" until that helper gives up by itself.
fn kill_group(child: &mut tokio::process::Child, group: &crate::util::os::proc::ProcGroup) {
    if child.id().is_some() {
        // The child is not reaped yet (`id()` is Some), so its pid, which is also its
        // process-group id, cannot have been reused.
        group.kill();
    }
    let _ = child.start_kill();
}

/// After a kill, how long to keep collecting output from processes that may have
/// escaped the group (a daemonized ssh master) before giving up on them.
const KILL_GRACE: Duration = Duration::from_secs(3);

async fn run(state: &AppState, repo: &Repo, spec: &RemoteOpSpec, op_id: &str, cancel: CancellationToken) -> OpResult {
    // Waiting for the write lock (an update while another one runs) can be cancelled too.
    let _guard = if spec.lock {
        tokio::select! {
            g = state.git.lock(&repo.top).lock_owned() => Some(g),
            _ = cancel.cancelled() => return OpResult::failed(format!("{} cancelled", spec.title)),
        }
    } else {
        None
    };
    let secrets = secrets_for(state, repo);
    let emit_line = |line: &str| {
        let line = truncate_line(&crate::secrets::redact(line, &secrets));
        state.events.emit("git.op", Some(&repo.project_id), json!({ "opId": op_id, "op": spec.op, "line": line }));
        line
    };
    emit_line(&format!("$ git {}", spec.args.join(" ")));

    let mut g = Git::write(&repo.top).args(spec.args.clone());
    for (k, v) in &spec.env {
        g = g.env(k, v.clone());
    }
    // GIT_ASKPASS (Windows: also ssh's SSH_ASKPASS); GIT_TERMINAL_PROMPT=0 comes with
    // every git command (`Git::command`).
    let root = project_root(state, repo);
    if let Some(askpass) = state.git.askpass.get() {
        for (k, v) in askpass {
            g = g.env(k, v.clone());
        }
        // Git's credential helpers are neither asked for the host askpass answers for nor
        // handed its token to store (Git Credential Manager, `~/.git-credentials`…).
        let cfg = state.config.read().clone();
        if let Some(key) = super::askpass::reset_helpers_key(&state.paths, &cfg, Some((&root, &repo.project_id))) {
            g = g.config(&key, "");
        }
    }
    g = g
        .env("WORKBENCH_PROJECT_ID", repo.project_id.clone())
        .env("WORKBENCH_PROJECT_ROOT", root.to_string_lossy().to_string())
        // Abort transfers that stall for a minute.
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
        .env("GIT_HTTP_LOW_SPEED_TIME", "60");
    let mut cmd = g.command();
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // A new session has no controlling terminal (Windows: no console), so ssh cannot
    // open /dev/tty and block on a passphrase prompt nobody can answer.
    crate::util::os::proc::ProcGroup::prepare_session(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return OpResult::failed(format!("cannot run git: {e}")),
    };
    let group = crate::util::os::proc::ProcGroup::attach(&child);
    let (tx, mut rx) = mpsc::channel::<(String, bool)>(256);
    let mut readers = vec![];
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>)]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut s = stream;
            let mut sp = LineSplitter { buf: vec![] };
            let mut chunk = vec![0u8; 8192];
            loop {
                let n = match s.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut lines = vec![];
                sp.push(&chunk[..n], &mut lines);
                for l in lines {
                    if tx.send(l).await.is_err() {
                        return;
                    }
                }
            }
            let mut lines = vec![];
            sp.finish(&mut lines);
            for l in lines {
                let _ = tx.send(l).await;
            }
        }));
    }
    drop(tx);

    let mut kept: Vec<String> = vec![];
    let mut last_progress = Instant::now() - PROGRESS_EVERY;
    let mut pending_progress: Option<String> = None;
    let deadline = tokio::time::sleep(OP_TIMEOUT);
    tokio::pin!(deadline);
    // Armed when the op is killed: stop waiting for stray pipe holders then.
    let grace = tokio::time::sleep(OP_TIMEOUT * 2);
    tokio::pin!(grace);
    let mut aborted: Option<&str> = None;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some((line, progress)) => {
                    if progress {
                        if last_progress.elapsed() >= PROGRESS_EVERY {
                            emit_line(&line);
                            last_progress = Instant::now();
                            pending_progress = None;
                        } else {
                            pending_progress = Some(line);
                        }
                    } else {
                        pending_progress = None;
                        let l = emit_line(&line);
                        state.git.ops.push_line(op_id, &l);
                        kept.push(l);
                    }
                }
                None => break,
            },
            _ = cancel.cancelled(), if aborted.is_none() => {
                kill_group(&mut child, &group);
                aborted = Some("cancelled");
                grace.as_mut().reset(tokio::time::Instant::now() + KILL_GRACE);
            }
            _ = &mut deadline, if aborted.is_none() => {
                kill_group(&mut child, &group);
                aborted = Some("timed out");
                grace.as_mut().reset(tokio::time::Instant::now() + KILL_GRACE);
            }
            _ = &mut grace, if aborted.is_some() => break,
        }
    }
    if let Some(p) = pending_progress {
        emit_line(&p);
    }
    for r in readers {
        if aborted.is_some() {
            r.abort();
        } else {
            let _ = r.await;
        }
    }
    let status = child.wait().await;
    if let Some(why) = aborted {
        if spec.op == "rebase" && super::status::detect_state(&repo.git_dir).0 == "rebasing" {
            return OpResult {
                ok: false,
                conflicts: false,
                stopped: true,
                message: format!("{} {why}: the rebase is still in progress; continue or abort it", spec.title),
            };
        }
        return OpResult::failed(format!("{} {why}", spec.title));
    }
    if spec.op == "rebase" {
        return rebase_result(repo, matches!(status, Ok(s) if s.success()), &kept).await;
    }
    // A pull can leave files unmerged: a merge/rebase conflict (non-zero exit), or
    // `--autostash` failing to re-apply the local changes, which still exits 0.
    let conflicted = if spec.op == "pull" { super::ops::conflicted_paths(repo).await.map(|v| v.len()).unwrap_or(0) } else { 0 };
    match status {
        Ok(s) if s.success() => {
            if conflicted > 0 {
                // Not a success to report (and to auto-dismiss): the user's file
                // now has conflict markers and their changes sit in the stash.
                return OpResult { ok: false, conflicts: true, stopped: false, message: super::ops::autostash_conflict_message("Updated", conflicted) };
            }
            OpResult::ok(summarize_success(spec, &kept))
        }
        Ok(s) => {
            let tail: Vec<&str> = kept.iter().rev().take(15).map(String::as_str).collect::<Vec<_>>().into_iter().rev().collect();
            let mut msg = clean_message(&tail.join("\n"));
            if msg.is_empty() {
                msg = format!("git exited with {s}");
            }
            OpResult { ok: false, conflicts: conflicted > 0, stopped: false, message: explain_failure(&msg) }
        }
        Err(e) => OpResult::failed(format!("git failed: {e}")),
    }
}

/// How an interactive rebase ended: done, stopped (edit step, conflicts), or failed.
async fn rebase_result(repo: &Repo, success: bool, lines: &[String]) -> OpResult {
    let (state, _) = super::status::detect_state(&repo.git_dir);
    let conflicted = super::ops::conflicted_paths(repo).await.map(|v| v.len()).unwrap_or(0);
    if state == "rebasing" {
        if conflicted > 0 {
            return OpResult {
                ok: false,
                conflicts: true,
                stopped: true,
                message: format!(
                    "The rebase stopped: {conflicted} file{} with conflicts. Resolve them, then Continue (or Skip / Abort).",
                    if conflicted == 1 { "" } else { "s" }
                ),
            };
        }
        return OpResult { ok: false, conflicts: false, stopped: true, message: super::ops::stop_message(repo).await };
    }
    if !success {
        let tail: Vec<&str> = lines.iter().rev().take(12).map(String::as_str).collect::<Vec<_>>().into_iter().rev().collect();
        let mut msg = clean_message(&tail.join("\n"));
        if msg.is_empty() {
            msg = "git rebase failed".into();
        }
        return OpResult { ok: false, conflicts: conflicted > 0, stopped: false, message: msg };
    }
    if conflicted > 0 {
        return OpResult { ok: false, conflicts: true, stopped: false, message: super::ops::autostash_conflict_message("Rebased", conflicted) };
    }
    OpResult::ok("Rebased: the history was rewritten".into())
}

fn project_root(state: &AppState, repo: &Repo) -> PathBuf {
    state.projects.get(&repo.project_id).map(|p| p.root.clone()).unwrap_or_else(|| repo.top.clone())
}

fn summarize_success(spec: &RemoteOpSpec, lines: &[String]) -> String {
    match spec.op {
        "fetch" => {
            let updated = lines.iter().filter(|l| l.contains("->")).count();
            if updated == 0 { "Fetched: everything is up to date".into() } else { format!("Fetched: {updated} ref(s) updated") }
        }
        "pull" => {
            if lines.iter().any(|l| l.contains("Already up to date")) {
                "Already up to date".into()
            } else if let Some(l) = lines.iter().rev().find(|l| l.contains("file changed") || l.contains("files changed")) {
                format!("Updated: {}", l.trim())
            } else if lines.iter().any(|l| l.contains("Successfully rebased")) {
                "Updated (rebased)".into()
            } else {
                "Updated".into()
            }
        }
        "push" => {
            if lines.iter().any(|l| l.contains("Everything up-to-date")) {
                "Everything up to date".into()
            } else {
                let dest = lines.iter().find(|l| l.contains("->")).map(|l| l.trim().to_string());
                match dest {
                    Some(d) => format!("Pushed: {d}"),
                    None => "Pushed".into(),
                }
            }
        }
        _ => format!("{} done", spec.title),
    }
}

/// Add a hint to the most common remote failures. Remote ops cannot answer ssh's own
/// questions (askpass refuses passphrase and host-key prompts), so those fail and say what
/// to do in a terminal instead.
fn explain_failure(msg: &str) -> String {
    if msg.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") || (msg.contains("Host key for") && msg.contains("has changed")) {
        // Never "accept it": a changed key is also what an intercepted connection shows.
        format!(
            "{msg}\n\nThe server's ssh host key is not the one you accepted before. That happens when the server was reinstalled or its key was rotated, and also when someone intercepts the connection. Check the new fingerprint with the server's administrators or against the fingerprints they publish before you replace the old key in known_hosts (`ssh-keygen -R <host>` removes it)."
        )
    } else if msg.contains("REVOKED HOST KEY") || (msg.contains("host key for") && msg.contains("revoked")) {
        // Never "accept it" either: a revoked key may be a stolen one. (ssh's revoked-key
        // lines do not say "has changed", and end in "Host key verification failed" too.)
        format!(
            "{msg}\n\nThe server's ssh host key is marked as revoked (known_hosts `@revoked`, or ssh's RevokedHostKeys). A revoked key can be a stolen key used to impersonate the server: do not trust it again, ask the server's administrators which key the server uses now."
        )
    } else if msg.contains("Host key verification failed") {
        format!(
            "{msg}\n\nssh does not know this server's host key yet, and Workbench cannot answer ssh's question. Connect once with ssh in a terminal (for example `ssh -T git@<host>`), check that the fingerprint it shows is one the server publishes, and accept it."
        )
    } else if msg.contains("Permission denied (") && msg.contains("publickey") {
        format!(
            "{msg}\n\nThe server accepted none of your ssh keys. If your key has a passphrase, load it into ssh-agent (`ssh-add`; on Windows, start the OpenSSH Authentication Agent service first): Workbench cannot answer passphrase prompts. Otherwise add your public key to your account on the server."
        )
    } else if msg.contains("could not read Username") || msg.contains("Authentication failed") || msg.contains("terminal prompts disabled") {
        format!("{msg}\n\nAuthentication failed. Configure [gitlab] (host + token secret) in config.toml, or a git credential helper / ssh key for this remote.")
    } else if msg.contains("[rejected]") && (msg.contains("fetch first") || msg.contains("non-fast-forward")) {
        format!("{msg}\n\nThe remote has commits you do not have. Update (pull) first, or push with force-with-lease.")
    } else if msg.contains("stale info") {
        format!("{msg}\n\nforce-with-lease refused: the remote branch moved since your last fetch. Fetch and review first.")
    } else if msg.contains("Need to specify how to reconcile divergent branches") {
        format!("{msg}\n\nChoose merge or rebase for the update.")
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_progress_and_lines() {
        let mut sp = LineSplitter { buf: vec![] };
        let mut out = vec![];
        sp.push(b"Receiving objects:  10% (1/10)\rReceiving objects: 100% (10/10), done.\nTo ", &mut out);
        sp.push(b"gitlab.com:x\n", &mut out);
        sp.finish(&mut out);
        assert_eq!(
            out,
            vec![
                ("Receiving objects:  10% (1/10)".to_string(), true),
                ("Receiving objects: 100% (10/10), done.".to_string(), false),
                ("To gitlab.com:x".to_string(), false),
            ]
        );
    }

    #[test]
    fn validates_op_ids() {
        assert_eq!(op_id(Some("abc-1_2")).unwrap(), "abc-1_2");
        assert!(op_id(Some("a b")).is_err());
        assert!(op_id(None).unwrap().starts_with("op-"));
    }

    #[test]
    fn registry_keeps_logs_and_rejects_duplicate_running_ids() {
        let r = OpRegistry::default();
        let info = |id: &str| OpInfo {
            op_id: id.into(),
            project_id: "p".into(),
            op: "fetch".into(),
            title: "Fetch".into(),
            started_at: 0,
            finished_at: None,
            done: false,
            ok: None,
            message: None,
            conflicts: false,
            stopped: false,
            lines: vec![],
        };
        r.insert(info("a"), CancellationToken::new()).unwrap();
        assert!(r.insert(info("a"), CancellationToken::new()).is_err());
        r.push_line("a", "hello");
        r.finish("a", &OpResult::ok("ok".into()));
        let got = r.get("a").unwrap();
        assert!(got.done && got.ok == Some(true));
        assert_eq!(got.lines, vec!["hello"]);
        assert_eq!(r.list("p").len(), 1);
        r.insert(info("a"), CancellationToken::new()).unwrap();
    }

    #[test]
    fn explains_auth_failures() {
        assert!(explain_failure("fatal: could not read Username for 'https://gitlab.com'").contains("Authentication failed"));
    }

    /// What git and ssh print (after `clean_message`) when ssh would have had to ask.
    #[test]
    fn explains_ssh_failures() {
        let unknown = explain_failure("Host key verification failed.\nCould not read from remote repository.\nPlease make sure you have the correct access rights\nand the repository exists.");
        assert!(unknown.starts_with("Host key verification failed."), "git's message first: {unknown}");
        assert!(unknown.contains("Connect once with ssh in a terminal") && unknown.contains("fingerprint"), "{unknown}");
        let strict = explain_failure("No ED25519 host key is known for gitlab.com and you have requested strict checking.\nHost key verification failed.");
        assert!(strict.contains("Connect once with ssh in a terminal"), "{strict}");

        // A changed key is never "accepted": it can be an intercepted connection.
        let changed = explain_failure(
            "Offending ED25519 key in /home/u/.ssh/known_hosts:3\nremove with:\nssh-keygen -f '/home/u/.ssh/known_hosts' -R 'gitlab.com'\nHost key for gitlab.com has changed and you have requested strict checking.\nHost key verification failed.",
        );
        assert!(changed.contains("not the one you accepted before") && changed.contains("intercepts"), "{changed}");
        assert!(!changed.contains("accept it"), "{changed}");
        let banner = explain_failure("@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\nHost key verification failed.");
        assert!(banner.contains("not the one you accepted before"), "{banner}");
        // Nor is a revoked one (OpenSSH's words for a key known_hosts marks `@revoked`).
        let revoked = explain_failure(
            "@       WARNING: REVOKED HOST KEY DETECTED!               @\nThe ED25519 host key for gitlab.com is marked as revoked.\nThis could mean that a stolen key is being used to\nimpersonate this host.\nED25519 host key for gitlab.com was revoked and you have requested strict checking.\nHost key verification failed.",
        );
        assert!(revoked.contains("marked as revoked (known_hosts") && revoked.contains("stolen key"), "{revoked}");
        assert!(!revoked.contains("accept it") && !revoked.contains("Connect once"), "{revoked}");
        let revoked = explain_failure("ED25519 host key for gitlab.com was revoked and you have requested strict checking.\nHost key verification failed.");
        assert!(revoked.contains("do not trust it again"), "{revoked}");

        for denied in ["git@gitlab.com: Permission denied (publickey).", "git@git.corp: Permission denied (publickey,password)."] {
            let got = explain_failure(&format!("{denied}\nCould not read from remote repository."));
            assert!(got.contains("ssh-agent") && got.contains("OpenSSH Authentication Agent") && got.contains("passphrase"), "{got}");
        }
        // Other failures keep their own hints, or none.
        assert!(!explain_failure("Permission denied (password).").contains("ssh-agent"));
        assert_eq!(explain_failure("fatal: repository 'x' not found"), "fatal: repository 'x' not found");
    }
}
