//! Jira Cloud (REST v3), available only when the site has Jira: search with JQL
//! (`/search/jql`, token-paginated), issue view with rendered HTML, edits in
//! markdown (converted to ADF), transitions, comments, assignment and creation.

use std::sync::LazyLock;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use regex::Regex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::client::{Api, Product, q};
use super::html::Rewrite;
use super::{attachments, jira_api, markdown, status};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// Fields a search returns (search/jql returns only `id` unless told otherwise).
pub(crate) const SEARCH_FIELDS: &str = "summary,status,assignee,priority,issuetype,updated,labels,project";
const ISSUE_FIELDS: &str =
    "summary,status,assignee,reporter,priority,labels,issuetype,created,updated,description,project,parent,duedate,resolution";

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]{0,30}-\d{1,12}$").unwrap());

/// Normalize and validate an issue key (`abc-1` → `ABC-1`); numeric ids are accepted too.
pub fn check_key(key: &str) -> Result<String, ApiError> {
    let k = key.trim().to_ascii_uppercase();
    if KEY.is_match(&k) || (!k.is_empty() && k.len() <= 20 && k.bytes().all(|b| b.is_ascii_digit())) {
        Ok(k)
    } else {
        Err(ApiError::bad_request(format!("invalid issue key {key:?}")))
    }
}

fn check_project_key(key: &str) -> Result<String, ApiError> {
    let k = key.trim().to_ascii_uppercase();
    if !k.is_empty() && k.len() <= 32 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        Ok(k)
    } else {
        Err(ApiError::bad_request(format!("invalid project key {key:?}")))
    }
}

