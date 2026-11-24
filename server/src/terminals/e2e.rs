//! End-to-end tests of the slice against a real AppState: the Rust contract other slices
//! use, the terminal WebSocket, and hook authorization. Everything runs in temp dirs, on
//! every OS: the terminals run small Python programs (`util::os::exe::python`) and the
//! fake agent CLIs of `testdata/fake_cli.py`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{SpawnSpec, TerminalKind, TerminalStatus};
use crate::app::{self, AppState};

async fn test_state(dir: &std::path::Path) -> AppState {
    test_state_with(dir, |_| {}).await
}

async fn test_state_with(dir: &std::path::Path, f: impl FnOnce(&mut crate::config::GlobalConfig)) -> AppState {
    let paths = crate::config::Paths { config_dir: dir.join("config"), data_dir: dir.join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.agents.restore_on_start = false;
    f(&mut cfg);
    // Port 0 is replaced by the real listener in `serve`.
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    super::start(&state).await;
    state
}

/// argv running the Python program `code`.
fn python_argv(code: &str) -> Vec<String> {
    let mut argv = crate::util::os::exe::python();
    argv.extend(["-c".to_string(), code.to_string()]);
    argv
}

/// `s` as a Python string literal (a JSON string is one).
fn py_str(s: &str) -> String {
    json!(s).to_string()
}

/// A command terminal running the Python program `code`.
fn spec(cwd: &Path, code: &str) -> SpawnSpec {
    SpawnSpec {
        kind: TerminalKind::Command,
        title: "test".into(),
        project_id: None,
        cwd: cwd.to_path_buf(),
        argv: python_argv(code),
        env: vec![("WB_TEST_VAR".into(), Some("from-spec".into()))],
        cols: Some(80),
        rows: Some(24),
        meta: json!({ "run": "demo" }),
    }
}

/// Programs the terminals below run.
const SLEEP_5: &str = "import time\ntime.sleep(5)";
const EXIT_0: &str = "pass";
/// Prints the terminal's size (`rows cols`, as `stty size`) for every line read.
const SIZE_PER_LINE: &str = "import os, sys\nfor l in sys.stdin:\n    s = os.get_terminal_size()\n    print(f'{s.lines} {s.columns}', flush=True)";

/// A program that leaves a job running (in a process group of its own on Unix, as a
/// shell's background job) and writes its pid to `pidfile`.
fn leave_job(pidfile: &Path, secs: u32, say: &str) -> String {
    format!(
        "import os, subprocess, sys\nkw = {{'preexec_fn': os.setpgrp}} if os.name == 'posix' else {{}}\n\
         c = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep({secs})'], **kw)\n\
         with open({}, 'w') as f:\n    f.write(str(c.pid))\nprint({}, flush=True)",
        py_str(&pidfile.display().to_string()),
        py_str(say)
    )
}

async fn wait_exit(state: &AppState, id: &str) -> super::ExitInfo {
    let mut rx = state.terminals.exit_watch(id).unwrap();
    let v = tokio::time::timeout(Duration::from_secs(10), async move { rx.wait_for(|v| v.is_some()).await.map(|v| v.clone()) })
        .await
        .expect("process did not exit")
        .unwrap();
    v.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn contract_spawn_input_output_exit_restart_forget() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;

    let mut s = spec(dir.path(), "import os, sys\nx = input()\nprint('got:' + x + ' env:' + os.environ['WB_TEST_VAR'])\nsys.exit(3)");
    // The spawner allows plain re-runs from the terminals UI.
    s.meta = json!({ "run": "demo", "restartable": true });
    let info = t.spawn(&state, s).await.unwrap();
    assert_eq!(info.status, TerminalStatus::Running);
    assert_eq!(info.meta["run"], "demo");
    let mut out = t.subscribe_output(&info.id).unwrap();
    t.send_text(&info.id, "hello", true).await.unwrap();
    let exit = wait_exit(&state, &info.id).await;
    assert_eq!(exit.code, Some(3));
    assert!(!exit.terminated, "it exited by itself");
    // The exit is recorded by the time it is announced.
    let now = t.info(&info.id).unwrap();
    assert_eq!((now.status, now.exit.as_ref().and_then(|e| e.code)), (TerminalStatus::Exited, Some(3)));
    // Live output reached the subscriber.
    let mut seen = String::new();
    while let Ok(chunk) = out.try_recv() {
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.contains("got:hello env:from-spec"), "live output: {seen:?}");
    // The mirror keeps the final screen.
    let text = t.screen_text(&info.id, 50).unwrap();
    assert!(text.contains("got:hello env:from-spec"), "screen: {text:?}");
    // Saved to disk with the final screen by the time the exit was announced.
    let tdir = state.paths.data_dir.join("terminals").join(&info.id);
    assert!(tdir.join("meta.json").is_file() && tdir.join("screen.bin").is_file());
    let saved: super::store::Record = crate::util::fs::read_json(&tdir.join("meta.json")).unwrap().unwrap();
    assert_eq!((saved.info.status, saved.info.exit.and_then(|e| e.code)), (TerminalStatus::Exited, Some(3)));

    // Restart re-runs the command below the old output.
    t.restart(&state, &info.id).await.unwrap();
    assert_eq!(t.info(&info.id).unwrap().status, TerminalStatus::Running);
    // Kill returns once the exit is recorded (on Windows it usually waits for the record:
    // the output ends only once the pseudoconsole has closed).
    t.kill(&info.id).await.unwrap();
    let now = t.info(&info.id).unwrap();
    assert_eq!(now.status, TerminalStatus::Exited);
    // Workbench ended it, on every OS.
    assert!(now.exit.is_some_and(|e| e.terminated), "a kill's exit is terminated");
    assert!(t.write(&info.id, b"x").is_err(), "writing to an exited terminal must fail");

    // Close keeps it in history; forget removes it and its files.
    t.close(&state, &info.id, false).await.unwrap();
    assert!(!t.info(&info.id).unwrap().open);
    t.close(&state, &info.id, true).await.unwrap();
    assert!(t.info(&info.id).is_none());
    assert!(!tdir.exists());
}

/// A terminal forgotten while its process runs stays forgotten: neither the save of its
/// exit nor a save already under way (the flusher's) brings its files back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_forgotten_while_it_runs_stays_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let info = t.spawn(&state, spec(dir.path(), "import time
print('up', flush=True)
time.sleep(30)")).await.unwrap();
    let entry = t.get(&info.id).unwrap();
    let tdir = state.paths.data_dir.join("terminals").join(&info.id);
    t.save_now(&entry).await;
    assert!(tdir.join("meta.json").is_file());

    t.close(&state, &info.id, true).await.unwrap();
    assert!(t.info(&info.id).is_none());
    assert!(!tdir.exists());
    t.save_now(&entry).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!tdir.exists(), "a forgotten terminal's files came back");
}

/// Holds a terminal's saves (each write waits for `files_removed`) until dropped, so the
/// save of an exit, and with it the exit's announcement, stays under way meanwhile. On a
/// real disk that window is the time of a few fsyncs; on the tmpfs tests use it is too
/// short to hit.
struct SavesHeld {
    _release: std::sync::mpsc::Sender<()>,
}

fn hold_saves(entry: &std::sync::Arc<super::Entry>) -> SavesHeld {
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (held_tx, held) = std::sync::mpsc::channel::<()>();
    let e = entry.clone();
    std::thread::spawn(move || {
        let _saves = e.files_removed.lock();
        let _ = held_tx.send(());
        let _ = released.recv();
    });
    held.recv().unwrap();
    SavesHeld { _release: release }
}

/// Make the terminal's process (waiting for a line) exit, and return once the exit is
/// recorded while its save is held, before it is announced.
async fn exit_while_saves_are_held(state: &AppState, id: &str) -> SavesHeld {
    let t = &state.terminals;
    let held = hold_saves(&t.get(id).unwrap());
    t.send_text(id, "bye", true).await.unwrap();
    wait_for("the exit to be recorded", || t.info(id).is_some_and(|i| i.status == TerminalStatus::Exited)).await;
    assert!(t.exit_watch(id).unwrap().borrow().is_none(), "announced before it was saved");
    held
}

/// A restart that comes while the exit before it is being saved starts once that exit is
/// announced: the new process never reads as exited (`exit_watch`, and the waits of the
/// next kill, restart or close on it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_during_the_save_of_an_exit_is_not_taken_for_exited() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let s = SpawnSpec { meta: json!({ "run": "demo", "restartable": true }), ..spec(dir.path(), "input()") };
    let id = t.spawn(&state, s).await.unwrap().id;

    let held = exit_while_saves_are_held(&state, &id).await;
    let restart = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move { state.terminals.restart(&state, &id).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let restarted_early = restart.is_finished();
    drop(held);
    restart.await.unwrap().unwrap();
    // Whatever was still under way for the old process is over by now.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(t.info(&id).unwrap().status, TerminalStatus::Running);
    let exit = t.exit_watch(&id).unwrap().borrow().clone();
    assert!(exit.is_none(), "the running process reads as exited: {exit:?}");
    assert!(!restarted_early, "the restart started before the exit was saved and announced");

    // Close (forget) while the next exit is being saved: it waits for that save, and the
    // files stay gone.
    let tdir = state.paths.data_dir.join("terminals").join(&id);
    let held = exit_while_saves_are_held(&state, &id).await;
    let close = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move { state.terminals.close(&state, &id, true).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!close.is_finished());
    drop(held);
    close.await.unwrap().unwrap();
    assert!(t.info(&id).is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!tdir.exists(), "a forgotten terminal's files came back");
}

/// Kill returns once the exit is saved, also when the process exited by itself and its
/// exit is being saved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kill_during_the_save_of_an_exit_waits_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let id = t.spawn(&state, spec(dir.path(), "input()")).await.unwrap().id;
    let held = exit_while_saves_are_held(&state, &id).await;
    let kill = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move { state.terminals.kill(&id).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!kill.is_finished(), "kill returned before the exit was saved");
    drop(held);
    kill.await.unwrap().unwrap();
    assert!(t.exit_watch(&id).unwrap().borrow().is_some());
    let meta = state.paths.data_dir.join("terminals").join(&id).join("meta.json");
    let saved: super::store::Record = crate::util::fs::read_json(&meta).unwrap().unwrap();
    assert_eq!(saved.info.status, TerminalStatus::Exited);
}

