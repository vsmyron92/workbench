//! Integration tests of the debug slice: a real AppState and router, and a fake DAP
//! adapter (`fake_dap.py`, run with Python 3: `os::exe::python`) that behaves like gdb's.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app::{self, AppState};

const FAKE: &str = include_str!("fake_dap.py");

fn have_python() -> bool {
    crate::util::which(&crate::util::os::exe::python()[0])
}

/// `s` as a TOML string (Windows paths hold backslashes).
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// A process that runs for half a minute and has nothing to do with the project.
fn sleeper() -> std::process::Child {
    let py = crate::util::os::exe::python();
    std::process::Command::new(&py[0]).args(&py[1..]).args(["-c", "import time; time.sleep(30)"]).spawn().unwrap()
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    state: AppState,
    addr: SocketAddr,
    http: reqwest::Client,
}

async fn setup(extra_toml: &str) -> Env {
    setup_with(extra_toml, Opts::default()).await
}

/// More of the machine's side: the project overlay, config.toml additions.
#[derive(Default)]
struct Opts<'a> {
    /// `config_dir/projects/proj.toml`.
    overlay: &'a str,
    /// Appended to config.toml's `[debug]` section (more adapters); `{python}`, `{dir}`
    /// and `{log}` become TOML strings of the interpreter, the temp dir and the log file.
    debug_toml: &'a str,
    /// A config.toml secret: `(name, value)`, kept in a 0600 file.
    secret: Option<(&'a str, &'a str)>,
    /// Files written into the project before it is loaded.
    files: &'a [(&'a str, &'a str)],
}

