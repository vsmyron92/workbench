//! REST under `/api/projects/{pid}/data/`, and the hook scripts post samples to.
//!
//! * `GET ` → `{series, plots, sources, limits}` (everything the viewer shows).
//! * `PUT series {name, kind?, unit?, label?, group?, offset?, maxPoints?, rate?}`: create or
//!   change a series; `POST series/clear {names?, group?}`, `POST series/delete {names}`.
//! * `POST append {series, …}` (see `AppendReq::from_json`): samples or events for one series;
//!   `{batch: [...]}` for several.
//! * `POST view {series: [names], from, to, buckets}` → what a plot of that width draws;
//!   `POST stats {series, from?, to?}`; `POST export {series, from?, to?}` → CSV.
//! * `GET plots`, `POST plots {title?, series?|lanes?, …}` (a new plot; lanes made from the
//!   series when only they are given), `PUT plots/{id}`, `DELETE plots/{id}`.
//! * `GET|POST sources`, `POST sources/{id}/stop`, `DELETE sources/{id}`: followed files.
//! * `POST /api/hooks/data/{terminalId}/append`: the same append, for a script running in a
//!   hosted session (`Authorization: Bearer $WORKBENCH_AGENT_TOKEN`); the project is the
//!   session's.
//!
//! Agents (in-process callers) may do all of it: filling the viewer is what they are for.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::plots::{self, Lane, PlotConfig, Trace};
use super::sources;
use super::store::{AppendReq, ClearTarget, DefineOpts, MAX_BATCH, clean_names};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// A request that carries samples may be large (a signed-in device's).
const BODY_LIMIT: usize = 64 * 1024 * 1024;
/// A script's post: smaller, and read before it is believed (see `hook_append`).
const HOOK_LIMIT: usize = 16 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    let b = "/api/projects/{pid}/data";
    Router::new()
        .route(b, get(snapshot))
        .route(&format!("{b}/series"), put(series_put))
        .route(&format!("{b}/series/clear"), post(series_clear))
        .route(&format!("{b}/series/delete"), post(series_delete))
        .route(&format!("{b}/append"), post(append).layer(DefaultBodyLimit::max(BODY_LIMIT)))
        .route(&format!("{b}/view"), post(view))
        .route(&format!("{b}/stats"), post(stats))
        .route(&format!("{b}/export"), post(export))
        .route(&format!("{b}/plots"), get(plots_list).post(plots_create))
        .route(&format!("{b}/plots/{{id}}"), put(plots_put).delete(plots_delete))
        .route(&format!("{b}/sources"), get(sources_list).post(sources_start))
        .route(&format!("{b}/sources/{{id}}/stop"), post(sources_stop))
        .route(&format!("{b}/sources/{{id}}"), delete(sources_remove))
        .route("/api/hooks/data/{tid}/append", post(hook_append).layer(DefaultBodyLimit::max(HOOK_LIMIT)))
}

fn project(state: &AppState, pid: &str) -> ApiResult<()> {
    state.projects.require(pid).map(|_| ())
}

/// The series a request names: once each, at most `MAX_NAMES`.
fn names_of(v: &Value, key: &str) -> ApiResult<Vec<String>> {
    let list: Vec<String> = match v.get(key) {
        Some(Value::Array(l)) => l.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    clean_names(list)
}

fn num(v: &Value, key: &str) -> ApiResult<Option<f64>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => x.as_f64().filter(|f| f.is_finite()).map(Some).ok_or_else(|| ApiError::bad_request(format!("{key} is a number of seconds"))),
    }
}

// ---------------------------------------------------------------- series

async fn snapshot(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    Ok(Json(state.data.snapshot(&state, &pid)))
}

async fn series_put(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    let name = body.get("name").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("name: the series to create or change"))?;
    Ok(Json(state.data.define(&state, &pid, name, &DefineOpts::from_json(&body)?)?))
}

async fn series_clear(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    let names = names_of(&body, "names")?;
    let group = body.get("group").and_then(Value::as_str).map(str::trim);
    let target = match (names.is_empty(), group, body.get("all").and_then(Value::as_bool) == Some(true)) {
        (false, _, _) => ClearTarget::Names(&names),
        (true, Some(g), _) if !g.is_empty() => ClearTarget::Group(g),
        (true, None, true) => ClearTarget::All,
        _ => return Err(ApiError::bad_request("names (a list), a group (not empty), or all: true: what to clear")),
    };
    let cleared = state.data.clear(&state, &pid, target);
    Ok(Json(json!({ "cleared": cleared })))
}

async fn series_delete(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    let names = names_of(&body, "names")?;
    if names.is_empty() {
        return Err(ApiError::bad_request("names: the series to delete"));
    }
    Ok(Json(json!({ "deleted": state.data.delete(&state, &pid, &names) })))
}

