//! Integration tests of the debug slice: a real AppState and router, and a fake DAP
//! adapter (`fake_dap.py`, run with Python 3: `os::exe::python`) that behaves like gdb's.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app::{self, AppState};

const FAKE: &str = include_str!("fake_dap.py");
const FAKE_SERVER: &str = include_str!("fake_server.py");

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
    /// and `{log}` become TOML strings of the interpreter, the temp dir and the log file,
    /// `{fake}`, `{server}` and `{pidfile}` of the fake adapter, the fake debug server
    /// and the file that records the server's pid, `{pyargs}` the interpreter's launcher
    /// arguments (`"-3", ` for `py` on Windows, else nothing).
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
    let server_script = base.join("fake_server.py");
    std::fs::write(&server_script, FAKE_SERVER).unwrap();
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
            .replace("{pyargs}", &py[1..].iter().map(|a| format!("{}, ", toml_str(a))).collect::<String>())
            .replace("{fake}", &toml_str(&script.display().to_string()))
            .replace("{server}", &toml_str(&server_script.display().to_string()))
            .replace("{pidfile}", &toml_str(&base.join("server.pid").display().to_string()))
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
                // What the session had said by then: a start that hangs is told apart from a slow one by it.
                let console = self.console(sid).await;
                let tail: String = console.chars().rev().take(3000).collect::<Vec<_>>().into_iter().rev().collect();
                panic!("timed out waiting for {what}: {v:#}\nthe console's tail:\n{tail}");
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

// ---------------------------------------------------------------- remote targets

/// A gdb-kind adapter and debug servers for the remote-target tests: `fakesrv` listens
/// after `--delay`, `impatient` is given a second to do so, `noport` has no `{port}`.
const REMOTE_DEBUG: &str = r#"
[adapters.fakegdb]
kind = "gdb"
command = {python}
args = [{pyargs}{fake}]
languages = ["embedded"]
env = { FAKE_LOG = {log}, FAKE_SLOW_LOAD = "0.6" }

[servers.fakesrv]
label = "Fake server"
command = {python}
args = [{pyargs}{server}, "--port", "{port}", "--also", "{port2}", "--pidfile", {pidfile}]
init = ["monitor halt"]
reset = ["monitor reset halt"]
download = true

[servers.impatient]
label = "Impatient server"
command = {python}
args = [{pyargs}{server}, "--port", "{port}", "--pidfile", {pidfile}]
ready_timeout_s = 1

[servers.noport]
command = {python}
args = [{pyargs}{server}]
"#;

async fn remote_env(project_toml: &str) -> Env {
    let env = setup_with(project_toml, Opts { debug_toml: REMOTE_DEBUG, ..Default::default() }).await;
    // The fake is a gdb to Workbench (remote targets need one) that `--version` cannot vouch for.
    let cfg = env.state.config.read().debug.clone();
    let fake = super::adapters::find(&cfg, "fakegdb").unwrap();
    super::adapters::seed_probe(&env.state, &fake, super::adapters::Availability { available: true, path: None, version: Some("fake gdb 99".into()), problem: None });
    env
}

impl Env {
    /// The pid the fake debug server recorded (waiting for it to start).
    async fn server_pid(&self) -> u32 {
        let file = self.root.parent().unwrap().join("server.pid");
        for _ in 0..200 {
            if let Some(pid) = std::fs::read_to_string(&file).ok().and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the fake server never wrote its pid");
    }

    /// The gdb commands sent through the console channel, in order.
    fn commands(&self) -> Vec<String> {
        self.requests("evaluate").iter().map(|r| r["arguments"]["expression"].as_str().unwrap_or("").to_string()).collect()
    }

    async fn console(&self, sid: &str) -> String {
        let out = self.get(&format!("sessions/{sid}/output")).await;
        out["lines"].as_array().unwrap().iter().map(|l| format!("[{}]{}", l["category"].as_str().unwrap(), l["text"].as_str().unwrap())).collect()
    }
}

async fn gone(pid: u32, secs: u64) -> bool {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    while crate::util::os::proc::pid_alive(pid) {
        if tokio::time::Instant::now() > end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

/// A remote target end to end against a fake gdb and a fake debug server: the server
/// starts on free ports and is waited for without being connected to, gdb attaches to
/// it (`target`, no `pid`), the target is initialised, reset, flashed, reset again and
/// run to `main` while the session still says "starting", and Stop ends the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_target_gets_its_server_a_download_and_a_run_to_main() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
stop_on_entry = true
[debug.remote]
server = "fakesrv"
server_args = ["--delay", "0.3"]
"#,
    )
    .await;

    // What the Start view will show: the command as it will run, nothing hidden.
    let configs = env.get("configs").await;
    let board = configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == "board").unwrap();
    assert_eq!(board["request"], "attach");
    assert_eq!(board["adapter"], "fakegdb");
    assert_eq!(board["problems"], json!([]), "{board}");
    let r = &board["remote"];
    assert_eq!((r["server"].clone(), r["serverLabel"].clone(), r["serverAvailable"].clone(), r["download"].clone()), (json!("fakesrv"), json!("Fake server"), json!(true), json!(true)));
    assert_eq!((r["init"].clone(), r["reset"].clone()), (json!(["monitor halt"]), json!(["monitor reset halt"])), "the server's defaults");
    let line = r["commandLine"].as_str().unwrap();
    assert!(line.contains("--port {port}") && line.ends_with("--delay 0.3"), "{line}");
    let servers = env.get("servers").await;
    assert!(servers["servers"].as_array().unwrap().iter().any(|s| s["id"] == "fakesrv" && s["availability"]["available"] == true), "{servers}");

    let mut events = env.state.events.subscribe();
    let sid = env.start("board").await;
    // The download takes 0.6 s: all that time the session is still starting, not stopped
    // (gdb's stop on connecting is held back), and says what it is doing.
    let mut saw_download = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let v = env.get(&format!("sessions/{sid}")).await;
        if v["phase"].as_str().is_some_and(|p| p.starts_with("Downloading")) {
            saw_download = true;
            assert_eq!((v["state"].clone(), v["stopped"].clone()), (json!("starting"), Value::Null), "{v}");
        }
        if v["state"] == "stopped" || v["state"] == "failed" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no stop: {v}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(saw_download, "never saw the download phase");
    let info = env.get(&format!("sessions/{sid}")).await;
    assert_eq!(info["state"], "stopped", "{info}");
    assert_eq!(info["stopped"]["reason"], "breakpoint");
    assert_eq!(info["stopped"]["description"], "Temporary breakpoint 1, main ()");
    assert_eq!(info["request"], "attach");
    // No "stopped" event reached the UI before the image was downloaded: in the order the
    // events were emitted, the console's "Loading section" comes first.
    let (mut loaded, mut stops) = (false, 0);
    while let Ok(ev) = events.try_recv() {
        if ev.kind == "debug.output" && ev.data["lines"].to_string().contains("Loading section") {
            loaded = true;
        }
        if ev.kind == "debug.session" && ev.data["state"] == "stopped" {
            stops += 1;
            assert!(loaded, "the connecting stop reached the UI before the download: {}", ev.data);
        }
    }
    assert!(loaded && stops > 0);

    // The requests: gdb attached to the server's port (never a launch, never a pid), and
    // the commands ran in order.
    assert!(env.requests("launch").is_empty());
    let attach = &env.requests("attach")[0]["arguments"];
    assert!(attach.get("pid").is_none() && attach.get("processId").is_none(), "{attach}");
    assert_eq!(attach["program"], env.root.join("prog.bin").display().to_string());
    let target = attach["target"].as_str().unwrap().to_string();
    assert!(target.starts_with("127.0.0.1:"), "{target}");
    assert_eq!(env.commands(), ["monitor halt", "monitor reset halt", "load", "monitor reset halt", "thbreak main"]);
    assert_eq!(env.requests("continue").len(), 1, "the program runs on to main");
    assert_eq!(info["remote"], json!({ "server": "Fake server", "target": target }));

    // The console: the server's own output next to the commands, and ports of their own.
    let text = env.console(&sid).await;
    let port = target.rsplit(':').next().unwrap();
    assert!(text.contains("[server]fake server starting") && text.contains("[server]fake server note"), "{text}");
    assert!(text.contains(&format!("[server]listening on {port}")), "{text}");
    assert_eq!(text.matches("[server]listening on").count(), 2, "a second free port for `{{port2}}`: {text}");
    assert!(text.contains("[repl-in]> load") && text.contains("[repl-out]Loading section .text"), "{text}");
    assert!(text.contains("[repl-out]ran: monitor reset halt\n"), "carriage returns are gone: {text:?}");
    assert!(!text.contains("connection from a client"), "waiting for the server must not connect to it: {text}");
    assert!(text.contains("$ ") && text.contains("--port"), "the server's command line is echoed: {text}");

    // An agent sees the server and target, and the registers when it asks.
    let state = env.mcp_state().await;
    let s0 = &state["sessions"][0];
    assert_eq!(s0["remote"], json!({ "server": "Fake server", "target": target }));
    assert!(s0.get("registers").is_none(), "registers only on request");
    let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == "debug_state").unwrap();
    let r = (tool.handler)(env.state.clone(), crate::mcp::McpCtx::default(), json!({ "projectId": env.pid(), "registers": true })).await.unwrap();
    let crate::mcp::ToolOutput::Json(r) = r else { panic!("json") };
    assert_eq!(r["sessions"][0]["registers"], json!({ "rip": "0x1" }));
    assert!(r["sessions"][0]["console"].as_str().unwrap().contains("fake server starting"), "the server's output is in the console tail");

    // Stop detaches (it never terminates a target) and takes the server with it.
    let pid = env.server_pid().await;
    assert!(crate::util::os::proc::pid_alive(pid));
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert!(gone(pid, 8).await, "the debug server outlived the session");
    let end = env.get(&format!("sessions/{sid}")).await;
    assert_eq!((end["state"].clone(), end["stopRequested"].clone()), (json!("terminated"), json!(true)));
    assert!(env.requests("terminate").is_empty());
    assert_eq!(env.requests("disconnect")[0]["arguments"]["terminateDebuggee"], false);
}