/// A process that exited by itself keeps its own end when a Kill (or Close, or Restart)
/// comes while its exit is still being recorded (the reader drains the output of what it
/// left running for up to 800 ms first): its code stands, and it is not `terminated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kill_after_a_process_exited_by_itself_leaves_its_code() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    // The job it leaves holds the terminal open, so the reader has not ended when it exits.
    let script = format!("{}\nsys.exit(2)", leave_job(&dir.path().join("job.pid"), 30, "started"));
    let id = t.spawn(&state, command(dir.path(), &script, json!({}))).await.unwrap().id;
    let pty = t.get(&id).and_then(|e| e.running_pty()).expect("its process");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !pty.has_exited() {
        assert!(tokio::time::Instant::now() < deadline, "the process did not exit");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(t.info(&id).unwrap().status, TerminalStatus::Running, "its exit was recorded already");
    t.kill(&id).await.unwrap();
    let exit = t.info(&id).unwrap().exit.unwrap();
    assert_eq!(exit.code, Some(2), "{exit:?}");
    assert!(!exit.terminated, "it exited by itself before the kill: {exit:?}");
}

/// A restart and a restore of an agent session that wait for the lifecycle lock while the
/// terminal is forgotten start nothing once it is gone: no agent token of it stays valid,
/// and its directory (`mcp.json` holds the token) does not come back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starts_waiting_while_an_agent_is_forgotten_leave_no_token_or_files() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "claude");
    let claude_home = dir.path().join("claude-home");
    std::fs::create_dir_all(&claude_home).unwrap();
    let claude = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        // Hermetic: never the user's own ~/.claude.
        env: [
            ("CLAUDE_CONFIG_DIR".to_string(), claude_home.display().to_string()),
            ("FAKE_CLAUDE_LOG".to_string(), dir.path().join("claude.log").display().to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("claude", claude)]).await;
    state.config.write().agents.restore_on_start = true;
    let t = &state.terminals;
    let id = t.spawn_agent(&state, super::agent::AgentRequest { project_id: pid, ..Default::default() }).await.unwrap().id;
    let tdir = state.paths.data_dir.join("terminals").join(&id);
    assert!(tdir.join("mcp.json").is_file() && state.auth.has_agent_token(&id));
    let entry = t.get(&id).unwrap();
    // As a restore finds a session that ran when Workbench stopped.
    t.update(&entry, |r| {
        r.was_running = true;
        true
    });

    // The close holds the lifecycle lock while the exit is being saved; a restart and a
    // restore wait for it meanwhile.
    let held = hold_saves(&entry);
    let close = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move { state.terminals.close(&state, &id, true).await }
    });
    wait_for("the close to take the lock", || entry.lifecycle.try_lock().is_err()).await;
    let restart = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move { state.terminals.restart(&state, &id).await }
    });
    super::agent::restore(state.clone()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!close.is_finished() && !restart.is_finished());
    drop(held);
    close.await.unwrap().unwrap();
    assert_eq!(restart.await.unwrap().unwrap_err().code, "not_found");
    // The restore's attempt comes after the restart's (the lock is fair).
    wait_for("the restore's attempt", || entry.lifecycle.try_lock().is_ok()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(t.info(&id).is_none());
    assert!(!state.auth.has_agent_token(&id), "a forgotten terminal has a valid agent token");
    assert!(!tdir.exists(), "a forgotten terminal's files came back");
}

/// A start that fails after its agent token was issued leaves no valid token: no process
/// holds it, and pruning forgets the terminal without revoking anything. (Unix: the
/// terminal's directory cannot be made in a read-only `terminals`.)
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_start_that_fails_leaves_no_valid_token() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "claude");
    let claude_home = dir.path().join("claude-home");
    std::fs::create_dir_all(&claude_home).unwrap();
    let claude = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [
            ("CLAUDE_CONFIG_DIR".to_string(), claude_home.display().to_string()),
            ("FAKE_CLAUDE_LOG".to_string(), dir.path().join("claude.log").display().to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("claude", claude)]).await;
    let root = state.paths.data_dir.join("terminals");
    crate::util::os::perm::apply(&root, 0o500).unwrap();
    if std::fs::create_dir(root.join("probe")).is_ok() {
        eprintln!("skip: permissions do not apply to this user");
        return;
    }
    let t = &state.terminals;
    let r = t.spawn_agent(&state, super::agent::AgentRequest { project_id: pid, ..Default::default() }).await;
    crate::util::os::perm::apply(&root, 0o700).unwrap();
    assert!(r.is_err(), "{r:?}");
    let info = t.list().into_iter().find(|i| i.kind == TerminalKind::Agent).expect("the agent terminal");
    assert!(!state.auth.has_agent_token(&info.id), "a start that failed left a valid agent token");
}

/// What Workbench starts in a terminal (a run, a pre-launch step, a command, an agent CLI)
/// gets `util::os::exe::child_env` (on Windows a cmd.exe among its processes never takes a
/// program from the current directory); an interactive shell keeps its usual lookup.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn programs_but_not_interactive_shells_get_the_child_env() {
    const VAR: &str = "NoDefaultCurrentDirectoryInExePath";
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let set = crate::util::os::exe::child_env().iter().find(|(k, _)| *k == VAR).map(|(_, v)| v.to_string());
    assert_eq!(set.as_deref(), cfg!(windows).then_some("1"));
    let code = format!("import os\nprint('value=' + str(os.environ.get({})), flush=True)", py_str(VAR));
    for (kind, want) in [(TerminalKind::Command, set.clone()), (TerminalKind::Run, set.clone()), (TerminalKind::Shell, None)] {
        let mut s = spec(dir.path(), &code);
        s.kind = kind;
        // Not whatever this process has (the test's own environment may set it), and a spec
        // cannot undo what Workbench sets.
        s.env.push((VAR.to_string(), None));
        let info = state.terminals.spawn(&state, s).await.unwrap();
        wait_exit(&state, &info.id).await;
        let text = state.terminals.screen_text(&info.id, 50).unwrap();
        let want = format!("value={}", want.as_deref().unwrap_or("None"));
        assert!(text.lines().any(|l| l.trim_end() == want), "{kind:?}: want {want:?} in {text:?}");
    }
    let agent: Vec<_> = super::program_env(TerminalKind::Agent).into_iter().map(|(k, v)| (k, v.unwrap_or_default())).collect();
    assert_eq!(agent, set.map(|v| (VAR.to_string(), v)).into_iter().collect::<Vec<_>>());
}

/// A terminal finds programs installed since Workbench started: on Windows its `PATH` is the
/// sign-in's, then Workbench's own entries (`util::os::env::fresh_path`), and a variable
/// set in the registry since reaches it too. On Unix it has Workbench's `PATH`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminals_get_the_path_of_a_new_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    #[cfg(windows)]
    let var = crate::util::os::env::UserVar::new("set-since");
    let out = dir.path().join("env.json");
    let code = format!("import json, os\nwith open({}, 'w') as f:\n    json.dump(dict(os.environ), f)", py_str(&out.display().to_string()));
    let info = state.terminals.spawn(&state, spec(dir.path(), &code)).await.unwrap();
    wait_exit(&state, &info.id).await;
    let env: std::collections::HashMap<String, String> = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let want = crate::util::os::env::fresh_path().unwrap_or_else(|| std::env::var("PATH").unwrap());
    // A version manager's shim for Python may put its own folder first.
    assert!(env["PATH"].ends_with(&want), "{:?}\nwant {want:?}", env["PATH"]);
    // Python's `os.environ` has uppercase names on Windows.
    #[cfg(windows)]
    assert_eq!(env.get(&var.name.to_uppercase()), Some(&var.value));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_output_tool_reads_a_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let info = state.terminals.spawn(&state, spec(dir.path(), "import time\nprint('mcp-visible-line', flush=True)\ntime.sleep(5)")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let tool = super::mcp_tools().into_iter().find(|t| t.name == "workbench_terminal_output").unwrap();
    let out = (tool.handler)(state.clone(), Default::default(), json!({ "terminalId": info.id, "lines": 20 })).await.unwrap();
    match out {
        crate::mcp::ToolOutput::Text(s) => assert!(s.contains("mcp-visible-line"), "{s:?}"),
        other => panic!("unexpected {other:?}"),
    }
    let bad = (tool.handler)(state.clone(), Default::default(), json!({ "terminalId": "../x" })).await;
    assert!(bad.is_err());
    // A hosted session of some project cannot read terminals outside it…
    let foreign = crate::mcp::McpCtx { terminal_id: Some("agent-1".into()), project_id: Some("other".into()) };
    let err = (tool.handler)(state.clone(), foreign, json!({ "terminalId": info.id })).await.unwrap_err();
    assert_eq!(err.code, "not_found");
    // …but can read itself.
    let own = crate::mcp::McpCtx { terminal_id: Some(info.id.clone()), project_id: None };
    assert!((tool.handler)(state.clone(), own, json!({ "terminalId": info.id })).await.is_ok());
    state.terminals.kill(&info.id).await.unwrap();
}

/// Serve the full router on a random loopback port.
async fn serve(state: &AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app::build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    addr
}

