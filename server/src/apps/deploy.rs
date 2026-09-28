//! Deploys: gates, confirmation and the deploy terminal.
//!
//! Gates are evaluated server-side on every attempt (the UI's preflight is only a
//! preview):
//! * `only_ref` — the checked-out branch must be that ref, and the sha must be on it;
//! * `require_green_pipeline` — GitLab's latest pipeline for the **full** sha is `success`
//!   (`forge::commit_ci_status`: GitLab pipelines or GitHub checks);
//! * `after` — when the other env's deployed version is known, it must be this sha
//!   (an unknown version is a warning, not a block);
//! * `confirm` — none / click / typed (the env name).
//!
//! The command (`{sha}` `{sha8}` `{branch}` expanded) runs over ssh on the env's host,
//! or locally when `deploy.local = true`, in a `Command` terminal with meta
//! `{env, action: "deploy", sha}`. Deploys are never exposed to agents (no MCP tool).

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::envs::{self, VersionInfo};
use super::expand;
use super::health::same_commit;
use super::remote;
use crate::app::AppState;
use crate::config::project::{Confirm, EnvKind, Environment};
use crate::error::ApiError;
use crate::forge::CiStatus;
use crate::projects::Project;
use crate::terminals::{SpawnSpec, TerminalInfo, TerminalKind};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    Fail,
    Warn,
    Skip,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub id: &'static str,
    pub label: String,
    pub status: GateStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn gate(id: &'static str, label: impl Into<String>, status: GateStatus, detail: impl Into<String>) -> Gate {
    Gate { id, label: label.into(), status, detail: detail.into(), url: None }
}

pub fn gate_ref(only_ref: Option<&str>, branch: Option<&str>, on_ref: Option<bool>) -> Gate {
    let Some(want) = only_ref else {
        return gate("ref", "Branch", GateStatus::Skip, "any branch may deploy");
    };
    let label = format!("Branch {want}");
    match branch {
        Some(b) if b == want => match on_ref {
            Some(true) => gate("ref", label, GateStatus::Pass, format!("on {want}")),
            Some(false) => gate("ref", label, GateStatus::Fail, format!("this commit is not on {want}")),
            None => gate("ref", label, GateStatus::Warn, format!("on {want}; could not verify the commit is on it")),
        },
        Some(b) => gate("ref", label, GateStatus::Fail, format!("the checked-out branch is {b}; deploys only from {want}")),
        None => gate("ref", label, GateStatus::Fail, format!("HEAD is detached; deploys only from {want}")),
    }
}

pub fn gate_pipeline(require: bool, status: Result<Option<CiStatus>, String>) -> Gate {
    if !require {
        return gate("pipeline", "Pipeline", GateStatus::Skip, "not required");
    }
    match status {
        Ok(Some(s)) => {
            let id = s.pipeline_id.map(|i| format!("#{i} ")).unwrap_or_default();
            let mut g = if s.status == "success" {
                gate("pipeline", "Pipeline green", GateStatus::Pass, format!("pipeline {id}passed"))
            } else {
                gate("pipeline", "Pipeline green", GateStatus::Fail, format!("pipeline {id}is {}", s.status))
            };
            g.url = s.web_url;
            g
        }
        Ok(None) => gate("pipeline", "Pipeline green", GateStatus::Fail, "no pipeline for this commit"),
        Err(e) => gate("pipeline", "Pipeline green", GateStatus::Fail, format!("could not read the pipeline status: {e}")),
    }
}

pub fn gate_after(after: Option<&str>, other_exists: bool, other_version: Option<&VersionInfo>, sha: &str) -> Gate {
    let Some(other) = after else {
        return gate("after", "Order", GateStatus::Skip, "no prerequisite environment");
    };
    let label = format!("Runs on {other} first");
    if !other_exists {
        return gate("after", label, GateStatus::Fail, format!("there is no environment {other:?}"));
    }
    match other_version.and_then(|v| v.sha.as_deref()) {
        Some(v) if same_commit(v, sha) => gate("after", label, GateStatus::Pass, format!("{other} runs {v}")),
        Some(v) => gate("after", label, GateStatus::Fail, format!("{other} runs {v}; deploy this commit there first")),
        None => gate("after", label, GateStatus::Warn, format!("{other}'s version is unknown; check it to verify")),
    }
}