/// `stop_at = "reset"`: the target stays halted at the reset vector (no `stop_on_entry`
/// needed), nothing is flashed when the configuration says so, and nothing resumes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_target_can_stay_halted_at_reset() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
"#,
    )
    .await;
    assert_eq!(env.get("configs").await["configs"][1]["stopOnEntry"], true, "naming a place is asking to stop there");
    let sid = env.start("board").await;
    let info = env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!(info["state"], "stopped", "{info}");
    assert_eq!(info["stopped"]["reason"], "entry");
    assert_eq!(info["stopped"]["description"], "Halted at the reset vector");
    // Whatever gdb's own late stop event does, it does not replace ours.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(env.get(&format!("sessions/{sid}")).await["stopped"]["reason"], "entry");
    assert_eq!(env.commands(), ["monitor halt", "monitor reset halt"], "no download, so no second reset and no thbreak");
    assert!(env.requests("continue").is_empty());
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// Without `stop_on_entry` the program runs (the target is resumed once prepared); a
/// location of the configuration's own is where it stops; one that does not exist
/// leaves the target halted, with the reason in the console.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_target_runs_or_stops_where_it_is_told() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "run"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false

[[debug]]
name = "app_main"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "app_main"

[[debug]]
name = "nowhere"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "nope"
"#,
    )
    .await;
    // Runs: the fake program has no breakpoint to hit and exits.
    let sid = env.start("run").await;
    env.wait_session(&sid, "the end", |v| v["state"] == "terminated" || v["state"] == "failed").await;
    assert_eq!(env.requests("continue").len(), 1);
    assert_eq!(env.commands(), ["monitor halt", "monitor reset halt"]);
    let pid = env.server_pid().await;
    assert!(gone(pid, 8).await, "the program ended: so did the session and its server");

    let sid = env.start("app_main").await;
    let info = env.wait_session(&sid, "the stop", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!(info["stopped"]["reason"], "breakpoint", "{info}");
    assert!(env.commands().contains(&"thbreak app_main".to_string()));
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;

    let sid = env.start("nowhere").await;
    let info = env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!((info["state"].clone(), info["stopped"]["reason"].clone()), (json!("stopped"), json!("entry")), "{info}");
    let text = env.console(&sid).await;
    assert!(text.contains("[repl-err]Function \"nope\" not defined.") && text.contains("Cannot stop at nope"), "{text}");
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// A command that fails ends the session with the command and gdb's words; the server goes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_gdb_command_ends_the_session() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
init = "monitor fail"
"#,
    )
    .await;
    let sid = env.start("board").await;
    let info = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    let error = info["error"].as_str().unwrap();
    assert!(error.contains("`monitor fail` failed") && error.contains("Target disconnected"), "{error}");
    assert_eq!(env.commands(), ["monitor fail"], "nothing after the failed command: no load, no run");
    assert!(gone(env.server_pid().await, 8).await);
}

/// A server that exits before it listens (no probe, a wrong board file) says why, and
/// gdb is never started; one that never listens is given up on; both are cleaned up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_does_not_come_up_fails_the_session_with_its_words() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "no probe"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
server_args = ["--fail", "Error: unable to find a matching CMSIS-DAP device"]

[[debug]]
name = "silent"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "impatient"
server_args = ["--delay", "30"]
"#,
    )
    .await;
    let sid = env.start("no probe").await;
    let info = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    let error = info["error"].as_str().unwrap();
    assert!(error.starts_with("Fake server exited before it was ready (exit code 1)"), "{error}");
    assert!(error.contains("Error: unable to find a matching CMSIS-DAP device"), "its error line, not its banner: {error}");
    assert!(env.requests("initialize").is_empty(), "gdb is not started for a server that is not there");
    let text = env.console(&sid).await;
    assert!(text.contains("[server]fake server starting") && text.contains("[server]Error: unable to find"), "{text}");

    let sid = env.start("silent").await;
    let info = env.wait_session(&sid, "the timeout", |v| v["state"] == "failed").await;
    let error = info["error"].as_str().unwrap();
    assert!(error.starts_with("Impatient server did not listen on port ") && error.contains("within 1s"), "{error}");
    assert!(env.requests("initialize").is_empty());
    assert!(gone(env.server_pid().await, 8).await, "a server that never listened is not left running");
}

/// The server dying under a live session (a pulled cable, a crash) ends the session and
/// says so; stopping a server that ignores SIGTERM kills it anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_server_going_away_ends_the_session() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "dies"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
server_args = ["--die-after", "4"]
"#,
    )
    .await;
    let sid = env.start("dies").await;
    let info = env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!(info["state"], "stopped", "{info}");
    let end = env.wait_session_for(&sid, "the server's end", Duration::from_secs(15), |v| v["state"] == "failed").await;
    assert!(end["error"].as_str().unwrap().starts_with("Fake server exited unexpectedly (exit code 7)"), "{end}");
    assert!(!env.state.debug.get(&env.pid(), &sid).unwrap().is_live());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_ignores_sigterm_is_killed() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "stubborn"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
server_args = ["--ignore-term"]
"#,
    )
    .await;
    let sid = env.start("stubborn").await;
    env.wait_session(&sid, "the halt", |v| v["state"] == "stopped").await;
    let pid = env.server_pid().await;
    let t = std::time::Instant::now();
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert!(gone(pid, 6).await, "SIGTERM was ignored and nothing followed it up");
    assert!(t.elapsed() < Duration::from_secs(8));
}

/// A stub that already runs (a board's own gdbserver, a probe started by hand) needs no
/// server: gdb connects to `connect` as given, and nothing of ours is started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stub_that_already_runs_needs_no_server() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let stub = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let connect = format!("localhost:{}", stub.local_addr().unwrap().port());
    let env = remote_env(&format!(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
connect = "{connect}"
stop_at = "reset"
"#
    ))
    .await;
    let c = &env.get("configs").await["configs"][1];
    assert_eq!((c["remote"]["download"].clone(), c["remote"]["connect"].clone(), c["remote"]["server"].clone()), (json!(false), json!(connect), Value::Null), "{c}");
    let sid = env.start("board").await;
    let info = env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!(info["state"], "stopped", "{info}");
    assert_eq!(env.requests("attach")[0]["arguments"]["target"], connect);
    assert_eq!(info["remote"], json!({ "target": connect }), "no server of ours");
    assert!(env.commands().is_empty(), "no server, no defaults: nothing to send but what the configuration says");
    assert!(!env.root.parent().unwrap().join("server.pid").exists());
    assert!(!env.console(&sid).await.contains("[server]"));
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// Mistakes in a remote configuration are told before anything runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_configurations_are_checked_before_anything_runs() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = taken.local_addr().unwrap().port();
    let env = remote_env(&format!(
        r#"
[[debug]]
name = "unknown server"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "mystery"

[[debug]]
name = "nothing"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
download = false

[[debug]]
name = "args without server"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
connect = "localhost:1"
server_args = ["-f", "x.cfg"]

[[debug]]
name = "not gdb"
adapter = "fake"
program = "prog.bin"
[debug.remote]
server = "fakesrv"

[[debug]]
name = "two lines"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
reset = "monitor reset\nshell touch pwned"

[[debug]]
name = "no port"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "noport"
download = false

[[debug]]
name = "download of nothing"
adapter = "fakegdb"
[debug.remote]
server = "fakesrv"

[[debug]]
name = "busy port"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
port = {busy}

[[debug]]
name = "good"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
"#
    ))
    .await;
    let configs = env.get("configs").await;
    let problems = |name: &str| -> String {
        configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect::<Vec<_>>().join(" | ")
    };
    assert_eq!(problems("good"), "");
    for (name, status, needle) in [
        ("unknown server", 412, "debug server \"mystery\""),
        ("nothing", 400, "`server` to start or a `connect`"),
        ("args without server", 400, "`server_args` without a `server`"),
        ("not gdb", 400, "not a gdb adapter"),
        ("two lines", 400, "one non-empty line"),
        ("no port", 400, "cannot tell when the debug server is ready"),
        ("download of nothing", 400, "needs a `program`"),
    ] {
        // The Start view says it beforehand…
        assert!(problems(name).contains(needle), "{name}: {}", problems(name));
        // …and starting says it again, without starting anything.
        let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": name })).await;
        assert_eq!(s, status, "{name}: {v}");
        assert!(v["error"]["message"].as_str().unwrap().contains(needle), "{name}: {v}");
    }
    assert!(env.requests("initialize").is_empty());
    assert!(!env.root.parent().unwrap().join("server.pid").exists(), "no server was started for a configuration that cannot work");
    assert!(env.state.debug.all().is_empty());

    // A port something else holds is never mistaken for our server's.
    let sid = env.start("busy port").await;
    let info = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    assert!(info["error"].as_str().unwrap().contains(&format!("port {busy} is already in use")), "{info}");
    assert!(env.requests("initialize").is_empty());
    drop(taken);
}

/// The REST routes refuse an in-process caller (an agent's own request): an agent starts a
/// session through its tool, which is flagged as a write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rest_routes_refuse_agents_a_session_start() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
"#,
    )
    .await;
    let ctx = crate::mcp::McpCtx::default();
    let p = format!("/api/projects/{}/debug/servers", env.pid());
    let listed = crate::mcp::call_api(&env.state, axum::http::Method::GET, &p, None, &ctx).await.unwrap();
    assert!(listed["servers"].as_array().unwrap().iter().any(|s| s["id"] == "openocd"), "{listed}");
    let p = format!("/api/projects/{}/debug/sessions", env.pid());
    let err = crate::mcp::call_api(&env.state, axum::http::Method::POST, &p, Some(json!({ "config": "board" })), &ctx).await.unwrap_err();
    assert_eq!(err.status, 403);
    assert!(!env.root.parent().unwrap().join("server.pid").exists());
}