async fn setup_with(extra_toml: &str, o: Opts<'_>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    // The project's root as Workbench keeps it (Windows' temp dir may be spelled `RUNNER~1`).
    let base = crate::util::os::path::canonicalize(dir.path()).unwrap();
    let root = base.join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.c"), "int main() {\n  return 0;\n}\n").unwrap();
    std::fs::write(root.join("prog.bin"), "").unwrap();
    std::fs::write(
        root.join(".workbench.toml"),
        format!(
            r#"
[[debug]]
name = "fake"
adapter = "fake"
program = "prog.bin"
args = ["--flag"]
env = {{ PLAIN = "1" }}
{extra_toml}
"#
        ),
    )
    .unwrap();
    let script = base.join("fake_dap.py");
    std::fs::write(&script, FAKE).unwrap();
    let log = base.join("fake.log");
    for (path, text) in o.files {
        let f = root.join(path);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }
    let paths = crate::config::Paths { config_dir: base.join("config"), data_dir: base.join("data") };
    std::fs::create_dir_all(paths.config_dir.join("projects")).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    if !o.overlay.is_empty() {
        std::fs::write(paths.config_dir.join("projects/proj.toml"), o.overlay).unwrap();
    }
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![root.display().to_string()];
    cfg.agents.restore_on_start = false;
    // Python 3: `python3`; on Windows `python`, or `py` with its `-3`.
    let py = crate::util::os::exe::python();
    let args: Vec<String> = py[1..].iter().map(|a| toml_str(a)).chain([toml_str(&script.display().to_string())]).collect();
    cfg.debug = toml::from_str(&format!(
        r#"
[adapters.fake]
command = {}
args = [{}]
languages = ["c"]
env = {{ FAKE_LOG = {} }}
{}
"#,
        toml_str(&py[0]),
        args.join(", "),
        toml_str(&log.display().to_string()),
        o.debug_toml
            .replace("{python}", &toml_str(&py[0]))
            .replace("{dir}", &toml_str(&base.display().to_string()))
            .replace("{log}", &toml_str(&log.display().to_string()))
    ))
    .unwrap();
    if let Some((name, value)) = o.secret {
        let f = base.join("secret.txt");
        std::fs::write(&f, value).unwrap();
        crate::util::os::perm::apply(&f, 0o600).unwrap();
        cfg.secrets.insert(name.to_string(), crate::config::project::SecretRef::File(f.display().to_string()));
    }
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    crate::terminals::start(&state).await;
    super::start(&state).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app::build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    Env { _dir: dir, root, log, state, addr, http: reqwest::Client::new() }
}

impl Env {
    fn pid(&self) -> String {
        self.state.projects.list()[0].id.clone()
    }

    fn url(&self, rest: &str) -> String {
        format!("http://{}/api/projects/{}/debug/{rest}", self.addr, self.pid())
    }

    async fn get(&self, rest: &str) -> Value {
        let r = self.http.get(self.url(rest)).bearer_auth(self.state.auth.master_token()).send().await.unwrap();
        let status = r.status();
        let v: Value = r.json().await.unwrap();
        assert!(status.is_success(), "GET {rest}: {status} {v}");
        v
    }

    async fn send(&self, method: reqwest::Method, rest: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(method, self.url(rest)).bearer_auth(self.state.auth.master_token()).json(&body).send().await.unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, rest: &str, body: Value) -> Value {
        let (s, v) = self.send(reqwest::Method::POST, rest, body).await;
        assert!((200..300).contains(&s), "POST {rest}: {s} {v}");
        v
    }

    fn requests(&self, command: &str) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|m| m["command"] == command)
            .collect()
    }

    /// Wait until the session satisfies `f` (polling its info).
    async fn wait_session(&self, sid: &str, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        self.wait_session_for(sid, what, Duration::from_secs(15), f).await
    }

    async fn wait_session_for(&self, sid: &str, what: &str, limit: Duration, f: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let v = self.get(&format!("sessions/{sid}")).await;
            if f(&v) {
                return v;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("timed out waiting for {what}: {v:#}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn bp_of<'a>(view: &'a Value, line: u64) -> &'a Value {
    view["breakpoints"].as_array().unwrap().iter().find(|b| b["line"] == line).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_against_a_fake_adapter() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    let mut events = env.state.events.subscribe();

    // Configurations and adapters.
    let configs = env.get("configs").await;
    let fake = configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == "fake").unwrap();
    assert_eq!(fake["adapter"], "fake");
    assert_eq!(fake["adapterAvailable"], true, "{fake}");
    assert_eq!(fake["source"], ".workbench.toml");

    // Breakpoints persist per project (a line that cannot hold one: 150).
    let (s, v) = env
        .send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "./src/main.c", "breakpoints": [{ "line": 5 }, { "line": 150 }, { "line": 9, "enabled": false }] }))
        .await;
    assert_eq!(s, 200, "{v}");
    assert!(bp_of(&v, 5)["status"].is_null(), "no session: no verification yet");
    let (s, _) = env.send(reqwest::Method::PUT, "watches", json!({ "expressions": ["x", "x * 2"] })).await;
    assert_eq!(s, 200);

    // Start: the adapter is initialized, launched and configured, and stops at line 5.
    let info = env.post("sessions", json!({ "config": "fake" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    assert_eq!(info["state"], "starting");
    let info = env.wait_session(&sid, "the stop at the breakpoint", |v| v["state"] == "stopped").await;
    assert_eq!(info["stopped"]["reason"], "breakpoint");
    assert_eq!(info["stopped"]["threadId"], 1);
    let bps = env.get("breakpoints").await;
    let hit = bp_of(&bps, 5);
    assert_eq!(info["stopped"]["hitBreakpointIds"], json!([hit["id"]]), "the adapter's id maps back to ours");
    assert_eq!(hit["status"]["verified"], true);
    assert_eq!(bp_of(&bps, 150)["status"]["verified"], false);
    assert_eq!(bp_of(&bps, 150)["status"]["message"], "no code at this line");
    // Disabled breakpoints are not sent; the source path is the host path.
    let sent = env.requests("setBreakpoints");
    let main_c = env.root.join("src").join("main.c").display().to_string();
    assert_eq!(sent[0]["arguments"]["source"]["path"], main_c);
    assert_eq!(sent[0]["arguments"]["breakpoints"], json!([{ "line": 5 }, { "line": 150 }]));
    // Launch arguments: program resolved in the project, args, env, cwd.
    let launch = &env.requests("launch")[0]["arguments"];
    assert_eq!(launch["program"], env.root.join("prog.bin").display().to_string());
    assert_eq!(launch["args"], json!(["--flag"]));
    assert_eq!(launch["env"]["PLAIN"], "1");
    assert_eq!(launch["cwd"], env.root.display().to_string());
    // Exception filters: the adapter's defaults.
    assert_eq!(env.requests("setExceptionBreakpoints")[0]["arguments"]["filters"], json!(["throw"]));

    // Frames map sources to project paths.
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["source"]["path"], "src/main.c");
    assert_eq!(stack["frames"][0]["source"]["inProject"], true);
    assert_eq!(stack["frames"][1]["source"]["inProject"], false);
    let scopes = env.get(&format!("sessions/{sid}/scopes?frameId=1000")).await;
    assert_eq!(scopes["scopes"][0]["name"], "Locals");
    let vars = env.get(&format!("sessions/{sid}/variables?ref=1")).await;
    assert_eq!(vars["variables"][0], json!({"name": "x", "value": "41", "type": "int", "variablesReference": 0, "namedVariables": null, "indexedVariables": null, "evaluateName": null, "presentationHint": null, "memoryReference": null}));
    let children = env.get(&format!("sessions/{sid}/variables?ref=2")).await;
    assert_eq!(children["variables"][0]["name"], "y");

    // Watches, the console (a failing expression is shown, not fatal), set value.
    let w = env.post(&format!("sessions/{sid}/evaluate"), json!({ "expression": "x", "frameId": 1000, "context": "watch" })).await;
    assert_eq!(w["value"], "41");
    let (s, e) = env.send(reqwest::Method::POST, &format!("sessions/{sid}/evaluate"), json!({ "expression": "boom", "frameId": 1000, "context": "repl" })).await;
    assert_eq!((s, e["error"]["message"].as_str()), (422, Some("No symbol \"boom\" in current context.")));
    let set = env.post(&format!("sessions/{sid}/set-variable"), json!({ "variablesReference": 1, "name": "x", "value": "5" })).await;
    assert_eq!(set["value"], "5");
    let c = env.post(&format!("sessions/{sid}/completions"), json!({ "text": "xy", "column": 3, "frameId": 1000 })).await;
    assert_eq!(c["targets"][0]["label"], "xylophone");
    let out = env.get(&format!("sessions/{sid}/output")).await;
    let text: String = out["lines"].as_array().unwrap().iter().map(|l| format!("[{}]{}", l["category"].as_str().unwrap(), l["text"].as_str().unwrap())).collect();
    assert!(text.contains("[stdout]hello from the debuggee"), "{text}");
    assert!(text.contains("[adapter]fake adapter ready"), "adapter stderr: {text}");
    assert!(text.contains("[adapter]fake adapter banner"), "non-DAP stdout: {text}");
    assert!(text.contains("[repl-in]> boom") && text.contains("[repl-err]No symbol"), "{text}");

    // Agents can read the state but never control a session.
    let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == "debug_state").unwrap();
    let ctx = crate::mcp::McpCtx::default();
    let r = (tool.handler)(env.state.clone(), ctx.clone(), json!({ "projectId": env.pid() })).await.unwrap();
    let crate::mcp::ToolOutput::Json(r) = r else { panic!("json") };
    let s0 = &r["sessions"][0];
    assert_eq!(s0["state"], "stopped");
    assert_eq!(s0["stop"]["reason"], "breakpoint");
    assert!(s0["stack"][0].as_str().unwrap().starts_with("#0 work at src/main.c:5"), "{s0}");
    let locals = s0["locals"].to_string();
    assert!(locals.contains("\"x\"") && !locals.contains("rip"), "registers stay out: {locals}");
    for (method, path, body) in [
        (axum::http::Method::POST, format!("sessions/{sid}/control"), json!({ "action": "continue" })),
        (axum::http::Method::POST, format!("sessions/{sid}/evaluate"), json!({ "expression": "x" })),
        (axum::http::Method::POST, "sessions".to_string(), json!({ "config": "fake" })),
        (axum::http::Method::PUT, "breakpoints/file".to_string(), json!({ "path": "src/main.c", "breakpoints": [] })),
        (axum::http::Method::POST, format!("sessions/{sid}/stop"), json!({})),
    ] {
        let p = format!("/api/projects/{}/debug/{path}", env.pid());
        let err = crate::mcp::call_api(&env.state, method, &p, Some(body), &ctx).await.unwrap_err();
        assert_eq!(err.status, 403, "{path}: {err}");
    }

    // A watch that calls a function that stops: the stop says so (the UI then stops
    // evaluating that watch by itself, or watches would loop through breakpoints).
    let before = env.get(&format!("sessions/{sid}")).await["stopEpoch"].as_u64().unwrap();
    let (s, _) = env.send(reqwest::Method::POST, &format!("sessions/{sid}/evaluate"), json!({ "expression": "call_stop()", "frameId": 1000, "context": "watch" })).await;
    assert_eq!(s, 422);
    let info = env.wait_session(&sid, "the stop inside the evaluation", |v| v["stopEpoch"].as_u64().unwrap() > before).await;
    assert_eq!(info["stopped"]["duringEvaluation"], "call_stop()");

    // Step over: running, then stopped again with a new epoch.
    let epoch = info["stopEpoch"].as_u64().unwrap();
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "next" })).await;
    let info = env.wait_session(&sid, "the step", |v| v["state"] == "stopped" && v["stopEpoch"].as_u64().unwrap() > epoch + 1).await;
    assert_eq!(info["stopped"]["reason"], "step");

    // Run to cursor: a temporary breakpoint, gone after the stop.
    env.post(&format!("sessions/{sid}/run-to"), json!({ "path": "src/main.c", "line": 20 })).await;
    env.wait_session(&sid, "run to line 20", |v| v["state"] == "stopped" && v["stopped"]["reason"] == "breakpoint").await;
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["line"], 20);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let last = env.requests("setBreakpoints").last().cloned().unwrap();
    assert_eq!(last["arguments"]["breakpoints"], json!([{ "line": 5 }, { "line": 150 }]), "the temporary breakpoint is removed");

    // A breakpoint added while suspended reaches the session at once.
    let (s, v) = env
        .send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }, { "line": 30, "condition": "x > 1" }] }))
        .await;
    assert_eq!(s, 200);
    assert_eq!(bp_of(&v, 30)["status"]["verified"], true);
    assert_eq!(env.requests("setBreakpoints").last().unwrap()["arguments"]["breakpoints"][1], json!({ "line": 30, "condition": "x > 1" }));

    // Continue to the end: output, exit code, terminated; the adapter is told to go.
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    env.wait_session(&sid, "line 30", |v| v["state"] == "stopped").await;
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    let end = env.wait_session(&sid, "the end", |v| v["state"] == "terminated").await;
    assert_eq!(end["exitCode"], 3);
    assert!(end["endedAt"].is_number());
    assert_eq!(env.requests("disconnect").len(), 1);
    let out = env.get(&format!("sessions/{sid}/output")).await;
    let text: String = out["lines"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap().to_string()).collect();
    assert!(text.contains("bye\n") && text.contains("Process finished with exit code 3"), "{text}");
    // Without a session, breakpoints carry no verification.
    assert!(bp_of(&env.get("breakpoints").await, 5)["status"].is_null());

    // The UI heard about it through events.
    let mut kinds = std::collections::HashSet::new();
    let mut outputs = String::new();
    while let Ok(ev) = events.try_recv() {
        kinds.insert(ev.kind.clone());
        if ev.kind == "debug.output" {
            for l in ev.data["lines"].as_array().unwrap() {
                outputs.push_str(l["text"].as_str().unwrap());
            }
        }
    }
    assert!(kinds.contains("debug.session") && kinds.contains("debug.breakpoints") && kinds.contains("debug.output"), "{kinds:?}");
    assert!(outputs.contains("hello from the debuggee"));

    // Ended sessions can be forgotten.
    let (s, _) = env.send(reqwest::Method::DELETE, &format!("sessions/{sid}"), json!({})).await;
    assert_eq!(s, 200);
    assert!(env.get("sessions").await.as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_terminates_and_debuggee_terminals_are_killed() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("extra = { wantTerminal = true }").await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 2 }] })).await;
    let info = env.post("sessions", json!({ "config": "fake" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    let info = env.wait_session(&sid, "the stop", |v| v["state"] == "stopped" && v["debuggeeTerminalId"].is_string()).await;
    // runInTerminal: the debuggee got a Workbench terminal of the project.
    let tid = info["debuggeeTerminalId"].as_str().unwrap().to_string();
    let t = env.state.terminals.info(&tid).unwrap();
    assert_eq!(t.meta["debug"], sid);
    assert_eq!(t.project_id.as_deref(), Some(env.pid().as_str()));
    assert_eq!(t.status, crate::terminals::TerminalStatus::Running);

    let end = env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert_eq!(end["state"], "terminated");
    assert_eq!(env.requests("terminate").len(), 1, "a launch is terminated, not detached");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while env.state.terminals.info(&tid).unwrap().status != crate::terminals::TerminalStatus::Exited {
        assert!(tokio::time::Instant::now() < deadline, "the debuggee terminal was not killed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Stopping again is harmless.
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_launch_commands_run_in_a_terminal_and_failures_end_the_session() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup(
        r#"pre_launch = "echo built > built.txt"

[[debug]]
name = "broken"
adapter = "fake"
program = "prog.bin"
preLaunch = "echo compiling; exit 7"

[[debug]]
name = "nowhere"
adapter = "missing"
program = "prog.bin"
"#,
    )
    .await;
    let info = env.post("sessions", json!({ "config": "fake" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    // No breakpoints: the fake program runs to its end.
    let end = env.wait_session(&sid, "the end", |v| v["state"] == "terminated").await;
    assert!(end["prelaunchTerminalId"].is_string());
    // Byte for byte on Unix (Windows PowerShell 5.1 writes UTF-16 and CRLF).
    let built = crate::util::os::shell::read_output(&env.root.join("built.txt")).unwrap();
    if cfg!(unix) {
        assert_eq!(built, "built\n");
    } else {
        assert_eq!(built.trim_end(), "built");
    }
    assert_eq!(end["exitCode"], 0);

    let info = env.post("sessions", json!({ "config": "broken" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    let end = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    assert!(end["error"].as_str().unwrap().contains("exit code 7"), "{end}");
    assert!(env.requests("initialize").len() == 1, "no adapter starts after a failed pre-launch step");

    // An adapter config.toml does not define: setup help, nothing starts.
    let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "nowhere" })).await;
    assert_eq!((s, v["error"]["code"].as_str()), (412, Some("not_configured")), "{v}");
    let (s, _) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "no such config" })).await;
    assert_eq!(s, 404);
}

/// A pre-launch run configuration whose terminal is killed (or closed) was terminated: the
/// error says so, not "failed (exit code 1)" with the code portable-pty gives a signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pre_launch_run_cut_short_was_terminated() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    // `sleep` is the same in bash and PowerShell.
    let env = setup(
        r#"
[[debug]]
name = "after a run"
adapter = "fake"
program = "prog.bin"
preLaunch = "wait"

[[run]]
name = "wait"
command = "sleep 30"
"#,
    )
    .await;
    let info = env.post("sessions", json!({ "config": "after a run" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    let v = env.wait_session(&sid, "the pre-launch run's terminal", |v| v["prelaunchTerminalId"].is_string()).await;
    let tid = v["prelaunchTerminalId"].as_str().unwrap().to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while env.state.terminals.info(&tid).unwrap().status != crate::terminals::TerminalStatus::Running {
        assert!(tokio::time::Instant::now() < deadline, "the pre-launch run did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    env.state.terminals.kill(&tid).await.unwrap();
    let end = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    assert!(end["error"].as_str().unwrap().contains("pre-launch run \"wait\" was terminated"), "{end}");
    assert!(env.requests("initialize").is_empty(), "no adapter starts after a pre-launch run cut short");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapters_can_start_child_sessions() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("extra = { wantChild = true }").await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 2 }] })).await;
    let info = env.post("sessions", json!({ "config": "fake" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    // `startDebugging`: a child session of the same adapter, with the configuration
    // the adapter gave (sent as is), listed under the parent.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let child = loop {
        let list = env.get("sessions").await;
        if let Some(c) = list.as_array().unwrap().iter().find(|s| s["parentId"] == sid.as_str()) {
            break c.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "no child session: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(child["name"], "fake child");
    let cid = child["id"].as_str().unwrap().to_string();
    env.wait_session(&cid, "the child's stop", |v| v["state"] == "stopped").await;
    let launches = env.requests("launch");
    assert!(launches.iter().any(|l| l["arguments"]["childOf"] == "fake"), "{launches:?}");
    for s in [&cid, &sid] {
        env.post(&format!("sessions/{s}/stop"), json!({})).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_ends_live_sessions() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 2 }] })).await;
    let info = env.post("sessions", json!({ "config": "fake" })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    super::shutdown(&env.state).await;
    let s = env.state.debug.get(&env.pid(), &sid).unwrap();
    assert!(!s.is_live());
    let (status, _) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "fake" })).await;
    assert_eq!(status, 409, "no new sessions while shutting down");
}

impl Env {
    /// `debug_state` as an agent of the project would call it.
    async fn mcp_state(&self) -> Value {
        let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == "debug_state").unwrap();
        let r = (tool.handler)(self.state.clone(), crate::mcp::McpCtx::default(), json!({ "projectId": self.pid() })).await.unwrap();
        match r {
            crate::mcp::ToolOutput::Json(v) => v,
            crate::mcp::ToolOutput::Text(t) => json!(t),
        }
    }

    async fn start(&self, config: &str) -> String {
        let info = self.post("sessions", json!({ "config": config })).await;
        info["id"].as_str().unwrap().to_string()
    }
}