/// Validate the user's confirmation against the env's `confirm` mode.
pub fn check_confirmation(confirm: Confirm, env: &str, confirmation: &Value) -> Result<(), String> {
    match confirm {
        Confirm::None => Ok(()),
        Confirm::Click => match confirmation {
            Value::Bool(true) => Ok(()),
            Value::String(s) if !s.trim().is_empty() => Ok(()),
            _ => Err(format!("deploying to {env} needs confirmation")),
        },
        Confirm::Typed => match confirmation.as_str() {
            Some(s) if s == env => Ok(()),
            _ => Err(format!("type {env:?} to confirm the deploy")),
        },
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployPlan {
    pub env: String,
    pub kind: EnvKind,
    pub sha: String,
    pub sha8: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `root@host` or `local`.
    pub target: String,
    /// The command with placeholders expanded (no secrets: commands never contain any).
    pub command: String,
    pub confirm: Confirm,
    pub gates: Vec<Gate>,
    /// No gate failed.
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploying: Option<String>,
}

fn valid_rev(s: &str) -> bool {
    (4..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn valid_ref(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
}

async fn git(root: &std::path::Path, args: &[&str]) -> Option<crate::util::proc::Output> {
    let mut c = tokio::process::Command::new("git");
    c.args(args).current_dir(root).env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0");
    crate::util::proc::run_cmd(c, Duration::from_secs(10)).await.ok()
}

/// Full sha of `rev` (hex, possibly abbreviated) or of HEAD.
async fn resolve_sha(root: &std::path::Path, rev: Option<&str>) -> Result<String, ApiError> {
    match rev.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => {
            if !valid_rev(r) {
                return Err(ApiError::bad_request(format!("{r:?} is not a commit sha")));
            }
            let spec = format!("{r}^{{commit}}");
            let out = git(root, &["rev-parse", "--verify", "--quiet", &spec]).await;
            match out {
                Some(o) if o.ok() && valid_rev(o.stdout.trim()) => Ok(o.stdout.trim().to_string()),
                _ => Err(ApiError::bad_request(format!("unknown commit {r}"))),
            }
        }
        None => crate::util::git::head_sha(root).await.ok_or_else(|| ApiError::bad_request("the repository has no commits")),
    }
}

/// Whether `sha` is on `branch` (local branch, else origin's).
async fn on_branch(root: &std::path::Path, sha: &str, branch: &str) -> Option<bool> {
    if !valid_ref(branch) {
        return None;
    }
    for r in [format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}")] {
        let exists = git(root, &["rev-parse", "--verify", "--quiet", &r]).await.is_some_and(|o| o.ok());
        if !exists {
            continue;
        }
        return match git(root, &["merge-base", "--is-ancestor", sha, &r]).await.and_then(|o| o.code) {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        };
    }
    None
}

pub async fn plan(state: &AppState, project: &Project, e: &Environment, rev: Option<&str>) -> Result<DeployPlan, ApiError> {
    let d = e.deploy.as_ref().ok_or_else(|| ApiError::not_configured(format!("{} has no [env.deploy] section", e.name)))?;
    let target = remote::deploy_target(project, e, d.local)?;
    let sha = resolve_sha(&project.root, rev).await?;
    let sha8: String = sha.chars().take(8).collect();
    let branch = crate::util::git::current_branch(&project.root).await;
    let subject = git(&project.root, &["log", "-1", "--format=%s", &sha]).await.filter(|o| o.ok()).map(|o| o.stdout.trim().to_string());

    let on_ref = match &d.only_ref {
        Some(r) => on_branch(&project.root, &sha, r).await,
        None => None,
    };
    let pipeline = if d.require_green_pipeline {
        crate::forge::commit_ci_status(state, project, &sha).await.map_err(|e| e.message)
    } else {
        Ok(None)
    };
    let other = d.after.as_deref();
    let other_exists = other.is_some_and(|o| project.config.envs.iter().any(|x| x.name == o));
    let other_version = other.and_then(|o| state.apps.envs.version(&project.id, o));
    let gates = vec![
        gate_ref(d.only_ref.as_deref(), branch.as_deref(), on_ref),
        gate_pipeline(d.require_green_pipeline, pipeline),
        gate_after(other, other_exists, other_version.as_ref(), &sha),
    ];
    let mut vars = expand::base_vars(project);
    vars.insert("sha".into(), sha.clone());
    vars.insert("sha8".into(), sha8.clone());
    let branch_var = branch.clone().unwrap_or_else(|| "HEAD".into());
    // The command runs in a shell on the production host: never splice an unsafe name.
    expand::check_branch(&branch_var, &[d.command.as_str()])?;
    vars.insert("branch".into(), branch_var);
    let ok = gates.iter().all(|g| g.status != GateStatus::Fail);
    Ok(DeployPlan {
        env: e.name.clone(),
        kind: e.kind,
        sha,
        sha8,
        branch,
        subject,
        target: target.label(),
        command: expand::placeholders(&d.command, &vars),
        confirm: d.confirm,
        gates,
        ok,
        deploying: deploying_terminal(state, &project.id, &e.name),
    })
}

/// Value of `Envs::deploys` while a deploy is being planned (before its terminal exists).
const PENDING: &str = "pending";

/// The terminal of the deploy to `env` that is under way, if any (not a reservation).
pub(crate) fn deploying_terminal(state: &AppState, pid: &str, env: &str) -> Option<String> {
    state.apps.envs.deploys.lock().get(&(pid.to_string(), env.to_string())).filter(|v| *v != PENDING).cloned()
}

/// Holds an env's deploy slot from the first check until the deploy terminal is
/// recorded; dropping it (any early return) frees the slot again.
struct DeployReservation<'a> {
    state: &'a AppState,
    key: (String, String),
    armed: bool,
}

impl DeployReservation<'_> {
    /// Reserve the slot atomically: a second request arriving while the first is
    /// still planning (git, GitLab) or running is refused.
    fn take<'a>(state: &'a AppState, key: (String, String), env: &str) -> Result<DeployReservation<'a>, ApiError> {
        let mut deploys = state.apps.envs.deploys.lock();
        if let Some(cur) = deploys.get(&key) {
            let busy = cur == PENDING || state.terminals.info(cur).is_some_and(|i| i.exit.is_none());
            if busy {
                return Err(ApiError::conflict(format!("a deploy to {env} is already running")));
            }
        }
        deploys.insert(key.clone(), PENDING.to_string());
        Ok(DeployReservation { state, key, armed: true })
    }

    /// The deploy terminal started: it now holds the slot.
    fn fulfil(mut self, terminal_id: &str) {
        self.state.apps.envs.deploys.lock().insert(self.key.clone(), terminal_id.to_string());
        self.armed = false;
    }
}

