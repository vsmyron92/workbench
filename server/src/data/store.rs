//! The data of every project: its series, its plots and what changed since the viewers last
//! heard. Samples live in memory only (a capture is rolling data, not a record); what a
//! project keeps on disk, in `data_dir/data/<project>.json`, is the *definitions* (names, kinds,
//! units, offsets) and the plots, so a restart brings the same plots back, empty, and the capture
//! that fills them again needs no setup.
//!
//! Everything that writes goes through [`DataState`], so the rules (time order, the memory
//! budget, what is announced) hold for every sender: an agent's tool call, a script posting from
//! its terminal, a followed file, the browser.

use std::collections::{BTreeSet, HashMap};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::plots::{self, MAX_PLOTS, PlotConfig};
use super::series::{self, Appended, Kind, Meta, Series, check_times, clean_text, valid_name};
use crate::app::AppState;
use crate::error::ApiError;

/// Series a project holds.
pub const MAX_SERIES: usize = 256;
/// Samples held across every project: past this the biggest series lose their oldest tenth.
pub const MAX_TOTAL_POINTS: usize = 30_000_000;
/// Samples (or events) one request may carry.
pub const MAX_REQUEST_POINTS: usize = 2_000_000;
/// What a view or an export of one series returns at most.
pub const MAX_VIEW_BUCKETS: usize = 4_000;
pub const MAX_EXPORT_ROWS: usize = 2_000_000;
/// Series one request may name (views, statistics, exports, clears), and items in one batch.
pub const MAX_NAMES: usize = 64;
pub const MAX_BATCH: usize = 256;

/// `names` once each, in order, at most [`MAX_NAMES`]: what every request that names series goes through.
pub fn clean_names(names: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut out: Vec<String> = Vec::with_capacity(names.len().min(MAX_NAMES));
    for n in names {
        if !out.contains(&n) {
            if out.len() >= MAX_NAMES {
                return Err(ApiError::bad_request(format!("at most {MAX_NAMES} series in one request")));
            }
            out.push(n);
        }
    }
    Ok(out)
}

/// Which series a clear acts on: said, never assumed.
pub enum ClearTarget<'a> {
    Names(&'a [String]),
    Group(&'a str),
    All,
}

#[derive(Default)]
struct ProjectData {
    loaded: bool,
    series: Vec<Series>,
    plots: Vec<PlotConfig>,
    /// Series whose samples changed since the last announcement.
    dirty: BTreeSet<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    version: u32,
    series: Vec<Meta>,
    plots: Vec<Value>,
}

#[derive(Default)]
pub struct DataState {
    projects: Mutex<HashMap<String, ProjectData>>,
    pub(crate) sources: super::sources::Sources,
}

/// What a sender may say about a series next to its samples.
#[derive(Debug, Clone, Default)]
pub struct DefineOpts {
    pub kind: Option<Kind>,
    pub unit: Option<String>,
    pub label: Option<String>,
    pub group: Option<String>,
    pub offset: Option<f64>,
    pub max_points: Option<usize>,
    /// Samples per second; 0 forgets it.
    pub rate: Option<f64>,
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

impl DefineOpts {
    pub fn from_json(v: &Value) -> Result<DefineOpts, ApiError> {
        let kind = match v.get("kind").and_then(Value::as_str) {
            Some(k) => Some(Kind::parse(k).ok_or_else(|| ApiError::bad_request(format!("a kind of analog, digital or events, not {k:?}")))?),
            None => None,
        };
        let num = |k: &str| -> Result<Option<f64>, ApiError> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(x) => x.as_f64().map(Some).ok_or_else(|| ApiError::bad_request(format!("{k} is a number"))),
            }
        };
        Ok(DefineOpts {
            kind,
            unit: str_of(v, "unit"),
            label: str_of(v, "label").or_else(|| str_of(v, "description")),
            group: str_of(v, "group"),
            offset: num("offset")?,
            max_points: num("maxPoints")?.map(|n| n.max(0.0) as usize),
            rate: num("rate")?,
        })
    }

    /// Nothing is said: the request only carries samples.
    pub fn is_empty(&self) -> bool {
        self.kind.is_none() && self.unit.is_none() && self.label.is_none() && self.group.is_none() && self.offset.is_none() && self.max_points.is_none() && self.rate.is_none()
    }

    fn apply(&self, s: &mut Series) -> Result<(), ApiError> {
        let mut m = s.meta.clone();
        if let Some(k) = self.kind {
            m.kind = k;
        }
        if let Some(u) = &self.unit {
            m.unit = clean_text("a unit", u, series::MAX_UNIT)?;
        }
        if let Some(l) = &self.label {
            m.label = clean_text("a label", l, series::MAX_LABEL)?;
        }
        if let Some(g) = &self.group {
            m.group = clean_text("a group", g, series::MAX_GROUP)?;
        }
        if let Some(o) = self.offset {
            m.offset = o;
        }
        if let Some(r) = self.rate {
            m.rate = if r == 0.0 { None } else { Some(r) };
        }
        if let Some(n) = self.max_points {
            m.max_points = n;
        } else if self.kind.is_some_and(|k| k != s.meta.kind) {
            // A new kind has its own default size.
            m.max_points = 0;
        }
        let m = m.validated()?;
        s.set_kind(m.kind);
        s.meta = m;
        s.trim_to_max();
        Ok(())
    }
}

/// What an append carries.
#[derive(Debug, Clone)]
pub enum Body {
    /// Nothing but the series' settings (creates it).
    None,
    /// Times and values, as given.
    Samples { t: Vec<f64>, v: Vec<f64> },
    /// Values at an even spacing: from `t0` (else where the series left off, else now).
    Values { values: Vec<f64>, t0: Option<f64>, dt: Option<f64> },
    /// Text at a time (`None`: now).
    Events(Vec<(Option<f64>, String)>),
}

