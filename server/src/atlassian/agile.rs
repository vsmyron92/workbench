//! Jira Software boards (`/rest/agile/1.0`): boards, a board's columns (from its
//! configuration) and quick filters, sprints, and the issues on a board, in a sprint or
//! in the backlog. Moving a card between columns is a workflow transition
//! (`jira::transition`), picked by the UI from `issue_transitions`.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{Api, q};
use super::jira::{IssueSummary, SEARCH_FIELDS, TransitionOut, check_key, status_ref, summary_of};
use super::jira_api;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Issues fetched for one board view (pages of 100).
const MAX_BOARD_ISSUES: usize = 500;
const MAX_BOARDS: usize = 500;
const MAX_SPRINTS: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardOut {
    pub id: u64,
    pub name: String,
    /// scrum | kanban | simple
    #[serde(rename = "type")]
    pub kind: String,
    pub project_key: Option<String>,
    pub project_name: Option<String>,
}

fn board_of(v: &Value) -> Option<BoardOut> {
    Some(BoardOut {
        id: v.get("id")?.as_u64()?,
        name: v.get("name").and_then(Value::as_str).unwrap_or("Board").to_string(),
        kind: v.get("type").and_then(Value::as_str).unwrap_or("kanban").to_string(),
        project_key: v.pointer("/location/projectKey").and_then(Value::as_str).map(str::to_string),
        project_name: v
            .pointer("/location/projectName")
            .or_else(|| v.pointer("/location/displayName"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardsOut {
    pub boards: Vec<BoardOut>,
    pub truncated: bool,
}

/// Boards the account can see, optionally by (part of) their name or a project.
pub async fn boards(api: &Api, name: Option<&str>, project: Option<&str>) -> Result<BoardsOut, ApiError> {
    let mut filter = String::new();
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        filter.push_str(&format!("&name={}", q(n)));
    }
    if let Some(p) = project.map(str::trim).filter(|p| !p.is_empty()) {
        let p = p.to_ascii_uppercase();
        if p.len() > 32 || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(ApiError::bad_request(format!("invalid project key {p:?}")));
        }
        filter.push_str(&format!("&projectKeyOrId={p}"));
    }
    let mut out = vec![];
    let mut start = 0usize;
    loop {
        let v: Value = api.get(&api.url(&format!("/rest/agile/1.0/board?startAt={start}&maxResults=50{filter}"))).await?;
        let vals = v.get("values").and_then(Value::as_array).cloned().unwrap_or_default();
        let n = vals.len();
        out.extend(vals.iter().filter_map(board_of));
        let last = v.get("isLast").and_then(Value::as_bool).unwrap_or(true);
        if n == 0 || last {
            return Ok(BoardsOut { boards: out, truncated: false });
        }
        if out.len() >= MAX_BOARDS {
            return Ok(BoardsOut { boards: out, truncated: true });
        }
        start += n;
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnOut {
    pub name: String,
    pub status_ids: Vec<String>,
    pub min: Option<u32>,
    pub max: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickFilterOut {
    pub id: u64,
    pub name: String,
    pub jql: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDetail {
    #[serde(flatten)]
    pub board: BoardOut,
    pub columns: Vec<ColumnOut>,
    pub quick_filters: Vec<QuickFilterOut>,
    /// Scrum boards have sprints and a backlog.
    pub has_sprints: bool,
    pub web_url: String,
}

fn check_board(id: u64) -> Result<(), ApiError> {
    if id == 0 {
        return Err(ApiError::bad_request("invalid board id"));
    }
    Ok(())
}

pub async fn board(api: &Api, id: u64) -> Result<BoardDetail, ApiError> {
    check_board(id)?;
    let (board_url, cfg_url, qf_url) = (
        api.url(&format!("/rest/agile/1.0/board/{id}")),
        api.url(&format!("/rest/agile/1.0/board/{id}/configuration")),
        api.url(&format!("/rest/agile/1.0/board/{id}/quickfilter?maxResults=50")),
    );
    let (b, cfg, qf) = tokio::join!(api.get::<Value>(&board_url), api.get::<Value>(&cfg_url), api.get::<Value>(&qf_url));
    let board = board_of(&b?).ok_or_else(|| ApiError::upstream("Jira returned a board without an id"))?;
    let cfg = cfg?;
    let columns = cfg
        .pointer("/columnConfig/columns")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| ColumnOut {
                    name: c.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    status_ids: c
                        .get("statuses")
                        .and_then(Value::as_array)
                        .map(|s| s.iter().filter_map(|x| x.get("id").and_then(Value::as_str)).map(str::to_string).collect())
                        .unwrap_or_default(),
                    min: c.get("min").and_then(Value::as_u64).map(|n| n as u32),
                    max: c.get("max").and_then(Value::as_u64).map(|n| n as u32),
                })
                .collect()
        })
        .unwrap_or_default();
    // Quick filters are optional: a board without them (or an error) shows none.
    let quick_filters = qf
        .ok()
        .and_then(|v| v.get("values").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            Some(QuickFilterOut {
                id: f.get("id")?.as_u64()?,
                name: f.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                jql: f.get("jql").and_then(Value::as_str).unwrap_or("").to_string(),
                description: f.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()).map(str::to_string),
            })
        })
        .filter(|f| !f.jql.trim().is_empty())
        .collect();
    let has_sprints = board.kind == "scrum";
    let web_url = match &board.project_key {
        Some(k) => format!("{}/jira/software/projects/{k}/boards/{id}", api.site.base),
        None => format!("{}/secure/RapidBoard.jspa?rapidView={id}", api.site.base),
    };
    Ok(BoardDetail { board, columns, quick_filters, has_sprints, web_url })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintOut {
    pub id: u64,
    pub name: String,
    /// active | future | closed
    pub state: String,
    pub goal: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub complete_date: Option<String>,
}

fn check_states(states: &str) -> Result<String, ApiError> {
    let list: Vec<&str> = states.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if list.is_empty() {
        return Ok("active,future".into());
    }
    if list.iter().any(|s| !matches!(*s, "active" | "future" | "closed")) {
        return Err(ApiError::bad_request("sprint states are active, future and closed"));
    }
    Ok(list.join(","))
}

/// A board's sprints (newest last, as Jira lists them). Boards without sprints
/// (kanban) have none.
pub async fn sprints(api: &Api, board: u64, states: &str) -> Result<Vec<SprintOut>, ApiError> {
    check_board(board)?;
    let states = check_states(states)?;
    let mut out = vec![];
    let mut start = 0usize;
    loop {
        let url = api.url(&format!("/rest/agile/1.0/board/{board}/sprint?state={states}&startAt={start}&maxResults=50"));
        let v: Value = match api.get(&url).await {
            Ok(v) => v,
            // "The board does not support sprints" (kanban).
            Err(e) if e.code == "bad_request" && out.is_empty() => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let vals = v.get("values").and_then(Value::as_array).cloned().unwrap_or_default();
        let n = vals.len();
        out.extend(vals.iter().filter_map(|s| {
            let str_of = |k: &str| s.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string);
            Some(SprintOut {
                id: s.get("id")?.as_u64()?,
                name: str_of("name").unwrap_or_else(|| "Sprint".into()),
                state: str_of("state").unwrap_or_else(|| "future".into()),
                goal: str_of("goal"),
                start_date: str_of("startDate"),
                end_date: str_of("endDate"),
                complete_date: str_of("completeDate"),
            })
        }));
        if n == 0 || v.get("isLast").and_then(Value::as_bool).unwrap_or(true) || out.len() >= MAX_SPRINTS {
            return Ok(out);
        }
        start += n;
    }
}

/// Which issues of a board: all of them (kanban), one sprint's, or the backlog's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Board,
    Sprint(u64),
    Backlog,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardIssuesOut {
    pub issues: Vec<IssueSummary>,
    pub total: u64,
    pub truncated: bool,
}

pub async fn board_issues(api: &Api, board: u64, scope: Scope, jql: Option<&str>) -> Result<BoardIssuesOut, ApiError> {
    check_board(board)?;
    let base = match scope {
        Scope::Board => format!("/rest/agile/1.0/board/{board}/issue"),
        Scope::Sprint(s) if s > 0 => format!("/rest/agile/1.0/board/{board}/sprint/{s}/issue"),
        Scope::Sprint(_) => return Err(ApiError::bad_request("invalid sprint id")),
        Scope::Backlog => format!("/rest/agile/1.0/board/{board}/backlog"),
    };
    let jq = match jql.map(str::trim).filter(|j| !j.is_empty()) {
        Some(j) if j.len() > 4000 => return Err(ApiError::bad_request("the filter is too long")),
        Some(j) => format!("&jql={}", q(j)),
        None => String::new(),
    };
    let mut issues = vec![];
    let mut start = 0usize;
    loop {
        let url = api.url(&format!("{base}?startAt={start}&maxResults=100&fields={SEARCH_FIELDS}{jq}"));
        let v: Value = api.get(&url).await.map_err(|e| {
            if e.code == "bad_request" { ApiError::bad_request(format!("Jira refused the board query: {}", e.message)) } else { e }
        })?;
        let page = v.get("issues").and_then(Value::as_array).cloned().unwrap_or_default();
        let n = page.len();
        issues.extend(page.iter().map(summary_of));
        let total = v.get("total").and_then(Value::as_u64).unwrap_or(issues.len() as u64);
        if n == 0 || issues.len() as u64 >= total {
            return Ok(BoardIssuesOut { issues, total, truncated: false });
        }
        if issues.len() >= MAX_BOARD_ISSUES {
            return Ok(BoardIssuesOut { issues, total, truncated: true });
        }
        start += n;
    }
}

/// The transitions an issue can take now (to move a card to another column).
pub async fn issue_transitions(api: &Api, key: &str) -> Result<Vec<TransitionOut>, ApiError> {
    let key = check_key(key)?;
    let v: Value = api.get(&api.url(&format!("/rest/api/3/issue/{key}/transitions"))).await?;
    Ok(v.get("transitions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|t| t.get("isAvailable").and_then(Value::as_bool).unwrap_or(true))
                .map(|t| TransitionOut {
                    id: t.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    name: t.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                    to: t.get("to").and_then(status_ref),
                    has_screen: t.get("hasScreen").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default())
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardsQuery {
    project_id: Option<String>,
    name: Option<String>,
    project: Option<String>,
}

pub async fn boards_handler(State(state): State<AppState>, Query(bq): Query<BoardsQuery>) -> ApiResult<Json<BoardsOut>> {
    let api = jira_api(&state, bq.project_id.as_deref()).await?;
    Ok(Json(boards(&api, bq.name.as_deref(), bq.project.as_deref()).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoardQuery {
    project_id: Option<String>,
    state: Option<String>,
    sprint_id: Option<u64>,
    backlog: Option<bool>,
    jql: Option<String>,
}

pub async fn board_handler(State(state): State<AppState>, Path(id): Path<u64>, Query(bq): Query<BoardQuery>) -> ApiResult<Json<BoardDetail>> {
    let api = jira_api(&state, bq.project_id.as_deref()).await?;
    Ok(Json(board(&api, id).await?))
}

pub async fn sprints_handler(State(state): State<AppState>, Path(id): Path<u64>, Query(bq): Query<BoardQuery>) -> ApiResult<Json<Vec<SprintOut>>> {
    let api = jira_api(&state, bq.project_id.as_deref()).await?;
    Ok(Json(sprints(&api, id, bq.state.as_deref().unwrap_or("active,future")).await?))
}

pub async fn issues_handler(State(state): State<AppState>, Path(id): Path<u64>, Query(bq): Query<BoardQuery>) -> ApiResult<Json<BoardIssuesOut>> {
    let api = jira_api(&state, bq.project_id.as_deref()).await?;
    let scope = match (bq.backlog.unwrap_or(false), bq.sprint_id) {
        (true, _) => Scope::Backlog,
        (false, Some(s)) => Scope::Sprint(s),
        (false, None) => Scope::Board,
    };
    Ok(Json(board_issues(&api, id, scope, bq.jql.as_deref()).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyQuery {
    project_id: Option<String>,
}

pub async fn transitions_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(kq): Query<KeyQuery>,
) -> ApiResult<Json<Vec<TransitionOut>>> {
    let api = jira_api(&state, kq.project_id.as_deref()).await?;
    Ok(Json(issue_transitions(&api, &key).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boards_and_states() {
        let b = board_of(&json!({ "id": 7, "name": "WB", "type": "scrum", "location": { "projectKey": "WB", "displayName": "Workbench (WB)" } })).unwrap();
        assert_eq!((b.id, b.kind.as_str(), b.project_key.as_deref(), b.project_name.as_deref()), (7, "scrum", Some("WB"), Some("Workbench (WB)")));
        assert!(board_of(&json!({ "name": "no id" })).is_none());
        assert_eq!(check_states("").unwrap(), "active,future");
        assert_eq!(check_states("closed, active").unwrap(), "closed,active");
        assert!(check_states("active,&x=1").is_err());
        assert!(check_board(0).is_err());
    }
}