/// Adapters run outside the project (`python3 -m …` puts its working directory first
/// on `sys.path`): a repository's `json.py` or `debugpy/` is never imported by an
/// adapter, for a launch or for an attach to an unrelated process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapters_never_import_the_repositorys_modules() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("REPO_CODE_RAN");
    let evil = format!("open({:?}, 'a').write(__name__ + '\\n')\n", marker.display().to_string());
    // The adapter module lives on PYTHONPATH (like debugpy); the project shadows the
    // modules it imports.
    let modules = dir.path().join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("fakemod.py"), FAKE).unwrap();
    let debug_toml = format!(
        r#"
[adapters.fakemod]
kind = "debugpy"
command = {{python}}
args = ["-m", "fakemod"]
languages = ["python"]
env = {{ FAKE_LOG = {{log}}, PYTHONPATH = {} }}
"#,
        toml_str(&modules.display().to_string())
    );
    // debugpy's availability probe imports `debugpy`.
    std::fs::create_dir_all(modules.join("debugpy")).unwrap();
    std::fs::write(modules.join("debugpy/__init__.py"), "__version__ = \"0-test\"\n").unwrap();
    let files: Vec<(&str, &str)> = vec![("json.py", &evil), ("socket.py", &evil), ("debugpy/__init__.py", &evil), ("fakemod.py", &evil)];
    let env = setup_with(
        r#"
[[debug]]
name = "py"
adapter = "fakemod"
program = "prog.bin"
"#,
        Opts { debug_toml: &debug_toml, files: &files, ..Default::default() },
    )
    .await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 2 }] })).await;
    let sid = env.start("py").await;
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    // The program's working directory is still the project's (a launch argument).
    assert_eq!(env.requests("launch")[0]["arguments"]["cwd"], env.root.display().to_string());
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;

    // Attach to a process that has nothing to do with the project.
    let mut victim = sleeper();
    let (s, v) = env.send(reqwest::Method::POST, "sessions/attach", json!({ "pid": victim.id(), "adapter": "fakemod" })).await;
    assert_eq!(s, 200, "{v}");
    let aid = v["id"].as_str().unwrap().to_string();
    env.wait_session(&aid, "the attach", |v| v["state"] != "starting").await;
    assert_eq!(env.requests("attach")[0]["arguments"]["processId"], victim.id());
    env.post(&format!("sessions/{aid}/stop"), json!({})).await;
    let _ = victim.kill();
    let _ = victim.wait();
    assert!(!marker.exists(), "the adapter imported the repository's modules: {}", std::fs::read_to_string(&marker).unwrap_or_default());
}