// ---------------------------------------------------------------- shapes

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRef {
    pub account_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusRef {
    pub id: Option<String>,
    pub name: String,
    /// new | indeterminate | done
    pub category: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedRef {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub key: String,
    pub id: String,
    pub summary: String,
    pub status: Option<StatusRef>,
    pub assignee: Option<UserRef>,
    pub priority: Option<NamedRef>,
    pub issue_type: Option<NamedRef>,
    pub updated: Option<String>,
    pub labels: Vec<String>,
    pub project_key: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn user(v: &Value) -> Option<UserRef> {
    let id = s(v, "accountId")?;
    Some(UserRef { display_name: s(v, "displayName").unwrap_or_else(|| id.clone()), account_id: id })
}

pub(crate) fn status_ref(v: &Value) -> Option<StatusRef> {
    Some(StatusRef {
        id: s(v, "id"),
        name: s(v, "name")?,
        category: v.pointer("/statusCategory/key").and_then(Value::as_str).unwrap_or("indeterminate").to_string(),
    })
}

fn named(v: &Value) -> Option<NamedRef> {
    Some(NamedRef { id: s(v, "id"), name: s(v, "name")? })
}

pub(crate) fn summary_of(issue: &Value) -> IssueSummary {
    let f = issue.get("fields").cloned().unwrap_or(Value::Null);
    IssueSummary {
        key: s(issue, "key").unwrap_or_default(),
        id: s(issue, "id").unwrap_or_default(),
        summary: s(&f, "summary").unwrap_or_default(),
        status: f.get("status").and_then(status_ref),
        assignee: f.get("assignee").and_then(user),
        priority: f.get("priority").and_then(named),
        issue_type: f.get("issuetype").and_then(named),
        updated: s(&f, "updated"),
        labels: f
            .get("labels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        project_key: f.pointer("/project/key").and_then(Value::as_str).map(str::to_string),
    }
}

// ---------------------------------------------------------------- search

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchIn {
    pub jql: String,
    pub next_page_token: Option<String>,
    pub max_results: Option<u32>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOut {
    pub issues: Vec<IssueSummary>,
    pub next_page_token: Option<String>,
    pub is_last: bool,
}

pub async fn search(api: &Api, input: &SearchIn) -> Result<SearchOut, ApiError> {
    let jql = input.jql.trim();
    if jql.is_empty() {
        return Err(ApiError::bad_request("JQL is empty"));
    }
    let max = input.max_results.unwrap_or(50).clamp(1, 100);
    let mut url = api.url(&format!("/rest/api/3/search/jql?jql={}&fields={SEARCH_FIELDS}&maxResults={max}", q(jql)));
    if let Some(t) = input.next_page_token.as_deref().filter(|t| !t.is_empty()) {
        url.push_str(&format!("&nextPageToken={}", q(t)));
    }
    let v: Value = api.get(&url).await.map_err(|e| {
        if e.code == "bad_request" { ApiError::bad_request(format!("Invalid JQL: {}", e.message)) } else { e }
    })?;
    let issues = v.get("issues").and_then(Value::as_array).map(|a| a.iter().map(summary_of).collect()).unwrap_or_default();
    let next_page_token = s(&v, "nextPageToken");
    Ok(SearchOut {
        is_last: v.get("isLast").and_then(Value::as_bool).unwrap_or(next_page_token.is_none()),
        next_page_token,
        issues,
    })
}

// ---------------------------------------------------------------- issue

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionOut {
    pub id: String,
    pub name: String,
    pub to: Option<StatusRef>,
    pub has_screen: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentOut {
    pub id: String,
    pub author: Option<UserRef>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub html: String,
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Editable {
    pub summary: bool,
    pub description: bool,
    pub labels: bool,
    pub priority: bool,
    pub assignee: bool,
    pub priorities: Vec<NamedRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueOut {
    #[serde(flatten)]
    pub summary: IssueSummary,
    pub reporter: Option<UserRef>,
    pub created: Option<String>,
    pub resolution: Option<String>,
    pub due: Option<String>,
    pub project_name: Option<String>,
    pub parent: Option<Value>,
    /// Sanitized rendered description.
    pub description_html: String,
    /// The description as markdown (the starting point of an edit).
    pub description_markdown: String,
    /// Saving an edited markdown description would lose rich content (media, mentions…).
    pub description_lossy: bool,
    pub transitions: Vec<TransitionOut>,
    pub editable: Editable,
    pub comments: Vec<CommentOut>,
    pub comments_total: u64,
    pub web_url: String,
}

fn rewrite<'a>(api: &'a Api, project_id: Option<&str>) -> Rewrite<'a> {
    Rewrite { site: &api.site.base, product: Product::Jira, project_id: project_id.map(str::to_string) }
}

async fn comments_page(api: &Api, key: &str, project_id: Option<&str>, start_at: u64) -> Result<(Vec<CommentOut>, u64), ApiError> {
    let url = api.url(&format!("/rest/api/3/issue/{key}/comment?expand=renderedBody&orderBy=created&maxResults=100&startAt={start_at}"));
    let v: Value = api.get(&url).await?;
    let rw = rewrite(api, project_id);
    let comments = v
        .get("comments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|c| CommentOut {
                    id: s(c, "id").unwrap_or_default(),
                    author: c.get("author").and_then(user),
                    created: s(c, "created"),
                    updated: s(c, "updated"),
                    html: rw.render(c.get("renderedBody").and_then(Value::as_str).unwrap_or("")),
                    markdown: c.get("body").map(markdown::adf_to_markdown).unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
    Ok((comments, total))
}

pub async fn get_issue(api: &Api, key: &str, project_id: Option<&str>) -> Result<IssueOut, ApiError> {
    let key = check_key(key)?;
    let url = api.url(&format!("/rest/api/3/issue/{key}?fields={ISSUE_FIELDS}&expand=renderedFields,transitions,editmeta"));
    let (issue, comments) = tokio::join!(api.get::<Value>(&url), comments_page(api, &key, project_id, 0));
    let issue = issue?;
    let (comments, comments_total) = comments.unwrap_or_default();
    let f = issue.get("fields").cloned().unwrap_or(Value::Null);
    let edit = issue.pointer("/editmeta/fields").cloned().unwrap_or(Value::Null);
    let can = |field: &str| edit.get(field).is_some();
    let priorities = edit
        .pointer("/priority/allowedValues")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(named).collect())
        .unwrap_or_default();
    let desc = f.get("description").cloned().unwrap_or(Value::Null);
    let rendered = issue.pointer("/renderedFields/description").and_then(Value::as_str).unwrap_or("");
    let key_out = s(&issue, "key").unwrap_or(key);
    Ok(IssueOut {
        summary: summary_of(&issue),
        reporter: f.get("reporter").and_then(user),
        created: s(&f, "created"),
        resolution: f.pointer("/resolution/name").and_then(Value::as_str).map(str::to_string),
        due: s(&f, "duedate"),
        project_name: f.pointer("/project/name").and_then(Value::as_str).map(str::to_string),
        parent: f.get("parent").filter(|p| !p.is_null()).map(|p| {
            json!({ "key": p.get("key"), "summary": p.pointer("/fields/summary") })
        }),
        description_html: rewrite(api, project_id).render(rendered),
        description_markdown: if desc.is_null() { String::new() } else { markdown::adf_to_markdown(&desc) },
        description_lossy: markdown::adf_is_lossy(&desc),
        transitions: issue
            .get("transitions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|t| t.get("isAvailable").and_then(Value::as_bool).unwrap_or(true))
                    .map(|t| TransitionOut {
                        id: s(t, "id").unwrap_or_default(),
                        name: s(t, "name").unwrap_or_default(),
                        to: t.get("to").and_then(status_ref),
                        has_screen: t.get("hasScreen").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        editable: Editable {
            summary: can("summary"),
            description: can("description"),
            labels: can("labels"),
            priority: can("priority"),
            assignee: can("assignee"),
            priorities,
        },
        comments,
        comments_total,
        web_url: format!("{}/browse/{key_out}", api.site.base),
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateIn {
    pub summary: Option<String>,
    /// Markdown; converted to ADF.
    pub description: Option<String>,
    pub labels: Option<Vec<String>>,
    pub priority_id: Option<String>,
}

pub async fn update_issue(state: &AppState, api: &Api, key: &str, input: UpdateIn) -> Result<(), ApiError> {
    let key = check_key(key)?;
    let mut fields = serde_json::Map::new();
    if let Some(sm) = input.summary {
        let sm = sm.trim().to_string();
        if sm.is_empty() || sm.chars().count() > 255 {
            return Err(ApiError::bad_request("the summary must be 1–255 characters"));
        }
        fields.insert("summary".into(), Value::String(sm));
    }
    if let Some(d) = input.description {
        fields.insert("description".into(), if d.trim().is_empty() { Value::Null } else { markdown::to_adf(&d) });
    }
    if let Some(l) = input.labels {
        if l.iter().any(|x| x.is_empty() || x.contains(char::is_whitespace)) {
            return Err(ApiError::bad_request("labels cannot be empty or contain spaces"));
        }
        fields.insert("labels".into(), json!(l));
    }
    if let Some(p) = input.priority_id {
        fields.insert("priority".into(), json!({ "id": p }));
    }
    if fields.is_empty() {
        return Err(ApiError::bad_request("nothing to update"));
    }
    let url = api.url(&format!("/rest/api/3/issue/{key}"));
    api.send_no_content(Method::PUT, &url, &json!({ "fields": fields })).await?;
    state.events.emit("jira.issue", None, json!({ "key": key, "action": "updated" }));
    Ok(())
}

pub async fn transition(state: &AppState, api: &Api, key: &str, transition_id: &str, comment: Option<&str>) -> Result<(), ApiError> {
    let key = check_key(key)?;
    if transition_id.is_empty() || !transition_id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::bad_request("invalid transition id"));
    }
    let mut body = json!({ "transition": { "id": transition_id } });
    if let Some(c) = comment.filter(|c| !c.trim().is_empty()) {
        body["update"] = json!({ "comment": [{ "add": { "body": markdown::to_adf(c) } }] });
    }
    api.send_no_content(Method::POST, &api.url(&format!("/rest/api/3/issue/{key}/transitions")), &body).await?;
    state.events.emit("jira.issue", None, json!({ "key": key, "action": "transitioned" }));
    Ok(())
}

pub async fn add_comment(state: &AppState, api: &Api, key: &str, md: &str) -> Result<Value, ApiError> {
    let key = check_key(key)?;
    if md.trim().is_empty() {
        return Err(ApiError::bad_request("the comment is empty"));
    }
    let v: Value = api
        .send_json(Method::POST, &api.url(&format!("/rest/api/3/issue/{key}/comment")), &json!({ "body": markdown::to_adf(md) }))
        .await?;
    state.events.emit("jira.issue", None, json!({ "key": key, "action": "commented" }));
    Ok(json!({ "id": v.get("id").cloned().unwrap_or(Value::Null) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateIn {
    pub project_key: String,
    pub issue_type_id: Option<String>,
    /// Used when no id is given: matched by name (e.g. "Task").
    pub issue_type: Option<String>,
    pub summary: String,
    pub description: Option<String>,
    pub labels: Option<Vec<String>>,
    pub parent_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueTypeOut {
    pub id: String,
    pub name: String,
    pub subtask: bool,
    pub description: Option<String>,
}

pub async fn issue_types(api: &Api, project: &str) -> Result<Vec<IssueTypeOut>, ApiError> {
    let project = check_project_key(project)?;
    let v: Value = api.get(&api.url(&format!("/rest/api/3/issue/createmeta/{project}/issuetypes?maxResults=50"))).await?;
    let arr = v.get("issueTypes").or_else(|| v.get("values")).and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(arr
        .iter()
        .map(|t| IssueTypeOut {
            id: s(t, "id").unwrap_or_default(),
            name: s(t, "name").unwrap_or_default(),
            subtask: t.get("subtask").and_then(Value::as_bool).unwrap_or(false),
            description: s(t, "description"),
        })
        .collect())
}

pub async fn create_issue(state: &AppState, api: &Api, input: CreateIn) -> Result<Value, ApiError> {
    let project = check_project_key(&input.project_key)?;
    let summary = input.summary.trim().to_string();
    if summary.is_empty() || summary.chars().count() > 255 {
        return Err(ApiError::bad_request("the summary must be 1–255 characters"));
    }
    let type_id = match input.issue_type_id.filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            let types = issue_types(api, &project).await?;
            let want = input.issue_type.as_deref().unwrap_or("Task").to_lowercase();
            types
                .iter()
                .find(|t| t.name.to_lowercase() == want)
                .or_else(|| types.iter().find(|t| !t.subtask))
                .map(|t| t.id.clone())
                .ok_or_else(|| ApiError::bad_request(format!("project {project} has no issue type {want:?}")))?
        }
    };
    let mut fields = json!({ "project": { "key": project }, "issuetype": { "id": type_id }, "summary": summary });
    if let Some(d) = input.description.filter(|d| !d.trim().is_empty()) {
        fields["description"] = markdown::to_adf(&d);
    }
    if let Some(l) = input.labels.filter(|l| !l.is_empty()) {
        fields["labels"] = json!(l);
    }
    if let Some(p) = input.parent_key.filter(|p| !p.is_empty()) {
        fields["parent"] = json!({ "key": check_key(&p)? });
    }
    let v: Value = api.send_json(Method::POST, &api.url("/rest/api/3/issue"), &json!({ "fields": fields })).await?;
    let key = s(&v, "key").unwrap_or_default();
    state.events.emit("jira.issue", None, json!({ "key": key, "action": "created" }));
    Ok(json!({ "id": v.get("id"), "key": key, "webUrl": format!("{}/browse/{key}", api.site.base) }))
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectQuery {
    project_id: Option<String>,
}

pub async fn myself_handler(State(state): State<AppState>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    let v: Value = api.get(&api.url("/rest/api/3/myself")).await?;
    Ok(Json(json!({
        "accountId": v.get("accountId"),
        "displayName": v.get("displayName"),
        "emailAddress": v.get("emailAddress"),
    })))
}

pub async fn projects_handler(State(state): State<AppState>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<Vec<Value>>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    let mut out = vec![];
    let mut start = 0;
    loop {
        let v: Value = api.get(&api.url(&format!("/rest/api/3/project/search?startAt={start}&maxResults=50&orderBy=name"))).await?;
        let vals = v.get("values").and_then(Value::as_array).cloned().unwrap_or_default();
        let n = vals.len();
        out.extend(vals.iter().map(|p| json!({ "id": p.get("id"), "key": p.get("key"), "name": p.get("name") })));
        if n == 0 || v.get("isLast").and_then(Value::as_bool).unwrap_or(true) || out.len() >= 500 {
            break;
        }
        start += n;
    }
    Ok(Json(out))
}

pub async fn search_get_handler(State(state): State<AppState>, Query(input): Query<SearchIn>) -> ApiResult<Json<SearchOut>> {
    let api = jira_api(&state, input.project_id.as_deref()).await?;
    Ok(Json(search(&api, &input).await?))
}

pub async fn search_post_handler(
    State(state): State<AppState>,
    Query(pq): Query<ProjectQuery>,
    Json(mut input): Json<SearchIn>,
) -> ApiResult<Json<SearchOut>> {
    if input.project_id.is_none() {
        input.project_id = pq.project_id;
    }
    let api = jira_api(&state, input.project_id.as_deref()).await?;
    Ok(Json(search(&api, &input).await?))
}

pub async fn issue_handler(State(state): State<AppState>, Path(key): Path<String>, Query(pq): Query<ProjectQuery>) -> ApiResult<Json<IssueOut>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    Ok(Json(get_issue(&api, &key, pq.project_id.as_deref()).await?))
}

pub async fn update_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<UpdateIn>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    update_issue(&state, &api, &key, body).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransitionIn {
    transition_id: String,
    comment: Option<String>,
}

pub async fn transition_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<TransitionIn>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    transition(&state, &api, &key, &body.transition_id, body.comment.as_deref()).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentsQuery {
    project_id: Option<String>,
    start_at: Option<u64>,
}

pub async fn comments_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(cq): Query<CommentsQuery>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, cq.project_id.as_deref()).await?;
    let key = check_key(&key)?;
    let (comments, total) = comments_page(&api, &key, cq.project_id.as_deref(), cq.start_at.unwrap_or(0)).await?;
    Ok(Json(json!({ "comments": comments, "total": total })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CommentIn {
    markdown: String,
}

pub async fn add_comment_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<CommentIn>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    Ok(Json(add_comment(&state, &api, &key, &body.markdown).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssignIn {
    /// An account id, `"me"`, or null to unassign.
    account_id: Option<String>,
}

pub async fn assign_handler(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<AssignIn>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    let key = check_key(&key)?;
    let account = match body.account_id.as_deref() {
        Some("me") => {
            let st = status::check(&state, pq.project_id.as_deref(), false).await?;
            Some(st.user.map(|u| u.account_id).ok_or_else(|| ApiError::upstream("cannot tell who you are on this site"))?)
        }
        Some(a) if !a.is_empty() => Some(a.to_string()),
        _ => None,
    };
    api.send_no_content(Method::PUT, &api.url(&format!("/rest/api/3/issue/{key}/assignee")), &json!({ "accountId": account }))
        .await?;
    state.events.emit("jira.issue", None, json!({ "key": key, "action": "assigned" }));
    Ok(Json(json!({ "ok": true })))
}

pub async fn create_handler(
    State(state): State<AppState>,
    Query(pq): Query<ProjectQuery>,
    Json(body): Json<CreateIn>,
) -> ApiResult<Json<Value>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    Ok(Json(create_issue(&state, &api, body).await?))
}

pub async fn createmeta_handler(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(pq): Query<ProjectQuery>,
) -> ApiResult<Json<Vec<IssueTypeOut>>> {
    let api = jira_api(&state, pq.project_id.as_deref()).await?;
    Ok(Json(issue_types(&api, &project).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttachmentQuery {
    project_id: Option<String>,
    thumb: Option<String>,
}

pub async fn attachment_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(aq): Query<AttachmentQuery>,
) -> ApiResult<Response> {
    if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::bad_request("invalid attachment id"));
    }
    let api = jira_api(&state, aq.project_id.as_deref()).await?;
    let thumb = aq.thumb.as_deref().is_some_and(|t| t == "1");
    let path = if thumb { "thumbnail" } else { "content" };
    let url = api.url(&format!("/rest/api/3/attachment/{path}/{id}"));
    let key = format!("{}|jira|{id}|{path}", api.site.base);
    // Jira attachments are immutable: an id always names the same bytes.
    attachments::proxy(&api, &state.atlassian.attachments, &url, key, None, false, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_keys() {
        assert_eq!(check_key("abc-12").unwrap(), "ABC-12");
        assert_eq!(check_key("10042").unwrap(), "10042");
        for bad in ["", "ABC", "ABC-", "-1", "AB C-1", "ABC-1/transitions", "../1"] {
            assert!(check_key(bad).is_err(), "{bad}");
        }
        assert!(check_project_key("SHOP").is_ok());
        assert!(check_project_key("HV/AC").is_err());
    }

    #[test]
    fn summarizes_search_results() {
        let v = json!({"key":"FRGE-2263","id":"1","fields":{"summary":"S","status":{"id":"6","name":"Closed","statusCategory":{"key":"done"}},"assignee":{"accountId":"a1","displayName":"Vicky"},"priority":{"id":"4","name":"Minor"},"issuetype":{"id":"1","name":"Bug"},"labels":["x"],"project":{"key":"FRGE"}}});
        let s = summary_of(&v);
        assert_eq!(s.key, "FRGE-2263");
        assert_eq!(s.status.as_ref().unwrap().category, "done");
        assert_eq!(s.assignee.as_ref().unwrap().display_name, "Vicky");
        assert_eq!(s.project_key.as_deref(), Some("FRGE"));
        assert_eq!(s.labels, vec!["x"]);
        let empty = summary_of(&json!({"key":"A-1","id":"2"}));
        assert!(empty.status.is_none() && empty.assignee.is_none());
    }
}
