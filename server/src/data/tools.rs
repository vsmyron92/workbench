//! MCP tools: an agent fills the data viewer while it captures, and reads the data back.
//!
//! * `data_append`: samples or events into a series (created on the way);
//! * `data_series`: list, describe, clear and delete series;
//! * `data_plot`: create, change, open and delete plots (lanes of traces under one time axis);
//! * `data_follow`: have Workbench read a file a capture tool is writing, live;
//! * `data_query`: statistics and thinned samples back: what the user sees, as numbers.
//!
//! All are confined to the calling session's project (`McpCtx::project_for`). Filling the
//! viewer is what agents are for, so nothing here is refused to them; the only files read are
//! the project's own, by the rules every agent file read follows (`sources::resolve`).

use serde_json::{Value, json};

use super::plots::PlotConfig;
use super::routes::{append_all, plot_from_spec};
use super::sources::{self, StartReq};
use super::store::{ClearTarget, DefineOpts, clean_names};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, McpTool, ToolOutput, tool};

fn project(state: &AppState, ctx: &McpCtx, args: &Value) -> Result<String, ApiError> {
    let pid = ctx.project_for(args.get("projectId").and_then(Value::as_str))?;
    state.projects.require(&pid)?;
    Ok(pid)
}

/// The series an argument names: once each, at most `MAX_NAMES`.
fn names(args: &Value, key: &str) -> Result<Vec<String>, ApiError> {
    let list = match args.get(key) {
        Some(Value::Array(l)) => l.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    clean_names(list)
}

fn open_plot(state: &AppState, pid: &str, plot: &PlotConfig) {
    state.events.emit(
        "ui.open",
        None,
        json!({ "panel": "data.plot", "id": format!("data.plot:{pid}:{}", plot.id), "params": { "projectId": pid, "plotId": plot.id }, "title": plot.title }),
    );
}

const PROJECT_ID: &str = "Workbench project id; defaults to the calling session's project.";

pub fn tools() -> Vec<McpTool> {
    vec![append_tool(), series_tool(), plot_tool(), follow_tool(), query_tool()]
}

// ---------------------------------------------------------------- data_append

fn append_tool() -> McpTool {
    tool(
        "data_append",
        "Put measurements into Workbench's live data viewer, where the user watches them as plots while you capture. \
         A series is a named signal: `analog` (a current, a voltage, a probe waveform), `digital` (a logic level; \
         only transitions are kept, so a fast capture of a quiet line is cheap) or `events` (text at a time: a \
         decoded byte, a trigger, 'radio on'). The series is created by its first append; say `unit` (A, V, W…) and \
         `kind` then. Times are seconds. Ways to give samples, per series: \
         `{series, t: [..], v: [..]}` (a time per value); `{series, points: [[t, v], ..]}`; \
         `{series, values: [..], t0, dt}` or with `rate` (samples per second): an even stream from t0, continuing \
         where the series left off when t0 is omitted; one sample `{series, v}` (time: now); events \
         `{series, events: [{t, text}, ..]}` or `{series, text}` (time: now). Several series at once: `batch: [ {..}, {..} ]`. \
         Times must not go backwards within a series: start a new capture with `clear: true` (or another series). \
         `null` in `v` is a failed reading (a gap). A series keeps the newest `maxPoints` (default 1,000,000), \
         so a capture that runs for hours rolls instead of filling memory. For a large capture write a file and use \
         data_follow instead; for a script that captures, POST this same body to \
         `$WORKBENCH_URL/api/hooks/data/$WORKBENCH_TERMINAL_ID/append` with `Authorization: Bearer $WORKBENCH_AGENT_TOKEN`. \
         Make a plot of it with data_plot. A series keeps its kind: events into an analog series (or numbers into \
         an events one) are refused, never converted. A request that fails changes nothing.",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": PROJECT_ID },
            "series": { "type": "string", "description": "The series' name, e.g. 'I_load' or 'SPI MOSI'. Created if new." },
            "kind": { "type": "string", "enum": ["analog", "digital", "events"], "description": "Default analog; events when `events`/`text` is given." },
            "unit": { "type": "string", "description": "Unit of the values: A, mA, V, W, °C… (the viewer scales it: µA, mA, A)." },
            "label": { "type": "string", "description": "What it measures, where the probe sits." },
            "group": { "type": "string", "description": "A heading in the list: 'power run 3', 'logic capture'." },
            "offset": { "type": "number", "description": "Seconds added to every time when shown: lines up instruments that started apart." },
            "rate": { "type": "number", "description": "Samples per second of an even stream (with `values`)." },
            "maxPoints": { "type": "integer", "description": "Newest samples kept (default 1,000,000; events 20,000)." },
            "clear": { "type": "boolean", "description": "Empty the series first: a new capture." },
            "t": { "description": "Times in seconds: a number, or an array parallel to `v`." },
            "v": { "description": "A value, or an array of values (parallel to `t`, or evenly spaced with t0/dt/rate). null: failed reading. true/false for levels." },
            "values": { "type": "array", "items": {}, "description": "Values at an even spacing (with t0 and dt, or rate)." },
            "points": { "type": "array", "items": { "type": "array", "items": {}, "minItems": 2, "maxItems": 2 }, "description": "[[t, v], …]" },
            "t0": { "type": "number", "description": "Time of the first of `values`." },
            "dt": { "type": "number", "description": "Seconds between `values`." },
            "events": { "type": "array", "items": { "type": "object", "properties": { "t": { "type": "number" }, "text": { "type": "string" } }, "required": ["text"] } },
            "text": { "type": "string", "description": "One event's text (time: `t`, else now)." },
            "batch": { "type": "array", "items": { "type": "object" }, "description": "Several of these requests (at most 256), applied in order; one that is refused stops the batch there and the earlier ones stay." }
        } }),
        true,
        |state, ctx, args| async move {
            let pid = project(&state, &ctx, &args)?;
            let out = append_all(&state, &pid, &args)?;
            let mut lines = vec![];
            for r in out["appended"].as_array().cloned().unwrap_or_default() {
                let mut l = format!(
                    "{}: +{} (now {} held{})",
                    r["series"].as_str().unwrap_or("?"),
                    r["taken"],
                    r["count"],
                    r["tEnd"].as_f64().map(|t| format!(", newest at {t:.6} s")).unwrap_or_default()
                );
                if r["created"] == json!(true) {
                    l.push_str(" [new series]");
                }
                if r["dropped"].as_u64().unwrap_or(0) > 0 {
                    l.push_str(&format!(" [{} oldest rolled off]", r["dropped"]));
                }
                lines.push(l);
            }
            Ok(ToolOutput::Text(lines.join("\n")))
        },
    )
}