// ---------------------------------------------------------------- real embedded tools

/// The tools of the real remote-target tests: each name is looked up in the directory
/// `WORKBENCH_TEST_EMBEDDED` names (wrapper scripts or the tools themselves), else on
/// PATH. `None`, with the reason on stderr, when one is missing: these tests need
/// gdb-multiarch, qemu-system-arm and an arm-none-eabi-gcc, or gdb, gdbserver and a C
/// compiler, and skip like the debugpy one.
fn embedded_tools(names: &[&str]) -> Option<Vec<String>> {
    let dir = std::env::var_os("WORKBENCH_TEST_EMBEDDED").map(PathBuf::from);
    let mut found = vec![];
    for n in names {
        let path = match &dir {
            Some(d) if d.join(n).exists() => Some(d.join(n)),
            _ => crate::util::which_path(n),
        };
        match path {
            Some(p) => found.push(p.display().to_string()),
            None => {
                eprintln!("skipped: {n} was not found (put it on PATH, or in the directory WORKBENCH_TEST_EMBEDDED names)");
                return None;
            }
        }
    }
    Some(found)
}

fn available(adapters: &Value, id: &str) -> bool {
    adapters["adapters"].as_array().unwrap().iter().find(|a| a["id"] == id).is_some_and(|a| a["availability"]["available"] == true)
}

fn port_of(target: &str) -> u16 {
    target.rsplit(':').next().unwrap().parse().unwrap()
}

/// A Cortex-M3 firmware built by the configuration's own pre-launch step and debugged in
/// QEMU through gdb-multiarch, the way an OpenOCD or J-Link session runs: the server
/// starts, gdb attaches with `target remote`, the image is downloaded and the target
/// reset, the program runs to `main`, hits a breakpoint, is paused while it runs, shows
/// its registers; Stop takes QEMU away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cortex_m_firmware_runs_in_qemu_through_gdb_multiarch() {
    let Some(tools) = embedded_tools(&["gdb-multiarch", "qemu-system-arm", "arm-none-eabi-gcc"]) else { return };
    let (gdb, qemu, gcc) = (&tools[0], &tools[1], &tools[2]);
    let debug_toml = format!("[adapters.gdb-multiarch]\ncommand = {}\n[servers.qemu-arm]\ncommand = {}\n", toml_str(gdb), toml_str(qemu));
    let build = format!(
        "{gcc} -mcpu=cortex-m3 -mthumb -g -O0 -nostdlib -ffreestanding -T link.ld src/startup.c src/main.c -o firmware.elf",
        gcc = crate::apps::expand::shell_quote(gcc)
    );
    let project = format!(
        r#"
[[debug]]
name = "fw"
program = "firmware.elf"
pre_launch = {build}
stop_on_entry = true
[debug.remote]
server = "qemu-arm"
server_args = ["-M", "lm3s6965evb", "-kernel", "{{program}}"]

[[debug]]
name = "fw flashed"
program = "firmware.elf"
pre_launch = {build}
stop_on_entry = true
[debug.remote]
server = "qemu-arm"
server_args = ["-M", "lm3s6965evb", "-kernel", "{{program}}"]
download = true
init = "monitor info registers"
reset = "monitor system_reset"
"#,
        build = toml_str(&build)
    );
    let files = [("link.ld", include_str!("testdata/cortex_m3.ld")), ("src/startup.c", include_str!("testdata/cortex_m3_startup.c")), ("src/main.c", include_str!("testdata/cortex_m3_main.c"))];
    let env = setup_with(&project, Opts { debug_toml: &debug_toml, files: &files, ..Default::default() }).await;
    if !available(&env.get("adapters").await, "gdb-multiarch") {
        eprintln!("skipped: gdb-multiarch is older than GDB 14 or has no Python");
        return;
    }
    let fw = env.get("configs").await["configs"].as_array().unwrap().iter().find(|c| c["name"] == "fw").unwrap().clone();
    assert_eq!((fw["adapter"].clone(), fw["remote"]["serverAvailable"].clone()), (json!("gdb-multiarch"), json!(true)), "{fw}");
    // Line 9 is `result = fib(...)`'s callee: `return n < 2 ? ...` in fib().
    let (s, v) = env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 9 }] })).await;
    assert_eq!(s, 200, "{v}");

    for config in ["fw", "fw flashed"] {
        let sid = env.start(config).await;
        let info = env.wait_session_for(&sid, "main", Duration::from_secs(60), |v| matches!(v["state"].as_str(), Some("stopped" | "failed"))).await;
        let console = env.console(&sid).await;
        assert_eq!(info["state"], "stopped", "{config}: {info}\n{console}");
        assert_eq!(info["request"], "attach");
        let target = info["remote"]["target"].as_str().unwrap().to_string();
        let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
        assert_eq!((stack["frames"][0]["name"].as_str(), stack["frames"][0]["source"]["path"].as_str()), (Some("main"), Some("src/main.c")), "{config}: {stack}");
        // Breakpoints set before the target was connected are verified at once.
        assert_eq!(env.get("breakpoints").await["breakpoints"][0]["status"]["verified"], true);
        if config == "fw flashed" {
            assert!(console.contains("[repl-out]Loading section .text") && console.contains("R15="), "the image was downloaded and the init command ran: {console}");
            assert_eq!(console.matches("> monitor system_reset").count(), 2, "reset before and after the download: {console}");
        } else {
            assert!(!console.contains("> load"), "QEMU loads the image itself: {console}");
        }

        // Resume: the firmware runs on to the breakpoint in fib(); its startup code
        // copied `.data` (a value no other memory holds) and counts in `counter`.
        env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
        let hit = env.wait_session(&sid, "the breakpoint", |v| v["state"] == "stopped" && v["stopped"]["hitBreakpointIds"].is_array()).await;
        assert_eq!(hit["stopped"]["reason"], "breakpoint");
        let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
        let names: Vec<&str> = stack["frames"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(names[..2], ["fib", "main"], "{stack}");
        let fid = stack["frames"][0]["id"].as_i64().unwrap();
        let eval = |expr: &'static str| {
            let (env, sid) = (&env, sid.clone());
            async move { env.post(&format!("sessions/{sid}/evaluate"), json!({ "expression": expr, "frameId": fid, "context": "watch" })).await["value"].as_str().unwrap().to_string() }
        };
        assert_eq!(eval("initialised").await, (0xC0FFEEu32).to_string(), "the startup code ran on the target");
        assert_eq!(eval("counter").await, "1");
        // An agent reading the halted core: pc is inside fib, the stack pointer in RAM.
        let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == "debug_state").unwrap();
        let r = (tool.handler)(env.state.clone(), crate::mcp::McpCtx::default(), json!({ "projectId": env.pid(), "sessionId": sid, "registers": true })).await.unwrap();
        let crate::mcp::ToolOutput::Json(r) = r else { panic!("json") };
        let regs = &r["sessions"][0]["registers"];
        assert!(regs["pc"].as_str().unwrap().contains("<fib+"), "{regs}");
        assert!(regs["sp"].as_str().unwrap().starts_with("0x2000"), "{regs}");
        assert_eq!(r["sessions"][0]["remote"]["server"], "QEMU (Arm)");

        // Step, then run free and pause it: a halted core answers.
        env.post(&format!("sessions/{sid}/control"), json!({ "action": "next" })).await;
        env.wait_session(&sid, "the step", |v| v["state"] == "stopped" && v["stopped"]["reason"] == "step").await;
        env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [] })).await;
        env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
        env.wait_session(&sid, "running", |v| v["state"] == "running").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        env.post(&format!("sessions/{sid}/control"), json!({ "action": "pause" })).await;
        let paused = env.wait_session(&sid, "the pause", |v| v["state"] == "stopped").await;
        assert_eq!(paused["stopped"]["reason"], "pause", "{paused}");
        // The monitor is the debug console's: QEMU answers its own commands.
        let (s, v) = env.send(reqwest::Method::POST, &format!("sessions/{sid}/evaluate"), json!({ "expression": "monitor info registers", "context": "repl" })).await;
        assert_eq!(s, 200, "{v}");
        assert!(v["value"].as_str().unwrap().contains("R13="), "{v}");

        // Stop: gdb detaches and QEMU goes (nothing listens on its port any more).
        let port = port_of(&target);
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() || config == "fw flashed", "QEMU was reachable before Stop");
        env.post(&format!("sessions/{sid}/stop"), json!({})).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            assert!(tokio::time::Instant::now() < deadline, "QEMU still listens on {port} after Stop");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let end = env.get(&format!("sessions/{sid}")).await;
        assert_eq!((end["state"].clone(), end["stopRequested"].clone()), (json!("terminated"), json!(true)), "{end}");
        // Put the breakpoint back for the next configuration.
        env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 9 }] })).await;
    }
}