/// `${secret:…}` values of the launch env are masked in everything that shows what
/// the program holds: variables, evaluations (watch, console, hover), the stop's
/// text, and `debug_state` for agents — not only in the console.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secret_values_reach_no_rest_or_mcp_answer() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    const SECRET: &str = "SuperSecretValue-9f3a7c";
    let env = setup_with(
        "",
        Opts {
            overlay: r#"
[[debug]]
name = "leaky"
adapter = "fake"
program = "prog.bin"
env = { TOKEN = "${secret:api_token}" }
"#,
            secret: Some(("api_token", SECRET)),
            ..Default::default()
        },
    )
    .await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await;
    let sid = env.start("leaky").await;
    let info = env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    assert_eq!(env.requests("launch")[0]["arguments"]["env"]["TOKEN"], SECRET, "the program gets the value");
    let mut answers = vec![info.clone()];
    answers.push(env.get(&format!("sessions/{sid}/stack?threadId=1")).await);
    let vars = env.get(&format!("sessions/{sid}/variables?ref=1")).await;
    let tok = vars["variables"].as_array().unwrap().iter().find(|v| v["name"] == "tok").unwrap();
    assert_eq!(tok["value"], "\"••••••\"");
    answers.push(vars);
    for context in ["watch", "repl", "hover", "clipboard"] {
        answers.push(env.post(&format!("sessions/{sid}/evaluate"), json!({ "expression": "tok", "frameId": 1000, "context": context })).await);
    }
    answers.push(env.get(&format!("sessions/{sid}/output")).await);
    answers.push(env.get("sessions").await);
    let mcp = env.mcp_state().await;
    assert!(mcp.to_string().contains("tok"), "{mcp}");
    answers.push(mcp);
    for a in &answers {
        assert!(!a.to_string().contains(SECRET), "a secret value leaked: {a}");
    }
    assert!(info["stopped"]["text"].as_str().unwrap().contains("••••••"));
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// Stop ends a session as stopped (not "exited with code 0"); an adapter that dies
/// fails the session with its exit status and last words.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_and_adapter_crashes_are_told_apart() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await;
    let sid = env.start("fake").await;
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    let end = env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert_eq!(end["state"], "terminated");
    assert_eq!(end["stopRequested"], true);
    assert!(end.get("exitCode").is_none(), "the killed program's code is not its exit code: {end}");
    let out = env.get(&format!("sessions/{sid}/output")).await.to_string();
    assert!(out.contains("Process stopped") && !out.contains("exit code 0"), "{out}");

    // The adapter dies.
    let sid = env.start("fake").await;
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    let _ = env.send(reqwest::Method::POST, &format!("sessions/{sid}/evaluate"), json!({ "expression": "crash()", "frameId": 1000, "context": "repl" })).await;
    let end = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    let error = end["error"].as_str().unwrap();
    assert!(error.contains("exited unexpectedly (exit code 9)") && error.contains("fatal: simulated crash"), "{error}");
    assert!(end.get("stopRequested").is_none());

    // A program that ends by itself keeps its exit code.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [] })).await;
    let sid = env.start("fake").await;
    let end = env.wait_session(&sid, "the end", |v| v["state"] == "terminated").await;
    assert_eq!(end["exitCode"], 0);
    assert!(end.get("error").is_none() && end.get("stopRequested").is_none(), "{end}");
}