// ---------------------------------------------------------------- data_series

fn series_tool() -> McpTool {
    tool(
        "data_series",
        "The series of the data viewer. `action`: `list` (default: every series with kind, unit, how many samples, \
         the time span it covers and its newest value, plus the plots and followed files), `define` (create a series \
         or change its kind, unit, label, group, offset, rate or maxPoints without sending samples; changing the kind \
         empties it), `clear` (empty series by `names` or `group`, keeping the definition: before a new capture), \
         `delete` (remove series). Series are the project's: the user sees the same list.",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": PROJECT_ID },
            "action": { "type": "string", "enum": ["list", "define", "clear", "delete"] },
            "series": { "type": "string", "description": "define: the series' name." },
            "names": { "type": "array", "items": { "type": "string" }, "description": "clear, delete: which series." },
            "group": { "type": "string", "description": "define: the heading; clear: every series of this group." },
            "kind": { "type": "string", "enum": ["analog", "digital", "events"] },
            "unit": { "type": "string" },
            "label": { "type": "string" },
            "offset": { "type": "number" },
            "rate": { "type": "number" },
            "maxPoints": { "type": "integer" }
        } }),
        true,
        |state, ctx, args| async move {
            let pid = project(&state, &ctx, &args)?;
            match args.get("action").and_then(Value::as_str).unwrap_or("list") {
                "list" => {
                    let snap = state.data.snapshot(&state, &pid);
                    if snap["series"].as_array().is_none_or(Vec::is_empty) && snap["sources"].as_array().is_none_or(Vec::is_empty) {
                        return Ok(ToolOutput::Text(format!("Project {pid} has no series yet. Put samples in with data_append, or follow a file with data_follow.")));
                    }
                    Ok(ToolOutput::Json(json!({ "projectId": pid, "series": snap["series"], "plots": snap["plots"].as_array().map(|l| l.iter().filter_map(|p| serde_json::from_value::<PlotConfig>(p.clone()).ok()).map(|p| p.summary()).collect::<Vec<_>>()), "sources": snap["sources"] })))
                }
                "define" => {
                    let name = args.get("series").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("series: the name to define"))?;
                    let info = state.data.define(&state, &pid, name, &DefineOpts::from_json(&args)?)?;
                    Ok(ToolOutput::Json(info))
                }
                "clear" => {
                    let n = names(&args, "names")?;
                    let group = args.get("group").and_then(Value::as_str).map(str::trim).filter(|g| !g.is_empty());
                    let target = match (n.is_empty(), group) {
                        (false, _) => ClearTarget::Names(&n),
                        (true, Some(g)) => ClearTarget::Group(g),
                        (true, None) => return Err(ApiError::bad_request("names or a group: which series to clear (every series is refused: say them)")),
                    };
                    let cleared = state.data.clear(&state, &pid, target);
                    Ok(ToolOutput::Text(format!("Cleared {}.", if cleared.is_empty() { "nothing (no such series)".to_string() } else { cleared.join(", ") })))
                }
                "delete" => {
                    let n = names(&args, "names")?;
                    if n.is_empty() {
                        return Err(ApiError::bad_request("names: the series to delete"));
                    }
                    let gone = state.data.delete(&state, &pid, &n);
                    Ok(ToolOutput::Text(format!("Deleted {}.", if gone.is_empty() { "nothing (no such series)".to_string() } else { gone.join(", ") })))
                }
                other => Err(ApiError::bad_request(format!("action: list, define, clear or delete, not {other:?}"))),
            }
        },
    )
}