/// gdbserver on this computer: the server the host's own gdb talks to. It serves one
/// connection and exits (`--once`), so this is the proof that waiting for it to be
/// ready does not connect to it: a probe that did would leave gdb with nothing to attach to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gdbserver_is_a_remote_target_whose_one_connection_is_gdbs() {
    let Some(tools) = embedded_tools(&["gdbserver", "gcc"]) else { return };
    let (gdbserver, cc) = (&tools[0], &tools[1]);
    let debug_toml = format!(
        "[servers.gdbserver]\nlabel = \"gdbserver\"\ncommand = {}\nargs = [\"--once\", \"127.0.0.1:{{port}}\", \"{{program}}\"]\ndownload = false\n",
        toml_str(gdbserver)
    );
    let project = format!(
        r#"
[[debug]]
name = "host"
program = "prog"
pre_launch = {build}
stop_on_entry = true
[debug.remote]
server = "gdbserver"
"#,
        build = toml_str(&format!("{} -g -O0 -o prog src/main.c", crate::apps::expand::shell_quote(cc)))
    );
    let env = setup_with(&project, Opts { debug_toml: &debug_toml, files: &[("src/main.c", include_str!("testdata/host_main.c"))], ..Default::default() }).await;
    if !available(&env.get("adapters").await, "gdb") {
        eprintln!("skipped: gdb is older than GDB 14 or has no Python");
        return;
    }
    // `add` is on line 7.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 7 }] })).await;
    let sid = env.start("host").await;
    let info = env.wait_session_for(&sid, "main", Duration::from_secs(60), |v| matches!(v["state"].as_str(), Some("stopped" | "failed"))).await;
    let console = env.console(&sid).await;
    assert_eq!(info["state"], "stopped", "{info}\n{console}");
    assert!(console.contains("Listening on port") || console.contains("Process "), "gdbserver's own words are in the console: {console}");
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["name"], "main", "{stack}");
    // The first call of add(): a = 100, b = 1.
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    let hit = env.wait_session(&sid, "add", |v| v["state"] == "stopped" && v["stopped"]["hitBreakpointIds"].is_array()).await;
    assert_eq!(hit["stopped"]["reason"], "breakpoint");
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["name"], "add");
    let fid = stack["frames"][0]["id"].as_i64().unwrap();
    let scopes = env.get(&format!("sessions/{sid}/scopes?frameId={fid}")).await;
    let args = scopes["scopes"].as_array().unwrap().iter().find(|s| s["name"] == "Arguments").unwrap();
    let vars = env.get(&format!("sessions/{sid}/variables?ref={}", args["variablesReference"])).await;
    let values: Vec<(String, String)> = vars["variables"].as_array().unwrap().iter().map(|v| (v["name"].as_str().unwrap().into(), v["value"].as_str().unwrap().into())).collect();
    assert_eq!(values, [("a".to_string(), "100".to_string()), ("b".to_string(), "1".to_string())]);
    // Let it finish: the program's own output arrives through gdbserver and the session
    // ends with the program.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [] })).await;
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    let end = env.wait_session_for(&sid, "the end", Duration::from_secs(30), |v| !matches!(v["state"].as_str(), Some("starting" | "running" | "stopped"))).await;
    assert_eq!(end["state"], "terminated", "{end}");
    assert_eq!(end["exitCode"], 0);
    assert!(env.console(&sid).await.contains("total=106"), "the program's output");
}

// ---------------------------------------------------------------- agents

impl Env {
    /// An agent session of this project calling one of the debug tools.
    async fn agent(&self, name: &str, args: Value) -> Result<Value, crate::error::ApiError> {
        self.agent_in(&self.pid(), name, args).await
    }

    async fn agent_in(&self, project: &str, name: &str, args: Value) -> Result<Value, crate::error::ApiError> {
        let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
        let ctx = crate::mcp::McpCtx { terminal_id: Some("agent-terminal".into()), project_id: Some(project.to_string()) };
        match (tool.handler)(self.state.clone(), ctx, args).await? {
            crate::mcp::ToolOutput::Json(v) => Ok(v),
            crate::mcp::ToolOutput::Text(t) => Ok(json!(t)),
        }
    }
}

/// The tools agents have for the debugger: `debug_state` reads; every other tool changes a
/// session or the breakpoints, so it is flagged as a write (the agent's own permission prompt,
/// the Activity badge).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_have_the_debug_tools_and_the_writes_are_flagged() {
    let names: Vec<String> = crate::debug::mcp_tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["debug_state", "debug_start", "debug_attach", "debug_restart", "debug_evaluate", "debug_control", "debug_breakpoints"]);
    for t in crate::debug::mcp_tools() {
        assert_eq!(t.mutating, t.name != "debug_state", "{}: what changes a session is flagged as a write", t.name);
    }
}

/// An agent steers a session the user started: breakpoints, continue (waiting for the next
/// stop), step, run to a line, stop — and gets the state back each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_steer_the_session_the_user_started() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    // Breakpoints: add keeps the others, a new one replaces the one on its line.
    let v = env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 5 }, { "line": 30 }] })).await.unwrap();
    let lines = |v: &Value| -> Vec<u64> { v["breakpoints"].as_array().unwrap().iter().map(|b| b["line"].as_u64().unwrap()).collect() };
    assert_eq!(lines(&v), [5, 30]);
    let v = env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 12 }, { "line": 30, "enabled": false }] })).await.unwrap();
    assert_eq!(lines(&v), [5, 12, 30]);
    assert_eq!(v["breakpoints"][2]["enabled"], false, "the new one on line 30 replaced the old");
    let v = env.agent("debug_breakpoints", json!({ "action": "remove", "path": "src/main.c", "lines": [12] })).await.unwrap();
    assert_eq!(lines(&v), [5, 30]);
    let v = env.agent("debug_breakpoints", json!({ "action": "set", "path": "src/main.c", "breakpoints": [{ "line": 5 }, { "line": 30 }] })).await.unwrap();
    assert_eq!(lines(&v), [5, 30]);
    assert_eq!(v["breakpoints"][1]["enabled"], true);
    assert_eq!(env.agent("debug_breakpoints", json!({ "action": "list" })).await.unwrap()["breakpoints"].as_array().unwrap().len(), 2);

    // No live session: the answer says how to get one (the user starts it).
    let err = env.agent("debug_control", json!({ "action": "continue" })).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("the user starts one"), "{}", err.message);

    // The user starts it; the agent drives it.
    let sid = env.start("fake").await;
    let info = env.wait_session(&sid, "the first stop", |v| v["state"] == "stopped").await;
    assert_eq!(info["stopped"]["reason"], "breakpoint");
    let r = env.agent("debug_control", json!({ "action": "continue" })).await.unwrap();
    assert_eq!((r["state"].clone(), r["settled"].clone(), r["stop"]["reason"].clone()), (json!("stopped"), json!(true), json!("breakpoint")), "{r}");
    assert!(r["stack"][0].as_str().unwrap().starts_with("#0 work at src/main.c:30"), "the answer is where it stopped: {r}");
    assert!(r["locals"].to_string().contains("\"x\""));
    let r = env.agent("debug_control", json!({ "action": "next", "sessionId": sid })).await.unwrap();
    assert_eq!((r["state"].clone(), r["stop"]["reason"].clone()), (json!("stopped"), json!("step")), "{r}");
    let r = env.agent("debug_control", json!({ "action": "runTo", "path": "src/main.c", "line": 40 })).await.unwrap();
    assert_eq!(r["state"], "stopped");
    assert!(r["stack"][0].as_str().unwrap().contains("src/main.c:40"), "{r}");
    // Pausing a program that is not running is the debugger's own refusal.
    let err = env.agent("debug_control", json!({ "action": "pause" })).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    assert_eq!(env.agent("debug_control", json!({ "action": "dance" })).await.unwrap_err().code, "bad_request");
    assert_eq!(env.agent("debug_control", json!({ "action": "runTo", "path": "src/main.c" })).await.unwrap_err().code, "bad_request");

    // The agent mutes breakpoints and sees what the debugger says about them.
    let v = env.agent("debug_breakpoints", json!({ "action": "mute" })).await.unwrap();
    assert_eq!(v["muted"], true);
    env.agent("debug_breakpoints", json!({ "action": "mute", "muted": false })).await.unwrap();

    // Continue to the end: the program exits, the session ends, and the answer says so.
    let r = env.agent("debug_control", json!({ "action": "continue", "waitSeconds": 10 })).await.unwrap();
    assert_eq!((r["state"].clone(), r["exitCode"].clone(), r["settled"].clone()), (json!("terminated"), json!(3), json!(true)), "{r}");

    // Another session, stopped by the agent.
    let sid2 = env.start("fake").await;
    env.wait_session(&sid2, "the stop", |v| v["state"] == "stopped").await;
    let r = env.agent("debug_control", json!({ "action": "stop" })).await.unwrap();
    assert_eq!((r["state"].clone(), r["sessionId"].clone()), (json!("terminated"), json!(sid2)));
    assert!(env.get(&format!("sessions/{sid2}")).await["stopRequested"].as_bool().unwrap());

    let v = env.agent("debug_breakpoints", json!({ "action": "clear" })).await.unwrap();
    assert!(v["breakpoints"].as_array().unwrap().is_empty());
}