#[derive(Debug, Clone)]
pub struct AppendReq {
    pub name: String,
    pub define: DefineOpts,
    /// Start from an empty series (a new capture).
    pub clear: bool,
    pub body: Body,
}

fn now_s() -> f64 {
    crate::util::now_ms() as f64 / 1000.0
}

/// One value of a request: a number, a boolean (a level), `null` (a failed reading), or a
/// numeric string.
fn value_of(x: &Value) -> Result<f64, ApiError> {
    match x {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Null => Ok(f64::NAN),
        Value::String(s) => {
            let s = s.trim();
            match s.to_ascii_lowercase().as_str() {
                "true" | "high" => Ok(1.0),
                "false" | "low" => Ok(0.0),
                "nan" | "" => Ok(f64::NAN),
                _ => s.parse::<f64>().map_err(|_| ApiError::bad_request(format!("a value that is a number, not {s:?}"))),
            }
        }
        _ => Err(ApiError::bad_request("a value that is a number, a boolean or null")),
    }
}

fn numbers(v: &Value, what: &str) -> Result<Vec<f64>, ApiError> {
    let arr = v.as_array().ok_or_else(|| ApiError::bad_request(format!("{what} is a list of numbers")))?;
    if arr.len() > MAX_REQUEST_POINTS {
        return Err(ApiError::bad_request(format!("at most {MAX_REQUEST_POINTS} samples in one request; send them in batches")));
    }
    arr.iter().map(value_of).collect()
}

/// A time given as one number (or a numeric string); `None` when absent or null.
fn scalar_time(a: &Value, key: &str) -> Result<Option<f64>, ApiError> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(Value::String(s)) => s.trim().parse::<f64>().map(Some).map_err(|_| ApiError::bad_request(format!("{key} is a number of seconds, not {s:?}"))),
        Some(_) => Err(ApiError::bad_request(format!("{key} is a number of seconds"))),
    }
}

impl AppendReq {
    /// The shapes senders use, all of them one request:
    /// `{series, t: [..], v: [..]}`, `{series, points: [[t, v], ..]}`,
    /// `{series, values: [..], t0?, dt? | rate?}`, `{series, t, v}` (one sample; no `t`: now),
    /// `{series, events: [{t?, text}, ..]}` or `{series, text, t?}` (one event). A request that mixes
    /// the shapes, or gives a field the wrong shape, is refused rather than guessed at.
    pub fn from_json(a: &Value) -> Result<AppendReq, ApiError> {
        let name = a.get("series").or_else(|| a.get("name")).and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("series: the name of the series to append to"))?;
        let name = valid_name(name)?;
        let define = DefineOpts::from_json(a)?;
        let clear = a.get("clear").and_then(Value::as_bool).unwrap_or(false);
        let given = |k: &str| a.get(k).filter(|v| !v.is_null());
        let numeric = given("points").is_some() || given("values").is_some() || given("v").is_some() || given("value").is_some();
        let eventful = given("events").is_some() || given("text").is_some();
        if numeric && eventful {
            return Err(ApiError::bad_request("a request carries samples (v, values, points) or events (events, text), not both: send two requests"));
        }
        let body = if let Some(evs) = given("events") {
            let list = evs.as_array().ok_or_else(|| ApiError::bad_request("events is a list of {t, text}"))?;
            if list.len() > MAX_REQUEST_POINTS {
                return Err(ApiError::bad_request("too many events in one request"));
            }
            let mut out = vec![];
            for e in list {
                let text = e.get("text").or_else(|| e.get("label")).or_else(|| e.get("message")).and_then(Value::as_str).or_else(|| e.as_str());
                let text = text.ok_or_else(|| ApiError::bad_request("an event is {t?, text}"))?;
                let t = match scalar_time(e, "t")? {
                    Some(t) => Some(t),
                    None => scalar_time(e, "time")?,
                };
                out.push((t, text.to_string()));
            }
            Body::Events(out)
        } else if let Some(text) = given("text") {
            let text = text.as_str().ok_or_else(|| ApiError::bad_request("text is the event's text"))?;
            Body::Events(vec![(scalar_time(a, "t")?, text.to_string())])
        } else if let Some(points) = given("points") {
            let arr = points.as_array().ok_or_else(|| ApiError::bad_request("points is a list of [t, value]"))?;
            if arr.len() > MAX_REQUEST_POINTS {
                return Err(ApiError::bad_request(format!("at most {MAX_REQUEST_POINTS} samples in one request; send them in batches")));
            }
            let (mut t, mut v) = (Vec::with_capacity(arr.len()), Vec::with_capacity(arr.len()));
            for p in arr {
                let pair = p.as_array().filter(|x| x.len() == 2).ok_or_else(|| ApiError::bad_request("a point is [t, value]"))?;
                t.push(pair[0].as_f64().ok_or_else(|| ApiError::bad_request("a point's t is a number of seconds"))?);
                v.push(value_of(&pair[1])?);
            }
            Body::Samples { t, v }
        } else if let Some(vals) = given("values").or_else(|| given("v").filter(|v| v.is_array())) {
            let values = numbers(vals, "values")?;
            if matches!(a.get("t"), Some(Value::Array(_))) {
                let t = numbers(&a["t"], "t")?;
                if t.len() != values.len() {
                    return Err(ApiError::bad_request(format!("t has {} entries but values has {}", t.len(), values.len())));
                }
                Body::Samples { t, v: values }
            } else {
                if scalar_time(a, "t")?.is_some() {
                    return Err(ApiError::bad_request("t is one number but there is a list of values: give a list of times, or t0 (the first time) with dt or rate"));
                }
                let dt = match given("dt") {
                    Some(d) => {
                        let d = d.as_f64().filter(|d| d.is_finite() && *d > 0.0).ok_or_else(|| ApiError::bad_request("dt is the seconds between samples, above zero"))?;
                        Some(d)
                    }
                    None => None,
                };
                Body::Values { values, t0: scalar_time(a, "t0")?, dt }
            }
        } else if let Some(v) = given("v").or_else(|| given("value")) {
            if matches!(a.get("t"), Some(Value::Array(_))) {
                return Err(ApiError::bad_request("t is a list but v is one value: give a list of values too"));
            }
            let value = value_of(v)?;
            match scalar_time(a, "t")? {
                Some(t) => Body::Samples { t: vec![t], v: vec![value] },
                None => Body::Values { values: vec![value], t0: None, dt: None },
            }
        } else {
            Body::None
        };
        Ok(AppendReq { name, define, clear, body })
    }
}