// ---------------------------------------------------------------- data_plot

fn plot_tool() -> McpTool {
    tool(
        "data_plot",
        "Plots of the data viewer: what the user watches live next to your session. A plot is **lanes** stacked \
         under one shared time axis, like an oscilloscope or a logic analyzer: each lane has its own vertical axis \
         and holds the traces that share it. `action`: `create`, `update`, `open`, `delete`, `list`. For `create` \
         give `title` and either `series: [names]` (lanes are made for you: analog series of one unit share a \
         lane, each digital line and each events series gets its own, in that order) or `lanes: [{title?, \
         series: [names], min?, max?, log?, weight?}]` to say them (power in µA beside mW wants separate lanes, and \
         current that spans decades wants `log: true`). `span` is the seconds in view while it follows the newest \
         samples (default 10; 0 = everything captured); `follow: false` leaves the view where the user put it; \
         `markers: [events series]` draws those events as vertical lines through every lane. A plot can be made \
         before its series have samples: make it when you start the capture, so the user sees it fill. `update` \
         changes any of that on an existing `plot` (id or title) and also takes `add: [names]` and `remove: \
         [names]`. A new plot opens in the user's Workbench window (`open: false` to not). Up to 8 analog traces get \
         the chart colours; digital lanes are unlimited (32 lanes).",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": PROJECT_ID },
            "action": { "type": "string", "enum": ["create", "update", "open", "delete", "list"] },
            "plot": { "type": "string", "description": "update, open, delete: the plot's id or title." },
            "title": { "type": "string" },
            "series": { "type": "array", "items": { "type": "string" }, "description": "Series to draw; lanes are chosen for you." },
            "lanes": { "type": "array", "items": { "type": "object", "properties": {
                "title": { "type": "string" },
                "series": { "type": "array", "items": { "type": "string" } },
                "min": { "type": "number" }, "max": { "type": "number" },
                "log": { "type": "boolean" },
                "weight": { "type": "number", "description": "Height relative to the other lanes." }
            }, "required": ["series"] } },
            "add": { "type": "array", "items": { "type": "string" } },
            "remove": { "type": "array", "items": { "type": "string" } },
            "markers": { "type": "array", "items": { "type": "string" } },
            "span": { "type": "number", "description": "Seconds in view while following (0: everything)." },
            "follow": { "type": "boolean" },
            "linked": { "type": "boolean", "description": "Pan and zoom together with the project's other linked plots (default true)." },
            "open": { "type": "boolean", "description": "Open it in the UI (default true for create)." }
        }, "required": ["action"] }),
        true,
        |state, ctx, args| async move {
            let pid = project(&state, &ctx, &args)?;
            let find = |key: &str| -> Result<PlotConfig, ApiError> {
                let want = args.get(key).and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("plot: the id or title of the plot"))?;
                state.data.plot(&state, &pid, want).ok_or_else(|| ApiError::not_found(format!("no plot {want} in project {pid}; data_plot list shows them")))
            };
            match args.get("action").and_then(Value::as_str).unwrap_or("list") {
                "list" => {
                    let list = state.data.plots(&state, &pid);
                    if list.is_empty() {
                        return Ok(ToolOutput::Text(format!("Project {pid} has no plots. Make one with data_plot create.")));
                    }
                    Ok(ToolOutput::Json(json!({ "projectId": pid, "plots": list.iter().map(PlotConfig::summary).collect::<Vec<_>>() })))
                }
                "create" => {
                    let title = args.get("title").and_then(Value::as_str).unwrap_or("Plot").to_string();
                    let id = state.data.free_plot_id(&state, &pid, &title);
                    let mut spec = args.clone();
                    if let Some(o) = spec.as_object_mut() {
                        o.insert("title".into(), json!(title));
                    }
                    let plot = state.data.put_plot(&state, &pid, plot_from_spec(&state, &pid, &id, &spec, None)?)?;
                    if args.get("open").and_then(Value::as_bool).unwrap_or(true) {
                        open_plot(&state, &pid, &plot);
                    }
                    Ok(ToolOutput::Json(json!({ "created": plot.summary() })))
                }
                "update" => {
                    let base = find("plot")?;
                    let plot = state.data.put_plot(&state, &pid, plot_from_spec(&state, &pid, &base.id.clone(), &args, Some(base))?)?;
                    if args.get("open").and_then(Value::as_bool).unwrap_or(false) {
                        open_plot(&state, &pid, &plot);
                    }
                    Ok(ToolOutput::Json(json!({ "updated": plot.summary() })))
                }
                "open" => {
                    let plot = find("plot")?;
                    open_plot(&state, &pid, &plot);
                    Ok(ToolOutput::Text(format!("Opened {} in the user's window.", plot.title)))
                }
                "delete" => {
                    let plot = find("plot")?;
                    state.data.delete_plot(&state, &pid, &plot.id);
                    Ok(ToolOutput::Text(format!("Deleted plot {}.", plot.title)))
                }
                other => Err(ApiError::bad_request(format!("action: create, update, open, delete or list, not {other:?}"))),
            }
        },
    )
}

