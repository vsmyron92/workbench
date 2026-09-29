//! Slice-level tests on a real `AppState` (temp config/data dirs, one temp project)
//! against a local mock HTTP server. Nothing here spawns a terminal, so the tests
//! do not depend on the terminals slice.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use serde_json::{Value, json};

use super::*;
use crate::app::AppState;

const PW: &str = "s3cr3t-pa55w0rd-xyz";

fn basic() -> String {
    use base64::Engine;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("u:{PW}")))
}
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, ToolOutput};

struct Fixture {
    state: AppState,
    _dirs: Vec<tempfile::TempDir>,
    mock: SocketAddr,
    health_code: Arc<AtomicU16>,
}

async fn mock_server(health_code: Arc<AtomicU16>) -> SocketAddr {
    let code = health_code.clone();
    let app = Router::new()
        .route(
            "/api/health",
            get(move || {
                let c = code.load(Ordering::SeqCst);
                async move {
                    match c {
                        200 => (StatusCode::OK, axum::Json(json!({ "status": "ok" }))).into_response(),
                        299 => (StatusCode::OK, axum::Json(json!({ "status": "starting" }))).into_response(),
                        c => StatusCode::from_u16(c).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR).into_response(),
                    }
                }
            }),
        )
        .route("/api/version", get(|| async { axum::Json(json!({ "build": { "sha": "4b8e2508c0ffee" } })) }))
        .route(
            "/",
            get(|h: HeaderMap| async move {
                if h.get("authorization").and_then(|v| v.to_str().ok()) == Some(basic().as_str()) {
                    ([("x-frame-options", "DENY")], "private page").into_response()
                } else {
                    (StatusCode::UNAUTHORIZED, [("www-authenticate", "Basic realm=\"x\"")]).into_response()
                }
            }),
        )
        .route(
            "/api/whoami",
            get(|h: HeaderMap| async move { axum::Json(json!({ "auth": h.contains_key("authorization") })) }),
        )
        .route(
            "/setcookie",
            get(|| async {
                axum::response::Response::builder()
                    .header("set-cookie", "sid=SECRET-A; Path=/; Domain=example.com; Secure; HttpOnly; SameSite=None")
                    .header("set-cookie", "wb_session_7777=tossed; Path=/api; HttpOnly")
                    .body(axum::body::Body::from("set"))
                    .unwrap()
            }),
        )
        .route(
            "/echo-cookie",
            get(|h: HeaderMap| async move {
                let all: Vec<String> = h.get_all("cookie").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
                all.join(" | ")
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    addr
}

async fn fixture() -> Fixture {
    let health_code = Arc::new(AtomicU16::new(200));
    let mock = mock_server(health_code.clone()).await;
    let cfg_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let proj_parent = tempfile::tempdir().unwrap();
    let root = proj_parent.path().join("demo");
    std::fs::create_dir_all(root.join("web")).unwrap();
    let pw = cfg_dir.path().join("pw");
    std::fs::write(&pw, PW).unwrap();
    crate::util::fs::set_mode(&pw, 0o600);
    let m = format!("http://{mock}");
    std::fs::write(
        root.join(".workbench.toml"),
        format!(
            r#"
[[run]]
name = "api"
kind = "server"
command = "true"
port = 1
[[run]]
name = "web"
kind = "server"
command = "true"
cwd = "web"
depends_on = ["api", "port:1"]
[[run]]
name = "loop-a"
command = "true"
depends_on = ["loop-b"]
[[run]]
name = "loop-b"
command = "true"
depends_on = ["loop-a"]
[[run]]
name = "needs-unity"
command = "{{unity}} -batchmode"
[[run]]
name = "bad-cwd"
command = "true"
cwd = "../outside"

[[env]]
name = "staging"
kind = "staging"
url = "{m}"
health = {{ url = "{m}/api/health", json_pointer = "/status", equals = "ok", interval_s = 3600 }}
version = {{ http = "{m}/api/version", json_pointer = "/build/sha" }}
auth = {{ user = "u", password = "pw", except = ["/api/*"] }}
deploy = {{ command = "echo {{sha8}} {{branch}}", local = true, confirm = "typed", only_ref = "main", after = "other" }}

[[env]]
name = "private"
url = "{m}"
health = {{ url = "{m}/", interval_s = 3600 }}
auth = {{ user = "u", password = "pw" }}

[[env]]
name = "public"
url = "{m}"

[[env]]
name = "nosecret"
url = "{m}"
health = {{ url = "{m}/", interval_s = 3600 }}
auth = {{ user = "u", password = "missing" }}

[[env]]
name = "remote"
url = "{m}"
host = "box"
deploy = {{ command = "./deploy.sh {{sha8}}" }}
"#
        ),
    )
    .unwrap();
    std::fs::create_dir_all(cfg_dir.path().join("projects")).unwrap();
    std::fs::write(cfg_dir.path().join("projects/demo.toml"), format!("[secrets]\npw = {{ file = {:?} }}\n", pw.to_string_lossy())).unwrap();
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.to_string_lossy().into_owned()];
    let paths = Paths { config_dir: cfg_dir.path().to_path_buf(), data_dir: data_dir.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    Fixture { state, _dirs: vec![cfg_dir, data_dir, proj_parent], mock, health_code }
}

fn project(f: &Fixture) -> Arc<crate::projects::Project> {
    f.state.projects.require("demo").unwrap()
}

#[tokio::test]
async fn planning_orders_dependencies_and_rejects_cycles() {
    let f = fixture().await;
    let p = project(&f);
    let plan = runs::plan_for_tests(&f.state, &p, "web").await.unwrap();
    let names: Vec<&str> = plan.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["api", "web"], "the owner of port 1 is api, started first");
    let err = runs::plan_for_tests(&f.state, &p, "loop-a").await.unwrap_err();
    assert!(err.message.contains("cycle"), "{}", err.message);
    let views = runs::list(&f.state, &p).await;
    let by: std::collections::HashMap<&str, &runs::RunView> = views.iter().map(|v| (v.name.as_str(), v)).collect();
    assert!(by["needs-unity"].problems.iter().any(|x| x.contains("{unity}")));
    assert!(by["bad-cwd"].problems.iter().any(|x| x.contains("escapes") || x.contains("outside")), "{:?}", by["bad-cwd"].problems);
    assert!(by["web"].problems.is_empty(), "{:?}", by["web"].problems);
    assert_eq!(by["web"].live.state, runs::RunState::Stopped);
    // Preflight errors reach the caller before anything is spawned.
    let e = runs::start(&f.state, &p, "needs-unity", false).await.unwrap_err();
    assert_eq!(e.code, "not_configured");
    let e = runs::start(&f.state, &p, "nope", false).await.unwrap_err();
    assert_eq!(e.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_checks_use_pointer_auth_and_notify_on_transitions() {
    let f = fixture().await;
    let p = project(&f);
    let mut events = f.state.events.subscribe();
    let staging = envs::find(&p, "staging").unwrap().clone();

    let h = envs::check(&f.state, &p, &staging).await;
    assert_eq!(h.status, health::HealthStatus::Up, "{:?}", h.error);
    assert_eq!(h.http_status, Some(200));
    assert_eq!(h.history.len(), 1);
    // The http version probe runs with the check.
    assert_eq!(f.state.apps.envs.version("demo", "staging").and_then(|v| v.sha).as_deref(), Some("4b8e2508c0ffee"));

    f.health_code.store(299, Ordering::SeqCst);
    let h = envs::check(&f.state, &p, &staging).await;
    assert_eq!(h.status, health::HealthStatus::Degraded);
    assert!(h.error.unwrap().contains("starting"));

    f.health_code.store(503, Ordering::SeqCst);
    let h = envs::check(&f.state, &p, &staging).await;
    assert_eq!(h.status, health::HealthStatus::Down);
    f.health_code.store(200, Ordering::SeqCst);
    let h = envs::check(&f.state, &p, &staging).await;
    assert_eq!(h.status, health::HealthStatus::Up);
    assert_eq!(h.history.len(), 4);

    let mut notes = vec![];
    let mut health_events = 0;
    while let Ok(ev) = events.try_recv() {
        match ev.kind.as_str() {
            "ui.notify" => notes.push(ev.data["message"].as_str().unwrap_or("").to_string()),
            "env.health" => health_events += 1,
            _ => {}
        }
    }
    assert!(health_events >= 4);
    assert!(notes.iter().any(|n| n.contains("staging is down")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("staging is back up")), "{notes:?}");

    // Guarded path: basic auth from the secret is injected.
    let private = envs::find(&p, "private").unwrap().clone();
    assert_eq!(envs::check(&f.state, &p, &private).await.status, health::HealthStatus::Up);
    // Missing secret: unknown with setup help, never "down".
    let nosecret = envs::find(&p, "nosecret").unwrap().clone();
    let h = envs::check(&f.state, &p, &nosecret).await;
    assert_eq!(h.status, health::HealthStatus::Unknown);
    assert!(h.error.unwrap().contains("missing"));
}

#[tokio::test]
async fn env_views_never_carry_secret_values() {
    let f = fixture().await;
    let p = project(&f);
    let v = serde_json::to_string(&envs::list(&f.state, &p)).unwrap();
    assert!(v.contains("\"secret\":\"pw\""), "the secret *name* is shown");
    assert_eq!(f.state.secret(Some(&p), "pw").unwrap().expose(), PW);
    assert!(!v.contains(PW), "the secret value never is");
    let remote = v.find("\"name\":\"remote\"").map(|i| &v[i..]).unwrap_or("");
    assert!(remote.contains("\"target\":null"), "an undefined host is not a target");
}

#[tokio::test]
async fn deploy_plans_gate_and_refuse_before_spawning() {
    let f = fixture().await;
    let p = project(&f);
    // The project is not a git repository: planning reports it instead of guessing.
    let staging = envs::find(&p, "staging").unwrap().clone();
    let e = deploy::plan(&f.state, &p, &staging, None).await.unwrap_err();
    assert!(e.message.contains("no commits"), "{}", e.message);
    // A remote deploy needs a defined host.
    let remote = envs::find(&p, "remote").unwrap().clone();
    let e = deploy::plan(&f.state, &p, &remote, None).await.unwrap_err();
    assert_eq!(e.code, "not_configured");

    // In a scratch repo on `main`, with `after = "other"` (no such env): blocked.
    let root = p.root.clone();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "c1"]);
    let plan = deploy::plan(&f.state, &p, &staging, None).await.unwrap();
    assert_eq!(plan.target, "local");
    assert_eq!(plan.command, format!("echo {} main", plan.sha8));
    assert!(!plan.ok);
    let gates: Vec<(&str, deploy::GateStatus)> = plan.gates.iter().map(|g| (g.id, g.status)).collect();
    assert_eq!(gates, vec![("ref", deploy::GateStatus::Pass), ("pipeline", deploy::GateStatus::Skip), ("after", deploy::GateStatus::Fail)]);
    let e = deploy::deploy(&f.state, &p, &staging, None, &json!("staging")).await.unwrap_err();
    assert_eq!(e.status, StatusCode::CONFLICT, "blocked gates refuse the deploy: {}", e.message);
}

#[tokio::test]
async fn proxy_injects_auth_behind_a_one_time_token() {
    let f = fixture().await;
    let mut h = HeaderMap::new();
    h.insert("host", "127.0.0.1:7777".parse().unwrap());
    let out = proxy::proxy_url(&f.state, "demo", "private", "/", &h).await.unwrap();
    let url = out.url.unwrap();
    assert!(url.starts_with("http://127.0.0.1:"));
    let port = out.port.unwrap();
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    // No cookie → refused.
    let r = client.get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let r = client.get(&url).send().await.unwrap();
    assert_eq!(r.status(), 302);
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let r = client.get(&url).send().await.unwrap();
    assert_eq!(r.status(), 403, "tokens are single-use");
    let r = client.get(format!("http://127.0.0.1:{port}/")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("x-frame-options").is_none());
    assert_eq!(r.text().await.unwrap(), "private page");
    // Remote callers get no URL.
    let mut remote = HeaderMap::new();
    remote.insert("host", "box.tailnet.ts.net".parse().unwrap());
    let out = proxy::proxy_url(&f.state, "demo", "private", "/", &remote).await.unwrap();
    assert!(out.url.is_none() && out.reason.is_some());
    let _ = f.mock;
    proxy::shutdown(&f.state).await;
}

/// Browsers share one cookie jar for every port of 127.0.0.1: a cookie one env sets
/// through its proxy must never reach another env's upstream, and neither may
/// Workbench's own cookies or those of other local services.
#[tokio::test]
async fn proxy_cookies_never_cross_environments() {
    let f = fixture().await;
    let mut h = HeaderMap::new();
    h.insert("host", "127.0.0.1:7777".parse().unwrap());
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    // Redeem a one-time URL for each env's proxy, like the browser does.
    let mut sessions = vec![];
    for env in ["private", "public"] {
        let out = proxy::proxy_url(&f.state, "demo", env, "/", &h).await.unwrap();
        let r = client.get(out.url.unwrap()).send().await.unwrap();
        let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
        sessions.push((out.port.unwrap(), cookie));
    }
    let (port_a, sess_a) = sessions[0].clone();
    let (port_b, sess_b) = sessions[1].clone();

    // Env A's upstream sets a Secure session cookie and tries to toss Workbench's cookie.
    let r = client.get(format!("http://127.0.0.1:{port_a}/setcookie")).header("cookie", &sess_a).send().await.unwrap();
    let set: Vec<String> = r.headers().get_all("set-cookie").iter().map(|v| v.to_str().unwrap().to_string()).collect();
    let pa = proxy::cookie_prefix("demo", "private", &format!("http://{}", f.mock));
    assert_eq!(set.len(), 2, "{set:?}");
    assert_eq!(set[0], format!("{pa}sid=SECRET-A; Path=/; HttpOnly; SameSite=Lax"));
    assert!(set.iter().all(|c| c.starts_with(&pa)), "no upstream cookie keeps a name of its own: {set:?}");
    let a_cookie = format!("{pa}sid=SECRET-A");

    // The browser now sends the whole 127.0.0.1 jar to every port.
    let jar = |own: &str| format!("{own}; wb_session_7777=wb-device; {a_cookie}; local_admin_session=LOCAL-SERVICE-SECRET");
    let r = client.get(format!("http://127.0.0.1:{port_b}/echo-cookie")).header("cookie", jar(&sess_b)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "", "env B's upstream sees none of the jar");
    let r = client.get(format!("http://127.0.0.1:{port_a}/echo-cookie")).header("cookie", jar(&sess_a)).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "sid=SECRET-A", "env A's upstream sees its own cookie only, under its real name");
    proxy::shutdown(&f.state).await;
}