/// Conditions and log messages are the agent's to set too; the debug tools reach any project;
/// several live sessions need naming.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_set_conditions_reach_any_project_and_name_the_session() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    let v = env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 5, "condition": "x > 1" }, { "line": 8, "logMessage": "x={x}" }] })).await.unwrap();
    let at = |line: u64| v["breakpoints"].as_array().unwrap().iter().find(|b| b["line"] == line).unwrap().clone();
    assert_eq!(at(5)["condition"], "x > 1");
    assert_eq!(at(8)["logMessage"], "x={x}");
    let v = env.agent("debug_breakpoints", json!({ "action": "functions", "breakpoints": [{ "name": "main", "condition": "argc > 1" }] })).await.unwrap();
    assert_eq!(v["functionBreakpoints"][0]["condition"], "argc > 1", "{v}");
    env.agent("debug_breakpoints", json!({ "action": "functions", "breakpoints": [{ "name": "main" }] })).await.unwrap();
    env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 5, "hitCondition": "2" }] })).await.unwrap();
    // The user's own conditional breakpoint is left alone by an unrelated add.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/other.c", "breakpoints": [{ "line": 3, "condition": "y" }] })).await;
    let v = env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 9 }] })).await.unwrap();
    assert!(v["breakpoints"].as_array().unwrap().iter().any(|b| b["path"] == "src/other.c" && b["condition"] == "y"), "{v}");

    // Not confined to their own project: a session of another project names this one (and a project that does not exist is not found).
    let err = env.agent_in("proj", "debug_breakpoints", json!({ "action": "list", "projectId": "elsewhere" })).await.unwrap_err();
    assert_eq!(err.code, "not_found");
    env.agent_in("elsewhere", "debug_breakpoints", json!({ "action": "list", "projectId": "proj" })).await.unwrap();
    let err = env.agent_in("elsewhere", "debug_control", json!({ "action": "continue", "projectId": "proj" })).await.unwrap_err();
    assert_eq!(err.code, "conflict", "{}", err.message);

    // Two live sessions: name one.
    let a = env.start("fake").await;
    let b = env.start("fake").await;
    env.wait_session(&a, "a", |v| v["state"] == "stopped").await;
    env.wait_session(&b, "b", |v| v["state"] == "stopped").await;
    let err = env.agent("debug_control", json!({ "action": "continue" })).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
    assert!(err.message.contains("2 live debug sessions") && err.message.contains(&a) && err.message.contains(&b), "{}", err.message);
    let err = env.agent("debug_control", json!({ "action": "continue", "sessionId": "nonesuch" })).await.unwrap_err();
    assert_eq!(err.code, "not_found");
    env.post(&format!("sessions/{a}/stop"), json!({})).await;
    env.post(&format!("sessions/{b}/stop"), json!({})).await;
}

// ---------------------------------------------------------------- extended-remote

/// `target extended-remote` stubs: one that runs the program (gdbserver --multi) is a launch
/// that gdb connects to first; one attached by number (a Black Magic Probe) scans before it
/// attaches and is prepared like any attached target.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_remote_stubs_run_the_program_or_are_attached_by_number() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "runs"
adapter = "fakegdb"
program = "prog.bin"
stop_on_entry = true
[debug.remote]
server = "fakesrv"
extended = true
exec_file = "/remote/prog"
init = ["monitor hello"]

[[debug]]
name = "attached"
adapter = "fakegdb"
program = "prog.bin"
stop_on_entry = true
[debug.remote]
server = "fakesrv"
extended = true
attach = 1
init = ["monitor swdp_scan"]
reset = []
download = false
"#,
    )
    .await;
    let configs = env.get("configs").await;
    let c = |n: &str| configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap().clone();
    assert_eq!((c("runs")["request"].clone(), c("runs")["remote"]["extended"].clone()), (json!("launch"), json!(true)), "{}", c("runs"));
    assert_eq!((c("attached")["request"].clone(), c("attached")["remote"]["attach"].clone()), (json!("attach"), json!(1)));

    // A stub that runs the program: the connection and the exec-file come first, the launch
    // is gdb's (stopAtBeginningOfMainSubprogram), and nothing is downloaded or reset.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await;
    let sid = env.start("runs").await;
    let info = env.wait_session(&sid, "the stop", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!((info["state"].clone(), info["request"].clone()), (json!("stopped"), json!("launch")), "{info}");
    let target = info["remote"]["target"].as_str().unwrap().to_string();
    assert_eq!(env.commands(), [format!("target extended-remote {target}"), "set remote exec-file /remote/prog".to_string(), "monitor hello".to_string()]);
    assert!(env.requests("attach").is_empty());
    let launch = &env.requests("launch")[0]["arguments"];
    assert_eq!(launch["stopAtBeginningOfMainSubprogram"], true);
    assert!(launch.get("target").is_none(), "{launch}");
    // Stop terminates what the stub started, as for any launch.
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert_eq!(env.requests("terminate").len(), 1);

    // Attached by number: the scan runs before the attach, which names the target and
    // not a `target remote` address; the session is prepared like an attached target.
    let _ = std::fs::remove_file(&env.log);
    let sid = env.start("attached").await;
    let info = env.wait_session(&sid, "the stop", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!((info["state"].clone(), info["request"].clone()), (json!("stopped"), json!("attach")), "{info}");
    let target = info["remote"]["target"].as_str().unwrap().to_string();
    assert_eq!(env.commands(), [format!("target extended-remote {target}"), "monitor swdp_scan".to_string(), "thbreak main".to_string()]);
    let attach = &env.requests("attach")[0]["arguments"];
    assert_eq!((attach["pid"].clone(), attach.get("target")), (json!(1), None), "{attach}");
    assert_eq!(attach["program"], env.root.join("prog.bin").display().to_string());
    assert_eq!(env.requests("continue").len(), 1);
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    assert!(env.requests("terminate").is_empty(), "an attach detaches");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extended_remote_mistakes_are_told_before_and_a_refused_connection_says_so() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "attach without extended"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
attach = 1

[[debug]]
name = "exec file of an attach"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
extended = true
attach = 1
exec_file = "/x"

[[debug]]
name = "runs but flashes"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
extended = true
download = true

[[debug]]
name = "runs without a program"
adapter = "fakegdb"
[debug.remote]
server = "fakesrv"
extended = true

[[debug]]
name = "refused"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
connect = "refuse.invalid:2345"
extended = true
"#,
    )
    .await;
    let configs = env.get("configs").await;
    let problems = |n: &str| configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap()["problems"].to_string();
    for (name, needle) in [
        ("attach without extended", "set `extended = true`"),
        ("exec file of an attach", "`exec_file` is the program's path"),
        ("runs but flashes", "`download`, `reset` and `stop_at` apply to attaching"),
        ("runs without a program", "runs the `program`"),
    ] {
        assert!(problems(name).contains(needle), "{name}: {}", problems(name));
        let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": name })).await;
        assert_eq!(s, 400, "{name}: {v}");
    }
    let sid = env.start("refused").await;
    let info = env.wait_session(&sid, "the failure", |v| v["state"] == "failed").await;
    let error = info["error"].as_str().unwrap();
    assert!(error.starts_with("gdb could not connect to refuse.invalid:2345") && error.contains("Connection refused."), "{error}");
    assert!(env.requests("launch").is_empty() && env.requests("attach").is_empty(), "nothing is launched on a stub that refused");
}

/// gdbserver in `--multi` mode is an extended-remote stub: gdb connects with `target
/// extended-remote`, `run` starts the program on the remote (here, the same computer), and
/// the server outlives the program. A launch through a real stub, end to end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gdbserver_multi_runs_the_program_for_an_extended_remote_launch() {
    let Some(tools) = embedded_tools(&["gdbserver", "gcc"]) else { return };
    let (gdbserver, cc) = (&tools[0], &tools[1]);
    let debug_toml = format!(
        "[servers.gdbmulti]\nlabel = \"gdbserver --multi\"\ncommand = {}\nargs = [\"--multi\", \"127.0.0.1:{{port}}\"]\ndownload = false\n",
        toml_str(gdbserver)
    );
    let project = format!(
        r#"
[[debug]]
name = "remote run"
program = "prog"
pre_launch = {build}
stop_on_entry = true
[debug.remote]
server = "gdbmulti"
extended = true
"#,
        build = toml_str(&format!("{} -g -O0 -o prog src/main.c", crate::apps::expand::shell_quote(cc)))
    );
    let env = setup_with(&project, Opts { debug_toml: &debug_toml, files: &[("src/main.c", include_str!("testdata/host_main.c"))], ..Default::default() }).await;
    if !available(&env.get("adapters").await, "gdb") {
        eprintln!("skipped: gdb is older than GDB 14 or has no Python");
        return;
    }
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [{ "line": 7 }] })).await;
    let sid = env.start("remote run").await;
    let info = env.wait_session_for(&sid, "main", Duration::from_secs(60), |v| matches!(v["state"].as_str(), Some("stopped" | "failed"))).await;
    let console = env.console(&sid).await;
    assert_eq!((info["state"].clone(), info["request"].clone()), (json!("stopped"), json!("launch")), "{info}\n{console}");
    assert!(console.contains("> target extended-remote 127.0.0.1:") && console.contains("> set remote exec-file "), "{console}");
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["name"], "main", "{stack}");
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    let hit = env.wait_session(&sid, "add", |v| v["state"] == "stopped" && v["stopped"]["hitBreakpointIds"].is_array()).await;
    assert_eq!(hit["stopped"]["reason"], "breakpoint");
    let stack = env.get(&format!("sessions/{sid}/stack?threadId=1")).await;
    assert_eq!(stack["frames"][0]["name"], "add", "{stack}");
    // The program finishes on the stub; its output comes through gdbserver.
    env.send(reqwest::Method::PUT, "breakpoints/file", json!({ "path": "src/main.c", "breakpoints": [] })).await;
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "continue" })).await;
    let end = env.wait_session_for(&sid, "the end", Duration::from_secs(30), |v| !matches!(v["state"].as_str(), Some("starting" | "running" | "stopped"))).await;
    assert_eq!((end["state"].clone(), end["exitCode"].clone()), (json!("terminated"), json!(0)), "{end}");
    assert!(env.console(&sid).await.contains("total=106"));
}

// ---------------------------------------------------------------- output channels

