//! Background pipeline poller. While at least one browser tab is connected, it
//! checks the newest pipeline of each GitLab project's current branch and
//! default branch — every 30 s, or every 10 s while one is running or shortly
//! after a pipeline action — and emits `gitlab.pipeline` when one changes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use parking_lot::Mutex;

use super::client::{self, GlCtx};
use super::model::{Pipeline, is_active_status};
use crate::app::AppState;
use crate::error::ApiResult;
use crate::projects::Project;

const TICK: Duration = Duration::from_secs(2);
const SLOW: Duration = Duration::from_secs(30);
const FAST: Duration = Duration::from_secs(10);
/// After a pipeline action, poll fast for this long even if nothing runs yet.
const HOT_FOR: Duration = Duration::from_secs(600);
/// Back-off for projects that are not set up or not reachable with the token.
const BROKEN: Duration = Duration::from_secs(300);
const FAILING: Duration = Duration::from_secs(60);

/// What the poller last saw, shared with the REST handlers (they record their
/// own changes so the poller does not announce them twice).
#[derive(Default)]
pub struct PollState {
    /// `(project id, ref)` → `(pipeline id, status)`
    last: Mutex<HashMap<(String, String), (u64, String)>>,
    hot_until: Mutex<HashMap<String, Instant>>,
}

#[derive(Debug, PartialEq)]
pub enum Change {
    /// Nothing known before for this ref (startup, branch switch).
    First,
    Unchanged,
    /// A different pipeline or status; carries the previous status for the same pipeline.
    Changed { previous: Option<String> },
}

impl PollState {
    pub fn mark_hot(&self, project_id: &str) {
        self.hot_until.lock().insert(project_id.to_string(), Instant::now() + HOT_FOR);
    }

    fn is_hot(&self, project_id: &str) -> bool {
        self.hot_until.lock().get(project_id).is_some_and(|t| *t > Instant::now())
    }

    /// Record the newest pipeline of a ref.
    pub fn observe(&self, project_id: &str, git_ref: &str, id: u64, status: &str) -> Change {
        let key = (project_id.to_string(), git_ref.to_string());
        let mut last = self.last.lock();
        let change = match last.get(&key) {
            None => Change::First,
            Some((i, s)) if *i == id && s == status => Change::Unchanged,
            Some((i, s)) => Change::Changed { previous: (*i == id).then(|| s.clone()) },
        };
        last.insert(key, (id, status.to_string()));
        change
    }

    /// Record a status we learned from an action (retry, cancel, a new pipeline).
    pub fn note(&self, project_id: &str, git_ref: &str, id: u64, status: &str) {
        let key = (project_id.to_string(), git_ref.to_string());
        let mut last = self.last.lock();
        // Only move forward: an older pipeline must not replace the newest one.
        if last.get(&key).is_none_or(|(i, _)| *i <= id) {
            last.insert(key, (id, status.to_string()));
        }
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
            let projects: Vec<Arc<Project>> = state.projects.list().into_iter().filter(|p| p.gitlab().is_some()).collect();
            due.retain(|id, _| projects.iter().any(|p| &p.id == id));
            for project in projects {
                if due.get(&project.id).is_some_and(|t| *t > Instant::now()) {
                    continue;
                }
                let next = match poll_project(&state, project.clone()).await {
                    Ok(active) if active || state.gitlab.poll.is_hot(&project.id) => FAST,
                    Ok(_) => SLOW,
                    Err(e)
                        if e.code == "not_configured"
                            || matches!(e.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) =>
                    {
                        BROKEN
                    }
                    Err(e) => {
                        tracing::debug!("gitlab poll of {} failed: {e}", project.id);
                        FAILING
                    }
                };
                due.insert(project.id.clone(), Instant::now() + next);
            }
        }
    });
}

/// Poll one project; returns whether any watched pipeline is still active.
pub(super) async fn poll_project(state: &AppState, project: Arc<Project>) -> ApiResult<bool> {
    let ctx = client::ctx_for(state, project.clone()).await?;
    if state.gitlab.rate_low(&ctx.host) {
        return Ok(true); // keep the schedule short but skip this round
    }
    let mut refs: Vec<String> = vec![];
    if let Some(b) = crate::util::git::current_branch(&project.root).await {
        refs.push(b);
    }
    if let Some(d) = ctx.default_branch() {
        if !refs.iter().any(|r| r == d) {
            refs.push(d.to_string());
        }
    }
    let mut active = false;
    for r in refs {
        let Some(p) = newest(&ctx, &r).await? else { continue };
        active |= is_active_status(&p.status);
        if let Change::Changed { previous } = state.gitlab.poll.observe(&project.id, &r, p.id, &p.status) {
            state.gitlab.invalidate_summary(&project.id);
            super::emit_pipeline(state, &project.id, p.id, p.iid, &p.status, &p.git_ref, &p.sha, &p.web_url, previous.as_deref());
        }
    }
    Ok(active)
}

async fn newest(ctx: &GlCtx, git_ref: &str) -> ApiResult<Option<Pipeline>> {
    let q = [("ref", git_ref.to_string()), ("per_page", "1".to_string())];
    Ok(ctx.get_page::<Pipeline>(&ctx.purl("/pipelines"), &q).await?.items.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_report_changes_once() {
        let s = PollState::default();
        assert_eq!(s.observe("p", "main", 1, "running"), Change::First);
        assert_eq!(s.observe("p", "main", 1, "running"), Change::Unchanged);
        assert_eq!(s.observe("p", "main", 1, "success"), Change::Changed { previous: Some("running".into()) });
        assert_eq!(s.observe("p", "main", 2, "pending"), Change::Changed { previous: None });
        // An action already announced the new status: the poller stays quiet.
        s.note("p", "main", 2, "running");
        assert_eq!(s.observe("p", "main", 2, "running"), Change::Unchanged);
        // Older pipelines never replace the newest.
        s.note("p", "main", 1, "failed");
        assert_eq!(s.observe("p", "main", 2, "running"), Change::Unchanged);
    }

    #[test]
    fn hot_projects() {
        let s = PollState::default();
        assert!(!s.is_hot("p"));
        s.mark_hot("p");
        assert!(s.is_hot("p"));
    }
}