/// Ended sessions beyond the six kept are forgotten, and the UI is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pruned_sessions_leave_the_ui() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    let mut events = env.state.events.subscribe();
    let mut ids = vec![];
    for _ in 0..8 {
        // No breakpoints: each runs to its end.
        let sid = env.start("fake").await;
        env.wait_session(&sid, "the end", |v| v["state"] == "terminated").await;
        ids.push(sid);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let listed: Vec<String> = env.get("sessions").await.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(listed.len(), 6);
    let mut removed = vec![];
    while let Ok(ev) = events.try_recv() {
        if ev.kind == "debug.session" && ev.data["removed"] == true {
            removed.push(ev.data["id"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(removed, ids[..2].to_vec());
}

/// An attach configuration without a pid asks for one (`pid_required`), then attaches
/// to the process the user picked; Rerun attaches to it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attach_configurations_without_a_pid_ask_for_one() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup(
        r#"
[[debug]]
name = "attach-nopid"
adapter = "fake"
request = "attach"
program = "prog.bin"
"#,
    )
    .await;
    let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "attach-nopid" })).await;
    assert_eq!((s, v["error"]["code"].as_str()), (400, Some("pid_required")), "{v}");
    assert!(env.requests("initialize").is_empty(), "nothing started");
    let (s, _) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "attach-nopid", "pid": 1 })).await;
    assert_eq!(s, 400);

    let mut victim = sleeper();
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await;
    let info = env.post("sessions", json!({ "config": "attach-nopid", "pid": victim.id() })).await;
    let sid = info["id"].as_str().unwrap().to_string();
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    assert_eq!(env.requests("attach")[0]["arguments"]["pid"], victim.id());
    let again = env.post(&format!("sessions/{sid}/restart"), json!({})).await;
    let rid = again["id"].as_str().unwrap().to_string();
    env.wait_session(&rid, "the second stop", |v| v["state"] == "stopped").await;
    assert_eq!(env.requests("attach")[1]["arguments"]["pid"], victim.id());
    // An attach detaches: the process keeps running.
    let end = env.post(&format!("sessions/{rid}/stop"), json!({})).await;
    assert_eq!(end["stopRequested"], true);
    assert_eq!(env.requests("disconnect").last().unwrap()["arguments"]["terminateDebuggee"], false);
    assert!(victim.try_wait().unwrap().is_none());
    let _ = victim.kill();
    let _ = victim.wait();
}