/// A target's text out of band (RTT, a UART on a socket, SWO): ports of the debug server that
/// the console reads, with unfinished lines flushed when the output goes quiet, the SWO
/// stream decoded, several channels told apart, and a channel that is not up yet retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_channels_stream_the_targets_text_into_the_console() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    // SWIT packets on stimulus port 0 ("SWO\n"), with a timestamp, a hardware packet and noise between.
    let swo = "01 53 30 01 57 0D 40 01 4F 01 0A".replace(' ', "");
    let env = remote_env(&format!(
        r#"
[[debug]]
name = "one"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
server_args = ["--channel", "{{port3}}=text:hello from the target\nsecond line\nprompt> "]
channels = [{{ name = "RTT", port = "{{port3}}" }}]

[[debug]]
name = "two"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
server_args = ["--channel", "{{port3}}=text:uart line\n", "--channel", "{{port4}}=hex:{swo}"]
channels = [{{ name = "UART", port = "{{port3}}" }}, {{ name = "SWO", port = "{{port4}}", format = "itm" }}]
"#
    ))
    .await;
    let c = env.get("configs").await["configs"].as_array().unwrap().iter().find(|c| c["name"] == "two").unwrap().clone();
    assert_eq!(c["remote"]["channels"], json!(["UART ({port3})", "SWO ({port4})"]), "{c}");

    let sid = env.start("one").await;
    env.wait_session_for(&sid, "the halt", Duration::from_secs(60), |v| v["state"] == "stopped" || v["state"] == "failed").await;
    let mut text = String::new();
    for _ in 0..100 {
        text = env.console(&sid).await;
        if text.contains("[target]prompt> ") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(text.contains("[workbench]RTT connected (port "), "{text}");
    // Lines as they are, the unfinished prompt too (flushed when the output went quiet), no prefix for a single channel.
    for piece in ["[target]hello from the target\n", "[target]second line\n", "[target]prompt> "] {
        assert!(text.contains(piece), "{piece:?} in {text:?}");
    }
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    text = env.console(&sid).await;
    assert!(text.contains("RTT closed") || text.contains("Debug session ended"), "{text}");

    // Two channels: each line says whose it is, the SWO stream arrives decoded.
    let sid = env.start("two").await;
    env.wait_session_for(&sid, "the halt", Duration::from_secs(60), |v| v["state"] == "stopped" || v["state"] == "failed").await;
    for _ in 0..100 {
        text = env.console(&sid).await;
        if text.contains("SWO\n") && text.contains("uart line") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(text.contains("[target][UART] uart line\n"), "{text:?}");
    assert!(text.contains("[target][SWO] SWO\n"), "the ITM stream was decoded: {text:?}");
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn output_channel_mistakes_are_told_before_anything_runs() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let many: String = (0..9).map(|i| format!("{{ port = {} }}, ", 2000 + i)).collect();
    let env = remote_env(&format!(
        r#"
[[debug]]
name = "port zero"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = 0 }}]

[[debug]]
name = "the gdb port"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = "{{port}}" }}]

[[debug]]
name = "unused placeholder"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = "{{port5}}" }}]

[[debug]]
name = "placeholder without a server"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
connect = "localhost:1234"
channels = [{{ port = "{{port2}}" }}]

[[debug]]
name = "bad format"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = 2000, format = "hex" }}]

[[debug]]
name = "itm port of text"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = 2000, itm_port = 3 }}]

[[debug]]
name = "itm port 40"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{{ port = 2000, format = "itm", itm_port = 40 }}]

[[debug]]
name = "nine channels"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
channels = [{many}]
"#
    ))
    .await;
    let configs = env.get("configs").await;
    let problems = |n: &str| configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap()["problems"].to_string();
    for (name, needle) in [
        ("port zero", "TCP port number"),
        ("the gdb port", "not \\\"{port}\\\""),
        ("unused placeholder", "no argument of the debug server uses"),
        ("placeholder without a server", "starts none"),
        ("bad format", "is not `text` or `itm`"),
        ("itm port of text", "`itm_port` is for"),
        ("itm port 40", "0 to 31"),
        ("nine channels", "at most 8"),
    ] {
        assert!(problems(name).contains(needle), "{name}: {}", problems(name));
        let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": name })).await;
        assert_eq!(s, 400, "{name}: {v}");
    }
    assert!(!env.root.parent().unwrap().join("server.pid").exists(), "nothing was started");
}