async fn next_text(ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
        if let Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_snapshot_input_resize_and_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let info = state
        .terminals
        .spawn(&state, spec(dir.path(), "import os, sys\nfor l in sys.stdin:\n    s = os.get_terminal_size()\n    print('echo:' + l.strip() + f'\\n{s.lines} {s.columns}', flush=True)"))
        .await
        .unwrap();
    let addr = serve(&state).await;
    let connect = || {
        let mut req = format!("ws://{addr}/api/terminals/{}/ws", info.id).into_client_request().unwrap();
        req.headers_mut().insert("Authorization", format!("Bearer {}", state.auth.master_token()).parse().unwrap());
        tokio_tungstenite::connect_async(req)
    };

    let (mut ws, _) = connect().await.unwrap();
    let hello = next_text(&mut ws).await;
    assert_eq!(hello["t"], "snapshot");
    assert_eq!(hello["cols"], 80);
    // The snapshot follows as a binary frame.
    let snap = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(snap, Message::Binary(_)));

    ws.send(Message::Text(json!({ "t": "resize", "cols": 100, "rows": 30 }).to_string().into())).await.unwrap();
    // Out-of-range sizes are ignored.
    ws.send(Message::Text(json!({ "t": "resize", "cols": 5, "rows": 3 }).to_string().into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    ws.send(Message::Binary(b"marker-one\r".to_vec().into())).await.unwrap();
    let mut got = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(got.contains("echo:marker-one") && got.contains("30 100")) && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Binary(b)))) = tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            got.push_str(&String::from_utf8_lossy(&b));
        }
    }
    assert!(got.contains("echo:marker-one") && got.contains("30 100"), "live: {got:?}");
    drop(ws);

    // A new connection's snapshot holds what happened before it.
    let (mut ws2, _) = connect().await.unwrap();
    let hello = next_text(&mut ws2).await;
    assert_eq!((hello["cols"].as_u64(), hello["rows"].as_u64()), (Some(100), Some(30)));
    let Message::Binary(snap) = tokio::time::timeout(Duration::from_secs(5), ws2.next()).await.unwrap().unwrap().unwrap() else {
        panic!("expected a binary snapshot");
    };
    let mut mirror = super::pty::new_mirror(30, 100, 100);
    mirror.process(&snap);
    assert!(mirror.screen().contents().contains("echo:marker-one"));

    // The exit is announced on the socket.
    state.terminals.kill(&info.id).await.unwrap();
    let exit = next_text(&mut ws2).await;
    assert_eq!(exit["t"], "exit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hooks_require_the_terminals_agent_token() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let info = state.terminals.spawn(&state, spec(dir.path(), SLEEP_5)).await.unwrap();
    let addr = serve(&state).await;
    let url = format!("http://{addr}/api/hooks/claude/{}", info.id);
    let client = reqwest::Client::new();
    let body = json!({ "hook_event_name": "Stop" }).to_string();
    let no_auth = client.post(&url).header("content-type", "application/json").body(body.clone()).send().await.unwrap();
    assert_eq!(no_auth.status(), 401);
    let other = state.auth.issue_agent_token("someotherterminal");
    let wrong = client.post(&url).bearer_auth(other).body(body.clone()).send().await.unwrap();
    assert_eq!(wrong.status(), 401);
    let mine = state.auth.issue_agent_token(&info.id);
    let ok = client.post(&url).bearer_auth(&mine).body(body.clone()).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.json::<Value>().await.unwrap(), json!({}));
    // Agent tokens are not valid on the rest of the API.
    let api = client.get(format!("http://{addr}/api/terminals")).bearer_auth(&mine).send().await.unwrap();
    assert_eq!(api.status(), 401);
    state.terminals.kill(&info.id).await.unwrap();
}

// ---------------------------------------------------------------- review regressions

