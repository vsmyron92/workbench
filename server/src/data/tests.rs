//! The slice through its real entry points: the router (REST and the script hook), the MCP
//! tools, and a followed file.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, ToolOutput};
use crate::terminals::{SpawnSpec, TerminalKind};

struct Env {
    state: AppState,
    pid: String,
    root: std::path::PathBuf,
    router: axum::Router,
    _dirs: [tempfile::TempDir; 3],
}

async fn env() -> Env {
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(proj.path().join("captures")).unwrap();
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap();
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:7999".parse().unwrap()).await.unwrap();
    let router = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    Env { state, pid, root, router, _dirs: [cfg, data, proj] }
}

impl Env {
    async fn send(&self, method: Method, path: &str, bearer: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1").header("authorization", format!("Bearer {bearer}"));
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
    }

    async fn api(&self, method: Method, rest: &str, body: Option<Value>) -> (u16, Value) {
        let token = self.state.auth.master_token().to_string();
        self.send(method, &format!("/api/projects/{}/data{rest}", self.pid), &token, body).await
    }

    async fn tool(&self, name: &str, args: Value) -> Result<ToolOutput, crate::error::ApiError> {
        let tools = crate::mcp::all_tools();
        let t = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
        let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(self.pid.clone()) };
        (t.handler)(self.state.clone(), ctx, args).await
    }
}

fn text(o: ToolOutput) -> String {
    match o {
        ToolOutput::Text(t) => t,
        ToolOutput::Json(v) => v.to_string(),
    }
}