/// A real target's UART as an output channel: QEMU serves the first serial port of a Cortex-M3
/// on a free port of the debug server, the firmware prints to it, and the console shows the
/// text as `[target]` lines while the program runs — and stays when it is paused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cortex_m_uart_is_an_output_channel_of_the_session() {
    let Some(tools) = embedded_tools(&["gdb-multiarch", "qemu-system-arm", "arm-none-eabi-gcc"]) else { return };
    let (gdb, qemu, gcc) = (&tools[0], &tools[1], &tools[2]);
    let debug_toml = format!(
        "[adapters.gdb-multiarch]\ncommand = {}\n[servers.qemu-uart]\nlabel = \"QEMU (UART on a port)\"\ncommand = {}\nargs = [\"-S\", \"-gdb\", \"tcp:127.0.0.1:{{port}}\", \"-display\", \"none\", \"-monitor\", \"none\", \"-serial\", \"tcp:127.0.0.1:{{port2}},server=on,wait=off\"]\ndownload = false\n",
        toml_str(gdb),
        toml_str(qemu)
    );
    let build = format!(
        "{gcc} -mcpu=cortex-m3 -mthumb -g -O0 -nostdlib -ffreestanding -T link.ld src/startup.c src/main.c -o firmware.elf",
        gcc = crate::apps::expand::shell_quote(gcc)
    );
    let project = format!(
        r#"
[[debug]]
name = "uart"
program = "firmware.elf"
pre_launch = {build}
[debug.remote]
server = "qemu-uart"
server_args = ["-M", "lm3s6965evb", "-kernel", "{{program}}"]
channels = [{{ name = "UART0", port = "{{port2}}" }}]
"#,
        build = toml_str(&build)
    );
    let files = [("link.ld", include_str!("testdata/cortex_m3.ld")), ("src/startup.c", include_str!("testdata/cortex_m3_startup.c")), ("src/main.c", include_str!("testdata/cortex_m3_main.c"))];
    let env = setup_with(&project, Opts { debug_toml: &debug_toml, files: &files, ..Default::default() }).await;
    if !available(&env.get("adapters").await, "gdb-multiarch") {
        eprintln!("skipped: gdb-multiarch is older than GDB 14 or has no Python");
        return;
    }
    let sid = env.start("uart").await;
    let info = env.wait_session_for(&sid, "running", Duration::from_secs(60), |v| matches!(v["state"].as_str(), Some("running" | "failed"))).await;
    assert_eq!(info["state"], "running", "{info}\n{}", env.console(&sid).await);
    let mut text = String::new();
    for _ in 0..200 {
        text = env.console(&sid).await;
        if text.contains("[target]tick ") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(text.contains("[workbench]UART0 connected (port ") && text.contains("[target]tick "), "{text}");
    // Whole lines of the firmware's own counter (multiples of 20000), nothing cut or invented.
    let ticks: Vec<&str> = text.split("[target]").filter(|l| l.starts_with("tick ")).collect();
    assert!(ticks.iter().all(|l| l.ends_with('\n') && l[5..l.len() - 1].parse::<u64>().is_ok_and(|n| n % 20_000 == 0)), "{ticks:?}");
    env.post(&format!("sessions/{sid}/control"), json!({ "action": "pause" })).await;
    env.wait_session(&sid, "the pause", |v| v["state"] == "stopped").await;
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

// ---------------------------------------------------------------- the Peripherals view

const CHIP_SVD: &str = r#"<?xml version="1.0"?>
<device><name>FAKECHIP</name><description>A chip for tests</description><size>32</size>
 <peripherals>
  <peripheral><name>TIMX</name><description>A timer</description><baseAddress>0x40000000</baseAddress>
   <registers>
    <register><name>CR1</name><addressOffset>0x0</addressOffset><resetValue>0x0</resetValue>
     <fields>
      <field><name>CEN</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
      <field><name>MODE</name><bitOffset>1</bitOffset><bitWidth>2</bitWidth>
       <enumeratedValues><enumeratedValue><name>Up</name><value>0</value></enumeratedValue><enumeratedValue><name>Down</name><value>1</value></enumeratedValue><enumeratedValue><name>Center</name><value>2</value></enumeratedValue></enumeratedValues>
      </field>
     </fields>
    </register>
    <register><name>SR</name><addressOffset>0x4</addressOffset><size>16</size><access>read-only</access><readAction>clear</readAction>
     <fields><field><name>UIF</name><bitOffset>0</bitOffset></field></fields>
    </register>
    <register><name>OUT</name><addressOffset>0x8</addressOffset><size>8</size><access>write-only</access></register>
   </registers>
  </peripheral>
  <peripheral><name>FAULTY</name><baseAddress>0xDEAD0000</baseAddress>
   <registers><register><name>R</name><addressOffset>0</addressOffset></register></registers>
  </peripheral>
 </peripherals>
</device>"#;

fn fake_word(addr: u64, n: u64) -> u64 {
    (0..n).fold(0, |v, i| v | ((((addr + i) * 7 + 3) & 0xFF) << (8 * i)))
}

/// A chip's register map beside a halted target: the peripherals and their registers by name,
/// values read with the access size of the register (never a neighbour), registers that change
/// the chip when read left alone unless asked for, faults reported per register, writes of a
/// register or of one field, and every route closed to agents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peripherals_are_read_and_written_by_name() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup_with(
        r#"
[[debug]]
name = "board"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
svd = "chip.svd"

[[debug]]
name = "no map"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"

[[debug]]
name = "missing map"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
svd = "gone.svd"

[[debug]]
name = "not a map"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
svd = "prog.bin"
"#,
        Opts { debug_toml: REMOTE_DEBUG, files: &[("chip.svd", CHIP_SVD)], ..Default::default() },
    )
    .await;
    let cfg = env.state.config.read().debug.clone();
    super::adapters::seed_probe(&env.state, &super::adapters::find(&cfg, "fakegdb").unwrap(), super::adapters::Availability { available: true, path: None, version: None, problem: None });

    let configs = env.get("configs").await;
    let c = |n: &str| configs["configs"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap().clone();
    assert_eq!(c("board")["remote"]["svd"], "chip.svd");
    assert!(c("missing map")["problems"].to_string().contains("the SVD file") && c("missing map")["problems"].to_string().contains("does not exist"), "{}", c("missing map"));
    assert_eq!(c("board")["problems"], json!([]));

    let sid = env.start("board").await;
    let info = env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    assert_eq!((info["state"].clone(), info["peripherals"].clone()), (json!("stopped"), json!(true)), "{info}");

    // The map: peripherals in address order, with how many registers each has.
    let list = env.get(&format!("sessions/{sid}/svd")).await;
    assert_eq!((list["device"].clone(), list["description"].clone()), (json!("FAKECHIP"), json!("A chip for tests")));
    let names: Vec<(&str, u64, u64)> = list["peripherals"].as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap(), p["base"].as_u64().unwrap(), p["registers"].as_u64().unwrap())).collect();
    assert_eq!(names, [("TIMX", 0x4000_0000, 3), ("FAULTY", 0xDEAD_0000, 1)]);

    // Without `read`, nothing touches the target.
    let before = env.requests("readMemory").len();
    let d = env.get(&format!("sessions/{sid}/svd/timx")).await;
    assert_eq!(env.requests("readMemory").len(), before);
    assert_eq!(d["registers"][0]["value"], Value::Null);
    assert_eq!((d["registers"][0]["address"].clone(), d["registers"][1]["address"].clone()), (json!(0x4000_0000u64), json!(0x4000_0004u64)));

    // Read: CR1 with the access size of its 32 bits; SR (it clears on read) and OUT (write-only) are left alone.
    let d = env.get(&format!("sessions/{sid}/svd/TIMX?read=true")).await;
    let cr1 = &d["registers"][0];
    let word = fake_word(0x4000_0000, 4);
    assert_eq!(cr1["value"], format!("0x{word:08X}"), "{cr1}");
    let cen = &cr1["fields"][0];
    assert_eq!((cen["name"].clone(), cen["value"].clone()), (json!("CEN"), json!(word & 1)));
    let mode = &cr1["fields"][1];
    assert_eq!(mode["value"], json!((word >> 1) & 3));
    assert_eq!(mode["valueName"], match (word >> 1) & 3 { 0 => json!("Up"), 1 => json!("Down"), 2 => json!("Center"), _ => Value::Null });
    assert_eq!(d["registers"][1]["skipped"], "reading it changes the chip");
    assert_eq!(d["registers"][2]["skipped"], "write-only");
    assert_eq!((d["registers"][1]["value"].clone(), d["registers"][2]["value"].clone()), (Value::Null, Value::Null));
    let reads = env.requests("readMemory");
    let sizes: Vec<(String, u64)> = reads[before..].iter().map(|r| (r["arguments"]["memoryReference"].as_str().unwrap().to_string(), r["arguments"]["count"].as_u64().unwrap())).collect();
    assert_eq!(sizes, [("0x40000000".to_string(), 4)], "one read, of CR1 only: {sizes:?}");
    // Named, the register that clears on read is read — with its own 16 bits.
    let d = env.get(&format!("sessions/{sid}/svd/TIMX?read=true&registers=sr")).await;
    assert_eq!(d["registers"][1]["value"], format!("0x{:04X}", fake_word(0x4000_0004, 2)));
    assert_eq!(env.requests("readMemory").last().unwrap()["arguments"]["count"], 2);
    // A fault is that register's, not the request's.
    let d = env.get(&format!("sessions/{sid}/svd/FAULTY?read=true")).await;
    assert_eq!(d["registers"][0]["error"], "Out of memory");

    // Write a register (a hex string), then one field by its value's name: a read-modify-write of CR1.
    let put = |path: String, body: Value| {
        let env = &env;
        async move { env.send(reqwest::Method::PUT, &path, body).await }
    };
    let (s, v) = put(format!("sessions/{sid}/svd/TIMX/CR1"), json!({ "value": "0x00000007" })).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["value"].clone(), v["fields"][1]["valueName"].clone()), (json!("0x00000007"), Value::Null), "MODE 3 has no name: {v}");
    let w = env.requests("writeMemory");
    assert_eq!((w.last().unwrap()["arguments"]["memoryReference"].clone(), w.last().unwrap()["arguments"]["data"].clone()), (json!("0x40000000"), json!("BwAAAA==")), "{w:?}");
    let (s, v) = put(format!("sessions/{sid}/svd/TIMX/CR1"), json!({ "field": "MODE", "value": "Down" })).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["value"].clone(), v["fields"][1]["valueName"].clone(), v["fields"][0]["value"].clone()), (json!("0x00000003"), json!("Down"), json!(1)), "CEN stays set: {v}");
    let (s, v) = put(format!("sessions/{sid}/svd/TIMX/cr1"), json!({ "field": "CEN", "value": 0 })).await;
    assert_eq!((s, v["value"].clone()), (200, json!("0x00000002")));
    // The 8-bit write-only register is written with one byte, and cannot be read back.
    let (s, v) = put(format!("sessions/{sid}/svd/TIMX/OUT"), json!({ "value": 255 })).await;
    assert_eq!((s, v["value"].clone()), (200, Value::Null), "{v}");
    assert_eq!(env.requests("writeMemory").last().unwrap()["arguments"]["data"], "/w==");
    let console = env.console(&sid).await;
    assert!(console.contains("[workbench]Wrote 0x00000007 to TIMX.CR1") && console.contains("(field MODE)"), "{console}");

    // Mistakes: a value that does not fit, a read-only register, a field of a register whose read has effects, unknown names.
    let before = env.requests("writeMemory").len();
    for (path, body, status, needle) in [
        ("TIMX/CR1", json!({ "field": "MODE", "value": 4 }), 400, "2 bits wide"),
        ("TIMX/CR1", json!({ "value": "0x100000000" }), 400, "32 bits wide"),
        ("TIMX/CR1", json!({ "value": "banana" }), 400, "not a number"),
        ("TIMX/SR", json!({ "value": 1 }), 400, "is read-only"),
        ("TIMX/CR1", json!({ "field": "NOPE", "value": 1 }), 404, "no field"),
        ("TIMX/NOPE", json!({ "value": 1 }), 404, "no register"),
        ("NOPE/CR1", json!({ "value": 1 }), 404, "no peripheral"),
    ] {
        let (s, v) = put(format!("sessions/{sid}/svd/{path}"), body.clone()).await;
        assert_eq!(s, status, "{path} {body}: {v}");
        assert!(v["error"]["message"].as_str().unwrap().contains(needle), "{path} {body}: {v}");
    }
    assert_eq!(env.requests("writeMemory").len(), before, "nothing was written for a refused request");

    // Agents get none of it: reading a register can change the chip.
    let ctx = crate::mcp::McpCtx::default();
    for (method, path, body) in [
        (axum::http::Method::GET, format!("sessions/{sid}/svd"), None),
        (axum::http::Method::GET, format!("sessions/{sid}/svd/TIMX?read=true"), None),
        (axum::http::Method::PUT, format!("sessions/{sid}/svd/TIMX/CR1"), Some(json!({ "value": 1 }))),
    ] {
        let p = format!("/api/projects/{}/debug/{path}", env.pid());
        assert_eq!(crate::mcp::call_api(&env.state, method, &p, body, &ctx).await.unwrap_err().status, 403, "{path}");
    }

    // Ended: no reading, no writing.
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    let (s, v) = env.send(reqwest::Method::GET, &format!("sessions/{sid}/svd/TIMX?read=true"), json!({})).await;
    assert_eq!(s, 409, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("not suspended"));

    // No map, a missing file, a file that is no SVD: each says so.
    for (cfg, needle) in [("no map", "no SVD file"), ("missing map", "cannot read"), ("not a map", "is not an SVD file")] {
        let sid = env.start(cfg).await;
        env.wait_session(&sid, "the halt", |v| v["state"] == "stopped").await;
        let (s, v) = env.send(reqwest::Method::GET, &format!("sessions/{sid}/svd"), json!({})).await;
        assert!(s == 404 || s == 422, "{cfg}: {s} {v}");
        assert!(v["error"]["message"].as_str().unwrap().contains(needle), "{cfg}: {v}");
        env.post(&format!("sessions/{sid}/stop"), json!({})).await;
    }
}