fn command(cwd: &std::path::Path, script: &str, meta: Value) -> SpawnSpec {
    SpawnSpec { meta, env: vec![], ..spec(cwd, script) }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn lines_in(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path).map(|s| s.lines().count()).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deploys_env_commands_and_runs_are_never_rerun_by_restart() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let addr = serve(&state).await;
    let client = reqwest::Client::new();
    let log = dir.path().join("deploys.log");
    let deploy = format!("import time\nwith open({}, 'a') as f:\n    f.write('DEPLOYED\\n')\ntime.sleep(30)", py_str(&log.display().to_string()));

    // A deploy as apps spawns it; restart must neither re-run it nor kill it.
    let info = t.spawn(&state, command(dir.path(), &deploy, json!({ "env": "production", "action": "deploy" }))).await.unwrap();
    wait_for("the deploy", || lines_in(&log) == 1).await;
    let r = client
        .post(format!("http://{addr}/api/terminals/{}/restart", info.id))
        .bearer_auth(state.auth.master_token())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "not_restartable");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(lines_in(&log), 1, "the deploy ran again");
    assert_eq!(t.info(&info.id).unwrap().status, TerminalStatus::Running, "a refused restart killed the deploy");
    t.kill(&info.id).await.unwrap();
    assert!(t.restart(&state, &info.id).await.is_err(), "an exited deploy must not re-run either");
    assert_eq!(lines_in(&log), 1);

    // Env commands (maybe confirmed), run configurations and unknown commands: refused.
    for (kind, meta) in [
        (TerminalKind::Command, json!({ "env": "production", "action": "command", "command": "migrate" })),
        (TerminalKind::Run, json!({ "run": "api" })),
        (TerminalKind::Command, json!({})),
    ] {
        let info = t.spawn(&state, SpawnSpec { kind, ..command(dir.path(), EXIT_0, meta.clone()) }).await.unwrap();
        wait_exit(&state, &info.id).await;
        let err = t.restart(&state, &info.id).await.unwrap_err();
        assert_eq!(err.code, "not_restartable", "{meta}");
    }
    // Log follows, Remote Control servers and opt-ins may re-run.
    for meta in [json!({ "env": "staging", "action": "logs" }), json!({ "remoteControlServer": true, "urls": [] }), json!({ "restartable": true })] {
        let info = t.spawn(&state, command(dir.path(), EXIT_0, meta.clone())).await.unwrap();
        wait_exit(&state, &info.id).await;
        t.restart(&state, &info.id).await.unwrap_or_else(|e| panic!("{meta}: {}", e.message));
        wait_exit(&state, &info.id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn secrets_are_masked_for_every_reader() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let secret_file = dir.path().join("token");
    std::fs::write(&secret_file, "glpat-FAKEGLOBALTOKEN123456\n").unwrap();
    let secret = state
        .secrets
        .resolve_ref("test", &crate::config::SecretRef::File(secret_file.display().to_string()))
        .unwrap();
    let mut s = command(dir.path(), "import os, time\nprint('token is ' + os.environ['MYTOK'], flush=True)\ntime.sleep(5)", json!({ "run": "leaky" }));
    s.env = vec![("MYTOK".into(), Some(secret.expose().to_string()))];
    let info = t.spawn_redacted(&state, s, vec![secret]).await.unwrap();
    wait_for("output", || t.screen_text(&info.id, 20).is_some_and(|x| x.contains("token is"))).await;

    let text = t.screen_text(&info.id, 20).unwrap();
    assert!(text.contains("token is ••••••") && !text.contains("FAKEGLOBAL"), "{text:?}");
    // REST /text
    let addr = serve(&state).await;
    let client = reqwest::Client::new();
    let body: Value = client
        .get(format!("http://{addr}/api/terminals/{}/text", info.id))
        .bearer_auth(state.auth.master_token())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!body["text"].as_str().unwrap().contains("FAKEGLOBAL"));
    // MCP
    let tool = super::mcp_tools().into_iter().find(|t| t.name == "workbench_terminal_output").unwrap();
    match (tool.handler)(state.clone(), Default::default(), json!({ "terminalId": info.id })).await.unwrap() {
        crate::mcp::ToolOutput::Text(s) => assert!(s.contains("••••••") && !s.contains("FAKEGLOBAL"), "{s:?}"),
        other => panic!("unexpected {other:?}"),
    }
    // WebSocket snapshot
    let mut ws = ws_connect(addr, &state, &info.id).await;
    let Message::Binary(snap) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap() else {
        panic!("expected a snapshot")
    };
    assert!(!String::from_utf8_lossy(&snap).contains("FAKEGLOBAL"));
    // Saved screen
    t.kill(&info.id).await.unwrap();
    let saved = std::fs::read(state.paths.data_dir.join("terminals").join(&info.id).join("screen.bin")).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("FAKEGLOBAL"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typing_into_a_claude_dialog_through_input_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    // A stand-in agent session that echoes whatever reaches it, like `cat`.
    let info = super::TerminalInfo {
        id: super::new_id(),
        kind: TerminalKind::Agent,
        title: "fake".into(),
        project_id: None,
        cwd: dir.path().display().to_string(),
        argv: vec![],
        status: TerminalStatus::Starting,
        exit: None,
        created_at: 1,
        last_output_at: 0,
        cols: 80,
        rows: 24,
        open: true,
        pinned: false,
        color: None,
        order: 1,
        agent: Some(super::AgentInfo {
            session_id: "s".into(),
            state: super::AgentState::NeedsPermission,
            attention: Some("Permission to use Bash".into()),
            ..super::test_agent()
        }),
        meta: json!({}),
        lingering: 0,
    };
    let rec = super::store::Record { info, launch: None, title_locked: true, transcript_path: None, was_running: false, aider_history: None };
    let entry = t.insert(rec, super::AGENT_SCROLLBACK);
    let launch = super::pty::LaunchSpec {
        argv: python_argv("import sys\nfor l in sys.stdin:\n    sys.stdout.write(l)\n    sys.stdout.flush()"),
        cwd: dir.path().to_path_buf(),
        env: vec![],
        cols: 80,
        rows: 24,
        redact: vec![],
    };
    t.launch(&state, &entry, launch, true).await.unwrap();
    let addr = serve(&state).await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/terminals/{}/input", entry.id);
    let send = |body: Value| client.post(&url).bearer_auth(state.auth.master_token()).json(&body).send();

    for st in [super::AgentState::NeedsPermission, super::AgentState::NeedsInput, super::AgentState::Starting] {
        t.update(&entry, |r| {
            r.info.agent.as_mut().unwrap().state = st;
            true
        });
        let r = send(json!({ "text": "no, don't do that", "submit": true })).await.unwrap();
        assert_eq!(r.status(), 409, "{st:?}");
        assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "agent_busy");
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!t.screen_text(&entry.id, 10).unwrap().contains("don't"), "text reached the dialog");
    // Idle: accepted. An explicit force works in any state.
    t.update(&entry, |r| {
        r.info.agent.as_mut().unwrap().state = super::AgentState::Idle;
        true
    });
    assert_eq!(send(json!({ "text": "hello-idle", "submit": true })).await.unwrap().status(), 200);
    t.update(&entry, |r| {
        r.info.agent.as_mut().unwrap().state = super::AgentState::NeedsInput;
        true
    });
    assert_eq!(send(json!({ "text": "forced-text", "submit": false, "force": true })).await.unwrap().status(), 200);
    wait_for("the accepted input", || t.screen_text(&entry.id, 10).is_some_and(|x| x.contains("hello-idle") && x.contains("forced-text"))).await;
    t.kill(&entry.id).await.unwrap();
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws_connect(addr: SocketAddr, state: &AppState, id: &str) -> Ws {
    let mut req = format!("ws://{addr}/api/terminals/{id}/ws").into_client_request().unwrap();
    req.headers_mut().insert("Authorization", format!("Bearer {}", state.auth.master_token()).parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    assert_eq!(next_text(&mut ws).await["t"], "snapshot");
    ws
}

/// Wait for a `{"t":"size"}` control frame announcing `want` (earlier sizes and binary
/// output are skipped); returns it.
async fn next_size(ws: &mut Ws, want: (u64, u64)) -> (u64, u64) {
    loop {
        let v = next_text(ws).await;
        if v["t"] == "size" && (v["cols"].as_u64().unwrap(), v["rows"].as_u64().unwrap()) == want {
            return want;
        }
    }
}

async fn resize(ws: &mut Ws, cols: u16, rows: u16) {
    ws.send(Message::Text(json!({ "t": "resize", "cols": cols, "rows": rows }).to_string().into())).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_that_leaves_gives_the_desktop_its_size_back() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let info = t.spawn(&state, command(dir.path(), SIZE_PER_LINE, json!({}))).await.unwrap();
    let addr = serve(&state).await;
    let size = || {
        let i = t.info(&info.id).unwrap();
        (i.cols, i.rows)
    };

    let mut desk = ws_connect(addr, &state, &info.id).await;
    resize(&mut desk, 120, 32).await;
    next_size(&mut desk, (120, 32)).await;
    // The phone attaches and takes the size; the desktop is told.
    let mut phone = ws_connect(addr, &state, &info.id).await;
    resize(&mut phone, 50, 20).await;
    next_size(&mut desk, (50, 20)).await;
    assert_eq!(size(), (50, 20));
    // Typing on the desktop takes it back; the phone is told.
    desk.send(Message::Binary(b"x\r".to_vec().into())).await.unwrap();
    next_size(&mut phone, (120, 32)).await;
    phone.send(Message::Binary(b"y\r".to_vec().into())).await.unwrap();
    next_size(&mut desk, (50, 20)).await;
    // Terminal-generated replies (focus, device reports) do not count as typing.
    desk.send(Message::Binary(b"\x1b[O".to_vec().into())).await.unwrap();
    desk.send(Message::Binary(b"\x1b[?1;2c".to_vec().into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(size(), (50, 20));
    // The phone leaves: the desktop's size comes back, and the program sees it.
    drop(phone);
    next_size(&mut desk, (120, 32)).await;
    assert_eq!(size(), (120, 32));
    desk.send(Message::Binary(b"z\r".to_vec().into())).await.unwrap();
    let mut got = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !got.contains("32 120") && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Binary(b)))) = tokio::time::timeout(Duration::from_millis(300), desk.next()).await {
            got.push_str(&String::from_utf8_lossy(&b));
        }
    }
    assert!(got.contains("32 120"), "stty saw {got:?}");
    t.kill(&info.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_terminal_starts_at_the_size_reported_while_it_was_exited() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let info = t.spawn(&state, command(dir.path(), "import os\ns = os.get_terminal_size()\nprint(f'{s.lines} {s.columns}')", json!({ "restartable": true }))).await.unwrap();
    wait_exit(&state, &info.id).await;
    let addr = serve(&state).await;
    let mut ws = ws_connect(addr, &state, &info.id).await;
    resize(&mut ws, 150, 40).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The final screen keeps its size while exited.
    assert_eq!(t.info(&info.id).unwrap().cols, 80);
    t.restart(&state, &info.id).await.unwrap();
    wait_for("the restarted process", || t.screen_text(&info.id, 20).is_some_and(|x| x.contains("40 150"))).await;
    next_size(&mut ws, (150, 40)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_jobs_left_behind_are_reported_and_killed() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let pidfile = dir.path().join("job.pid");
    let script = leave_job(&pidfile, 4242, "started");
    let info = t.spawn(&state, command(dir.path(), &script, json!({ "restartable": true }))).await.unwrap();
    wait_exit(&state, &info.id).await;
    wait_for("the pid file", || pidfile.is_file()).await;
    let job: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    assert!(crate::util::os::proc::pid_alive(job));
    wait_for("the lingering count", || t.info(&info.id).is_some_and(|i| i.lingering == 1)).await;
    assert_eq!(t.info(&info.id).unwrap().status, TerminalStatus::Exited);
    // Kill reaches the job although the terminal's own process is gone.
    t.kill(&info.id).await.unwrap();
    wait_for("the job to die", || !crate::util::os::proc::pid_alive(job)).await;
    wait_for("the count to clear", || t.info(&info.id).is_some_and(|i| i.lingering == 0)).await;

    // Close (and forget) reach them too.
    let pidfile2 = dir.path().join("job2.pid");
    let script = leave_job(&pidfile2, 4243, "");
    let info = t.spawn(&state, command(dir.path(), &script, json!({}))).await.unwrap();
    wait_exit(&state, &info.id).await;
    wait_for("the second pid file", || pidfile2.is_file()).await;
    let job2: u32 = std::fs::read_to_string(&pidfile2).unwrap().trim().parse().unwrap();
    wait_for("the second lingering count", || t.info(&info.id).is_some_and(|i| i.lingering == 1)).await;
    t.close(&state, &info.id, true).await.unwrap();
    wait_for("the second job to die", || !crate::util::os::proc::pid_alive(job2)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exited_terminals_hibernate_and_repeated_runs_are_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let info = t.spawn(&state, command(dir.path(), "print('\\n'.join(str(i) for i in range(1, 3001)))", json!({ "run": "gen" }))).await.unwrap();
    wait_exit(&state, &info.id).await;
    let entry = t.get(&info.id).unwrap();
    wait_for("hibernation", || entry.screen.is_hibernated()).await;
    // Readers still get everything.
    let text = t.screen_text(&info.id, 5).unwrap();
    assert!(text.ends_with("2999\n3000"), "{text:?}");
    wait_for("hibernation after the read", || entry.screen.is_hibernated()).await;
    let addr = serve(&state).await;
    let mut ws = ws_connect(addr, &state, &info.id).await;
    let Message::Binary(snap) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap() else {
        panic!("expected a snapshot")
    };
    assert!(String::from_utf8_lossy(&snap).contains("2999"));
    assert!(!entry.screen.is_hibernated(), "an attached client keeps the mirror");
    drop(ws);
    wait_for("hibernation after the client left", || entry.screen.is_hibernated()).await;

    // Five more starts of the same run: only the newest few exited ones stay.
    let mut ids = vec![info.id.clone()];
    for _ in 0..5 {
        let i = t.spawn(&state, command(dir.path(), EXIT_0, json!({ "run": "gen" }))).await.unwrap();
        wait_exit(&state, &i.id).await;
        ids.push(i.id);
    }
    // One more start prunes (pruning runs on spawn).
    let last = t.spawn(&state, command(dir.path(), SLEEP_5, json!({ "run": "gen" }))).await.unwrap();
    let kept: Vec<&String> = ids.iter().filter(|id| t.info(id).is_some()).collect();
    assert_eq!(kept, ids[ids.len() - super::KEEP_EXITED_PER_GROUP..].iter().collect::<Vec<_>>());
    assert!(!state.paths.data_dir.join("terminals").join(&ids[0]).exists(), "a pruned terminal's files stay");
    // Other groups are untouched.
    let other = t.spawn(&state, command(dir.path(), EXIT_0, json!({ "run": "other" }))).await.unwrap();
    wait_exit(&state, &other.id).await;
    assert!(t.info(&other.id).is_some());
    t.kill(&last.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_remote_control_server_shows_its_new_link_only() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    let script = "import os, time\nprint(f'Remote Control ready: https://claude.ai/code?environment=env_{os.getpid()}', flush=True)\ntime.sleep(30)";
    let info = t.spawn(&state, command(dir.path(), script, json!({ "remoteControlServer": true, "urls": [] }))).await.unwrap();
    let entry = t.get(&info.id).unwrap();
    super::agent::watch_remote_control(&state, &entry);
    let urls = || -> Vec<String> {
        t.info(&info.id).unwrap().meta["urls"].as_array().unwrap().iter().map(|u| u.as_str().unwrap().to_string()).collect()
    };
    wait_for("the first link", || urls().len() == 1).await;
    let first = urls();
    t.restart(&state, &info.id).await.unwrap();
    wait_for("the new link", || urls().len() == 1 && urls() != first).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(urls().len(), 1, "the dead link came back: {:?}", urls());
    t.kill(&info.id).await.unwrap();
}

// ---------------------------------------------------------------- agent providers

/// The fake agent CLI `name` (`testdata/fake_cli.py`, which acts as the CLI it is named
/// after) in `dir`: what a provider's `command` names (`util::os::exe::test_cli`: on Windows
/// an npm-style shim, so it starts the way npm's CLIs do).
pub(super) fn fake_cli(dir: &Path, name: &str) -> PathBuf {
    crate::util::os::exe::test_cli(dir, name, include_str!("testdata/fake_cli.py"))
}

/// argv running the fake CLI `name` of `dir` directly (a session outside Workbench).
pub(super) fn fake_cli_argv(dir: &Path, name: &str) -> Vec<String> {
    let mut argv = crate::util::os::exe::python();
    argv.push(crate::util::os::exe::test_cli_script(dir, name).display().to_string());
    argv
}

/// `argv` as a process with its standard input open (and never written) and its output
/// dropped.
fn outside(argv: &[String], cwd: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(&argv[0]);
    c.args(&argv[1..])
        .current_dir(cwd)
        .env("PWD", cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c
}

/// A state with one project (`proj`) and the given providers.
pub(super) async fn provider_state(dir: &std::path::Path, providers: Vec<(&str, crate::config::global::ProviderConfig)>) -> (AppState, String) {
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let state = test_state_with(dir, |cfg| {
        cfg.projects.include = vec![proj.display().to_string()];
        for (id, p) in providers {
            cfg.agents.providers.insert(id.to_string(), p);
        }
    })
    .await;
    let pid = state.projects.list().first().expect("the project").id.clone();
    (state, pid)
}

pub(super) fn agent_of(state: &AppState, id: &str) -> super::AgentInfo {
    state.terminals.info(id).and_then(|i| i.agent).expect("an agent terminal")
}

/// Wait up to `secs` for `ok`.
pub(super) async fn wait_long(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    // Generous headroom: these drive real PTYs and fake CLIs while the rest of the
    // suite (~500 tests) competes for the CPU; a 10 s wait flaked under that load.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs * 3);
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_sessions_are_discovered_tracked_and_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "codex");
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("argv.log");
    let codex = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [
            ("CODEX_HOME".to_string(), home.display().to_string()),
            ("FAKE_CODEX_TURN_SECS".to_string(), "0.3".to_string()),
            ("FAKE_CODEX_LOG".to_string(), log.display().to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("codex", codex)]).await;
    let t = &state.terminals;
    let req = |prompt: Option<&str>| super::agent::AgentRequest {
        project_id: pid.clone(),
        provider: Some("codex".into()),
        prompt: prompt.map(str::to_string),
        ..Default::default()
    };

    // Two sessions in one folder: one starts with a prompt, the other waits.
    let a = t.spawn_agent(&state, req(Some("first task"))).await.unwrap();
    let b = t.spawn_agent(&state, req(None)).await.unwrap();
    let aa = a.agent.clone().unwrap();
    assert_eq!((aa.provider, aa.provider_id.as_deref(), aa.session_id.as_str()), (super::ProviderKind::Codex, Some("codex"), ""));
    wait_long("a's answer", 20, || {
        let x = agent_of(&state, &a.id);
        x.state == super::AgentState::Idle && x.unread && x.last_message.as_deref() == Some("Done: first task")
    })
    .await;
    let a_id = agent_of(&state, &a.id).session_id;
    assert!(super::transcript::is_uuid(&a_id), "{a_id:?}");
    assert_eq!(t.info(&a.id).unwrap().title, "first task");
    assert_eq!(agent_of(&state, &a.id).model.as_deref(), Some("fake-model"));
    // The command line: flags the installed version supports, Workbench's MCP server,
    // the Workspace folders, the prompt after `--`. The token is not in it.
    let argv = std::fs::read_to_string(&log).unwrap();
    // a and b start together, so their lines may come in either order: a's has its id.
    let first = argv
        .lines()
        .find(|l| l.contains(&format!("X-Workbench-Terminal\"=\"{}\"", a.id)))
        .unwrap_or_else(|| panic!("{argv}"));
    assert!(first.starts_with("--no-daemon --no-alt-screen -c mcp_servers.workbench.url=\"http://127.0.0.1:"), "{first}");
    assert!(first.contains(&format!("--add-dir {}", state.paths.data_dir.join("workspace").join("home").display())), "{first}");
    assert!(first.ends_with("-- first task"), "{first}");
    assert!(!first.contains("wba_"), "a token reached argv: {first}");

    // b has no rollout before its first turn; it becomes idle and waits.
    wait_long("b to settle", 20, || agent_of(&state, &b.id).state == super::AgentState::Idle).await;
    assert_eq!(agent_of(&state, &b.id).session_id, "");
    t.send_text(&b.id, "second task", true).await.unwrap();
    wait_long("b's answer", 20, || agent_of(&state, &b.id).last_message.as_deref() == Some("Done: second task")).await;
    let b_id = agent_of(&state, &b.id).session_id;
    assert!(super::transcript::is_uuid(&b_id) && b_id != a_id, "{b_id:?} vs {a_id:?}");
    // a is untouched by b's turn.
    assert_eq!(agent_of(&state, &a.id).last_message.as_deref(), Some("Done: first task"));

    // History lists both, with their terminals.
    let h = t.history(&state, &pid, Some("codex"), 10).await.unwrap();
    let mut ids: Vec<(&str, Option<&str>)> = h.iter().map(|e| (e.id.as_str(), e.terminal_id.as_deref())).collect();
    ids.sort();
    let mut want = vec![(a_id.as_str(), Some(a.id.as_str())), (b_id.as_str(), Some(b.id.as_str()))];
    want.sort();
    assert_eq!(ids, want);
    assert!(h.iter().all(|e| e.provider == "codex"));

    // Stop a and resume it: `codex resume <id>`, and new turns are followed.
    t.kill(&a.id).await.unwrap();
    t.restart(&state, &a.id).await.unwrap();
    wait_long("the resumed banner", 10, || t.screen_text(&a.id, 30).is_some_and(|s| s.contains(&format!("Resumed session {a_id}")))).await;
    assert_eq!(agent_of(&state, &a.id).session_id, a_id);
    assert!(std::fs::read_to_string(&log).unwrap().lines().last().unwrap().starts_with("resume --no-daemon"));
    wait_long("the resumed session to settle", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle).await;
    t.send_text(&a.id, "third task", true).await.unwrap();
    wait_long("the resumed answer", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some("Done: third task")).await;

    // Asking the project's agents reaches the most recent session of any provider.
    let asked = t
        .ask(
            &state,
            super::agent::AskRequest {
                project_id: Some(pid.clone()),
                prompt: "fourth task".into(),
                terminal_id: None,
                provider: Some("codex".into()),
                new_session: false,
                name: None,
                submit: Some(true),
            },
        )
        .await
        .unwrap();
    wait_long("the asked answer", 20, || agent_of(&state, &asked.id).last_message.as_deref() == Some("Done: fourth task")).await;
    t.kill(&a.id).await.unwrap();
    t.kill(&b.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kimi_and_custom_sessions_follow_output_activity() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "kimi");
    let home = dir.path().join("kimi-home");
    std::fs::create_dir_all(&home).unwrap();
    let kimi = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [("KIMI_CODE_HOME".to_string(), home.display().to_string())].into(),
        ..Default::default()
    };
    let mut scripted = python_argv(
        "import sys, time\nprint('ready', flush=True)\nfor l in sys.stdin:\n    for i in range(1, 26):\n        print(f'step {i} of {l.strip()}', flush=True)\n        time.sleep(0.1)",
    );
    let custom = crate::config::global::ProviderConfig {
        command: Some(scripted.remove(0)),
        args: scripted,
        label: Some("Scripted".into()),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("kimi", kimi), ("scripted", custom)]).await;
    let t = &state.terminals;

    let k = t
        .spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("kimi".into()), ..Default::default() })
        .await
        .unwrap();
    // Its id comes from Kimi's session index; startup ends at the first quiet spell.
    wait_long("the kimi id", 20, || agent_of(&state, &k.id).session_id.starts_with("session_")).await;
    wait_long("kimi idle", 20, || agent_of(&state, &k.id).state == super::AgentState::Idle).await;
    t.send_text(&k.id, "a question", true).await.unwrap();
    wait_long("kimi working", 10, || agent_of(&state, &k.id).state == super::AgentState::Working).await;
    wait_long("kimi idle again", 20, || agent_of(&state, &k.id).state == super::AgentState::Idle).await;
    let a = agent_of(&state, &k.id);
    assert!(!a.unread && a.attention.is_none(), "the heuristic promises no unread answer");
    // History comes from the index and state.json; resuming passes `--session <id>`.
    let h = t.history(&state, &pid, Some("kimi"), 10).await.unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!((h[0].id.as_str(), h[0].title.as_str()), (a.session_id.as_str(), "Fake Kimi session"));
    t.kill(&k.id).await.unwrap();
    t.restart(&state, &k.id).await.unwrap();
    wait_long("the resume", 10, || t.screen_text(&k.id, 20).is_some_and(|s| s.contains(&format!("Resumed {}", a.session_id)))).await;
    assert_eq!(super::kimi::load_index(&home).len(), 1, "a resume adds no session");
    t.kill(&k.id).await.unwrap();

    // A custom CLI with an initial prompt: pasted once it is up, then working, then idle.
    let c = t
        .spawn_agent(
            &state,
            super::agent::AgentRequest { project_id: pid.clone(), provider: Some("scripted".into()), prompt: Some("go".into()), ..Default::default() },
        )
        .await
        .unwrap();
    assert_eq!(c.title, "go");
    let ca = c.agent.clone().unwrap();
    assert_eq!((ca.provider, ca.session_id.as_str()), (super::ProviderKind::Custom, ""));
    wait_long("the pasted prompt to run", 30, || t.screen_text(&c.id, 40).is_some_and(|s| s.contains("step 3 of go"))).await;
    wait_long("custom working", 10, || agent_of(&state, &c.id).state == super::AgentState::Working).await;
    wait_long("custom idle", 20, || agent_of(&state, &c.id).state == super::AgentState::Idle).await;
    // Custom CLIs cannot resume or fork; unknown or disabled providers are setup errors.
    let err = t
        .spawn_agent(
            &state,
            super::agent::AgentRequest { project_id: pid.clone(), provider: Some("scripted".into()), resume: Some("x".into()), ..Default::default() },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "bad_request");
    let err = t
        .spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("nope".into()), ..Default::default() })
        .await
        .unwrap_err();
    assert_eq!(err.code, "not_configured");
    t.kill(&c.id).await.unwrap();
}