/// A request's body with its times settled and checked: what is about to be kept.
enum Resolved {
    None,
    Samples(Vec<f64>, Vec<f64>),
    Events(Vec<(f64, String)>),
}

/// Settle a body's times against a series that holds nothing newer than `base_end` and samples at
/// `rate`; every check that could refuse it is here, so nothing has changed when it does.
fn resolve_body(name: &str, body: Body, rate: Option<f64>, base_end: Option<f64>, now: f64) -> Result<Resolved, ApiError> {
    match body {
        Body::None => Ok(Resolved::None),
        Body::Samples { t, v } => {
            check_times(name, t.iter().copied(), base_end)?;
            Ok(Resolved::Samples(t, v))
        }
        Body::Values { values, t0, dt } => {
            if values.is_empty() {
                return Ok(Resolved::None);
            }
            let step = dt.or_else(|| rate.map(|r| 1.0 / r));
            if step.is_none() && values.len() > 1 {
                return Err(ApiError::bad_request(format!(
                    "{name}: {} values without times: give `t` (a time per value), `t0` with `dt`, or `rate` (samples per second)",
                    values.len()
                )));
            }
            let step = step.unwrap_or(0.0);
            let start = match (t0, base_end) {
                (Some(t0), _) => t0,
                // An even stream continues where it left off.
                (None, Some(end)) if step > 0.0 => end + step,
                // Taken just now: the last value is the newest.
                (None, _) => now - step * (values.len() - 1) as f64,
            };
            let t: Vec<f64> = (0..values.len()).map(|i| start + step * i as f64).collect();
            check_times(name, t.iter().copied(), base_end)?;
            Ok(Resolved::Samples(t, values))
        }
        Body::Events(list) => {
            // Stamped on arrival, but never before the newest event the series has.
            let stamp = now.max(base_end.unwrap_or(f64::MIN));
            let evs: Vec<(f64, String)> = list.into_iter().map(|(t, text)| (t.unwrap_or(stamp), text)).collect();
            check_times(name, evs.iter().map(|e| e.0), base_end)?;
            Ok(Resolved::Events(evs))
        }
    }
}

/// What an append did, for the sender.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendReport {
    pub series: String,
    pub taken: usize,
    pub dropped: usize,
    pub count: usize,
    pub t_end: Option<f64>,
    pub created: bool,
}

fn data_file(state: &AppState, pid: &str) -> std::path::PathBuf {
    state.paths.data_dir.join("data").join(format!("{pid}.json"))
}

impl DataState {
    /// Run `f` on a project's data, loading its definitions first when this is the first look.
    fn with<R>(&self, state: &AppState, pid: &str, f: impl FnOnce(&mut ProjectData) -> R) -> R {
        let mut all = self.projects.lock();
        let pd = all.entry(pid.to_string()).or_default();
        if !pd.loaded {
            pd.loaded = true;
            let file = data_file(state, pid);
            let saved: Saved = match crate::util::fs::read_json(&file) {
                Ok(v) => v.unwrap_or_default(),
                Err(e) => {
                    // Not overwritten by the next save: kept aside for whoever wants to look.
                    tracing::warn!(project = pid, "data viewer: {} is not readable ({e}); keeping it as .corrupt", file.display());
                    let _ = std::fs::rename(&file, file.with_extension("json.corrupt"));
                    Saved::default()
                }
            };
            for m in saved.series {
                if let Ok(m) = m.validated() {
                    if !pd.series.iter().any(|s| s.meta.name == m.name) {
                        pd.series.push(Series::new(m));
                    }
                }
            }
            pd.plots = plots::lenient(saved.plots);
        }
        f(pd)
    }

    fn save(&self, state: &AppState, pid: &str, pd: &ProjectData) {
        let saved = Saved {
            version: 1,
            series: pd.series.iter().map(|s| s.meta.clone()).collect(),
            plots: pd.plots.iter().filter_map(|p| serde_json::to_value(p).ok()).collect(),
        };
        let path = data_file(state, pid);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = crate::util::fs::write_json(&path, &saved) {
            tracing::warn!(project = pid, "data viewer: could not save definitions: {e}");
        }
    }

    fn announce_series(&self, state: &AppState, pid: &str, pd: &ProjectData) {
        state.events.emit("data.series", Some(pid), json!({ "projectId": pid, "series": pd.series.iter().map(Series::info).collect::<Vec<_>>() }));
    }

    fn announce_plots(&self, state: &AppState, pid: &str, pd: &ProjectData) {
        state.events.emit("data.plots", Some(pid), json!({ "projectId": pid, "plots": pd.plots }));
    }

    // ------------------------------------------------------------ reading

    pub fn snapshot(&self, state: &AppState, pid: &str) -> Value {
        let sources = self.sources.list(pid);
        self.with(state, pid, |pd| {
            json!({
                "series": pd.series.iter().map(Series::info).collect::<Vec<_>>(),
                "plots": pd.plots,
                "sources": sources,
                "limits": limits(),
            })
        })
    }