fn json_of(o: ToolOutput) -> Value {
    match o {
        ToolOutput::Json(v) => v,
        ToolOutput::Text(t) => panic!("expected JSON, got {t}"),
    }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn count(e: &Env, series: &str) -> u64 {
    e.state.data.series_infos(&e.state, &e.pid).iter().find(|i| i["name"] == series).and_then(|i| i["count"].as_u64()).unwrap_or(0)
}

// ---------------------------------------------------------------- REST

#[tokio::test(flavor = "multi_thread")]
async fn a_capture_goes_through_the_rest_routes() {
    let e = env().await;
    let (s, v) = e.api(Method::PUT, "/series", Some(json!({ "name": "I_load", "unit": "A", "label": "load current", "group": "run 1" }))).await;
    assert_eq!((s, v["unit"].as_str()), (200, Some("A")), "{v}");

    let t: Vec<f64> = (0..1000).map(|i| f64::from(i) * 0.001).collect();
    let v: Vec<f64> = t.iter().map(|x| 0.010 + 0.001 * (x * 6.28).sin()).collect();
    let (s, r) = e.api(Method::POST, "/append", Some(json!({ "series": "I_load", "t": t, "v": v }))).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["appended"][0]["count"], 1000);
    let (s, _) = e.api(Method::POST, "/append", Some(json!({ "batch": [
        { "series": "CLK", "kind": "digital", "t": [0.0, 0.1, 0.2, 0.3], "v": [0, 1, 0, 1] },
        { "series": "marks", "events": [{ "t": 0.25, "text": "radio on" }] },
    ] }))).await;
    assert_eq!(s, 200);

    // A malformed item changes nothing. One the series refuses stops the batch there, says which,
    // and the items before it stay.
    let (s, r) = e.api(Method::POST, "/append", Some(json!({ "batch": [
        { "series": "never", "t": [1.0], "v": [1] },
        { "series": "never2", "points": [[1]] },
    ] }))).await;
    assert_eq!(s, 400, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().starts_with("item 2 (never2)"), "{r}");
    assert_eq!(count(&e, "never"), 0);
    let (s, r) = e.api(Method::POST, "/append", Some(json!({ "batch": [
        { "series": "late", "t": [1.0], "v": [1] },
        { "series": "I_load", "t": [-5.0], "v": [1] },
    ] }))).await;
    assert_eq!(s, 400, "{r}");
    let msg = r["error"]["message"].as_str().unwrap();
    assert!(msg.starts_with("item 2 (I_load)") && msg.contains("1 earlier item(s) were applied"), "{msg}");
    assert_eq!(count(&e, "late"), 1);

    // What a 200 px plot draws: thinned, with its extremes.
    let (s, r) = e.api(Method::POST, "/view", Some(json!({ "series": ["I_load", "CLK", "marks", "nope"], "from": 0.0, "to": 1.0, "buckets": 100 }))).await;
    assert_eq!(s, 200);
    let views = &r["views"];
    assert_eq!(views["I_load"]["thinned"], true);
    assert!(views["I_load"]["t"].as_array().unwrap().len() <= 204);
    assert_eq!(views["CLK"]["v"], json!([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(views["marks"]["events"], json!([[0.25, "radio on"]]));
    assert_eq!(views["nope"]["missing"], true);

    let (_, r) = e.api(Method::POST, "/stats", Some(json!({ "series": ["I_load", "CLK"], "from": 0.0, "to": 0.2 }))).await;
    assert_eq!(r["stats"]["I_load"]["count"], 201);
    assert_eq!(r["stats"]["I_load"]["unit"], "A");
    assert_eq!(r["stats"]["CLK"]["kind"], "digital");

    // A plot from series names: lanes by unit and kind; and it is announced and kept.
    let (s, plot) = e.api(Method::POST, "/plots", Some(json!({ "title": "Power profile", "series": ["CLK", "I_load", "marks"], "span": 2 }))).await;
    assert_eq!(s, 200, "{plot}");
    assert_eq!(plot["id"], "power-profile");
    let lanes: Vec<String> = plot["lanes"].as_array().unwrap().iter().map(|l| l["traces"][0]["series"].as_str().unwrap().to_string()).collect();
    assert_eq!(lanes, vec!["I_load", "CLK", "marks"]);
    assert_eq!(plot["lanes"][0]["traces"][0]["color"], 1);
    assert_eq!(plot["lanes"][1]["traces"][0]["color"], 0, "a digital lane has no chart colour");
    let (_, snap) = e.api(Method::GET, "", None).await;
    assert_eq!(snap["plots"][0]["id"], "power-profile");
    assert_eq!(snap["series"].as_array().unwrap().len(), 4, "I_load, CLK, marks and late");

    // Edited through PUT (the viewer's own save).
    let mut edited = plot.clone();
    edited["follow"] = json!(false);
    edited["lanes"][0]["log"] = json!(true);
    let (s, r) = e.api(Method::PUT, "/plots/power-profile", Some(edited)).await;
    assert_eq!((s, r["follow"].clone(), r["lanes"][0]["log"].clone()), (200, json!(false), json!(true)), "{r}");
    let (s, r) = e.api(Method::PUT, "/plots/bad%20id", Some(json!({ "title": "x", "lanes": [] }))).await;
    assert_eq!(s, 400, "{r}");

    // CSV out.
    let (s, csv) = e.api(Method::POST, "/export", Some(json!({ "series": ["CLK", "marks"] }))).await;
    assert_eq!(s, 200);
    assert!(csv.as_str().unwrap().starts_with("time_s,CLK,marks\n0,0,\n0.1,1,\n"), "{csv}");

    // Clear and delete.
    let (_, r) = e.api(Method::POST, "/series/clear", Some(json!({ "names": ["I_load"] }))).await;
    assert_eq!(r["cleared"], json!(["I_load"]));
    assert_eq!(count(&e, "I_load"), 0);
    let (s, _) = e.api(Method::POST, "/series/clear", Some(json!({}))).await;
    assert_eq!(s, 400, "clearing everything must be asked for by name");
    let (_, r) = e.api(Method::POST, "/series/delete", Some(json!({ "names": ["I_load"] }))).await;
    assert_eq!(r["deleted"], json!(["I_load"]));
    let (s, _) = e.api(Method::DELETE, "/plots/power-profile", None).await;
    assert_eq!(s, 200);
    let (s, _) = e.api(Method::DELETE, "/plots/power-profile", None).await;
    assert_eq!(s, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_needs_a_signed_in_caller() {
    let e = env().await;
    let (s, _) = e.send(Method::GET, &format!("/api/projects/{}/data", e.pid), "wrong-token", None).await;
    assert_eq!(s, 401);
    let (s, _) = e.api(Method::GET, "", None).await;
    assert_eq!(s, 200);
    let token = e.state.auth.master_token().to_string();
    let (s, _) = e.send(Method::GET, "/api/projects/nope/data", &token, None).await;
    assert_eq!(s, 404);
}

// ---------------------------------------------------------------- the hook

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_script_posts_samples_with_its_sessions_token() {
    let e = env().await;
    let spec = SpawnSpec {
        kind: TerminalKind::Command,
        title: "capture".into(),
        project_id: Some(e.pid.clone()),
        cwd: e.root.clone(),
        argv: {
            // The tests of every slice run Python, which CI has on both systems.
            let mut a = crate::util::os::exe::python();
            a.extend(["-c".to_string(), "import time; time.sleep(5)".to_string()]);
            a
        },
        env: vec![],
        cols: None,
        rows: None,
        meta: json!({}),
    };
    let info = e.state.terminals.spawn(&e.state, spec).await.unwrap();
    let url = format!("/api/hooks/data/{}/append", info.id);
    let body = json!({ "series": "VDD", "unit": "V", "values": [3.3, 3.29, 3.31], "t0": 0.0, "dt": 0.5 });

    let (s, _) = e.send(Method::POST, &url, "not-a-token", Some(body.clone())).await;
    assert_eq!(s, 401);
    // Another session's token is not this one's.
    let other = e.state.auth.issue_agent_token("someoneelse");
    let (s, _) = e.send(Method::POST, &url, &other, Some(body.clone())).await;
    assert_eq!(s, 401);
    let mine = e.state.auth.issue_agent_token(&info.id);
    let (s, r) = e.send(Method::POST, &url, &mine, Some(body)).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(count(&e, "VDD"), 3);
    // Agent tokens stay out of the rest of the API.
    let (s, _) = e.send(Method::GET, &format!("/api/projects/{}/data", e.pid), &mine, None).await;
    assert_eq!(s, 401);
    e.state.terminals.kill(&info.id).await.unwrap();
}

// ---------------------------------------------------------------- tools

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_captures_plots_and_reads_back() {
    let e = env().await;
    let mut rx = e.state.events.subscribe();

    let r = text(e.tool("data_append", json!({ "series": "I_load", "unit": "A", "values": [0.001, 0.002, 0.050, 0.002], "t0": 0, "dt": 0.25 })).await.unwrap());
    assert!(r.contains("I_load: +4") && r.contains("[new series]"), "{r}");
    e.tool("data_append", json!({ "series": "CS", "kind": "digital", "t": [0, 0.3, 0.6], "v": [1, 0, 1] })).await.unwrap();
    e.tool("data_append", json!({ "series": "marks", "text": "wifi connect", "t": 0.5 })).await.unwrap();

    // The plot is made while the data flows and opens in the user's window.
    let r = json_of(e.tool("data_plot", json!({ "action": "create", "title": "Power", "series": ["I_load", "CS", "marks"], "span": 5 })).await.unwrap());
    assert_eq!(r["created"]["id"], "power");
    assert_eq!(r["created"]["lanes"].as_array().unwrap().len(), 3);
    let open = loop {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        if ev.kind == "ui.open" {
            break ev.data.clone();
        }
    };
    assert_eq!(open["panel"], "data.plot");
    assert_eq!(open["id"], format!("data.plot:{}:power", e.pid));
    assert_eq!(open["params"]["plotId"], "power");

    // Change it while the capture runs: another lane, a log axis.
    e.tool("data_append", json!({ "series": "VDD", "unit": "V", "t": [0, 1], "v": [3.3, 3.2] })).await.unwrap();
    let r = json_of(e.tool("data_plot", json!({ "action": "update", "plot": "Power", "add": ["VDD"], "remove": ["marks"], "markers": ["marks"] })).await.unwrap());
    let shape: Vec<String> = r["updated"]["lanes"].as_array().unwrap().iter().map(|l| l["series"][0].as_str().unwrap().into()).collect();
    assert_eq!(shape, vec!["I_load", "CS", "VDD"]);
    assert_eq!(r["updated"]["markers"], json!(["marks"]));

    // What the agent reads back is what the user sees.
    let q = json_of(e.tool("data_query", json!({ "plot": "power", "maxPoints": 50 })).await.unwrap());
    let i = &q["series"]["I_load"];
    assert_eq!(i["stats"]["max"], 0.05);
    assert_eq!(i["stats"]["min"], 0.001);
    assert_eq!(i["samplesInWindow"], 4);
    assert_eq!(q["series"]["CS"]["stats"]["falling"], 1);
    let q = json_of(e.tool("data_query", json!({ "series": ["marks"] })).await.unwrap());
    assert_eq!(q["series"]["marks"]["events"], json!([[0.5, "wifi connect"]]));
    let q = json_of(e.tool("data_query", json!({ "series": ["I_load"], "last": 0.3, "maxPoints": 0 })).await.unwrap());
    assert_eq!(q["series"]["I_load"]["stats"]["count"], 2);
    assert!(q["series"]["I_load"].get("t").is_none());

    // Clearing needs names; a new capture starts clean.
    assert!(e.tool("data_series", json!({ "action": "clear" })).await.is_err());
    e.tool("data_series", json!({ "action": "clear", "names": ["I_load"] })).await.unwrap();
    let r = text(e.tool("data_append", json!({ "series": "I_load", "t": [0], "v": [1] })).await.unwrap());
    assert!(r.contains("now 1 held"), "{r}");
    let list = json_of(e.tool("data_series", json!({})).await.unwrap());
    assert_eq!(list["plots"][0]["id"], "power");
    assert!(list["series"].as_array().unwrap().len() >= 4);

    // A plot that names nothing is refused with what to pass.
    let err = e.tool("data_plot", json!({ "action": "create", "title": "Empty" })).await.unwrap_err();
    assert!(err.message.contains("series"), "{}", err.message);
    let err = e.tool("data_plot", json!({ "action": "update", "plot": "nope" })).await.unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_cannot_reach_another_projects_data() {
    let e = env().await;
    let tools = crate::mcp::all_tools();
    let t = tools.iter().find(|t| t.name == "data_append").unwrap();
    let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(e.pid.clone()) };
    let err = (t.handler)(e.state.clone(), ctx, json!({ "projectId": "someone-elses", "series": "x", "v": 1 })).await.unwrap_err();
    assert_eq!(err.code, "forbidden");
}

// ---------------------------------------------------------------- followed files

fn append_to(path: &std::path::Path, text: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_is_followed_while_the_capture_writes_it() {
    let e = env().await;
    let file = e.root.join("captures/power.csv");
    // The capture has not started: the file is waited for.
    let started = json_of(e.tool("data_follow", json!({ "path": "captures/power.csv", "group": "run 1" })).await.unwrap());
    assert_eq!(started["state"], "waiting");
    let id = started["id"].as_str().unwrap().to_string();

    append_to(&file, "Time [s],Current [uA],Mode\n0.0,10.5,sleep\n0.1,11.0,sleep\n0.2,5200,tx\n");
    // Each column is its own series, appended one after the other: wait for all of them.
    wait_for("the first rows", || count(&e, "Current") == 3 && count(&e, "Mode") == 3).await;
    let infos = e.state.data.series_infos(&e.state, &e.pid);
    let cur = infos.iter().find(|i| i["name"] == "Current").unwrap();
    assert_eq!((cur["unit"].as_str(), cur["group"].as_str(), cur["kind"].as_str()), (Some("uA"), Some("run 1"), Some("analog")));
    assert_eq!(count(&e, "Mode"), 3, "text cells are events");

    // A row half written is not read until it is whole.
    append_to(&file, "0.3,12");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(count(&e, "Current"), 3);
    append_to(&file, ".5,idle\n0.4,12.6,idle\n");
    // The row count is set after the samples are in, so it is waited for too.
    wait_for("the rest", || count(&e, "Current") == 5 && e.state.data.sources.list(&e.pid)[0]["rows"] == 5).await;
    assert_eq!(e.state.data.stats(&e.state, &e.pid, &["Current".into()], None, None)["Current"]["max"], 5200.0);

    // The tool lists it, and it shows progress.
    let list = json_of(e.tool("data_follow", json!({ "action": "list" })).await.unwrap());
    assert_eq!(list["sources"][0]["state"], "running");
    assert_eq!(list["sources"][0]["rows"], 5);

    // A new capture over the same file (it shrinks): the series start over.
    std::fs::write(&file, "Time [s],Current [uA],Mode\n0.0,1.0,sleep\n").unwrap();
    wait_for("the restart", || count(&e, "Current") == 1).await;

    e.tool("data_follow", json!({ "action": "stop", "id": id })).await.unwrap();
    wait_for("it to stop", || e.state.data.sources.list(&e.pid)[0]["state"] == "stopped").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_vcd_is_imported_and_ends() {
    let e = env().await;
    std::fs::write(
        e.root.join("captures/la.vcd"),
        "$timescale 1 us $end\n$scope module la $end\n$var wire 1 ! SCL $end\n$var wire 1 \" SDA $end\n$upscope $end\n$enddefinitions $end\n#0\n1!\n1\"\n#10\n0\"\n#15\n0!\n#40\n1!\n#45\n1\"\n",
    )
    .unwrap();
    let r = json_of(e.tool("data_follow", json!({ "path": "captures/la.vcd", "follow": false })).await.unwrap());
    assert_eq!(r["format"], "vcd");
    wait_for("the import to end", || e.state.data.sources.list(&e.pid)[0]["state"] == "done").await;
    let infos = e.state.data.series_infos(&e.state, &e.pid);
    let scl = infos.iter().find(|i| i["name"] == "la.SCL").expect("scoped name");
    assert_eq!(scl["kind"], "digital");
    assert_eq!(scl["count"], 3);
    let st = &e.state.data.stats(&e.state, &e.pid, &["la.SDA".into()], None, None)["la.SDA"];
    assert_eq!(st["falling"], 1);
    assert_eq!(st["rising"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_csv_without_times_counts_rows_as_seconds() {
    let e = env().await;
    std::fs::write(e.root.join("captures/adc.txt"), "# adc dump\n1.0\n2.0\n4.0\n").unwrap();
    e.tool("data_follow", json!({ "path": "captures/adc.txt", "follow": false, "format": "csv" })).await.unwrap();
    wait_for("the import", || count(&e, "col1") == 3).await;
    let v = e.state.data.views(&e.state, &e.pid, &["col1".into()], 0.0, 10.0, 50);
    assert_eq!(v["col1"]["t"], json!([0.0, 1.0, 2.0]));
}

#[tokio::test(flavor = "multi_thread")]
async fn files_that_may_hold_secrets_or_leave_the_project_are_not_read() {
    let e = env().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("data.csv"), "t,v\n0,1\n").unwrap();
    std::fs::write(e.root.join(".env"), "t,v\n0,1\n").unwrap();
    std::fs::create_dir_all(e.root.join(".hidden")).unwrap();
    std::fs::write(e.root.join(".hidden/a.csv"), "t,v\n0,1\n").unwrap();
    std::fs::write(e.root.join("captures/server.pem"), "t,v\n0,1\n").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path().join("data.csv"), e.root.join("captures/link.csv")).unwrap();
        std::os::unix::fs::symlink(e.root.join(".env"), e.root.join("captures/env-link.csv")).unwrap();
    }
    let refused = |path: String| {
        let e = &e;
        async move { e.tool("data_follow", json!({ "path": path, "follow": false })).await.unwrap_err() }
    };
    let outside_file = outside.path().join("data.csv").display().to_string();
    for p in [".env", ".hidden/a.csv", "captures/server.pem", "../escape.csv", "/etc/passwd", outside_file.as_str()] {
        let err = refused(p.to_string()).await;
        assert!(matches!(err.code, "forbidden" | "bad_request"), "{p}: {err}");
    }
    #[cfg(unix)]
    for p in ["captures/link.csv", "captures/env-link.csv"] {
        let err = refused(p.to_string()).await;
        assert_eq!(err.code, "forbidden", "{p}: {err}");
    }
    assert!(e.state.data.sources.list(&e.pid).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_file_is_not_followed_twice_and_binary_is_refused() {
    let e = env().await;
    append_to(&e.root.join("captures/a.csv"), "t,v\n0,1\n");
    e.tool("data_follow", json!({ "path": "captures/a.csv" })).await.unwrap();
    let err = e.tool("data_follow", json!({ "path": "captures/a.csv" })).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    std::fs::write(e.root.join("captures/blob.csv"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    e.tool("data_follow", json!({ "path": "captures/blob.csv", "follow": false })).await.unwrap();
    wait_for("the refusal", || e.state.data.sources.list(&e.pid).iter().any(|s| s["path"] == "captures/blob.csv" && s["state"] == "error")).await;
    e.state.data.sources.shutdown();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_becomes_a_link_out_of_the_project_is_not_read() {
    let e = env().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.csv"), "t,v\n0,1\n1,2\n").unwrap();
    // Followed before it exists: only its name could be checked.
    let r = json_of(e.tool("data_follow", json!({ "path": "captures/late.csv" })).await.unwrap());
    assert_eq!(r["state"], "waiting");
    std::os::unix::fs::symlink(outside.path().join("secret.csv"), e.root.join("captures/late.csv")).unwrap();
    wait_for("the refusal", || e.state.data.sources.list(&e.pid).iter().any(|s| s["state"] == "error")).await;
    let src = &e.state.data.sources.list(&e.pid)[0];
    assert!(src["error"].as_str().unwrap().contains("outside the project"), "{src}");
    assert!(e.state.data.series_infos(&e.state, &e.pid).is_empty(), "nothing of it was read");
}

// ---------------------------------------------------------------- what review found

#[tokio::test(flavor = "multi_thread")]
async fn a_column_with_text_in_it_stays_a_numeric_series() {
    let e = env().await;
    // An instrument that prints OVLD once among its readings, and a UTF-8 byte order mark first.
    std::fs::write(e.root.join("captures/dmm.csv"), "\u{feff}Time [s],Current [mA]\n0.0,1.5\n0.1,OVLD\n0.2,1.7\n0.3,3.0\n").unwrap();
    e.tool("data_follow", json!({ "path": "captures/dmm.csv", "follow": false })).await.unwrap();
    wait_for("the import", || e.state.data.sources.list(&e.pid).iter().any(|s| s["state"] == "done")).await;
    let infos = e.state.data.series_infos(&e.state, &e.pid);
    let kind_of = |n: &str| infos.iter().find(|i| i["name"] == n).map(|i| (i["kind"].as_str().unwrap().to_string(), i["count"].as_u64().unwrap()));
    // The numbers stay a series of numbers (the byte order mark did not hide the time column either) …
    assert_eq!(kind_of("Current"), Some(("analog".into(), 3)));
    assert!(kind_of("Time").is_none(), "the time column is the time");
    // … and the text went beside it.
    assert_eq!(kind_of("Current (text)"), Some(("events".into(), 1)));
    let v = e.state.data.views(&e.state, &e.pid, &["Current".into()], 0.0, 1.0, 50);
    assert_eq!(v["Current"]["t"], json!([0.0, 0.2, 0.3]));
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_out_of_order_are_skipped_and_counted_not_fatal() {
    let e = env().await;
    std::fs::write(e.root.join("captures/messy.csv"), "t,v\n1.0,1\n0.5,2\n2.0,3\ninf,4\n1.9,5\n3.0,6\n").unwrap();
    e.tool("data_follow", json!({ "path": "captures/messy.csv", "follow": false })).await.unwrap();
    wait_for("the import", || e.state.data.sources.list(&e.pid).iter().any(|s| s["state"] == "done")).await;
    let src = &e.state.data.sources.list(&e.pid)[0];
    assert_eq!(src["samples"], 3, "{src}");
    assert_eq!(src["skipped"], 3, "{src}");
    let v = e.state.data.views(&e.state, &e.pid, &["v".into()], 0.0, 10.0, 50);
    assert_eq!(v["v"]["v"], json!([1.0, 3.0, 6.0]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_replaced_by_another_starts_the_capture_over() {
    let e = env().await;
    let file = e.root.join("captures/run.csv");
    std::fs::write(&file, "t,A\n0,1\n1,2\n2,3\n3,4\n").unwrap();
    e.tool("data_follow", json!({ "path": "captures/run.csv" })).await.unwrap();
    wait_for("the first run", || count(&e, "A") == 4).await;
    // Another capture over the same name that is longer than the first, so its length alone says nothing.
    std::fs::write(&file, "time,B\n0,10\n1,20\n2,30\n3,40\n4,50\n5,60\n").unwrap();
    wait_for("the second run", || count(&e, "B") == 6).await;
    assert_eq!(count(&e, "A"), 0, "the series the old file wrote were cleared");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_path_still_gets_its_units_and_kinds() {
    let e = env().await;
    let dir = "captures/a-folder-with-a-rather-long-name-for-the-sake-of-it/and-another-below-it";
    std::fs::create_dir_all(e.root.join(dir)).unwrap();
    let path = format!("{dir}/logic.csv");
    std::fs::write(e.root.join(&path), "Time [s],CLK\n0,0\n1,1\n2,0\n").unwrap();
    e.tool("data_follow", json!({ "path": path, "follow": false, "digital": ["CLK"] })).await.unwrap();
    wait_for("the import", || count(&e, "CLK") == 3).await;
    let infos = e.state.data.series_infos(&e.state, &e.pid);
    let clk = infos.iter().find(|i| i["name"] == "CLK").unwrap();
    assert_eq!(clk["kind"], "digital");
    assert!(clk["group"].as_str().unwrap().ends_with("logic.csv") && clk["group"].as_str().unwrap().chars().count() <= 64);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_names_for_one_file_are_one_source() {
    let e = env().await;
    std::fs::write(e.root.join("captures/one.csv"), "t,v\n0,1\n").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(e.root.join("captures/one.csv"), e.root.join("captures/alias.csv")).unwrap();
        e.tool("data_follow", json!({ "path": "captures/one.csv" })).await.unwrap();
        let err = e.tool("data_follow", json!({ "path": "captures/alias.csv" })).await.unwrap_err();
        assert_eq!(err.code, "conflict");
    }
    e.state.data.sources.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hook_answers_before_it_reads_the_body() {
    let e = env().await;
    // Not JSON at all, and no token: unauthorized, which is only possible if the token was looked at first.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/hooks/data/anything/append")
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from("[[[[ not json"))
        .unwrap();
    let resp = e.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn bodies_of_the_wrong_shape_are_refused_and_requests_are_bounded() {
    let e = env().await;
    // A plot from something that is not an object used to panic the handler.
    for body in [json!([]), json!(5), json!("plot")] {
        let (s, r) = e.api(Method::POST, "/plots", Some(body)).await;
        assert_eq!(s, 400, "{r}");
    }
    // An existing id is not silently replaced.
    let (s, _) = e.api(Method::POST, "/plots", Some(json!({ "id": "mine", "title": "A", "series": ["x"] }))).await;
    assert_eq!(s, 200);
    let (s, r) = e.api(Method::POST, "/plots", Some(json!({ "id": "mine", "title": "B", "series": ["y"] }))).await;
    assert_eq!(s, 409, "{r}");
    // Clearing says what it clears: an empty group is no group, and nothing at all is refused.
    for body in [json!({ "group": "" }), json!({}), json!({ "names": [] })] {
        let (s, r) = e.api(Method::POST, "/series/clear", Some(body)).await;
        assert_eq!(s, 400, "{r}");
    }
    let (s, _) = e.api(Method::POST, "/series/clear", Some(json!({ "all": true }))).await;
    assert_eq!(s, 200);
    // Names and batches have limits.
    let names: Vec<String> = (0..100).map(|i| format!("s{i}")).collect();
    let (s, r) = e.api(Method::POST, "/view", Some(json!({ "series": names, "from": 0, "to": 1, "buckets": 10 }))).await;
    assert_eq!(s, 400, "{r}");
    let batch: Vec<Value> = (0..300).map(|i| json!({ "series": format!("b{i}"), "v": 1 })).collect();
    let (s, r) = e.api(Method::POST, "/append", Some(json!({ "batch": batch }))).await;
    assert_eq!(s, 400, "{r}");
    // Duplicates in a request are one series, once.
    let (s, r) = e.api(Method::POST, "/stats", Some(json!({ "series": ["x", "x", "x"] }))).await;
    assert_eq!(s, 200);
    assert_eq!(r["stats"].as_object().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_file_that_cannot_be_read_is_kept_aside_not_overwritten() {
    let e = env().await;
    let dir = e.state.paths.data_dir.join("data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{}.json", e.pid)), "{ this is not json").unwrap();
    // The first look finds nothing usable, and the next save must not be what destroys the file.
    e.tool("data_append", json!({ "series": "x", "unit": "A", "t": [0], "v": [1] })).await.unwrap();
    assert!(dir.join(format!("{}.json.corrupt", e.pid)).exists());
    let saved = std::fs::read_to_string(dir.join(format!("{}.json", e.pid))).unwrap();
    assert!(saved.contains("\"x\""), "{saved}");
}