fn ask_req(pid: &str, prompt: &str, terminal: Option<&str>) -> super::agent::AskRequest {
    super::agent::AskRequest {
        project_id: Some(pid.to_string()),
        prompt: prompt.into(),
        terminal_id: terminal.map(str::to_string),
        provider: Some("codex".into()),
        new_session: false,
        name: None,
        submit: Some(true),
    }
}

pub(super) fn log_lines(path: &std::path::Path, prefix: &str) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter(|l| l.starts_with(prefix)).map(str::to_string).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_never_types_into_a_codex_approval_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "codex");
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("codex.log");
    let codex = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [
            ("CODEX_HOME".to_string(), home.display().to_string()),
            ("FAKE_CODEX_TURN_SECS".to_string(), "0.3".to_string()),
            ("FAKE_CODEX_LOG".to_string(), log.display().to_string()),
            ("FAKE_CODEX_APPROVAL".to_string(), "1".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("codex", codex)]).await;
    let t = &state.terminals;
    let spawn = |mode: Option<&str>| super::agent::AgentRequest {
        project_id: pid.clone(),
        provider: Some("codex".into()),
        permission_mode: mode.map(str::to_string),
        ..Default::default()
    };
    let a = t.spawn_agent(&state, spawn(None)).await.unwrap();
    wait_long("a to settle", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle).await;

    // A Codex session runs its commands in its sandbox: Workbench starts no run for it.
    let run_start = crate::apps::mcp_tools().into_iter().find(|x| x.name == "run_start").unwrap();
    let ctx = crate::mcp::McpCtx { terminal_id: Some(a.id.clone()), project_id: Some(pid.clone()) };
    let err = (run_start.handler)(state.clone(), ctx, json!({ "name": "test" })).await.unwrap_err();
    assert_eq!(err.code, "forbidden");
    assert!(err.message.contains("sandbox"), "{}", err.message);
    let open = t.spawn_agent(&state, spawn(Some("bypass"))).await.unwrap();
    assert!(t.sandboxed_agent(&state, &a.id).is_some() && t.sandboxed_agent(&state, &open.id).is_none());
    t.kill(&open.id).await.unwrap();

    // Idle right after startup, before any turn (a trust or sign-in screen could be up):
    // `ask` starts a new session instead.
    let b = t.ask(&state, ask_req(&pid, "first ask", None)).await.unwrap();
    assert_ne!(b.id, a.id);
    wait_long("b's answer", 20, || agent_of(&state, &b.id).last_message.as_deref() == Some("Done: first ask")).await;
    t.kill(&b.id).await.unwrap();

    // a's first turn asks for approval; nothing in the rollout says so, the screen does.
    t.send_text(&a.id, "deploy it", true).await.unwrap();
    wait_long("the approval prompt", 20, || agent_of(&state, &a.id).state == super::AgentState::NeedsPermission).await;
    assert!(agent_of(&state, &a.id).attention.is_some_and(|m| m.contains("approval")));
    // `ask` without a terminal does not pick it: a new session takes the prompt.
    let c = t.ask(&state, ask_req(&pid, "Look at the failing CI job and fix it", None)).await.unwrap();
    assert_ne!(c.id, a.id);
    // Naming it is refused, and so is the phone's compose box.
    let err = t.ask(&state, ask_req(&pid, "Look at the failing CI job and fix it", Some(&a.id))).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    let addr = serve(&state).await;
    let r = reqwest::Client::new()
        .post(format!("http://{addr}/api/terminals/{}/input", a.id))
        .bearer_auth(state.auth.master_token())
        .json(&json!({ "text": "Look at the failing CI job and fix it", "submit": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(log_lines(&log, "APPROVAL").is_empty(), "the dialog was answered: {:?}", log_lines(&log, "APPROVAL"));

    // The user answers it: the turn goes on and ends; now `ask` may pick a.
    t.write(&a.id, b"y\r").unwrap();
    wait_long("a's answer", 20, || {
        let x = agent_of(&state, &a.id);
        x.state == super::AgentState::Idle && x.last_message.as_deref() == Some("Done: deploy it") && x.attention.is_none()
    })
    .await;
    assert_eq!(log_lines(&log, "APPROVAL"), ["APPROVAL y"]);
    t.kill(&c.id).await.unwrap();
    // Once the dialog is known to be gone (a second look), `ask` takes a again.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let again = t.ask(&state, ask_req(&pid, "second ask", None)).await.unwrap();
    assert_eq!(again.id, a.id);
    wait_long("the asked answer", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some("Done: second ask")).await;
    t.kill(&a.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_rollouts_of_sessions_outside_workbench_are_never_adopted() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "codex");
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let codex = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [("CODEX_HOME".to_string(), home.display().to_string()), ("FAKE_CODEX_TURN_SECS".to_string(), "0.3".to_string())].into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("codex", codex)]).await;
    let t = &state.terminals;
    let root = state.projects.get(&pid).unwrap().root.clone();
    let a = t
        .spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("codex".into()), ..Default::default() })
        .await
        .unwrap();
    wait_long("a to settle", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle).await;

    // The user's own Codex in the same folder, outside Workbench, keeps its rollout open.
    let mut argv = fake_cli_argv(dir.path(), "codex");
    argv.extend(["--".into(), "my own external task".into()]);
    let mut outside = outside(&argv, &root).env("CODEX_HOME", &home).env("FAKE_CODEX_TURN_SECS", "0.2").spawn().unwrap();
    let rollouts = || -> Vec<std::path::PathBuf> {
        let day = home.join("sessions").join(chrono::Local::now().format("%Y/%m/%d").to_string());
        std::fs::read_dir(day).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    };
    wait_long("the outside rollout", 10, || rollouts().iter().any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("Done: my own external task")))).await;
    let foreign = rollouts()[0].file_name().unwrap().to_string_lossy().into_owned();
    // a keeps waiting for its own: no id, title, answer or unread of the other one.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let x = agent_of(&state, &a.id);
    assert_eq!((x.session_id.as_str(), x.last_message.as_deref(), x.unread), ("", None, false), "{x:?}");
    assert_eq!(t.info(&a.id).unwrap().title, "Codex");

    // Its own first turn is found by the file its process holds.
    t.send_text(&a.id, "hosted task", true).await.unwrap();
    wait_long("a's answer", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some("Done: hosted task")).await;
    let id = agent_of(&state, &a.id).session_id;
    assert!(super::transcript::is_uuid(&id) && !foreign.contains(&id), "{id} vs {foreign}");
    let _ = outside.kill();
    let _ = outside.wait();
    t.kill(&a.id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kimi_sessions_in_one_folder_find_their_own_ids() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "kimi");
    let home = dir.path().join("kimi-home");
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("kimi.log");
    let env = |extra: &[(&str, &str)]| {
        let mut m: std::collections::BTreeMap<String, String> =
            [("KIMI_CODE_HOME".to_string(), home.display().to_string()), ("FAKE_KIMI_LOG".to_string(), log.display().to_string())].into();
        m.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        m
    };
    let kimi = crate::config::global::ProviderConfig { command: Some(fake.display().to_string()), env: env(&[]), ..Default::default() };
    let lazy = crate::config::global::ProviderConfig {
        kind: Some("kimi".into()),
        command: Some(fake.display().to_string()),
        env: env(&[("FAKE_KIMI_LAZY", "1"), ("FAKE_KIMI_SHOW_ID", "1"), ("FAKE_KIMI_TURN_SECS", "1")]),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("kimi", kimi), ("kimi-lazy", lazy)]).await;
    let t = &state.terminals;
    let root = state.projects.get(&pid).unwrap().root.clone();
    let start = |provider: &str| super::agent::AgentRequest { project_id: pid.clone(), provider: Some(provider.into()), ..Default::default() };
    let index = || super::kimi::load_index(&home).into_iter().map(|e| e.session_id).collect::<Vec<_>>();

    // Two sessions started one after the other, both still running.
    let a = t.spawn_agent(&state, start("kimi")).await.unwrap();
    wait_long("a's index entry", 10, || index().len() == 1).await;
    let b = t.spawn_agent(&state, start("kimi")).await.unwrap();
    wait_long("b's index entry", 10, || index().len() == 2).await;
    wait_long("both ids", 20, || !agent_of(&state, &a.id).session_id.is_empty() && !agent_of(&state, &b.id).session_id.is_empty()).await;
    let ids = index();
    assert_eq!((agent_of(&state, &a.id).session_id, agent_of(&state, &b.id).session_id), (ids[0].clone(), ids[1].clone()));
    // A restart resumes each conversation.
    for x in [&a, &b] {
        t.kill(&x.id).await.unwrap();
        t.restart(&state, &x.id).await.unwrap();
    }
    for (x, id) in [(&a, &ids[0]), (&b, &ids[1])] {
        wait_long("the resume", 10, || t.screen_text(&x.id, 20).is_some_and(|s| s.contains(&format!("Resumed {id}")))).await;
        assert!(log_lines(&log, "--session").iter().any(|l| l.starts_with(&format!("--session {id}"))), "{:?}", log_lines(&log, ""));
    }
    assert_eq!(index().len(), 2, "a resume adds no session");
    t.kill(&a.id).await.unwrap();
    t.kill(&b.id).await.unwrap();

    // A Kimi outside Workbench in the same folder: its new entry is not taken on
    // elimination alone; the session's own screen settles its id later.
    let c = t.spawn_agent(&state, start("kimi-lazy")).await.unwrap();
    wait_long("c to settle", 20, || agent_of(&state, &c.id).state == super::AgentState::Idle).await;
    let mut outside = outside(&fake_cli_argv(dir.path(), "kimi"), &root).env("KIMI_CODE_HOME", &home).spawn().unwrap();
    wait_long("the outside entry", 10, || index().len() == 3).await;
    let theirs = index()[2].clone();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(agent_of(&state, &c.id).session_id, "");
    t.send_text(&c.id, "hello", true).await.unwrap();
    wait_long("c's own id", 20, || !agent_of(&state, &c.id).session_id.is_empty()).await;
    let own = agent_of(&state, &c.id).session_id;
    assert!(own != theirs && index().contains(&own), "{own} vs {theirs}");
    let _ = outside.kill();
    let _ = outside.wait();
    t.kill(&c.id).await.unwrap();
}