    pub fn series_infos(&self, state: &AppState, pid: &str) -> Vec<Value> {
        self.with(state, pid, |pd| pd.series.iter().map(Series::info).collect())
    }

    pub fn plots(&self, state: &AppState, pid: &str) -> Vec<PlotConfig> {
        self.with(state, pid, |pd| pd.plots.clone())
    }

    pub fn plot(&self, state: &AppState, pid: &str, id_or_title: &str) -> Option<PlotConfig> {
        self.with(state, pid, |pd| {
            pd.plots.iter().find(|p| p.id == id_or_title).or_else(|| pd.plots.iter().find(|p| p.title.eq_ignore_ascii_case(id_or_title))).cloned()
        })
    }

    /// Kind and unit of a series, when the project has it.
    pub fn kind_and_unit(&self, state: &AppState, pid: &str, name: &str) -> Option<(Kind, String)> {
        self.with(state, pid, |pd| pd.series.iter().find(|s| s.meta.name == name).map(|s| (s.meta.kind, s.meta.unit.clone())))
    }

    /// The newest time a series holds, as stored (before its offset): what an appended sample may not precede.
    pub fn end_time(&self, state: &AppState, pid: &str, name: &str) -> Option<f64> {
        self.with(state, pid, |pd| pd.series.iter().find(|s| s.meta.name == name).and_then(Series::end_time))
    }

    /// Several series' windows, for a plot: `[{name, kind, view…}]`.
    pub fn views(&self, state: &AppState, pid: &str, names: &[String], from: f64, to: f64, buckets: usize) -> Value {
        let buckets = buckets.clamp(1, MAX_VIEW_BUCKETS);
        self.with(state, pid, |pd| {
            let mut out = serde_json::Map::new();
            for n in names {
                let Some(s) = pd.series.iter().find(|s| &s.meta.name == n) else {
                    out.insert(n.clone(), json!({ "missing": true }));
                    continue;
                };
                let mut v = json!({
                    "kind": s.meta.kind,
                    "generation": s.generation,
                    "tStart": s.info()["tStart"],
                    "tEnd": s.info()["tEnd"],
                    "count": s.len(),
                });
                if s.meta.kind == Kind::Events {
                    let (list, n) = s.events_in(from, to);
                    v["events"] = json!(list.into_iter().map(|(t, text)| json!([t, text])).collect::<Vec<_>>());
                    v["n"] = json!(n);
                } else {
                    let view = s.view(from, to, buckets);
                    v["t"] = json!(view.t);
                    // A failed reading is `null`: JSON has no NaN.
                    v["v"] = json!(view.v.iter().map(|x| if x.is_finite() { json!(x) } else { Value::Null }).collect::<Vec<_>>());
                    v["n"] = json!(view.n);
                    v["thinned"] = json!(view.thinned);
                }
                out.insert(n.clone(), v);
            }
            Value::Object(out)
        })
    }

    pub fn stats(&self, state: &AppState, pid: &str, names: &[String], from: Option<f64>, to: Option<f64>) -> Value {
        self.with(state, pid, |pd| {
            let mut out = serde_json::Map::new();
            for n in names {
                match pd.series.iter().find(|s| &s.meta.name == n) {
                    Some(s) => {
                        let mut st = s.stats(from, to);
                        st["unit"] = json!(s.meta.unit);
                        out.insert(n.clone(), st);
                    }
                    None => {
                        out.insert(n.clone(), json!({ "missing": true }));
                    }
                }
            }
            Value::Object(out)
        })
    }

    /// The samples of series in `[from, to]` as CSV: one column of times and one per series
    /// (blank where a series has no sample at that time; a digital series has samples only at its
    /// transitions). Events go in one `text` column per events series.
    pub fn export_csv(&self, state: &AppState, pid: &str, names: &[String], from: Option<f64>, to: Option<f64>) -> Result<String, ApiError> {
        self.with(state, pid, |pd| {
            let (a, b) = (from.unwrap_or(f64::NEG_INFINITY), to.unwrap_or(f64::INFINITY));
            let mut cols: Vec<(String, Vec<f64>, Vec<String>)> = vec![];
            for n in names {
                let s = pd.series.iter().find(|s| &s.meta.name == n).ok_or_else(|| ApiError::not_found(format!("no series {n}")))?;
                let head = if s.meta.unit.is_empty() { n.clone() } else { format!("{n} [{}]", s.meta.unit) };
                if s.meta.kind == Kind::Events {
                    let (list, _) = s.events_in(a, b);
                    cols.push((head, list.iter().map(|e| e.0).collect(), list.into_iter().map(|e| e.1).collect()));
                } else {
                    let (t, v) = s.samples_in(a, b, (MAX_EXPORT_ROWS / names.len().max(1)).max(1));
                    cols.push((head, t, v.iter().map(|x| if x.is_finite() { format!("{x}") } else { String::new() }).collect()));
                }
            }
            let cell = |s: &str| {
                // A spreadsheet runs a cell starting with = + - @ as a formula; the names and event texts are text others typed.
                let s = if s.starts_with(['=', '+', '@']) || (s.starts_with('-') && s.parse::<f64>().is_err()) { format!("'{s}") } else { s.to_string() };
                if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
            };
            let mut out = String::new();
            out.push_str("time_s");
            for c in &cols {
                out.push(',');
                out.push_str(&cell(&c.0));
            }
            out.push('\n');
            let mut at = vec![0usize; cols.len()];
            let mut rows = 0;
            loop {
                let next = cols.iter().zip(&at).filter_map(|(c, &i)| c.1.get(i).copied()).fold(f64::INFINITY, f64::min);
                if !next.is_finite() || rows >= MAX_EXPORT_ROWS {
                    break;
                }
                out.push_str(&format!("{next}"));
                for (c, i) in cols.iter().zip(at.iter_mut()) {
                    out.push(',');
                    if c.1.get(*i).copied() == Some(next) {
                        out.push_str(&cell(&c.2[*i]));
                        *i += 1;
                    }
                }
                out.push('\n');
                rows += 1;
            }
            Ok(out)
        })
    }