impl Drop for DeployReservation<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut deploys = self.state.apps.envs.deploys.lock();
            if deploys.get(&self.key).is_some_and(|v| v == PENDING) {
                deploys.remove(&self.key);
            }
        }
    }
}

/// Re-check every gate and the confirmation, then start the deploy terminal.
pub async fn deploy(state: &AppState, project: &Project, e: &Environment, rev: Option<&str>, confirmation: &Value) -> Result<TerminalInfo, ApiError> {
    let d = e.deploy.clone().ok_or_else(|| ApiError::not_configured(format!("{} has no [env.deploy] section", e.name)))?;
    let key = (project.id.clone(), e.name.clone());
    let reservation = DeployReservation::take(state, key.clone(), &e.name)?;
    let p = plan(state, project, e, rev).await?;
    if !p.ok {
        let why: Vec<String> = p.gates.iter().filter(|g| g.status == GateStatus::Fail).map(|g| format!("{}: {}", g.label, g.detail)).collect();
        return Err(ApiError::conflict(format!("deploy to {} blocked — {}", e.name, why.join("; "))));
    }
    check_confirmation(d.confirm, &e.name, confirmation)
        .map_err(|m| ApiError::new(axum::http::StatusCode::PRECONDITION_REQUIRED, "confirmation_required", m))?;
    let target = remote::deploy_target(project, e, d.local)?;
    let argv = remote::argv(&target, &p.command, false);
    let info = state
        .terminals
        .spawn(
            state,
            SpawnSpec {
                kind: TerminalKind::Command,
                title: format!("Deploy {} → {}", p.sha8, e.name),
                project_id: Some(project.id.clone()),
                cwd: project.root.clone(),
                argv,
                env: vec![],
                cols: None,
                rows: None,
                meta: json!({ "env": e.name, "action": "deploy", "sha": p.sha, "target": target.label() }),
            },
        )
        .await?;
    reservation.fulfil(&info.id);
    tracing::info!(project = %project.id, env = %e.name, sha = %p.sha8, target = %target.label(), "deploy started");
    state.events.notify("info", &format!("Deploying {} to {}", p.sha8, e.name));
    state.events.emit("env.health", Some(&project.id), json!({ "env": e.name, "deploying": info.id }));

    // Follow the deploy: report the outcome, then re-check health and version.
    let st = state.clone();
    let tid = info.id.clone();
    let pid = project.id.clone();
    let env_name = e.name.clone();
    let sha8 = p.sha8.clone();
    tokio::spawn(async move {
        let code = match st.terminals.exit_watch(&tid) {
            Some(mut rx) => loop {
                let cur = rx.borrow_and_update().clone();
                if let Some(x) = cur {
                    break x.code;
                }
                if rx.changed().await.is_err() {
                    let last = rx.borrow().clone();
                    break last.and_then(|x| x.code);
                }
            },
            None => None,
        };
        {
            let mut deploys = st.apps.envs.deploys.lock();
            if deploys.get(&(pid.clone(), env_name.clone())) == Some(&tid) {
                deploys.remove(&(pid.clone(), env_name.clone()));
            }
        }
        match code {
            Some(0) => st.events.notify("success", &format!("Deployed {sha8} to {env_name}")),
            Some(c) => st.events.notify("error", &format!("Deploy of {sha8} to {env_name} failed (exit {c})")),
            None => st.events.notify("warning", &format!("Deploy of {sha8} to {env_name} ended")),
        }
        let Some(project) = st.projects.get(&pid) else { return };
        let Ok(env) = envs::find(&project, &env_name).cloned() else { return };
        envs::check(&st, &project, &env).await;
        if code == Some(0) && env.version.is_some() {
            let _ = envs::probe_version(&st, &project, &env).await;
        }
    });
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(sha: &str) -> VersionInfo {
        VersionInfo { sha: Some(sha.into()), raw: None, checked_at: 0, source: "command", error: None }
    }

    #[test]
    fn ref_gate() {
        assert_eq!(gate_ref(None, Some("feature"), None).status, GateStatus::Skip);
        assert_eq!(gate_ref(Some("main"), Some("main"), Some(true)).status, GateStatus::Pass);
        assert_eq!(gate_ref(Some("main"), Some("main"), Some(false)).status, GateStatus::Fail);
        assert_eq!(gate_ref(Some("main"), Some("main"), None).status, GateStatus::Warn);
        let g = gate_ref(Some("main"), Some("feature/x"), Some(true));
        assert_eq!(g.status, GateStatus::Fail);
        assert!(g.detail.contains("feature/x"));
        assert_eq!(gate_ref(Some("main"), None, None).status, GateStatus::Fail);
    }

    #[test]
    fn pipeline_gate() {
        let ci = |s: &str| CiStatus { status: s.into(), pipeline_id: Some(42), web_url: Some("https://gitlab.com/p/-/pipelines/42".into()), sha: None, git_ref: None };
        assert_eq!(gate_pipeline(false, Err("x".into())).status, GateStatus::Skip);
        let g = gate_pipeline(true, Ok(Some(ci("success"))));
        assert_eq!(g.status, GateStatus::Pass);
        assert_eq!(g.url.as_deref(), Some("https://gitlab.com/p/-/pipelines/42"));
        assert_eq!(gate_pipeline(true, Ok(Some(ci("running")))).status, GateStatus::Fail);
        assert_eq!(gate_pipeline(true, Ok(Some(ci("failed")))).status, GateStatus::Fail);
        assert_eq!(gate_pipeline(true, Ok(None)).status, GateStatus::Fail);
        assert_eq!(gate_pipeline(true, Err("gitlab is not configured".into())).status, GateStatus::Fail);
    }

    #[test]
    fn after_gate() {
        let sha = "4b8e2508c0ffee00112233445566778899aabbcc";
        assert_eq!(gate_after(None, false, None, sha).status, GateStatus::Skip);
        assert_eq!(gate_after(Some("staging"), false, None, sha).status, GateStatus::Fail);
        assert_eq!(gate_after(Some("staging"), true, None, sha).status, GateStatus::Warn);
        assert_eq!(gate_after(Some("staging"), true, Some(&version("4b8e2508")), sha).status, GateStatus::Pass);
        assert_eq!(gate_after(Some("staging"), true, Some(&version("5babfd54")), sha).status, GateStatus::Fail);
        let unknown = VersionInfo { sha: None, ..version("x") };
        assert_eq!(gate_after(Some("staging"), true, Some(&unknown), sha).status, GateStatus::Warn);
    }

    #[test]
    fn confirmations() {
        assert!(check_confirmation(Confirm::None, "production", &Value::Null).is_ok());
        assert!(check_confirmation(Confirm::Click, "staging", &json!(true)).is_ok());
        assert!(check_confirmation(Confirm::Click, "staging", &json!(false)).is_err());
        assert!(check_confirmation(Confirm::Click, "staging", &Value::Null).is_err());
        assert!(check_confirmation(Confirm::Typed, "production", &json!("production")).is_ok());
        assert!(check_confirmation(Confirm::Typed, "production", &json!(true)).is_err());
        assert!(check_confirmation(Confirm::Typed, "production", &json!("Production")).is_err());
    }

    #[test]
    fn rev_and_ref_validation() {
        assert!(valid_rev("4b8e2508"));
        assert!(!valid_rev("--output=/tmp/x"));
        assert!(!valid_rev("HEAD"));
        assert!(valid_ref("main") && valid_ref("release/1.2"));
        assert!(!valid_ref("-x") && !valid_ref("a..b") && !valid_ref("a b"));
    }

    #[tokio::test]
    async fn resolves_shas_in_a_scratch_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a"), "1").unwrap();
        run(&["add", "a"]);
        run(&["commit", "-q", "-m", "first"]);
        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("a"), "2").unwrap();
        run(&["commit", "-q", "-am", "second"]);
        let head = resolve_sha(root, None).await.unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(resolve_sha(root, Some(&head[..8])).await.unwrap(), head);
        assert!(resolve_sha(root, Some("deadbeef")).await.is_err());
        assert!(resolve_sha(root, Some("--all")).await.is_err());
        assert_eq!(on_branch(root, &head, "main").await, Some(false));
        assert_eq!(on_branch(root, &head, "feature").await, Some(true));
        assert_eq!(on_branch(root, &head, "nope").await, None);
    }
}