// ---------------------------------------------------------------- the Avatar option

/// The avatar brief a session on the host is given, in the avatar slice's own words.
pub(super) fn avatar_brief(variant: crate::avatar::BriefVariant, via: crate::avatar::Via, whole: bool) -> String {
    crate::avatar::session_brief(&crate::avatar::Brief { variant, via, in_container: false, shell: crate::util::os::shell::Dialect::HOST, whole })
}

/// Whether the avatar of terminal `id` remembers that its session was not told of it, as its file
/// says (`"unbriefed": true`, only when it is so).
pub(super) fn unbriefed(state: &AppState, id: &str) -> bool {
    let file = state.paths.data_dir.join("avatar").join(format!("{id}.json"));
    let stored: Value = serde_json::from_slice(&std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))).unwrap();
    stored.get("unbriefed") == Some(&Value::Bool(true))
}

/// A Workbench restart (`restore_on_start`) of a session whose avatar is on and whose CLI starts a
/// new conversation (a custom CLI): the brief is pasted as soon as the session is up. The restore
/// waits for the same signal of a session being up, and does not take it from the brief.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restored_session_is_told_of_its_avatar_as_soon_as_it_is_up() {
    use crate::avatar::{BriefVariant, Via};
    let dir = tempfile::tempdir().unwrap();
    let heard = dir.path().join("heard.log");
    let mut scripted = python_argv(&format!(
        "import json, sys\nprint('ready', flush=True)\nfor l in sys.stdin:\n    with open({}, 'a', encoding='utf-8') as f:\n        f.write(json.dumps(l.rstrip('\\r\\n')) + '\\n')\n    print('noted', flush=True)",
        py_str(&heard.display().to_string())
    ));
    let custom = crate::config::global::ProviderConfig { command: Some(scripted.remove(0)), args: scripted, label: Some("Scripted".into()), ..Default::default() };
    let (state, pid) = provider_state(dir.path(), vec![("scripted", custom)]).await;
    let _router = app::build_router(state.clone());
    crate::avatar::start(&state).await;
    let t = &state.terminals;
    let lines = || -> Vec<String> { std::fs::read_to_string(&heard).unwrap_or_default().lines().map(|l| serde_json::from_str::<String>(l).unwrap()).collect() };
    let c = t
        .spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("scripted".into()), avatar: Some(true), ..Default::default() })
        .await
        .unwrap();
    let whole = avatar_brief(BriefVariant::First, Via::Command, true);
    wait_long("the brief at its start", 30, || lines().len() == 1).await;
    assert_eq!(lines()[0], whole);
    wait_long("the session idle", 20, || agent_of(&state, &c.id).state == super::AgentState::Idle).await;

    // As a start of Workbench finds it: it was running when Workbench stopped.
    t.kill(&c.id).await.unwrap();
    t.update(&t.get(&c.id).unwrap(), |r| {
        r.was_running = true;
        true
    });
    state.config.write().agents.restore_on_start = true;
    super::agent::restore(state.clone()).await;
    // The brief waits for the session to be up (at most a minute), not for its own time-out.
    wait_long("the brief of the restored conversation", 10, || lines().len() == 2).await;
    assert_eq!(lines()[1], whole);
    t.kill(&c.id).await.unwrap();
}