    // ------------------------------------------------------------ writing

    /// Create or change a series' settings.
    pub fn define(&self, state: &AppState, pid: &str, name: &str, opts: &DefineOpts) -> Result<Value, ApiError> {
        let name = valid_name(name)?;
        let r = self.with(state, pid, |pd| -> Result<Value, ApiError> {
            ensure(pd, &name, opts)?;
            let info = pd.series.iter().find(|s| s.meta.name == name).map(Series::info).unwrap_or(Value::Null);
            self.save(state, pid, pd);
            self.announce_series(state, pid, pd);
            Ok(info)
        })?;
        Ok(r)
    }

    /// Take samples (creating the series on the way). Everything that could refuse the request is
    /// checked before anything changes: a request that fails leaves the series, its settings and its
    /// samples as they were, and a series it would have created does not exist.
    pub fn append(&self, state: &AppState, pid: &str, req: AppendReq) -> Result<AppendReport, ApiError> {
        let AppendReq { name, define, clear, body } = req;
        let report = self.with(state, pid, |pd| -> Result<AppendReport, ApiError> {
            let now = now_s();
            let idx = pd.series.iter().position(|s| s.meta.name == name);
            let events = matches!(body, Body::Events(_));
            // A new series that is sent events is an events series; an existing one keeps its kind unless
            // the sender says otherwise (a note into a current trace is refused, not turned into one).
            let mut opts = define;
            if idx.is_none() && opts.kind.is_none() && events {
                opts.kind = Some(Kind::Events);
            }
            if idx.is_none() && pd.series.len() >= MAX_SERIES {
                return Err(ApiError::bad_request(format!("at most {MAX_SERIES} series in a project; delete some first (data_series delete)")));
            }
            // The series as it will be, on a copy with no samples.
            let current = idx.map(|i| pd.series[i].meta.clone());
            let mut probe = Series::new(match &current {
                Some(m) => m.clone(),
                None => Meta { name: name.clone(), ..Meta::default() }.validated()?,
            });
            opts.apply(&mut probe)?;
            let kind = probe.meta.kind;
            match (events, kind) {
                (true, k) if k != Kind::Events => {
                    return Err(ApiError::bad_request(format!(
                        "{name} is a {} series: events go to a series of kind events (use another name, or data_series define it as events, which empties it)",
                        kind_word(k)
                    )));
                }
                (false, Kind::Events) if !matches!(body, Body::None) => {
                    return Err(ApiError::bad_request(format!("{name} is an events series: append `events` (time and text), not numbers")));
                }
                _ => {}
            }
            let kind_changes = current.as_ref().is_some_and(|m| m.kind != kind);
            let fresh = idx.is_none() || clear || kind_changes;
            let base_end = if fresh { None } else { idx.and_then(|i| pd.series[i].end_time()) };
            let resolved = resolve_body(&name, body, probe.meta.rate, base_end, now)?;

            // Nothing below can fail.
            let created = idx.is_none();
            let i = match idx {
                Some(i) => i,
                None => {
                    pd.series.push(Series::new(Meta { name: name.clone(), ..Meta::default() }.validated()?));
                    pd.series.len() - 1
                }
            };
            let s = &mut pd.series[i];
            opts.apply(s)?;
            if clear {
                s.clear();
            }
            let done: Appended = match resolved {
                Resolved::None => Appended::default(),
                Resolved::Samples(t, v) => s.push(&t, &v)?,
                Resolved::Events(e) => s.push_events(&e)?,
            };
            let report = AppendReport { series: name.clone(), taken: done.taken, dropped: done.dropped, count: s.len(), t_end: s.end_time().map(|t| t + s.meta.offset), created };
            pd.dirty.insert(name);
            if created || !opts.is_empty() {
                self.save(state, pid, pd);
                self.announce_series(state, pid, pd);
            } else if clear {
                self.announce_series(state, pid, pd);
            }
            Ok(report)
        })?;
        self.enforce_budget(pid);
        Ok(report)
    }

    /// Past the budget the biggest series lose their oldest tenth: the writer's own project first,
    /// so one project's capture never pushes another's out while it has data of its own to give.
    fn enforce_budget(&self, writer: &str) {
        let mut all = self.projects.lock();
        let mut total: usize = all.values().flat_map(|p| p.series.iter()).map(Series::points).sum();
        while total > MAX_TOTAL_POINTS {
            let own = all.get_mut(writer).and_then(|p| p.series.iter_mut().filter(|s| s.points() > 0).max_by_key(|s| s.points()));
            let big = match own {
                Some(b) => b,
                None => match all.values_mut().flat_map(|p| p.series.iter_mut()).filter(|s| s.points() > 0).max_by_key(|s| s.points()) {
                    Some(b) => b,
                    None => break,
                },
            };
            let before = big.points();
            big.shrink();
            let gone = before.saturating_sub(big.points());
            if gone == 0 {
                break;
            }
            total = total.saturating_sub(gone);
        }
    }

    pub fn clear(&self, state: &AppState, pid: &str, target: ClearTarget<'_>) -> Vec<String> {
        self.with(state, pid, |pd| {
            let mut cleared = vec![];
            for s in &mut pd.series {
                let hit = match &target {
                    ClearTarget::Names(n) => n.contains(&s.meta.name),
                    ClearTarget::Group(g) => s.meta.group == *g,
                    ClearTarget::All => true,
                };
                if hit {
                    s.clear();
                    cleared.push(s.meta.name.clone());
                }
            }
            if !cleared.is_empty() {
                self.announce_series(state, pid, pd);
            }
            cleared
        })
    }

