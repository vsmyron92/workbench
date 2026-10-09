//! Background Actions watcher. While at least one browser tab is connected, it
//! checks the newest workflow runs of each GitHub repository's current branch and
//! default branch, and emits `github.run` when one appears or changes state. A project
//! with several repositories is watched repository by repository (`forge::repo_views`),
//! those not on GitHub not at all.
//!
//! With a token: every 30 s, or every 10 s while something runs or shortly after
//! an action here. Without one (60 requests an hour per IP, shared by every
//! project) it polls every 10 minutes (5 while something runs), only projects
//! someone looked at in the last `VIEWED_FOR`, and never when the quota is low.
//! Everything is keyed by `Project::scope_key`: the project id for its default
//! repository, `<id>@<repository>` for another.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use parking_lot::Mutex;

use super::ci::branch_runs;
use super::client::{self, GhCtx};
use super::model::is_active;
use crate::app::AppState;
use crate::error::ApiResult;
use crate::projects::Project;

const TICK: Duration = Duration::from_secs(2);
const SLOW: Duration = Duration::from_secs(30);
const FAST: Duration = Duration::from_secs(10);
const SLOW_ANON: Duration = Duration::from_secs(600);
pub(super) const FAST_ANON: Duration = Duration::from_secs(300);
/// After an action, poll fast for this long even if nothing runs yet.
const HOT_FOR: Duration = Duration::from_secs(600);
/// Back-off for projects that are not set up or not reachable.
const BROKEN: Duration = Duration::from_secs(300);
const FAILING: Duration = Duration::from_secs(60);
/// Runs watched per ref.
const PER_REF: u32 = 10;
/// Anonymous projects are polled only this long after the UI (or an agent)
/// last asked for their GitHub data. Longer than the UI's anonymous summary
/// refresh, so an open view keeps its project watched.
const VIEWED_FOR: Duration = Duration::from_secs(900);

/// What the poller last saw, shared with the REST handlers (they record their
/// own changes so the poller does not announce them twice).
#[derive(Default)]
pub struct PollState {
    /// `(repository scope key, ref)` → run id → state
    last: Mutex<HashMap<(String, String), HashMap<u64, String>>>,
    hot_until: Mutex<HashMap<String, Instant>>,
}

#[derive(Debug, PartialEq)]
pub enum Change {
    /// A run we had not seen on a ref we already watch.
    New,
    /// A different state; carries the previous one.
    Changed(String),
}

impl PollState {
    pub fn mark_hot(&self, project_id: &str) {
        self.hot_until.lock().insert(project_id.to_string(), Instant::now() + HOT_FOR);
    }

    fn is_hot(&self, project_id: &str) -> bool {
        self.hot_until.lock().get(project_id).is_some_and(|t| *t > Instant::now())
    }

    /// Record the newest runs of a ref; returns what changed since last time.
    /// The first observation of a ref only records (no events on startup).
    pub fn observe(&self, project_id: &str, git_ref: &str, runs: &[(u64, String)]) -> Vec<(u64, Change)> {
        let key = (project_id.to_string(), git_ref.to_string());
        let mut last = self.last.lock();
        let now: HashMap<u64, String> = runs.iter().cloned().collect();
        let out = match last.get(&key) {
            None => vec![],
            Some(prev) => {
                // Runs older than everything we watched are not new, just scrolled in.
                let oldest = prev.keys().min().copied().unwrap_or(0);
                runs.iter()
                    .filter_map(|(id, state)| match prev.get(id) {
                        Some(p) if p == state => None,
                        Some(p) => Some((*id, Change::Changed(p.clone()))),
                        None if *id > oldest => Some((*id, Change::New)),
                        None => None,
                    })
                    .collect()
            }
        };
        last.insert(key, now);
        out
    }

    /// Record a state we learned from an action (re-run, cancel).
    pub fn note(&self, project_id: &str, git_ref: &str, id: u64, state: &str) {
        let key = (project_id.to_string(), git_ref.to_string());
        if let Some(m) = self.last.lock().get_mut(&key) {
            m.insert(id, state.to_string());
        }
    }