async fn call_tool(f: &Fixture, name: &str, args: Value) -> Result<ToolOutput, crate::error::ApiError> {
    let tool = mcp_tools().into_iter().find(|t| t.name == name).unwrap();
    let ctx = McpCtx { terminal_id: None, project_id: Some("demo".into()) };
    (tool.handler)(f.state.clone(), ctx, args).await
}

#[tokio::test]
async fn mcp_tools_report_runs_and_environments() {
    let f = fixture().await;
    let names: Vec<String> = mcp_tools().iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, vec!["env_status", "run_list", "run_start", "run_stop", "run_output"]);
    assert!(mcp_tools().iter().all(|t| !t.mutating && t.input_schema["type"] == "object"));
    assert!(!names.iter().any(|n| n.contains("deploy")), "deploys are never exposed to agents");

    let ToolOutput::Json(v) = call_tool(&f, "run_list", json!({})).await.unwrap() else { panic!() };
    assert_eq!(v["project"], "demo");
    assert!(v["runs"].as_array().unwrap().iter().any(|r| r["name"] == "web" && r["state"] == "stopped"));

    let p = project(&f);
    envs::check(&f.state, &p, envs::find(&p, "staging").unwrap()).await;
    let ToolOutput::Json(v) = call_tool(&f, "env_status", json!({})).await.unwrap() else { panic!() };
    let st = v["environments"].as_array().unwrap().iter().find(|e| e["env"] == "staging").unwrap().clone();
    assert_eq!(st["status"], "up");
    assert_eq!(st["version"], "4b8e2508c0ffee");

    let ToolOutput::Text(t) = call_tool(&f, "run_output", json!({ "name": "web", "lines": 9999 })).await.unwrap() else { panic!() };
    assert!(t.contains("no output"), "{t}");
    assert!(call_tool(&f, "run_output", json!({ "name": "nope" })).await.is_err());
    assert!(call_tool(&f, "run_start", json!({})).await.is_err(), "name is required");
    let e = call_tool(&f, "run_list", json!({ "projectId": "other" })).await.unwrap_err();
    assert_eq!(e.status, StatusCode::NOT_FOUND);
    // A hosted session of another project cannot start or stop this project's runs.
    let foreign = McpCtx { terminal_id: Some("t-other".into()), project_id: Some("other".into()) };
    let start = mcp_tools().into_iter().find(|t| t.name == "run_start").unwrap();
    let e = (start.handler)(f.state.clone(), foreign, json!({ "name": "web", "projectId": "demo" })).await.unwrap_err();
    assert_eq!(e.status, StatusCode::FORBIDDEN);
    assert_eq!(f.state.apps.runs.live("demo", "web").terminal_id, None, "nothing was started");
}