// ---------------------------------------------------------------- data_follow

fn follow_tool() -> McpTool {
    tool(
        "data_follow",
        "Have Workbench read a file into the viewer while a capture tool is still writing it, or import a finished \
         one. Start the capture so it writes a file inside the project (sigrok-cli -O csv, a script's `tee`, an \
         instrument CLI's export; keep it in a normal folder like captures/, not a hidden one), then follow it: \
         series appear as rows arrive and the user's plots fill live. Formats: `csv` (comma, semicolon, tab or \
         white-space separated; a header line names the columns, and `Current [uA]` or `Time (ms)` give units), \
         `jsonl` (one JSON object per line), `vcd` (logic analyzer and simulator waveforms: signals become digital \
         series, buses numbers). A column called time/t/timestamp is the time (seconds, or `timeUnit`: ms, us, ns; \
         dates work); with none, give `rate` (samples per second) or rows are stamped as they arrive. Text cells \
         become events. Columns are analog; `digital: [names]` or `kind` says otherwise. A file that shrinks or is \
         replaced starts the capture over. `follow: false` imports to the end and stops. `action`: `start` \
         (default), `list`, `stop` (by `id`). Series are named after the columns (`prefix` goes before).",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": PROJECT_ID },
            "action": { "type": "string", "enum": ["start", "list", "stop"] },
            "path": { "type": "string", "description": "The file, relative to the project (it may not exist yet: it is waited for, up to a minute)." },
            "format": { "type": "string", "enum": ["auto", "csv", "jsonl", "vcd"], "description": "Default: from the extension." },
            "follow": { "type": "boolean", "description": "Keep reading what is appended (default true); false: import and stop." },
            "timeColumn": { "type": "string", "description": "The time column: its header name, or its 1-based number." },
            "timeUnit": { "type": "string", "enum": ["s", "ms", "us", "ns"], "description": "What the time column counts in (default: its header's unit, else seconds)." },
            "rate": { "type": "number", "description": "Samples per second, for a file with no time column." },
            "kind": { "type": "string", "enum": ["analog", "digital", "events"], "description": "Of every numeric column." },
            "digital": { "type": "array", "items": { "type": "string" }, "description": "Columns that are logic levels." },
            "unit": { "type": "string", "description": "Unit of columns whose header gives none." },
            "columns": { "type": "array", "items": { "type": "string" }, "description": "Only these columns (names or 1-based numbers)." },
            "prefix": { "type": "string", "description": "Put before every series name." },
            "group": { "type": "string", "description": "List heading of the series (default: the file's name)." },
            "delimiter": { "type": "string", "description": "Default: found from the first line." },
            "header": { "type": "boolean", "description": "Whether the first line names the columns (default: when it holds text)." },
            "id": { "type": "string", "description": "stop: the source id from list." }
        } }),
        true,
        |state, ctx, args| async move {
            let pid = project(&state, &ctx, &args)?;
            match args.get("action").and_then(Value::as_str).unwrap_or("start") {
                "list" => Ok(ToolOutput::Json(json!({ "sources": state.data.sources.list(&pid) }))),
                "stop" => {
                    let id = args.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::bad_request("id: the source to stop (data_follow list)"))?;
                    if !state.data.sources.stop(&pid, id) {
                        return Err(ApiError::not_found(format!("no source {id}")));
                    }
                    Ok(ToolOutput::Text(format!("Stopped {id}; what it read stays in the series.")))
                }
                "start" => {
                    let out = sources::start(&state, &pid, StartReq::from_json(&args)?)?;
                    Ok(ToolOutput::Json(out))
                }
                other => Err(ApiError::bad_request(format!("action: start, list or stop, not {other:?}"))),
            }
        },
    )
}