/// debugpy-style child sessions: `startDebugging` with `configuration.connect` makes
/// the child talk to the parent's adapter over a new loopback connection (the adapter
/// knows the subprocess; a new adapter would not). Stopping the parent stops it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_sessions_connect_to_the_parents_adapter() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("extra = { wantChildConnect = true }").await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 2 }] })).await;
    let sid = env.start("fake").await;
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let child = loop {
        let list = env.get("sessions").await;
        if let Some(c) = list.as_array().unwrap().iter().find(|s| s["parentId"] == sid.as_str()) {
            break c.clone();
        }
        assert!(tokio::time::Instant::now() < deadline, "no child session: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(child["name"], "fake subprocess");
    let cid = child["id"].as_str().unwrap().to_string();
    // The stop is published before its threads arrive: wait for both.
    let c = env.wait_session(&cid, "the child's stop with its threads", |v| v["state"] == "stopped" && v["threads"][0]["name"].is_string()).await;
    assert_eq!(c["threads"][0]["name"], "subprocess main");
    let log: Vec<Value> = std::fs::read_to_string(&env.log).unwrap().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let child_attach = log.iter().find(|m| m["command"] == "attach" && m["_child"] == true).expect("the child attached over the connection");
    assert_eq!(child_attach["arguments"]["subProcessId"], 99);
    assert_eq!(log.iter().filter(|m| m["command"] == "initialize" && m["_child"].is_null()).count(), 1, "no second adapter");
    // Stopping the parent stops its child first (on the parent's adapter).
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    let c = env.wait_session(&cid, "the child's end", |v| v["state"] == "terminated").await;
    assert!(c.get("error").is_none(), "{c}");
    let log = std::fs::read_to_string(&env.log).unwrap();
    assert!(log.lines().any(|l| l.contains("\"disconnect\"") && l.contains("\"_child\": true")), "the child disconnected: {log}");
}

/// Frames outside the project: their files are readable for the session's source
/// view — only files the adapter named, never Workbench's own files, never by agents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_outside_the_project_are_served_only_when_a_frame_named_them() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let lib = tempfile::tempdir().unwrap();
    let header = lib.path().join("helper.h");
    std::fs::write(&header, "int helper(int);\n").unwrap();
    let env = setup("").await;
    let private = env.state.paths.data_dir.join("auth-ish.json");
    std::fs::write(&private, "{\"token\": \"x\"}").unwrap();
    // The launch configuration names both as sources (like debug information would).
    let text = std::fs::read_to_string(env.root.join(".workbench.toml")).unwrap();
    std::fs::write(
        env.root.join(".workbench.toml"),
        text.replace("env = { PLAIN = \"1\" }", &format!("env = {{ PLAIN = \"1\" }}\nextra = {{ libSource = {:?}, privateSource = {:?} }}", header.display().to_string(), private.display().to_string())),
    )
    .unwrap();
    env.state.projects.reload(&env.state).await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await;
    let sid = env.start("fake").await;
    env.wait_session(&sid, "the stop", |v| v["state"] == "stopped").await;
    let q = |p: &std::path::Path| format!("sessions/{sid}/file?path={}", urlencode(&p.display().to_string()));
    let (s, _) = env.send(reqwest::Method::GET, &q(&header), Value::Null).await;
    assert_eq!(s, 403, "not named by a frame yet");
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][1]["source"]["path"], header.display().to_string());
    assert_eq!(stack["frames"][1]["source"]["inProject"], false);
    let f = env.get(&q(&header)).await;
    assert_eq!(f["content"], "int helper(int);\n");
    let (s, _) = env.send(reqwest::Method::GET, &q(&private), Value::Null).await;
    assert_eq!(s, 403, "Workbench's own files are never shown");
    let (s, _) = env.send(reqwest::Method::GET, &q(std::path::Path::new("/etc/passwd")), Value::Null).await;
    assert_eq!(s, 403);
    let ctx = crate::mcp::McpCtx::default();
    let p = format!("/api/projects/{}/debug/{}", env.pid(), q(&header));
    let err = crate::mcp::call_api(&env.state, axum::http::Method::GET, &p, None, &ctx).await.unwrap_err();
    assert_eq!(err.status, 403, "agents read sessions through debug_state");
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_./".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Real debugpy (when `WORKBENCH_TEST_DEBUGPY` names a directory to put on
/// PYTHONPATH that holds it): a program that runs Python subprocesses is debugged
/// with a child session per subprocess, whose breakpoints hit; stopping the parent
/// ends the child and its process. The project shadows `debugpy` and `platform`:
/// the adapter never imports them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_debugpy_debugs_subprocesses() {
    let Ok(pythonpath) = std::env::var("WORKBENCH_TEST_DEBUGPY") else {
        eprintln!("skipped: set WORKBENCH_TEST_DEBUGPY to a PYTHONPATH directory holding debugpy");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("REPO_CODE_RAN");
    let evil = format!("open({:?}, 'a').write(__name__ + '\\n')\nraise RuntimeError('repository module imported')\n", marker.display().to_string());
    let parent = "import subprocess, sys\nprint('parent starting', flush=True)\nsubprocess.run([sys.executable, 'child.py'])\nprint('parent done', flush=True)\n";
    let child = "def work():\n    x = 41\n    x += 1\n    print('child x', x, flush=True)\nwork()\n";
    let files: Vec<(&str, &str)> = vec![("parent.py", parent), ("child.py", child), ("platform.py", &evil), ("debugpy/__init__.py", &evil)];
    let debug_toml = format!("[adapters.debugpy]\nenv = {{ PYTHONPATH = {pythonpath:?} }}\n");
    let env = setup_with(
        r#"
[[debug]]
name = "parent"
program = "parent.py"
language = "python"
console = "console"
"#,
        Opts { debug_toml: &debug_toml, files: &files, ..Default::default() },
    )
    .await;
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "child.py", "breakpoints": [{ "line": 4 }] })).await;

    let find_child = |sid: String| {
        let env = &env;
        async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
            loop {
                let list = env.get("sessions").await;
                if let Some(c) = list.as_array().unwrap().iter().find(|s| s["parentId"] == sid.as_str() && s["state"] == "stopped") {
                    return c.clone();
                }
                assert!(tokio::time::Instant::now() < deadline, "no stopped child session: {list:#}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };

    // 1. The child's breakpoint hits; resuming lets both end normally.
    let sid = env.start("parent").await;
    let child = find_child(sid.clone()).await;
    let cid = child["id"].as_str().unwrap().to_string();
    let tid = child["stopped"]["threadId"].as_i64().unwrap();
    let stack = env.get(&format!("sessions/{cid}/stack?threadId={tid}")).await;
    assert_eq!((stack["frames"][0]["source"]["path"].as_str(), stack["frames"][0]["line"].as_i64()), (Some("child.py"), Some(4)), "{stack}");
    env.post(&format!("sessions/{cid}/control"), json!({ "action": "continue", "threadId": tid })).await;
    let end = env.wait_session_for(&sid, "the parent's end", Duration::from_secs(40), |v| !matches!(v["state"].as_str(), Some("starting" | "running" | "stopped"))).await;
    assert_eq!(end["state"], "terminated", "{end}");
    let out = env.get(&format!("sessions/{sid}/output")).await.to_string();
    assert!(out.contains("parent done"), "{out}");
    let c = env.get(&format!("sessions/{cid}")).await;
    assert_eq!(c["state"], "terminated", "{c}");

    // 2. Stop the parent while the child is suspended: both end, and so does the
    // subprocess.
    let sid = env.start("parent").await;
    let child = find_child(sid.clone()).await;
    let cid = child["id"].as_str().unwrap().to_string();
    let child_pid = child["process"]["pid"].as_i64();
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    let c = env.wait_session(&cid, "the child's end", |v| v["state"] == "terminated").await;
    assert!(c.get("error").is_none(), "{c}");
    if let Some(p) = child_pid {
        let p = u32::try_from(p).expect("a pid");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while crate::util::os::proc::pid_running(p) {
            assert!(tokio::time::Instant::now() < deadline, "the subprocess {p} outlived the stop");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    assert!(!marker.exists(), "the adapter imported the repository's modules: {}", std::fs::read_to_string(&marker).unwrap_or_default());
}