/// `{batch: [...]}` or one request.
pub(super) fn append_all(state: &AppState, pid: &str, body: &Value) -> ApiResult<Value> {
    let items: Vec<&Value> = match body.get("batch").and_then(Value::as_array) {
        Some(l) => l.iter().collect(),
        None => vec![body],
    };
    if items.is_empty() {
        return Err(ApiError::bad_request("batch is empty"));
    }
    if items.len() > MAX_BATCH {
        return Err(ApiError::bad_request(format!("at most {MAX_BATCH} requests in a batch")));
    }
    // Every item is read before any is applied, so a malformed one changes nothing. Items are then
    // applied in order: one the series refuses (time going backwards) stops the batch there, and
    // the ones before it stay: the error says which.
    let reqs: Vec<AppendReq> = items
        .into_iter()
        .enumerate()
        .map(|(i, v)| AppendReq::from_json(v).map_err(|e| in_item(e, i, v.get("series").and_then(Value::as_str), 0)))
        .collect::<Result<_, _>>()?;
    let mut reports = vec![];
    for (i, r) in reqs.into_iter().enumerate() {
        let name = r.name.clone();
        match state.data.append(state, pid, r) {
            Ok(rep) => reports.push(rep),
            Err(e) => return Err(in_item(e, i, Some(&name), reports.len())),
        }
    }
    Ok(json!({ "appended": reports }))
}

/// An error of one item of a batch, naming it (and how many items before it were applied).
fn in_item(e: ApiError, index: usize, name: Option<&str>, applied: usize) -> ApiError {
    let name = name.unwrap_or("?");
    let tail = if applied > 0 { format!(" ({applied} earlier item(s) were applied)") } else { String::new() };
    ApiError::new(e.status, e.code, format!("item {} ({name}): {}{tail}", index + 1, e.message))
}

async fn append(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    Ok(Json(append_all(&state, &pid, &body)?))
}

/// A script in a hosted session posts samples as it captures them. The route is outside the device
/// guard (agent tokens authenticate in their handlers), so the body is taken as bytes and parsed only
/// after the token has been checked: an unauthenticated post costs a buffer, not a document tree.
async fn hook_append(State(state): State<AppState>, Path(tid): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult<Json<Value>> {
    if state.auth.agent_from_headers(&headers).as_deref() != Some(tid.as_str()) && !state.auth.is_master_bearer(&headers) {
        return Err(ApiError::unauthorized("invalid agent token"));
    }
    let pid = state
        .terminals
        .info(&tid)
        .and_then(|i| i.project_id)
        .ok_or_else(|| ApiError::not_found("this terminal has no project"))?;
    project(&state, &pid)?;
    let body: Value = serde_json::from_slice(&body).map_err(|e| ApiError::bad_request(format!("the body is JSON: {e}")))?;
    Ok(Json(append_all(&state, &pid, &body)?))
}

async fn view(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    let names = names_of(&body, "series")?;
    let (from, to) = (num(&body, "from")?.unwrap_or(f64::NEG_INFINITY), num(&body, "to")?.unwrap_or(f64::INFINITY));
    let buckets = body.get("buckets").and_then(Value::as_u64).unwrap_or(1000) as usize;
    Ok(Json(json!({ "views": state.data.views(&state, &pid, &names, from, to, buckets) })))
}

async fn stats(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    let names = names_of(&body, "series")?;
    Ok(Json(json!({ "stats": state.data.stats(&state, &pid, &names, num(&body, "from")?, num(&body, "to")?) })))
}

async fn export(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<impl IntoResponse> {
    project(&state, &pid)?;
    let names = names_of(&body, "series")?;
    if names.is_empty() {
        return Err(ApiError::bad_request("series: the series to export"));
    }
    let csv = state.data.export_csv(&state, &pid, &names, num(&body, "from")?, num(&body, "to")?)?;
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"workbench-data.csv\"")], csv))
}

// ---------------------------------------------------------------- plots

async fn plots_list(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    Ok(Json(json!({ "plots": state.data.plots(&state, &pid) })))
}

fn lane_from(l: &Value) -> ApiResult<Lane> {
    let mut lane = Lane { title: l.get("title").and_then(Value::as_str).unwrap_or("").to_string(), ..Lane::default() };
    let trace_of = |t: &Value| -> ApiResult<Trace> {
        match t {
            Value::String(s) => Ok(Trace { series: s.clone(), ..Trace::default() }),
            Value::Object(_) => Ok(Trace {
                series: t.get("series").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("a trace is {series, color?, hidden?, style?}"))?.to_string(),
                color: t.get("color").and_then(Value::as_u64).unwrap_or(0).min(255) as u8,
                hidden: t.get("hidden").and_then(Value::as_bool).unwrap_or(false),
                style: t.get("style").and_then(Value::as_str).unwrap_or("").to_string(),
            }),
            _ => Err(ApiError::bad_request("a trace is a series name or {series, color?, hidden?, style?}")),
        }
    };
    for key in ["series", "traces"] {
        if let Some(Value::Array(list)) = l.get(key) {
            for t in list {
                lane.traces.push(trace_of(t)?);
            }
        } else if let Some(Value::String(s)) = l.get(key) {
            lane.traces.push(Trace { series: s.clone(), ..Trace::default() });
        }
    }
    lane.min = num(l, "min")?;
    lane.max = num(l, "max")?;
    lane.log = l.get("log").and_then(Value::as_bool).unwrap_or(false);
    lane.weight = l.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
    Ok(lane)
}