/// A CLI without Workbench's tools has its first prompt pasted: the brief follows the user's
/// prompt on the same line, or is the whole prompt in its wrapped form, and never names the
/// tab. A restart of such a CLI starts a new conversation, which is told of the avatar for as
/// long as it is on, whatever the session was started with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pasted_first_prompt_carries_the_brief_and_a_new_conversation_is_told_again() {
    use crate::avatar::{BriefVariant, Via};
    let dir = tempfile::tempdir().unwrap();
    let heard = dir.path().join("heard.log");
    // It notes every line it reads, as it read it.
    let mut scripted = python_argv(&format!(
        "import json, sys\nprint('ready', flush=True)\nfor l in sys.stdin:\n    with open({}, 'a', encoding='utf-8') as f:\n        f.write(json.dumps(l.rstrip('\\r\\n')) + '\\n')\n    print('noted', flush=True)",
        py_str(&heard.display().to_string())
    ));
    let custom = crate::config::global::ProviderConfig { command: Some(scripted.remove(0)), args: scripted, label: Some("Scripted".into()), ..Default::default() };
    let (state, pid) = provider_state(dir.path(), vec![("scripted", custom)]).await;
    // The avatar slice's own background work: it makes a session's record when its terminal is
    // created, and tells a session that was not told of its avatar (through the ask route, in-process).
    let _router = app::build_router(state.clone());
    crate::avatar::start(&state).await;
    let t = &state.terminals;
    let lines = || -> Vec<String> { std::fs::read_to_string(&heard).unwrap_or_default().lines().map(|l| serde_json::from_str::<String>(l).unwrap()).collect() };
    let start = |prompt: Option<&str>, avatar: Option<bool>| super::agent::AgentRequest {
        project_id: pid.clone(),
        provider: Some("scripted".into()),
        prompt: prompt.map(str::to_string),
        avatar,
        ..Default::default()
    };
    let restart = |id: String| {
        let state = state.clone();
        async move {
            state.terminals.kill(&id).await.unwrap();
            state.terminals.restart(&state, &id).await.unwrap();
        }
    };

    // After the user's prompt: one space, one line, and the tab is named after the user's words alone.
    let c = t.spawn_agent(&state, start(Some("go"), Some(true))).await.unwrap();
    assert_eq!((c.title.as_str(), &c.meta["avatar"]), ("go", &json!(true)));
    wait_long("the pasted prompt", 30, || lines().len() == 1).await;
    let first = avatar_brief(BriefVariant::First, Via::Command, false);
    assert_eq!(lines()[0], format!("go {first}"));
    assert!(first.contains(" avatar guide`") && !first.contains("avatar_status"), "a CLI without the tools is given its command: {first}");
    assert_eq!(t.info(&c.id).unwrap().title, "go");

    // Without a prompt of the user's the brief is the whole prompt, wrapped; the tab is the CLI's.
    let w = t.spawn_agent(&state, start(None, Some(true))).await.unwrap();
    assert_eq!(w.title, "Scripted");
    wait_long("the brief alone", 30, || lines().len() == 2).await;
    let whole = avatar_brief(BriefVariant::First, Via::Command, true);
    assert_eq!(lines()[1], whole);
    assert!(whole.starts_with("<workbench-avatar>[Workbench avatar] ") && whole.ends_with("</workbench-avatar>"), "{whole}");
    assert_eq!(t.info(&w.id).unwrap().title, "Scripted");

    // Not asked for: the user's prompt and nothing else.
    let plain = t.spawn_agent(&state, start(Some("just this"), None)).await.unwrap();
    wait_long("the plain prompt", 30, || lines().len() == 3).await;
    assert_eq!(lines()[2], "just this");
    assert!(plain.meta.get("avatar").is_none());
    wait_long("the avatars", 10, || crate::avatar::active(&state, &c.id) && crate::avatar::active(&state, &w.id)).await;
    assert!(!crate::avatar::active(&state, &plain.id));

    // A restart of this CLI is a new conversation. Its avatar is on and has no look of its own
    // yet (this CLI drew none): it is told of the avatar as at its start, and draws one. The
    // option and the tab's name stay.
    restart(c.id.clone()).await;
    wait_long("the brief of a new conversation", 30, || lines().len() == 4).await;
    assert_eq!(lines()[3], whole);
    let info = t.info(&c.id).unwrap();
    assert_eq!((info.title.as_str(), &info.meta["avatar"]), ("go", &json!(true)));
    assert!(t.get(&c.id).unwrap().rec.lock().launch.as_ref().is_some_and(|l| l.avatar && l.name.is_none()));
    // Once it has a look, a new conversation is told that it still has its avatar, and not to
    // draw another.
    wait_long("the restarted session", 20, || agent_of(&state, &c.id).state == super::AgentState::Idle).await;
    let pass = crate::avatar::session::gate(&state, &c.id, crate::avatar::session::Call::Design, Via::Command, crate::util::now_ms()).unwrap();
    crate::avatar::session::design(&state, &pass, &json!({ "name": "Pip", "svg": "<svg viewBox='0 0 200 200'><g id='body'><circle cx='100' cy='110' r='50' fill='#ffcc00'/></g></svg>" }), crate::util::now_ms()).unwrap();
    restart(c.id.clone()).await;
    wait_long("the brief of a conversation that has its look", 30, || lines().len() == 5).await;
    let again = avatar_brief(BriefVariant::Again, Via::Command, true);
    assert_eq!(lines()[4], again);
    assert!(again.contains("still has its live avatar") && again.contains("Do not design a new look"), "{again}");
    assert_eq!(t.info(&c.id).unwrap().title, "go");

    // The user turned the avatar off: a restart says nothing, though the session was started
    // with the option. A session that never had one hears nothing either.
    crate::avatar::device::turn_off(&state, &w.id, crate::util::now_ms()).unwrap();
    for id in [&w.id, &plain.id] {
        restart(id.clone()).await;
        wait_long("the restarted session", 20, || agent_of(&state, id).state == super::AgentState::Idle).await;
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(lines().len(), 5, "{:?}", lines());
    // The avatar slice keeps count of who heard of it: the new conversation of the session whose
    // avatar is off has not, and is told when the avatar is turned on again. Those that were told
    // at their restart are not told again.
    assert_eq!((unbriefed(&state, &w.id), unbriefed(&state, &c.id)), (true, false));

    // A device gave it an avatar meanwhile: its next new conversation is told, though the
    // session was started without the option. Told then, it is not told once more when it is
    // idle (the avatar slice tells a session it was given to while it runs).
    crate::avatar::device::turn_on(&state, &plain.id, crate::util::now_ms()).unwrap();
    assert!(unbriefed(&state, &plain.id), "the session was not told yet");
    restart(plain.id.clone()).await;
    wait_long("the brief after the avatar was given", 30, || lines().len() == 6).await;
    assert_eq!(lines()[5], whole);
    assert!(t.info(&plain.id).unwrap().meta.get("avatar").is_none(), "the option stays what the session was started with");
    wait_long("the restarted session", 20, || agent_of(&state, &plain.id).state == super::AgentState::Idle).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(lines().len(), 6, "{:?}", lines());
    assert!(!unbriefed(&state, &plain.id));
    for id in [&c.id, &w.id, &plain.id] {
        t.kill(id).await.unwrap();
    }
}