/// The Peripherals view against a real core: SysTick's registers (an SVD written from Arm's
/// manual) read from QEMU's Cortex-M3 after the firmware enabled the timer, one changed through
/// a field and read back, and the register that clears on read left alone until named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cortex_m_systick_is_read_and_written_through_its_svd() {
    let Some(tools) = embedded_tools(&["gdb-multiarch", "qemu-system-arm", "arm-none-eabi-gcc"]) else { return };
    let (gdb, qemu, gcc) = (&tools[0], &tools[1], &tools[2]);
    let debug_toml = format!("[adapters.gdb-multiarch]\ncommand = {}\n[servers.qemu-arm]\ncommand = {}\n", toml_str(gdb), toml_str(qemu));
    let build = format!(
        "{gcc} -mcpu=cortex-m3 -mthumb -g -O0 -nostdlib -ffreestanding -T link.ld src/startup.c src/main.c -o firmware.elf",
        gcc = crate::apps::expand::shell_quote(gcc)
    );
    let project = format!(
        r#"
[[debug]]
name = "systick"
program = "firmware.elf"
pre_launch = {build}
stop_on_entry = true
[debug.remote]
server = "qemu-arm"
server_args = ["-M", "lm3s6965evb", "-kernel", "{{program}}"]
svd = "systick.svd"
"#,
        build = toml_str(&build)
    );
    let files = [
        ("link.ld", include_str!("testdata/cortex_m3.ld")),
        ("src/startup.c", include_str!("testdata/cortex_m3_startup.c")),
        ("src/main.c", include_str!("testdata/cortex_m3_main.c")),
        ("systick.svd", include_str!("testdata/cortex_m_systick.svd")),
    ];
    let env = setup_with(&project, Opts { debug_toml: &debug_toml, files: &files, ..Default::default() }).await;
    if !available(&env.get("adapters").await, "gdb-multiarch") {
        eprintln!("skipped: gdb-multiarch is older than GDB 14 or has no Python");
        return;
    }
    let sid = env.start("systick").await;
    let info = env.wait_session_for(&sid, "main", Duration::from_secs(60), |v| matches!(v["state"].as_str(), Some("stopped" | "failed"))).await;
    assert_eq!(info["state"], "stopped", "{info}\n{}", env.console(&sid).await);
    let regs = |d: &Value, name: &str| d["registers"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap().clone();

    // At main nothing has run yet: the reload register is as the core reset it.
    let d = env.get(&format!("sessions/{sid}/svd/SYST?read=true")).await;
    assert_eq!(regs(&d, "RVR")["value"], "0x00000000", "{d}");
    // Two steps run the firmware's two stores: the reload value and the enable bits.
    for _ in 0..2 {
        env.post(&format!("sessions/{sid}/control"), json!({ "action": "next" })).await;
        env.wait_session(&sid, "the step", |v| v["state"] == "stopped" && v["stopped"]["reason"] == "step").await;
    }
    let d = env.get(&format!("sessions/{sid}/svd/SYST?read=true")).await;
    assert_eq!(regs(&d, "RVR")["value"], "0x00FFFFFF", "{d}");
    assert_eq!(regs(&d, "RVR")["fields"][0]["value"], 0xFFFFFF);
    assert_eq!(regs(&d, "CSR")["skipped"], "reading it changes the chip", "COUNTFLAG clears when CSR is read");
    // CALIB is read-only and readable; CVR is the live counter (a number in range).
    assert_eq!(regs(&d, "CALIB")["access"], "read-only");
    assert!(regs(&d, "CVR")["fields"][0]["value"].as_u64().unwrap() <= 0xFFFFFF);
    let d = env.get(&format!("sessions/{sid}/svd/SYST?read=true&registers=CSR")).await;
    let csr = regs(&d, "CSR");
    assert_eq!((csr["fields"][0]["value"].clone(), csr["fields"][2]["value"].clone(), csr["fields"][2]["valueName"].clone()), (json!(1), json!(1), json!("Processor")), "ENABLE and CLKSOURCE: {csr}");
    // Change the reload value through its field, and read it back from the core.
    let (s, v) = env.send(reqwest::Method::PUT, &format!("sessions/{sid}/svd/SYST/RVR"), json!({ "field": "RELOAD", "value": 1000 })).await;
    assert_eq!((s, v["value"].clone()), (200, json!("0x000003E8")), "{v}");
    let d = env.get(&format!("sessions/{sid}/svd/SYST?read=true")).await;
    assert_eq!(regs(&d, "RVR")["value"], "0x000003E8");
    // And the whole CSR: tick interrupt on, counter off.
    let (s, v) = env.send(reqwest::Method::PUT, &format!("sessions/{sid}/svd/SYST/CSR"), json!({ "value": "0x6" })).await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = env.send(reqwest::Method::PUT, &format!("sessions/{sid}/svd/SYST/CSR"), json!({ "field": "ENABLE", "value": 1 })).await;
    assert_eq!(s, 400, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("changes the chip"), "{v}");
    let d = env.get(&format!("sessions/{sid}/svd/SYST?read=true&registers=csr")).await;
    assert_eq!((regs(&d, "CSR")["fields"][0]["value"].clone(), regs(&d, "CSR")["fields"][1]["value"].clone()), (json!(0), json!(1)), "{d}");
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// Where the program's source was built is not where it is here: `source_map` reaches gdb as
/// `set substitute-path` before it reads the program, `{root}` expanded, quoted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_source_map_reaches_gdb_before_the_program() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = remote_env(
        r#"
[[debug]]
name = "ci build"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
download = false
stop_at = "reset"
source_map = [["/ci/workspace", "{root}/src"], ["/opt/sdk", "/usr/src/sdk"]]

[[debug]]
name = "bad pair"
adapter = "fakegdb"
program = "prog.bin"
[debug.remote]
server = "fakesrv"
source_map = [["/a\nb", "/c"]]
"#,
    )
    .await;
    let (s, v) = env.send(reqwest::Method::POST, "sessions", json!({ "config": "bad pair" })).await;
    assert_eq!(s, 400, "{v}");
    let sid = env.start("ci build").await;
    env.wait_session(&sid, "the halt", |v| v["state"] == "stopped" || v["state"] == "failed").await;
    let log = std::fs::read_to_string(&env.log).unwrap();
    let argv: Vec<String> = log.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find_map(|v| v.get("_argv").cloned()).unwrap().as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
    // `{root}/src` is the root as the computer spells it plus what the configuration wrote; a
    // backslash in a Windows path is escaped for gdb's own quoting.
    let src = format!("{}/src", env.root.display()).replace('\\', "\\\\");
    assert_eq!(
        argv,
        ["-iex".to_string(), format!("set substitute-path \"/ci/workspace\" \"{src}\""), "-iex".to_string(), "set substitute-path \"/opt/sdk\" \"/usr/src/sdk\"".to_string(), env.root.join("prog.bin").display().to_string()],
        "the pairs, then the program"
    );
    env.post(&format!("sessions/{sid}/stop"), json!({})).await;
}

/// An agent starts a configuration, evaluates in the stopped session, reruns it and attaches
/// to a process; each answers with the state, and what it ran is told to the user's console.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agents_start_evaluate_rerun_and_attach() {
    if !have_python() {
        eprintln!("skipped: Python 3 is not installed");
        return;
    }
    let env = setup("").await;
    // Nothing to evaluate in yet.
    let err = env.agent("debug_evaluate", json!({ "expression": "x" })).await.unwrap_err();
    assert_eq!(err.code, "conflict", "{}", err.message);
    // Start: the answer is the stop at the breakpoint the agent set.
    env.agent("debug_breakpoints", json!({ "action": "add", "path": "src/main.c", "breakpoints": [{ "line": 5 }] })).await.unwrap();
    let r = env.agent("debug_start", json!({ "config": "fake", "waitSeconds": 20 })).await.unwrap();
    assert_eq!((r["state"].clone(), r["settled"].clone()), (json!("stopped"), json!(true)), "{r}");
    assert!(r["stack"].as_array().is_some_and(|s| !s.is_empty()), "{r}");
    let sid = r["sessionId"].as_str().unwrap().to_string();
    // A start of a configuration that is not there, and without one.
    let err = env.agent("debug_start", json!({ "config": "nope" })).await.unwrap_err();
    assert!(err.message.contains("nope"), "{}", err.message);
    assert_eq!(env.agent("debug_start", json!({})).await.unwrap_err().code, "bad_request");

    // Evaluate: the same value the user's own evaluation gets; a failing one says why.
    let mine = env.post(&format!("sessions/{sid}/evaluate"), json!({ "expression": "x", "context": "watch" })).await;
    let v = env.agent("debug_evaluate", json!({ "expression": "x" })).await.unwrap();
    assert_eq!((v["context"].clone(), v["result"]["value"].clone()), (json!("watch"), mine["value"].clone()), "{v}");
    let v = env.agent("debug_evaluate", json!({ "expression": "x", "frame": 1, "threadId": 1 })).await.unwrap();
    assert_eq!(v["result"]["value"], mine["value"], "{v}");
    let err = env.agent("debug_evaluate", json!({ "expression": "boom" })).await.unwrap_err();
    assert!(err.message.contains("No symbol"), "{}", err.message);
    for bad in [json!(""), json!("a\u{0}b"), json!("x".repeat(10_001))] {
        assert_eq!(env.agent("debug_evaluate", json!({ "expression": bad })).await.unwrap_err().code, "bad_request");
    }
    // A console command is echoed into the user's console, marked as the agent's.
    env.agent("debug_evaluate", json!({ "expression": "x", "context": "repl" })).await.unwrap();
    let console = env.console(&sid).await;
    assert!(console.contains("> x   (agent)"), "{console}");

    // Rerun: a new session, the old one gone.
    let r = env.agent("debug_restart", json!({ "waitSeconds": 20 })).await.unwrap();
    let new = r["sessionId"].as_str().unwrap().to_string();
    assert_ne!(new, sid);
    assert_eq!(r["state"], "stopped", "{r}");
    let listed = env.get("sessions").await;
    assert!(listed.as_array().unwrap().iter().all(|s| s["id"] != sid.as_str()), "{listed}");
    env.post(&format!("sessions/{new}/stop"), json!({})).await;

    // Attach: not to Workbench or to init, the pid is needed, and a process answers with the attach.
    for bad in [json!({}), json!({ "pid": 1 }), json!({ "pid": std::process::id() })] {
        assert_eq!(env.agent("debug_attach", bad).await.unwrap_err().code, "bad_request");
    }
    let mut victim = sleeper();
    let r = env.agent("debug_attach", json!({ "pid": victim.id(), "adapter": "fake", "waitSeconds": 20 })).await.unwrap();
    assert_eq!(r["request"], "attach", "{r}");
    assert_eq!(env.requests("attach")[0]["arguments"]["processId"], victim.id());
    env.agent("debug_control", json!({ "action": "stop" })).await.unwrap();
    let _ = victim.kill();
    let _ = victim.wait();

    // A session of another project starts this project's configuration by naming it.
    let r = env.agent_in("elsewhere", "debug_start", json!({ "config": "fake", "projectId": "proj", "waitSeconds": 20 })).await.unwrap();
    assert_eq!(r["state"], "stopped", "{r}");
    env.agent_in("elsewhere", "debug_control", json!({ "action": "stop", "projectId": "proj" })).await.unwrap();
    // Without a project of its own and without projectId there is nothing to act on.
    let tool = crate::mcp::all_tools().into_iter().find(|t| t.name == "debug_start").unwrap();
    let err = (tool.handler)(env.state.clone(), crate::mcp::McpCtx::default(), json!({ "config": "fake" })).await.err().unwrap();
    assert_eq!(err.code, "bad_request");
}