    fn forget_project(&self, keep: &dyn Fn(&str) -> bool) {
        self.last.lock().retain(|(p, _), _| keep(p));
        self.hot_until.lock().retain(|p, _| keep(p));
    }
}

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut due: HashMap<String, Instant> = HashMap::new();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if state.events.ui_clients() == 0 {
                continue;
            }
            let projects: Vec<(String, Arc<Project>)> = crate::forge::repo_views(&state)
                .into_iter()
                .filter(|p| p.github().is_some())
                .map(|p| (p.scope_key(), p))
                .collect();
            due.retain(|key, _| projects.iter().any(|(k, _)| k == key));
            state.github.poll.forget_project(&|key| projects.iter().any(|(k, _)| k == key));
            for (scope, project) in projects {
                if due.get(&scope).is_some_and(|t| *t > Instant::now()) || !wants_poll(&state, &project) {
                    continue;
                }
                let next = match poll_project(&state, project.clone()).await {
                    Ok((active, anon)) => {
                        let hot = active || state.github.poll.is_hot(&scope);
                        match (hot, anon) {
                            (true, false) => FAST,
                            (false, false) => SLOW,
                            (true, true) => FAST_ANON,
                            (false, true) => SLOW_ANON,
                        }
                    }
                    Err(e)
                        if e.code == "not_configured"
                            || e.code == "rate_limited"
                            || matches!(e.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) =>
                    {
                        BROKEN
                    }
                    Err(e) => {
                        tracing::debug!("github poll of {scope} failed: {e}");
                        FAILING
                    }
                };
                due.insert(scope, Instant::now() + next);
            }
        }
    });
}

/// Whether the poller should watch this repository now. With a token always (a
/// 304 is free); anonymous requests all share one small per-IP quota, so only
/// while someone looks at it.
pub(super) fn wants_poll(state: &AppState, project: &Project) -> bool {
    let Some((host, _)) = project.github() else { return false };
    match client::conn_for(state, project, &host) {
        Ok(c) if c.is_anonymous() => state.github.viewed_within(&project.scope_key(), VIEWED_FOR),
        // A token, or a setup error that polling reports (and backs off from).
        _ => true,
    }
}

/// Poll one repository (a project seen through it); returns whether a watched run is
/// still active, and whether the connection is anonymous.
pub(super) async fn poll_project(state: &AppState, project: Arc<Project>) -> ApiResult<(bool, bool)> {
    let ctx = client::ctx_for(state, project.clone()).await?;
    let anon = ctx.is_anonymous();
    if ctx.rate_low() {
        return Ok((false, anon));
    }
    let mut refs: Vec<String> = vec![];
    if let Some(b) = crate::util::git::current_branch_logged(project.repo_dir()).await {
        refs.push(b);
    }
    if let Some(d) = ctx.default_branch() {
        if !refs.iter().any(|r| r == d) {
            refs.push(d.to_string());
        }
    }
    let mut active = false;
    for r in refs {
        active |= poll_ref(&ctx, &r).await?;
    }
    Ok((active, anon))
}

async fn poll_ref(ctx: &GhCtx, git_ref: &str) -> ApiResult<bool> {
    let runs = branch_runs(ctx, git_ref, PER_REF).await?;
    let seen: Vec<(u64, String)> = runs.iter().map(|r| (r.id, r.state.clone())).collect();
    let scope = ctx.project.scope_key();
    let changes = ctx.state.github.poll.observe(&scope, git_ref, &seen);
    if !changes.is_empty() {
        ctx.state.github.invalidate_summary(&scope);
    }
    for (id, change) in changes {
        let Some(run) = runs.iter().find(|r| r.id == id) else { continue };
        // Views refetch on the event: they must not get an older cached copy.
        ctx.forget_run(run.id, &run.head_sha);
        let previous = match change {
            Change::Changed(p) => Some(p),
            Change::New => None,
        };
        super::emit_run(&ctx.state, &ctx.project, run, "poll", previous.as_deref());
    }
    Ok(runs.iter().any(|r| is_active(&r.state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(v: &[(u64, &str)]) -> Vec<(u64, String)> {
        v.iter().map(|(i, s)| (*i, s.to_string())).collect()
    }

    #[test]
    fn observations_report_changes_once() {
        let s = PollState::default();
        assert!(s.observe("p", "main", &runs(&[(2, "running"), (1, "success")])).is_empty(), "first sight only records");
        assert!(s.observe("p", "main", &runs(&[(2, "running"), (1, "success")])).is_empty());
        let c = s.observe("p", "main", &runs(&[(3, "pending"), (2, "failed"), (1, "success")]));
        assert_eq!(c, vec![(3, Change::New), (2, Change::Changed("running".into()))]);
        // An action already announced the new state: the poller stays quiet.
        s.note("p", "main", 3, "running");
        assert!(s.observe("p", "main", &runs(&[(3, "running"), (2, "failed")])).is_empty());
        // A run that scrolls into view from below is not new.
        assert!(s.observe("p", "main", &runs(&[(3, "running"), (2, "failed"), (0, "success")])).is_empty());
    }

    #[test]
    fn hot_projects() {
        let s = PollState::default();
        assert!(!s.is_hot("p"));
        s.mark_hot("p");
        assert!(s.is_hot("p"));
        s.forget_project(&|p| p != "p");
        assert!(!s.is_hot("p"));
    }
}