/// Codex takes its first prompt in the command line and names an untitled session after its
/// first message: the brief stands below the user's prompt there, and neither the tab nor the
/// History row shows a word of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_codex_session_with_an_avatar_is_named_after_the_users_own_prompt() {
    use crate::avatar::{BriefVariant, Via};
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "codex");
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("argv.log");
    let codex = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [
            ("CODEX_HOME".to_string(), home.display().to_string()),
            ("FAKE_CODEX_TURN_SECS".to_string(), "0.3".to_string()),
            ("FAKE_CODEX_LOG".to_string(), log.display().to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("codex", codex)]).await;
    let t = &state.terminals;
    let start = |prompt: Option<&str>| super::agent::AgentRequest { project_id: pid.clone(), provider: Some("codex".into()), prompt: prompt.map(str::to_string), avatar: Some(true), ..Default::default() };
    let brief = avatar_brief(BriefVariant::First, Via::Tools, false);
    let whole = avatar_brief(BriefVariant::First, Via::Tools, true);

    // With a prompt: in the command line, below a blank line. Codex on the host has Workbench's tools.
    let a = t.spawn_agent(&state, start(Some("first task"))).await.unwrap();
    assert_eq!((a.title.as_str(), &a.meta["avatar"]), ("first task", &json!(true)));
    // (Its answer repeats the whole message; what the session keeps of it is the end.)
    wait_long("a's answer", 20, || agent_of(&state, &a.id).last_message.is_some_and(|m| m.ends_with("is not from the user."))).await;
    let argv = std::fs::read_to_string(&log).unwrap();
    assert!(argv.contains(&format!("-- first task\n\n{brief}\n")), "{argv}");
    assert_eq!(argv.matches(crate::avatar::BRIEF_MARK).count(), 1, "{argv}");
    assert!(brief.contains("avatar_status") && !argv.contains("wba_"), "{argv}");
    // Its rollout's first message holds the brief; the tab keeps the user's words.
    let a_id = agent_of(&state, &a.id).session_id;
    assert!(super::transcript::is_uuid(&a_id), "{a_id:?}");
    assert_eq!(t.info(&a.id).unwrap().title, "first task");
    assert_eq!(agent_of(&state, &a.id).title.as_deref(), Some("first task"));

    // Without a prompt: the brief alone, wrapped. The tab stays the CLI's.
    let b = t.spawn_agent(&state, start(None)).await.unwrap();
    assert_eq!(b.title, "Codex");
    wait_long("b's answer", 20, || agent_of(&state, &b.id).last_message.is_some()).await;
    let argv = std::fs::read_to_string(&log).unwrap();
    assert!(argv.contains(&format!("-- {whole}\n")), "{argv}");
    let b_id = agent_of(&state, &b.id).session_id;
    assert!(super::transcript::is_uuid(&b_id) && b_id != a_id, "{b_id:?}");
    assert_eq!((t.info(&b.id).unwrap().title.as_str(), agent_of(&state, &b.id).title), ("Codex", None));

    // History names each by the user's own prompt, or by none.
    let h = t.history(&state, &pid, Some("codex"), 10).await.unwrap();
    let row = |id: &str| h.iter().find(|e| e.id == id).unwrap_or_else(|| panic!("no row for {id}: {h:?}"));
    assert_eq!((row(&a_id).title.as_str(), row(&a_id).first_prompt.as_deref()), ("first task", None));
    assert_eq!(row(&b_id).title, "(untitled session)");
    assert!(h.iter().all(|e| !format!("{} {}", e.title, e.first_prompt.as_deref().unwrap_or("")).contains("avatar")), "{h:?}");
    t.kill(&a.id).await.unwrap();
    t.kill(&b.id).await.unwrap();
}

/// `ask` with a named session pastes its text and presses Enter. A dialog that opens while the
/// text is on its way would take that Enter as its answer ("Yes"): the Enter is withheld, and
/// the caller is told where its text is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_enter_after_a_paste_is_withheld_when_a_dialog_opened_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "codex");
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("codex.log");
    let codex = crate::config::global::ProviderConfig {
        command: Some(fake.display().to_string()),
        env: [
            ("CODEX_HOME".to_string(), home.display().to_string()),
            ("FAKE_CODEX_TURN_SECS".to_string(), "0.3".to_string()),
            ("FAKE_CODEX_LOG".to_string(), log.display().to_string()),
            // The approval overlay comes up the moment text arrives at the prompt.
            ("FAKE_CODEX_PASTE_APPROVAL".to_string(), "1".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, pid) = provider_state(dir.path(), vec![("codex", codex)]).await;
    let t = &state.terminals;
    let a = t.spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("codex".into()), ..Default::default() }).await.unwrap();
    wait_long("a to settle", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle).await;
    let entry = t.get(&a.id).unwrap();
    assert!(t.prompt_refusal(&entry).is_none(), "nothing is in the way before the text is sent");

    // The text goes in, the dialog opens on it, and the Enter stays out.
    let text = "Look at the failing CI job and fix it";
    let err = t.ask(&state, ask_req(&pid, text, Some(&a.id))).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (409, "conflict"), "{err}");
    assert_eq!(err.message, "a dialog opened while your text was being pasted: it is in the session's input box, not sent");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(log_lines(&log, "APPROVAL").is_empty(), "the Enter answered the dialog: {:?}", log_lines(&log, "APPROVAL"));
    let screen = t.screen_text(&a.id, 30).unwrap();
    assert!(screen.contains("Would you like to run the following command?"), "{screen}");
    wait_long("the session to read as asking", 10, || agent_of(&state, &a.id).state == super::AgentState::NeedsPermission).await;

    // While the dialog is up nothing more is pasted: the refusal that was there before.
    let err = t.ask(&state, ask_req(&pid, "and one more thing", Some(&a.id))).await.unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("answer it first"), "{}", err.message);

    // The user's own Enter is the first to reach the dialog: it was unanswered until now, and
    // the text that was pasted is what stood before it.
    t.write(&a.id, b"\r").unwrap();
    wait_long("the user's answer", 10, || !log_lines(&log, "APPROVAL").is_empty()).await;
    assert_eq!(log_lines(&log, "APPROVAL"), [format!("APPROVAL {text}")]);

    // With no dialog in the way the same call pastes and submits.
    wait_long("the dialog to be gone", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle && t.prompt_refusal(&entry).is_none()).await;
    t.ask(&state, ask_req(&pid, "second task", Some(&a.id))).await.unwrap();
    wait_long("the submitted turn", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some("Done: second task")).await;

    // A text that itself reads like a dialog of this CLI (a choice a session offered and the
    // user clicked) is no dialog once it is echoed: its Enter is pressed. The screen alone
    // would have said otherwise.
    wait_long("the session to be idle", 20, || agent_of(&state, &a.id).state == super::AgentState::Idle && t.prompt_refusal(&entry).is_none()).await;
    let choice = "Answer to question Q1: choice 1, \"Yes, proceed\"";
    t.ask(&state, ask_req(&pid, choice, Some(&a.id))).await.unwrap();
    wait_long("the turn of the text that reads like a dialog", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some(&format!("Done: {choice}")[..])).await;
    assert_eq!(log_lines(&log, "APPROVAL").len(), 1);
    // Nor does the periodic look take its echo for one, while it stands in the input box and
    // later in the history above it: the session stays idle, nobody is called, and what is sent
    // next goes in.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let x = agent_of(&state, &a.id);
    assert_eq!((x.state, x.attention.as_deref()), (super::AgentState::Idle, None), "{x:?}");
    assert!(t.screen_text(&a.id, 30).unwrap().contains("Yes, proceed"), "the echo is on the screen");
    assert!(t.screen_dialog(&entry).is_none() && t.prompt_refusal(&entry).is_none(), "the echoed text is no dialog");
    t.ask(&state, ask_req(&pid, "next", Some(&a.id))).await.unwrap();
    wait_long("the turn after it", 20, || agent_of(&state, &a.id).last_message.as_deref() == Some("Done: next")).await;
    t.kill(&a.id).await.unwrap();

    // Such a text does not disarm the check: a dialog that opens on it says more than the text
    // does, and the Enter stays out all the same.
    let b = t.spawn_agent(&state, super::agent::AgentRequest { project_id: pid.clone(), provider: Some("codex".into()), ..Default::default() }).await.unwrap();
    wait_long("b to settle", 20, || agent_of(&state, &b.id).state == super::AgentState::Idle).await;
    let err = t.ask(&state, ask_req(&pid, choice, Some(&b.id))).await.unwrap_err();
    assert_eq!(err.message, "a dialog opened while your text was being pasted: it is in the session's input box, not sent");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(log_lines(&log, "APPROVAL").len(), 1, "the Enter answered the dialog: {:?}", log_lines(&log, "APPROVAL"));
    t.kill(&b.id).await.unwrap();
}

/// The same for Claude Code, whose dialogs are known from its hooks: a permission request that
/// arrives while the text is on its way keeps the Enter out, and the text waits in the input
/// box for whatever is sent next.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_enter_is_withheld_from_a_claude_session_that_asks_for_permission_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path()).await;
    let t = &state.terminals;
    // A stand-in Claude Code session: a program that answers every line it is sent.
    let id = t.insert_test_agent(None, json!({})).id;
    let entry = t.get(&id).unwrap();
    let launch = super::pty::LaunchSpec {
        argv: python_argv("import sys\nfor l in sys.stdin:\n    sys.stdout.write('got ' + l)\n    sys.stdout.flush()"),
        cwd: dir.path().to_path_buf(),
        env: vec![],
        cols: 80,
        rows: 24,
        redact: vec![],
    };
    t.launch(&state, &entry, launch, true).await.unwrap();
    let set_state = |s: super::AgentState| {
        t.update(&entry, |r| {
            r.info.agent.as_mut().unwrap().state = s;
            true
        });
    };
    assert_eq!(agent_of(&state, &id).state, super::AgentState::Idle);

    // The permission request arrives while the text is on its way: a send holds the session's
    // input from its paste to its Enter.
    let asks = tokio::spawn({
        let (state, id) = (state.clone(), id.clone());
        async move {
            let entry = state.terminals.get(&id).unwrap();
            while entry.input_lock.try_lock().is_ok() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            state.terminals.update(&entry, |r| {
                r.info.agent.as_mut().unwrap().state = super::AgentState::NeedsPermission;
                true
            });
        }
    });
    let err = t.ask(&state, ask_req("", "fix the build", Some(&id))).await.unwrap_err();
    assert_eq!((err.status.as_u16(), err.code), (409, "conflict"), "{err}");
    assert_eq!(err.message, "a dialog opened while your text was being pasted: it is in the session's input box, not sent");
    asks.await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let screen = t.screen_text(&id, 10).unwrap();
    assert!(screen.contains("fix the build") && !screen.contains("got "), "the Enter was pressed: {screen:?}");

    // While it asks, nothing is pasted at all.
    let err = t.ask(&state, ask_req("", "one more", Some(&id))).await.unwrap_err();
    assert!(err.message.contains("answer it first"), "{}", err.message);
    assert!(!t.screen_text(&id, 10).unwrap().contains("one more"));

    // Once the request is answered the next text goes in and is sent: the session reads the
    // text that waited in its input box with it.
    set_state(super::AgentState::Working);
    t.ask(&state, ask_req("", ", then the tests", Some(&id))).await.unwrap();
    wait_for("the submitted line", || t.screen_text(&id, 10).is_some_and(|s| s.contains("got fix the build, then the tests"))).await;
    t.kill(&id).await.unwrap();
}