    pub fn delete(&self, state: &AppState, pid: &str, names: &[String]) -> Vec<String> {
        self.with(state, pid, |pd| {
            let before = pd.series.len();
            let gone: Vec<String> = pd.series.iter().filter(|s| names.contains(&s.meta.name)).map(|s| s.meta.name.clone()).collect();
            pd.series.retain(|s| !names.contains(&s.meta.name));
            if pd.series.len() != before {
                self.save(state, pid, pd);
                self.announce_series(state, pid, pd);
            }
            gone
        })
    }

    pub fn put_plot(&self, state: &AppState, pid: &str, plot: PlotConfig) -> Result<PlotConfig, ApiError> {
        let mut plot = plot.validated()?;
        self.with(state, pid, |pd| {
            let kinds: HashMap<String, Kind> = pd.series.iter().map(|s| (s.meta.name.clone(), s.meta.kind)).collect();
            plot.assign_colors(&|n| kinds.get(n).copied());
            match pd.plots.iter_mut().find(|p| p.id == plot.id) {
                Some(slot) => *slot = plot.clone(),
                None => {
                    if pd.plots.len() >= MAX_PLOTS {
                        return Err(ApiError::bad_request(format!("at most {MAX_PLOTS} plots in a project; delete one first")));
                    }
                    pd.plots.push(plot.clone());
                }
            }
            self.save(state, pid, pd);
            self.announce_plots(state, pid, pd);
            Ok(plot)
        })
    }

    pub fn delete_plot(&self, state: &AppState, pid: &str, id: &str) -> bool {
        self.with(state, pid, |pd| {
            let before = pd.plots.len();
            pd.plots.retain(|p| p.id != id);
            let changed = pd.plots.len() != before;
            if changed {
                self.save(state, pid, pd);
                self.announce_plots(state, pid, pd);
            }
            changed
        })
    }

    /// An id for a new plot titled `title` that no plot of the project has.
    pub fn free_plot_id(&self, state: &AppState, pid: &str, title: &str) -> String {
        let base = plots::slug(title);
        self.with(state, pid, |pd| {
            let mut id = base.clone();
            let mut n = 2;
            while pd.plots.iter().any(|p| p.id == id) {
                let suffix = format!("-{n}");
                id = format!("{}{suffix}", base.chars().take(40 - suffix.len()).collect::<String>());
                n += 1;
            }
            id
        })
    }

    /// Announce the series that changed since the last call. Called by the ticker.
    pub fn flush_dirty(&self, state: &AppState) {
        let mut all = self.projects.lock();
        for (pid, pd) in all.iter_mut() {
            if pd.dirty.is_empty() {
                continue;
            }
            let names = std::mem::take(&mut pd.dirty);
            let infos: Vec<Value> = pd.series.iter().filter(|s| names.contains(&s.meta.name)).map(Series::info).collect();
            if !infos.is_empty() {
                state.events.emit("data.changed", Some(pid), json!({ "projectId": pid, "series": infos }));
            }
        }
    }

    /// Whether any series of any project has data (the ticker's idle check is the dirty sets).
    pub fn any_dirty(&self) -> bool {
        self.projects.lock().values().any(|p| !p.dirty.is_empty())
    }
}

/// The series called `name`, created from `opts` when it is new; its settings changed otherwise.
/// Returns whether it was created.
fn ensure(pd: &mut ProjectData, name: &str, opts: &DefineOpts) -> Result<bool, ApiError> {
    if let Some(s) = pd.series.iter_mut().find(|s| s.meta.name == name) {
        opts.apply(s)?;
        return Ok(false);
    }
    if pd.series.len() >= MAX_SERIES {
        return Err(ApiError::bad_request(format!("at most {MAX_SERIES} series in a project; delete some first (data_series delete)")));
    }
    let mut s = Series::new(Meta { name: name.to_string(), ..Meta::default() }.validated()?);
    opts.apply(&mut s)?;
    pd.series.push(s);
    Ok(true)
}

fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Analog => "analog",
        Kind::Digital => "digital",
        Kind::Events => "events",
    }
}