// ---------------------------------------------------------------- live runs and deploys
// These start the terminals slice and spawn real (trivial, local) processes in temp dirs.

struct Live {
    state: AppState,
    root: std::path::PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

fn scratch_git(root: &std::path::Path, args: &[&str]) {
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
}

async fn live_fixture(workbench_toml: &str) -> Live {
    let cfg_dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("live");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".workbench.toml"), workbench_toml).unwrap();
    std::fs::write(root.join(".gitignore"), "*.log\n.started\n").unwrap();
    scratch_git(&root, &["init", "-q", "-b", "main"]);
    scratch_git(&root, &["add", "-A"]);
    scratch_git(&root, &["commit", "-q", "-m", "c1"]);
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.to_string_lossy().into_owned()];
    config.agents.restore_on_start = false;
    let paths = Paths { config_dir: cfg_dir.path().to_path_buf(), data_dir: data_dir.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    crate::terminals::start(&state).await;
    Live { state, root, _dirs: vec![cfg_dir, data_dir, parent] }
}

async fn wait_run(state: &AppState, name: &str, what: &str, ok: impl Fn(&runs::RunLive) -> bool) -> runs::RunLive {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let live = state.apps.runs.live("live", name);
        if ok(&live) {
            return live;
        }
        assert!(std::time::Instant::now() < deadline, "{name}: not {what}: {live:?}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// A run keeps one terminal across starts: an open output tab follows the new
/// process, terminals do not pile up, and the previous process's ready line still
/// on the screen does not make the new one "ready".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_reuse_their_terminal_across_starts() {
    // In the run shell: bash, or PowerShell on Windows.
    let command = if cfg!(windows) {
        "if (Test-Path .started) { echo second-start; sleep 30 } else { New-Item .started | Out-Null; echo READY-LINE; sleep 30 }"
    } else {
        "if [ -f .started ]; then echo second-start; sleep 30; else touch .started; echo READY-LINE; sleep 30; fi"
    };
    let l = live_fixture(&format!(
        r#"
[[run]]
name = "srv"
kind = "server"
command = "{command}"
ready = {{ log = "READY-LINE", timeout_s = 60 }}
"#
    ))
    .await;
    let p = l.state.projects.require("live").unwrap();
    runs::start(&l.state, &p, "srv", false).await.unwrap();
    let first = wait_run(&l.state, "srv", "ready", |x| x.state == runs::RunState::Ready).await;
    let tid = first.terminal_id.clone().unwrap();

    runs::stop(&l.state, &p, "srv").await.unwrap();
    assert!(l.state.terminals.info(&tid).unwrap().exit.is_some());
    runs::restart(&l.state, &p, "srv", false).await.unwrap();
    let second = wait_run(&l.state, "srv", "running", |x| x.state == runs::RunState::Running).await;
    assert_eq!(second.terminal_id.as_deref(), Some(tid.as_str()), "the same terminal is reused");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !l.state.terminals.screen_text(&tid, 50).unwrap_or_default().contains("second-start") {
        assert!(std::time::Instant::now() < deadline, "the new process did not print");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert_eq!(l.state.apps.runs.live("live", "srv").state, runs::RunState::Running, "the old READY-LINE on the screen is not a ready line");
    let info = l.state.terminals.info(&tid).unwrap();
    assert!(info.exit.is_none() && info.open);
    let run_terminals = l.state.terminals.list().into_iter().filter(|t| t.meta["run"] == "srv").count();
    assert_eq!(run_terminals, 1, "no pile of dead terminals");
    runs::stop(&l.state, &p, "srv").await.unwrap();
}

/// `free_port = true` is standing consent: the start frees the port instead of
/// answering `port_in_use`. Without it, a busy port is still refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn free_port_config_is_honoured() {
    // Linux frees ports with fuser; Windows asks the TCP table (`os::net`).
    let py = crate::util::os::exe::python();
    if (cfg!(unix) && !crate::util::which("fuser")) || !crate::util::which(&py[0]) {
        eprintln!("skip: fuser or Python 3 missing");
        return;
    }
    // A foreign process (a child, never this test process) holding a port.
    let hold = |port: u16| {
        std::process::Command::new(&py[0])
            .args(&py[1..])
            .args(["-c", &format!("import socket,time\ns=socket.socket()\ns.bind(('127.0.0.1',{port}))\ns.listen()\ntime.sleep(60)")])
            .spawn()
            .unwrap()
    };
    let pick = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (pa, pb) = (pick(), pick());
    let mut a = hold(pa);
    let mut b = hold(pb);
    for port in [pa, pb] {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !runs::port_open(port, std::time::Duration::from_millis(200)).await {
            assert!(std::time::Instant::now() < deadline, "listener on {port} did not start");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
    let l = live_fixture(&format!(
        r#"
[[run]]
name = "freeing"
kind = "server"
command = "sleep 30"
port = {pa}
free_port = true
[[run]]
name = "polite"
kind = "server"
command = "sleep 30"
port = {pb}
"#
    ))
    .await;
    let p = l.state.projects.require("live").unwrap();
    let e = runs::start(&l.state, &p, "polite", false).await.unwrap_err();
    assert_eq!(e.code, "port_in_use");
    runs::start(&l.state, &p, "freeing", false).await.unwrap();
    wait_run(&l.state, "freeing", "running", |x| x.state == runs::RunState::Running).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while a.try_wait().unwrap().is_none() {
        assert!(std::time::Instant::now() < deadline, "the process on :{pa} was not freed");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(b.try_wait().unwrap().is_none(), "the other port's process is untouched");
    let _ = b.kill();
    let _ = b.wait();
    runs::stop(&l.state, &p, "freeing").await.unwrap();
}

/// Deploy-like runs need the user's confirmation (also as a dependency), and agents
/// can start neither them nor documentation suggestions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn risky_runs_need_confirmation_and_are_closed_to_agents() {
    let l = live_fixture(
        r#"
[[run]]
name = "deploy-prod"
command = "echo SHOULD-NOT-RUN > shipped.log"
[[run]]
name = "needs-it"
command = "true"
depends_on = ["deploy-prod"]
[[run]]
name = "from-docs"
command = "true"
group = "suggested"
[[run]]
name = "tests"
kind = "test"
command = "true"
"#,
    )
    .await;
    let p = l.state.projects.require("live").unwrap();
    let views = runs::list(&l.state, &p).await;
    let needs: Vec<(&str, bool)> = views.iter().map(|v| (v.name.as_str(), v.needs_confirm)).collect();
    assert_eq!(needs, vec![("deploy-prod", true), ("needs-it", false), ("from-docs", false), ("tests", false)]);
    assert_eq!(runs::gated(&l.state, &p, "needs-it", false, false).await.unwrap(), vec!["deploy-prod"]);
    assert!(runs::gated(&l.state, &p, "tests", false, true).await.unwrap().is_empty());

    // REST: 428 until confirmed.
    let e = super::require_confirmation(&l.state, &p, "needs-it", false, false).await.unwrap_err();
    assert_eq!((e.status, e.code), (StatusCode::PRECONDITION_REQUIRED, "confirmation_required"));
    assert!(super::require_confirmation(&l.state, &p, "needs-it", false, true).await.is_ok());
    assert!(super::require_confirmation(&l.state, &p, "tests", false, false).await.is_ok());

    // MCP: refused, nothing starts.
    let ctx = McpCtx { terminal_id: None, project_id: Some("live".into()) };
    let tool = mcp_tools().into_iter().find(|t| t.name == "run_start").unwrap();
    for name in ["deploy-prod", "needs-it", "from-docs"] {
        let e = (tool.handler)(l.state.clone(), ctx.clone(), json!({ "name": name })).await.err().unwrap();
        assert_eq!(e.code, "forbidden", "{name}");
        assert!(e.message.contains("ask the user"), "{}", e.message);
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(l.state.apps.runs.live("live", "deploy-prod").state, runs::RunState::Stopped);
    assert!(!l.root.join("shipped.log").exists());
}

/// A run whose program is not installed (a detected `go run ./cmd/api` without Go)
/// says so before it starts, as a problem, and after it fails, instead of only
/// "exited with code 127". (POSIX command lines: `bash -lc` runs them.)
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_programs_are_named() {
    let l = live_fixture(
        r#"
[[run]]
name = "api"
kind = "server"
command = "PORT=8080 wb-no-such-tool-4242 run ./cmd/api"
[[run]]
name = "inner"
command = "true && wb-no-such-tool-4243"
[[run]]
name = "fine"
command = "cd . && sh -c true"
"#,
    )
    .await;
    let p = l.state.projects.require("live").unwrap();
    let views = runs::list(&l.state, &p).await;
    let by: std::collections::HashMap<&str, &runs::RunView> = views.iter().map(|v| (v.name.as_str(), v)).collect();
    assert_eq!(by["api"].problems, vec![format!("`wb-no-such-tool-4242` is not installed (not found on PATH){}", crate::util::os::exe::INSTALLED_SINCE)]);
    assert!(by["inner"].problems.is_empty() && by["fine"].problems.is_empty(), "{:?} {:?}", by["inner"].problems, by["fine"].problems);

    runs::start(&l.state, &p, "api", false).await.unwrap();
    let live = wait_run(&l.state, "api", "failed", |x| x.state == runs::RunState::Failed).await;
    assert_eq!(live.error.as_deref(), Some("command not found: wb-no-such-tool-4242"));
    runs::start(&l.state, &p, "inner", false).await.unwrap();
    let live = wait_run(&l.state, "inner", "failed", |x| x.state == runs::RunState::Failed).await;
    assert_eq!(live.error.as_deref(), Some("exited with code 127 (command not found)"));
}

/// The program a command line starts, in each run shell's language.
#[test]
fn programs_of_command_lines() {
    use crate::util::os::shell::Dialect;
    for (cmd, prog) in [
        ("go run ./cmd/api", Some("go")),
        ("PORT=3000 npm start", Some("npm")),
        ("cd server && cargo build", Some("cargo")),
        ("env RUST_LOG=debug cargo run", Some("cargo")),
        ("./gradlew build", None),
        ("{unity} -batchmode", None),
        ("source .venv/bin/activate && pytest", None),
        ("echo hi", None),
        (". ./env.sh", None),
    ] {
        assert_eq!(runs::command_program_in(Dialect::Posix, cmd).as_deref(), prog, "{cmd}");
    }
    // The run shell on Windows: PowerShell answers its keywords, aliases and cmdlets.
    for (cmd, prog) in [
        ("npm run dev", Some("npm")),
        ("$env:PORT=3000; npm start", Some("npm")),
        ("cd server; cargo build", Some("cargo")),
        ("golangci-lint run", Some("golangci-lint")),
        (r".\build\Debug\app.exe", None),
        (r"& '.venv\Scripts\my tool.exe'", None),
        ("Remove-Item -Recurse dist", None),
        ("get-childitem", None),
        ("ls", None),
        ("if ($x) { make }", None),
    ] {
        assert_eq!(runs::command_program_in(Dialect::PowerShell, cmd).as_deref(), prog, "{cmd}");
    }
}

/// Two deploy requests racing through the (slow) planning phase: only one runs. A
/// branch name is never spliced into a deploy command when it could inject shell code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deploys_run_once_and_branch_names_cannot_inject() {
    let l = live_fixture(
        r#"
[[env]]
name = "sandbox"
url = "http://127.0.0.1:9"
deploy = { command = "echo DEPLOYED {sha8} >> deploys.log; sleep 1", local = true, confirm = "click" }

[[env]]
name = "branchy"
url = "http://127.0.0.1:9"
deploy = { command = "echo SANDBOX {sha8} on {branch} >> deploys.log", local = true, confirm = "click" }
"#,
    )
    .await;
    let p = l.state.projects.require("live").unwrap();
    let env = envs::find(&p, "sandbox").unwrap().clone();
    let (a, b) = tokio::join!(
        deploy::deploy(&l.state, &p, &env, None, &json!(true)),
        deploy::deploy(&l.state, &p, &env, None, &json!(true)),
    );
    let (ok, err): (Vec<_>, Vec<_>) = [a, b].into_iter().partition(|r| r.is_ok());
    assert_eq!((ok.len(), err.len()), (1, 1), "exactly one deploy starts");
    assert_eq!(err[0].as_ref().err().unwrap().status, StatusCode::CONFLICT);
    let tid = ok[0].as_ref().unwrap().id.clone();
    let plan = deploy::plan(&l.state, &p, &env, None).await.unwrap();
    assert_eq!(plan.deploying.as_deref(), Some(tid.as_str()));
    let mut rx = l.state.terminals.exit_watch(&tid).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.wait_for(|x| x.is_some())).await.unwrap().unwrap();
    // (Windows PowerShell 5.1 appends UTF-16.)
    let log = crate::util::os::shell::read_output(&l.root.join("deploys.log")).unwrap();
    assert_eq!(log.lines().count(), 1, "{log}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while deploy::deploying_terminal(&l.state, "live", "sandbox").is_some() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A failed plan releases the reservation.
    let e = deploy::deploy(&l.state, &p, &env, Some("deadbeef"), &json!(true)).await.unwrap_err();
    assert_eq!(e.code, "bad_request");
    assert!(l.state.apps.envs.deploys.lock().is_empty(), "no reservation is left behind");

    scratch_git(&l.root, &["checkout", "-q", "-b", "fix;touch${IFS}INJECTED"]);
    let branchy = envs::find(&p, "branchy").unwrap().clone();
    let e = deploy::deploy(&l.state, &p, &branchy, None, &json!(true)).await.unwrap_err();
    assert_eq!(e.code, "bad_request");
    assert!(e.message.contains("unsafe"), "{}", e.message);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!l.root.join("INJECTED").exists());
}