/// A plot from what a sender gave, over `base` when it changes an existing one. `series` alone
/// makes lanes by unit and kind; `lanes` says them; `add` and `remove` change a plot's series.
pub(super) fn plot_from_spec(state: &AppState, pid: &str, id: &str, spec: &Value, base: Option<PlotConfig>) -> ApiResult<PlotConfig> {
    let existed = base.is_some();
    let mut p = base.unwrap_or_else(|| PlotConfig { id: id.to_string(), ..PlotConfig::default() });
    p.id = id.to_string();
    if let Some(t) = spec.get("title").and_then(Value::as_str) {
        p.title = t.to_string();
    }
    let info = |n: &str| state.data.kind_and_unit(state, pid, n);
    if let Some(Value::Array(lanes)) = spec.get("lanes") {
        p.lanes = lanes.iter().map(lane_from).collect::<ApiResult<_>>()?;
    } else if spec.get("series").is_some() {
        p.lanes = plots::auto_lanes(&names_of(spec, "series")?, &info);
    }
    let remove = names_of(spec, "remove")?;
    if !remove.is_empty() {
        for l in &mut p.lanes {
            l.traces.retain(|t| !remove.contains(&t.series));
        }
        p.lanes.retain(|l| !l.traces.is_empty());
        p.markers.retain(|m| !remove.contains(m));
    }
    let add = names_of(spec, "add")?;
    if !add.is_empty() {
        let have: Vec<String> = p.lanes.iter().flat_map(|l| l.traces.iter().map(|t| t.series.clone())).collect();
        let fresh: Vec<String> = add.into_iter().filter(|n| !have.contains(n)).collect();
        p.lanes.extend(plots::auto_lanes(&fresh, &info));
    }
    if let Some(s) = num(spec, "span")? {
        p.span = s;
    }
    if let Some(b) = spec.get("follow").and_then(Value::as_bool) {
        p.follow = b;
    }
    if let Some(b) = spec.get("linked").and_then(Value::as_bool) {
        p.linked = b;
    }
    if spec.get("markers").is_some() {
        p.markers = names_of(spec, "markers")?;
    }
    if !existed && p.lanes.is_empty() {
        return Err(ApiError::bad_request("a plot needs series: pass `series` (names) or `lanes`"));
    }
    Ok(p)
}

async fn plots_create(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<PlotConfig>> {
    project(&state, &pid)?;
    let mut spec = body.as_object().cloned().map(Value::Object).ok_or_else(|| ApiError::bad_request("the body is an object: {title?, series?, lanes?, …}"))?;
    let title = spec.get("title").and_then(Value::as_str).unwrap_or("Plot").to_string();
    let id = match spec.get("id").and_then(Value::as_str) {
        Some(i) => {
            if state.data.plot(&state, &pid, i).is_some_and(|p| p.id == i) {
                return Err(ApiError::conflict(format!("a plot {i} exists: change it with PUT plots/{i}")));
            }
            i.to_string()
        }
        None => state.data.free_plot_id(&state, &pid, &title),
    };
    spec["title"] = json!(title);
    let plot = plot_from_spec(&state, &pid, &id, &spec, None)?;
    Ok(Json(state.data.put_plot(&state, &pid, plot)?))
}

async fn plots_put(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>, Json(mut plot): Json<PlotConfig>) -> ApiResult<Json<PlotConfig>> {
    project(&state, &pid)?;
    plot.id = id;
    Ok(Json(state.data.put_plot(&state, &pid, plot)?))
}

async fn plots_delete(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    if !state.data.delete_plot(&state, &pid, &id) {
        return Err(ApiError::not_found(format!("no plot {id}")));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- sources

async fn sources_list(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    Ok(Json(json!({ "sources": state.data.sources.list(&pid) })))
}

async fn sources_start(State(state): State<AppState>, Path(pid): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    Ok(Json(sources::start(&state, &pid, sources::StartReq::from_json(&body)?)?))
}

async fn sources_stop(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    if !state.data.sources.stop(&pid, &id) {
        return Err(ApiError::not_found(format!("no source {id}")));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn sources_remove(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    project(&state, &pid)?;
    if !state.data.sources.remove(&pid, &id) {
        return Err(ApiError::not_found(format!("no source {id}")));
    }
    state.events.emit("data.sources", Some(&pid), json!({ "projectId": pid, "sources": state.data.sources.list(&pid) }));
    Ok(Json(json!({ "ok": true })))
}