pub fn limits() -> Value {
    json!({
        "maxSeries": MAX_SERIES,
        "maxPlots": MAX_PLOTS,
        "defaultMaxPoints": series::DEFAULT_MAX_POINTS,
        "maxPoints": series::MAX_POINTS_LIMIT,
        "maxRequestPoints": MAX_REQUEST_POINTS,
        "totalPoints": MAX_TOTAL_POINTS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn app() -> crate::platform::testutil::TestApp {
        crate::platform::testutil::app().await
    }

    fn req(v: Value) -> AppendReq {
        AppendReq::from_json(&v).unwrap()
    }

    #[test]
    fn append_requests_take_every_shape() {
        let r = req(json!({ "series": "a", "t": [0, 1], "v": [1.5, null], "unit": "A", "kind": "analog" }));
        assert!(matches!(&r.body, Body::Samples { t, v } if t == &[0.0, 1.0] && v[0] == 1.5 && v[1].is_nan()));
        assert_eq!(r.define.unit.as_deref(), Some("A"));
        let r = req(json!({ "series": "a", "points": [[0, 1], [0.5, true]] }));
        assert!(matches!(&r.body, Body::Samples { v, .. } if v == &[1.0, 1.0]));
        let r = req(json!({ "series": "a", "values": [1, 2, 3], "t0": 10, "dt": 0.5 }));
        assert!(matches!(&r.body, Body::Values { t0: Some(t), dt: Some(d), .. } if *t == 10.0 && *d == 0.5));
        let r = req(json!({ "series": "a", "v": "3.3" }));
        assert!(matches!(&r.body, Body::Values { values, t0: None, .. } if values == &[3.3]));
        let r = req(json!({ "series": "m", "events": [{ "t": 2, "text": "boot" }, { "text": "late" }] }));
        // The kind is the series': a new one becomes events when it is appended to, an existing one is refused.
        assert_eq!(r.define.kind, None);
        assert!(matches!(&r.body, Body::Events(e) if e.len() == 2 && e[1].0.is_none()));
        let r = req(json!({ "series": "m", "text": "radio on", "t": 1.5 }));
        assert!(matches!(&r.body, Body::Events(e) if e[0].0 == Some(1.5)));
        assert!(matches!(req(json!({ "series": "a", "unit": "V" })).body, Body::None));
        // Mistakes are said.
        assert!(AppendReq::from_json(&json!({ "t": [1] })).unwrap_err().message.contains("series"));
        assert!(AppendReq::from_json(&json!({ "series": "a", "t": [1, 2], "values": [1] })).unwrap_err().message.contains("2 entries"));
        assert!(AppendReq::from_json(&json!({ "series": "a", "kind": "wave" })).is_err());
        assert!(AppendReq::from_json(&json!({ "series": "a", "points": [[1]] })).is_err());
        // Shapes that would be guessed at are refused instead.
        let bad = |v: Value| AppendReq::from_json(&v).unwrap_err().message;
        assert!(bad(json!({ "series": "a", "t": [1, 2], "v": 5 })).contains("one value"));
        assert!(bad(json!({ "series": "a", "t": "soon", "v": 5 })).contains("number of seconds"));
        assert!(bad(json!({ "series": "a", "values": 5 })).contains("list"));
        assert!(bad(json!({ "series": "a", "t": 3, "values": [1, 2] })).contains("t0"));
        assert!(bad(json!({ "series": "a", "v": 1, "text": "note" })).contains("not both"));
        assert!(bad(json!({ "series": "a", "values": [1], "events": [{ "text": "x" }] })).contains("not both"));
        assert!(bad(json!({ "series": "a", "values": [1], "dt": -1 })).contains("dt"));
    }

    #[tokio::test]
    async fn append_creates_series_and_checks_before_keeping() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        let r = d.append(s, "p", req(json!({ "series": "I", "unit": "A", "t": [0, 1, 2], "v": [1, 2, 3] }))).unwrap();
        assert!((r.created, r.taken, r.count, r.t_end) == (true, 3, 3, Some(2.0)));
        // A request that fails leaves nothing behind, a new series included.
        assert!(d.append(s, "p", req(json!({ "series": "Z", "unit": "x".repeat(30), "t": [0], "v": [0] }))).is_err());
        assert!(d.series_infos(s, "p").iter().all(|i| i["name"] != "Z"));
        let e = d.append(s, "p", req(json!({ "series": "I", "t": [1.0], "v": [9] }))).unwrap_err();
        assert!(e.message.contains("backwards"), "{}", e.message);
        // `clear` starts a capture over.
        let r = d.append(s, "p", req(json!({ "series": "I", "clear": true, "t": [0.0], "v": [7] }))).unwrap();
        assert_eq!(r.count, 1);
        let v = d.views(s, "p", &["I".into(), "nope".into()], -1.0, 10.0, 100);
        assert_eq!(v["I"]["v"], json!([7.0]));
        assert_eq!(v["nope"]["missing"], true);
    }

    fn infos(t: &crate::platform::testutil::TestApp, pid: &str) -> Vec<Value> {
        t.state.data.series_infos(&t.state, pid)
    }

    #[tokio::test]
    async fn a_request_that_fails_changes_nothing() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "p", req(json!({ "series": "I", "unit": "A", "t": [0, 1, 2], "v": [1, 2, 3] }))).unwrap();
        let before = infos(&t, "p");
        // A new series whose samples cannot be placed is not made.
        assert!(d.append(s, "p", req(json!({ "series": "new", "values": [1, 2, 3] }))).is_err());
        // A clear that goes with samples going backwards does not clear.
        assert!(d.append(s, "p", req(json!({ "series": "I", "clear": true, "t": [2, 1], "v": [0, 0] }))).is_err());
        // A kind change that goes with samples the new kind refuses does not change the kind.
        assert!(d.append(s, "p", req(json!({ "series": "I", "kind": "events", "values": [1, 2, 3] }))).is_err());
        // Settings that go with a bad body are not applied.
        assert!(d.append(s, "p", req(json!({ "series": "I", "unit": "V", "t": [0.5], "v": [1] }))).is_err());
        assert_eq!(infos(&t, "p"), before, "series, kinds, units and samples as they were");
        // And nothing was left to fill the quota.
        for i in 0..5 {
            assert!(d.append(s, "p", req(json!({ "series": format!("ghost{i}"), "values": [1, 2] }))).is_err());
        }
        assert_eq!(infos(&t, "p").len(), 1);
    }

    #[tokio::test]
    async fn a_series_keeps_its_kind_unless_told_otherwise() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "p", req(json!({ "series": "I_load", "t": [0, 1], "v": [1, 2] }))).unwrap();
        // A note into a current trace is refused: it is not turned into an events series.
        let e = d.append(s, "p", req(json!({ "series": "I_load", "text": "boot" }))).unwrap_err();
        assert!(e.message.contains("analog") && e.message.contains("events"), "{}", e.message);
        assert_eq!(infos(&t, "p")[0]["count"], 2);
        // A new series that is sent text is an events series.
        d.append(s, "p", req(json!({ "series": "notes", "text": "boot", "t": 0.5 }))).unwrap();
        assert_eq!(infos(&t, "p")[1]["kind"], "events");
        // And numbers into it are refused too.
        assert!(d.append(s, "p", req(json!({ "series": "notes", "t": [1], "v": [1] }))).unwrap_err().message.contains("events series"));
        // The sender may change the kind on purpose: that empties the series.
        d.append(s, "p", req(json!({ "series": "I_load", "kind": "digital", "t": [5], "v": [1] }))).unwrap();
        assert_eq!((infos(&t, "p")[0]["kind"].as_str(), infos(&t, "p")[0]["count"].as_u64()), (Some("digital"), Some(1)));
    }

    #[tokio::test]
    async fn names_are_capped_and_clears_are_said() {
        assert_eq!(clean_names(vec!["a".into(), "b".into(), "a".into()]).unwrap(), vec!["a", "b"]);
        let many: Vec<String> = (0..MAX_NAMES + 1).map(|i| format!("s{i}")).collect();
        assert!(clean_names(many).unwrap_err().message.contains("at most"));
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "p", req(json!({ "series": "a", "group": "g", "t": [0], "v": [1] }))).unwrap();
        d.append(s, "p", req(json!({ "series": "b", "t": [0], "v": [1] }))).unwrap();
        // A group clears its own series only, and an empty one is no group.
        assert_eq!(d.clear(s, "p", ClearTarget::Group("g")), vec!["a"]);
        assert!(d.clear(s, "p", ClearTarget::Names(&[])).is_empty());
        assert_eq!(d.clear(s, "p", ClearTarget::All).len(), 2);
    }

    #[tokio::test]
    async fn an_even_stream_continues_where_it_left_off() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "p", req(json!({ "series": "adc", "values": [0, 1, 2], "t0": 100.0, "rate": 10 }))).unwrap();
        let r = d.append(s, "p", req(json!({ "series": "adc", "values": [3, 4] }))).unwrap();
        assert!((r.t_end.unwrap() - 100.4).abs() < 1e-9, "{:?}", r.t_end);
        // Many values and no way to place them is an error that says what to add.
        let e = d.append(s, "p", req(json!({ "series": "fresh", "values": [1, 2, 3] }))).unwrap_err();
        assert!(e.message.contains("dt") && e.message.contains("rate"), "{}", e.message);
        // One value with no time is taken now.
        let r = d.append(s, "p", req(json!({ "series": "fresh2", "v": 4.2 }))).unwrap();
        assert!(r.t_end.unwrap() > series::WALL_CLOCK_MIN);
    }

    #[tokio::test]
    async fn definitions_and_plots_survive_a_restart_but_samples_do_not() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "keep", req(json!({ "series": "I", "unit": "A", "group": "run 1", "t": [0, 1], "v": [1, 2] }))).unwrap();
        let p = PlotConfig { id: "pw".into(), title: "Power".into(), lanes: vec![plots::Lane { traces: vec![plots::Trace { series: "I".into(), ..Default::default() }], ..Default::default() }], ..Default::default() };
        let stored = d.put_plot(s, "keep", p).unwrap();
        assert_eq!(stored.lanes[0].traces[0].color, 1, "colours are assigned once, on the way in");
        // A new state over the same data dir: what a restart sees.
        let fresh = DataState::default();
        let infos = fresh.series_infos(s, "keep");
        assert_eq!(infos.len(), 1);
        assert_eq!((infos[0]["name"].as_str(), infos[0]["unit"].as_str(), infos[0]["group"].as_str(), infos[0]["count"].as_u64()), (Some("I"), Some("A"), Some("run 1"), Some(0)));
        assert_eq!(fresh.plots(s, "keep")[0].id, "pw");
    }

    #[tokio::test]
    async fn plots_are_bounded_and_ids_stay_free() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        let mk = |id: &str| PlotConfig { id: id.into(), title: id.into(), ..Default::default() };
        for i in 0..MAX_PLOTS {
            d.put_plot(s, "many", mk(&format!("p{i}"))).unwrap();
        }
        assert!(d.put_plot(s, "many", mk("one-more")).unwrap_err().message.contains("at most"));
        // Replacing one is not adding.
        d.put_plot(s, "many", mk("p0")).unwrap();
        assert!(d.delete_plot(s, "many", "p1"));
        assert!(!d.delete_plot(s, "many", "p1"));
        assert_eq!(d.free_plot_id(s, "many", "p0"), "p0-2");
        assert_eq!(d.free_plot_id(s, "many", "Power Profile"), "power-profile");
    }

    #[tokio::test]
    async fn csv_export_merges_on_time_and_defuses_formulas() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        d.append(s, "x", req(json!({ "series": "=cmd", "t": [0, 2], "v": [1, 3], "unit": "V" }))).unwrap();
        d.append(s, "x", req(json!({ "series": "b", "t": [1, 2], "v": [10, 20] }))).unwrap();
        d.append(s, "x", req(json!({ "series": "marks", "events": [{ "t": 1, "text": "a,b" }] }))).unwrap();
        let csv = d.export_csv(s, "x", &["=cmd".into(), "b".into(), "marks".into()], None, None).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "time_s,'=cmd [V],b,marks");
        assert_eq!(lines[1], "0,1,,");
        assert_eq!(lines[2], "1,,10,\"a,b\"");
        assert_eq!(lines[3], "2,3,20,");
    }

    #[tokio::test]
    async fn changes_are_announced_once_per_tick() {
        let t = app().await;
        let (s, d) = (&t.state, &t.state.data);
        let mut rx = s.events.subscribe();
        d.append(s, "ev", req(json!({ "series": "a", "t": [0], "v": [1] }))).unwrap();
        d.append(s, "ev", req(json!({ "series": "a", "t": [1], "v": [2] }))).unwrap();
        assert!(d.any_dirty());
        d.flush_dirty(s);
        assert!(!d.any_dirty());
        let mut kinds = vec![];
        while let Ok(e) = rx.try_recv() {
            kinds.push(e.kind.clone());
        }
        // The new series' definition, then one data event for both appends.
        assert_eq!(kinds.iter().filter(|k| *k == "data.changed").count(), 1, "{kinds:?}");
        assert!(kinds.iter().any(|k| k == "data.series"));
    }
}