// ---------------------------------------------------------------- data_query

fn query_tool() -> McpTool {
    tool(
        "data_query",
        "Read measurements back from the data viewer: for each series statistics over a time window (analog: \
         min, max, peak-to-peak, mean, rms, median, p95, p99, the integral — charge for a current in A, energy for \
         W — and how many readings failed; digital: edges, frequency, period, duty cycle, pulse widths; events: \
         count) and the samples, thinned to about `maxPoints` so a spike survives. Name the series with `series`, \
         or give `plot` for the series of a plot. The window is `from`/`to` (seconds, as the series' times; \
         default: everything held) or `last` (seconds before the newest sample of the series). Use it to check \
         what a capture shows: the sleep current, the width of a pulse, whether a rail dipped. Read-only.",
        json!({ "type": "object", "properties": {
            "projectId": { "type": "string", "description": PROJECT_ID },
            "series": { "type": "array", "items": { "type": "string" } },
            "plot": { "type": "string", "description": "A plot's id or title: its series." },
            "from": { "type": "number" },
            "to": { "type": "number" },
            "last": { "type": "number", "description": "Seconds back from the newest sample." },
            "maxPoints": { "type": "integer", "description": "Samples per series (default 200, at most 2000; 0: statistics only)." },
            "events": { "type": "integer", "description": "Events per series: the newest of the window (default 100, at most 500)." }
        } }),
        false,
        |state, ctx, args| async move {
            let pid = project(&state, &ctx, &args)?;
            let mut list = names(&args, "series")?;
            if let Some(p) = args.get("plot").and_then(Value::as_str) {
                let plot = state.data.plot(&state, &pid, p).ok_or_else(|| ApiError::not_found(format!("no plot {p} in project {pid}")))?;
                list.extend(plot.series_names().into_iter().map(str::to_string));
            }
            let list = clean_names(list)?;
            if list.is_empty() {
                return Err(ApiError::bad_request("series or plot: what to read (data_series list shows what exists)"));
            }
            let infos = state.data.series_infos(&state, &pid);
            let info_of = |n: &str| infos.iter().find(|i| i["name"] == n);
            let newest = list.iter().filter_map(|n| info_of(n)).filter_map(|i| i["tEnd"].as_f64()).fold(f64::NEG_INFINITY, f64::max);
            let mut to = args.get("to").and_then(Value::as_f64);
            let mut from = args.get("from").and_then(Value::as_f64);
            if let Some(last) = args.get("last").and_then(Value::as_f64).filter(|l| *l > 0.0) {
                if newest.is_finite() {
                    to.get_or_insert(newest);
                    from = Some(to.unwrap_or(newest) - last);
                }
            }
            let max_points = args.get("maxPoints").and_then(Value::as_u64).unwrap_or(200).min(2000) as usize;
            let max_events = args.get("events").and_then(Value::as_u64).unwrap_or(100).min(500) as usize;
            let stats = state.data.stats(&state, &pid, &list, from, to);
            let (a, b) = (from.unwrap_or(f64::NEG_INFINITY), to.unwrap_or(f64::INFINITY));
            let views = if max_points > 0 { state.data.views(&state, &pid, &list, a, b, (max_points / 2).max(2)) } else { json!({}) };
            let mut out = serde_json::Map::new();
            for n in &list {
                let Some(info) = info_of(n) else {
                    out.insert(n.clone(), json!({ "error": "no such series" }));
                    continue;
                };
                let mut o = json!({
                    "kind": info["kind"], "unit": info["unit"], "label": info["label"],
                    "count": info["count"], "tStart": info["tStart"], "tEnd": info["tEnd"], "last": info["last"],
                    "stats": stats[n],
                });
                if let Some(v) = views.get(n) {
                    if let Some(ev) = v["events"].as_array() {
                        // The newest of the window's events (an even sample of it when it holds very many).
                        o["events"] = json!(ev.iter().skip(ev.len().saturating_sub(max_events)).collect::<Vec<_>>());
                        o["eventsInWindow"] = v["n"].clone();
                    } else if v.get("t").is_some() {
                        o["t"] = v["t"].clone();
                        o["v"] = v["v"].clone();
                        o["samplesInWindow"] = v["n"].clone();
                        o["thinned"] = v["thinned"].clone();
                    }
                }
                out.insert(n.clone(), o);
            }
            Ok(ToolOutput::Json(json!({ "projectId": pid, "window": { "from": from, "to": to }, "series": out })))
        },
    )
}
