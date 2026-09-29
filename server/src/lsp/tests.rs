//! Integration tests against a real AppState, the router on a loopback port and a fake
//! language server (`testdata/fake_ls.py`). Everything runs in temp dirs.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::config::ServerConfig;
use crate::app::{self, AppState};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn fake_ls() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lsp/testdata/fake_ls.py")
}

struct Env {
    state: AppState,
    addr: SocketAddr,
    pid: String,
    root: PathBuf,
    /// A file outside the project that the fake server points to.
    external: PathBuf,
    _dir: tempfile::TempDir,
}

async fn env_with(extra_env: &[(&str, &str)], repo_config: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.fk"), "def alpha\nuse beta\nERROR here\n").unwrap();
    std::fs::write(root.join("b.fk"), "def beta\nalpha beta\n").unwrap();
    if let Some(c) = repo_config {
        std::fs::write(root.join(".workbench.toml"), c).unwrap();
    }
    let external = dir.path().join("outside/lib.fk");
    std::fs::create_dir_all(external.parent().unwrap()).unwrap();
    std::fs::write(&external, "def external\n").unwrap();
    let paths = crate::config::Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![root.display().to_string()];
    cfg.agents.restore_on_start = false;
    let mut env: std::collections::BTreeMap<String, String> = [("FAKE_LS_EXTERNAL".to_string(), external.display().to_string())].into();
    for (k, v) in extra_env {
        env.insert(k.to_string(), v.to_string());
    }
    cfg.lsp.servers.insert(
        "fake".into(),
        ServerConfig {
            command: Some("python3".into()),
            args: Some(vec![fake_ls().display().to_string()]),
            extensions: Some(vec!["fk".into()]),
            env,
            initialization_options: Some(json!({ "x": 1 })),
            settings: Some(json!({ "fake": { "a": 1, "nested": { "b": 2 } } })),
            ..Default::default()
        },
    );
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    super::start(&state).await;
    let pid = state.projects.list()[0].id.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app::build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    Env { state, addr, pid, root, external, _dir: dir }
}

impl Env {
    fn bearer(&self) -> String {
        format!("Bearer {}", self.state.auth.master_token())
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let r = reqwest::Client::new().get(format!("http://{}{path}", self.addr)).header("authorization", self.bearer()).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let r = reqwest::Client::new().post(format!("http://{}{path}", self.addr)).header("authorization", self.bearer()).json(&body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn enable(&self) {
        let (code, body) = self.post(&format!("/api/projects/{}/lsp/enable", self.pid), json!({})).await;
        assert_eq!(code, 200, "{body}");
        assert_eq!(body["enabled"], true);
    }

    async fn connect(&self) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
        let mut req = format!("ws://{}/api/projects/{}/lsp/ws", self.addr, self.pid).into_client_request().unwrap();
        req.headers_mut().insert("Authorization", self.bearer().parse().unwrap());
        tokio_tungstenite::connect_async(req).await.map(|(ws, _)| ws)
    }

    fn uri(&self, rel: &str) -> String {
        format!("file:///{}/{rel}", self.pid)
    }

    /// The URI the language server knows the file by.
    fn server_uri(&self, rel: &str) -> String {
        super::uri::file_uri(&self.root.join(rel).to_string_lossy())
    }

    async fn source_status(&self, uri: &str) -> u16 {
        self.get(&format!("/api/projects/{}/lsp/source?uri={}", self.pid, urlencoding::encode(uri))).await.0
    }

    fn text(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Read until a message matches; others are kept in `seen`.
async fn until(ws: &mut Ws, seen: &mut Vec<Value>, pred: impl Fn(&Value) -> bool) -> Value {
    if let Some(i) = seen.iter().position(&pred) {
        return seen.remove(i);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let m = tokio::time::timeout_at(deadline, ws.next()).await.unwrap_or_else(|_| panic!("timed out; saw {seen:#?}"));
        match m {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if pred(&v) {
                    return v;
                }
                seen.push(v);
            }
            Some(Ok(_)) => {}
            other => panic!("socket ended: {other:?}; saw {seen:#?}"),
        }
    }
}

async fn req(ws: &mut Ws, seen: &mut Vec<Value>, id: i64, method: &str, params: Value, server: Option<&str>) -> Value {
    let mut m = json!({ "t": "req", "id": id, "method": method, "params": params });
    if let Some(s) = server {
        m["server"] = json!(s);
    }
    send(ws, m).await;
    until(ws, seen, |v| v["t"] == "res" && v["id"] == id).await
}

fn pos(uri: &str, line: u32, character: u32) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } })
}

async fn wait_for(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not reached");
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_starts_before_the_user_enables_it() {
    let e = env_with(&[], None).await;
    // The socket is refused, and the status says why.
    assert!(e.connect().await.is_err());
    let (code, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
    assert_eq!(code, 200);
    assert_eq!(st["enabled"], false);
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
    assert_eq!((fake["state"].as_str(), fake["available"].as_bool()), (Some("off"), Some(true)), "{fake}");
    // Documents opened some other way do not start a server either.
    let p = e.state.projects.require(&e.pid).unwrap();
    let lsp = e.state.lsp.project(&p);
    lsp.open(&e.state, 7, &e.uri("a.fk"), None, e.text("a.fk"), true).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(lsp.server("fake").is_none(), "no process without enablement");
    let (_, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
    assert_eq!(fake["state"], "failed", "{fake}");
    // MCP tools never start one.
    let tool = super::mcp_tools().into_iter().find(|t| t.name == "code_diagnostics").unwrap();
    let ctx = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some(e.pid.clone()) };
    let out = (tool.handler)(e.state.clone(), ctx.clone(), json!({})).await.unwrap();
    assert!(matches!(out, crate::mcp::ToolOutput::Text(t) if t.contains("never start")));
    let refs = super::mcp_tools().into_iter().find(|t| t.name == "code_references").unwrap();
    let out = (refs.handler)(e.state.clone(), ctx.clone(), json!({ "path": "a.fk", "line": 1, "column": 5 })).await.unwrap();
    assert!(matches!(out, crate::mcp::ToolOutput::Text(t) if t.contains("never start")));
    assert!(lsp.server("fake").is_none());
    // Agents cannot enable it (in-process callers are refused).
    let err = crate::mcp::call_api(&e.state, axum::http::Method::POST, &format!("/api/projects/{}/lsp/enable", e.pid), Some(json!({})), &ctx).await.unwrap_err();
    assert_eq!(err.status.as_u16(), 403);
    assert!(!super::manager::enabled(&e.state, &p));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repository_config_cannot_add_or_change_servers() {
    let repo = r#"
        [lsp.servers.evil]
        command = "sh"
        args = ["-c", "touch /tmp/pwned"]
        extensions = ["fk"]
        [lsp.servers.fake]
        command = "sh"
    "#;
    let e = env_with(&[], Some(repo)).await;
    let (_, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
    let ids: Vec<&str> = st["servers"].as_array().unwrap().iter().filter_map(|s| s["id"].as_str()).collect();
    assert!(!ids.contains(&"evil"), "{ids:?}");
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap();
    assert!(fake["command"].as_str().unwrap().starts_with("python3 "), "{fake}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn editor_features_through_the_socket() {
    let e = env_with(&[], None).await;
    e.enable().await;
    let mut events = e.state.events.subscribe();
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    let hello = until(&mut ws, &mut seen, |v| v["t"] == "hello").await;
    assert_eq!(hello["project"], e.pid.as_str());
    let (a, b) = (e.uri("a.fk"), e.uri("b.fk"));
    send(&mut ws, json!({ "t": "open", "uri": a, "languageId": "plaintext", "text": e.text("a.fk") })).await;
    send(&mut ws, json!({ "t": "open", "uri": b, "text": e.text("b.fk") })).await;
    let opened = until(&mut ws, &mut seen, |v| v["t"] == "opened" && v["uri"] == a.as_str()).await;
    assert_eq!(opened["server"], "fake");
    let caps = until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
    assert_eq!(caps["capabilities"]["hoverProvider"], true);
    // Diagnostics arrive with browser URIs.
    let d = until(&mut ws, &mut seen, |v| v["t"] == "diagnostics" && v["uri"] == a.as_str() && !v["diagnostics"].as_array().unwrap().is_empty()).await;
    assert_eq!(d["diagnostics"][0]["message"], "error here");
    assert_eq!(d["diagnostics"][0]["range"]["start"]["line"], 2);

    // Hover, definition across files, references, rename.
    let r = req(&mut ws, &mut seen, 1, "textDocument/hover", pos(&a, 1, 5), None).await;
    assert_eq!(r["server"], "fake");
    assert_eq!(r["result"]["contents"]["value"], "**beta** (v1)");
    let r = req(&mut ws, &mut seen, 2, "textDocument/definition", pos(&a, 1, 5), None).await;
    assert_eq!(r["result"][0]["uri"], b.as_str(), "{r}");
    assert_eq!(r["result"][0]["range"]["start"], json!({ "line": 0, "character": 4 }));
    let r = req(&mut ws, &mut seen, 3, "textDocument/references", json!({ "textDocument": { "uri": a }, "position": { "line": 0, "character": 5 }, "context": { "includeDeclaration": true } }), None).await;
    let uris: Vec<&str> = r["result"].as_array().unwrap().iter().filter_map(|l| l["uri"].as_str()).collect();
    assert_eq!(uris.len(), 2, "{r}");
    assert!(uris.contains(&a.as_str()) && uris.contains(&b.as_str()));
    let r = req(&mut ws, &mut seen, 4, "textDocument/rename", json!({ "textDocument": { "uri": a }, "position": { "line": 1, "character": 5 }, "newName": "gamma" }), None).await;
    let changes = r["result"]["changes"].as_object().unwrap();
    assert!(changes.contains_key(&a) && changes.contains_key(&b), "{r}");
    assert_eq!(changes[&b].as_array().unwrap().len(), 2);

    // Completion, then resolve through the server that answered (data round-trips).
    let r = req(&mut ws, &mut seen, 5, "textDocument/completion", pos(&a, 0, 1), None).await;
    let alpha = r["result"]["items"][0].clone();
    assert_eq!(alpha["data"]["uri"], e.server_uri("a.fk"), "data is the server's own, handed back as it is");
    let r = req(&mut ws, &mut seen, 6, "completionItem/resolve", alpha, Some("fake")).await;
    assert_eq!(r["result"]["documentation"]["value"], "docs for alpha");

    // Workspace symbols from every ready server, merged.
    let r = req(&mut ws, &mut seen, 7, "workspace/symbol", json!({ "query": "a" }), None).await;
    let names: Vec<&str> = r["result"].as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(names.contains(&"alpha") && names.contains(&"beta"), "{r}");
    assert_eq!(r["result"][0]["_server"], "fake");

    // Changes: full text, versions only grow, diagnostics follow.
    send(&mut ws, json!({ "t": "change", "uri": a, "text": "def alpha\nuse beta\nfine now\n" })).await;
    let d = until(&mut ws, &mut seen, |v| v["t"] == "diagnostics" && v["uri"] == a.as_str() && v["version"] == 2).await;
    assert!(d["diagnostics"].as_array().unwrap().is_empty());
    let r = req(&mut ws, &mut seen, 8, "textDocument/hover", pos(&a, 1, 5), None).await;
    assert_eq!(r["result"]["contents"]["value"], "**beta** (v2)");

    // Into a file outside the project: an lsp-src URI that the source route serves.
    std::fs::write(e.root.join("c.fk"), "external\n").unwrap();
    let c = e.uri("c.fk");
    send(&mut ws, json!({ "t": "open", "uri": c, "text": "external\n" })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "opened" && v["uri"] == c.as_str()).await;
    let r = req(&mut ws, &mut seen, 9, "textDocument/definition", pos(&c, 0, 2), None).await;
    let src = r["result"][0]["uri"].as_str().unwrap().to_string();
    assert!(src.starts_with(&format!("lsp-src://{}/", e.pid)) && src.ends_with("/outside/lib.fk"), "{src}");
    let (code, body) = e.get(&format!("/api/projects/{}/lsp/source?uri={}", e.pid, urlencoding::encode(&src))).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["content"], "def external\n");
    // Opening it keeps navigation going inside the source panel.
    send(&mut ws, json!({ "t": "open", "uri": src, "text": "def external\n" })).await;
    let o = until(&mut ws, &mut seen, |v| v["t"] == "opened" && v["uri"] == src.as_str()).await;
    assert_eq!(o["server"], "fake", "{o}");
    // Anything else is refused.
    let forged = format!("lsp-src://{}/etc/passwd", e.pid);
    let (code, _) = e.get(&format!("/api/projects/{}/lsp/source?uri={}", e.pid, urlencoding::encode(&forged))).await;
    assert_eq!(code, 403);

    // What the server was told: settings by section, init options, the project root.
    let r = req(&mut ws, &mut seen, 10, "fake/state", json!({}), Some("fake")).await;
    let s = &r["result"];
    assert_eq!(s["config"], json!([{ "a": 1, "nested": { "b": 2 } }, { "b": 2 }, { "fake": { "a": 1, "nested": { "b": 2 } } }]));
    assert_eq!(s["initOptions"], json!({ "x": 1 }));
    // (Only URI fields are mapped: these are plain values of a custom answer.)
    assert_eq!(s["root"], super::uri::file_uri(&e.root.to_string_lossy()));
    assert_eq!(s["docs"][&e.server_uri("a.fk")], 2);

    // Files that change on disk while nobody has them open reach the server (by its
    // globs); open documents and unwatched kinds do not.
    std::fs::write(e.root.join("new.fk"), "def fresh\n").unwrap();
    std::fs::write(e.root.join("notes.txt"), "x").unwrap();
    std::fs::write(e.root.join("a.fk"), "changed on disk\n").unwrap();
    e.state.events.emit("fs.changed", Some(&e.pid), json!({ "paths": ["new.fk", "notes.txt", "a.fk"] }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let r = req(&mut ws, &mut seen, 11, "fake/state", json!({}), Some("fake")).await;
    let watched: Vec<String> = r["result"]["watched"].as_array().unwrap().iter().filter_map(|c| c["uri"].as_str().map(str::to_string)).collect();
    assert_eq!(watched, vec![e.uri("new.fk")], "{r}");

    // Cancellation reaches the server.
    send(&mut ws, json!({ "t": "req", "id": 12, "method": "fake/slow", "params": {}, "server": "fake" })).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    send(&mut ws, json!({ "t": "cancel", "id": 12 })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = req(&mut ws, &mut seen, 13, "fake/state", json!({}), Some("fake")).await;
    assert_eq!(r["result"]["cancelled"].as_array().unwrap().len(), 1, "{r}");

    // A server's workspace/applyEdit goes to the editor, whose answer goes back.
    send(&mut ws, json!({ "t": "req", "id": 14, "method": "fake/applyEdit", "params": {}, "server": "fake" })).await;
    let ask = until(&mut ws, &mut seen, |v| v["t"] == "request").await;
    assert_eq!(ask["method"], "workspace/applyEdit");
    let keys: Vec<&String> = ask["params"]["edit"]["changes"].as_object().unwrap().keys().collect();
    assert!(keys[0].starts_with(&format!("file:///{}/", e.pid)) || keys[0].starts_with("lsp-src://"), "{keys:?}");
    send(&mut ws, json!({ "t": "reply", "id": ask["id"], "result": { "applied": true } })).await;
    let r = until(&mut ws, &mut seen, |v| v["t"] == "res" && v["id"] == 14).await;
    assert_eq!(r["result"], json!({ "applied": true }));

    // Status, progress events, the log.
    let (_, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
    assert_eq!(fake["state"], "ready", "{fake}");
    assert_eq!(fake["side"], "host");
    assert!(fake["openDocs"].as_u64().unwrap() >= 2);
    let mut states = vec![];
    while let Ok(ev) = events.try_recv() {
        if ev.kind == "lsp.state" {
            states.push(ev.data["state"].as_str().unwrap_or("").to_string());
        }
    }
    assert!(states.contains(&"starting".to_string()) && states.contains(&"ready".to_string()), "{states:?}");
    let (_, log) = e.get(&format!("/api/projects/{}/lsp/servers/fake/log", e.pid)).await;
    let text: Vec<&str> = log["lines"].as_array().unwrap().iter().filter_map(|l| l["text"].as_str()).collect();
    assert!(text.iter().any(|t| t.contains("fake-ls initialized")), "{text:?}");
    assert!(text.iter().any(|t| t.contains("fake-ls ready")), "{text:?}");
    let (_, diags) = e.get(&format!("/api/projects/{}/lsp/diagnostics", e.pid)).await;
    assert!(diags["counts"]["errors"].as_u64().is_some(), "{diags}");

    // MCP tools use the running server.
    let ctx = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some(e.pid.clone()) };
    let tool = |n: &str| super::mcp_tools().into_iter().find(|t| t.name == n).unwrap();
    let text_of = |o: crate::mcp::ToolOutput| match o {
        crate::mcp::ToolOutput::Text(t) => t,
        other => panic!("{other:?}"),
    };
    let out = text_of((tool("code_definition").handler)(e.state.clone(), ctx.clone(), json!({ "path": "a.fk", "line": 2, "column": 6 })).await.unwrap());
    assert!(out.starts_with("b.fk:1:5  def beta"), "{out}");
    // b.fk is open: its buffer text is used. A closed file is opened for the request.
    let out = text_of((tool("code_references").handler)(e.state.clone(), ctx.clone(), json!({ "path": "new.fk", "line": 1, "column": 6 })).await.unwrap());
    assert!(out.contains("new.fk:1:5  def fresh"), "{out}");
    let out = text_of((tool("code_symbols").handler)(e.state.clone(), ctx.clone(), json!({ "query": "bet" })).await.unwrap());
    assert!(out.contains("Function beta") && out.contains("b.fk:1:5"), "{out}");
    send(&mut ws, json!({ "t": "change", "uri": b, "text": "def beta\nERROR\n" })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "diagnostics" && v["uri"] == b.as_str() && !v["diagnostics"].as_array().unwrap().is_empty()).await;
    let out = text_of((tool("code_diagnostics").handler)(e.state.clone(), ctx.clone(), json!({ "path": "b.fk" })).await.unwrap());
    assert!(out.contains("b.fk:2:1: error [fake E1]: error here"), "{out}");
    let foreign = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some("other".into()) };
    assert!((tool("code_diagnostics").handler)(e.state.clone(), foreign, json!({ "projectId": e.pid })).await.is_err());
    // The temporary document is gone again.
    assert!(e.state.lsp.get(&e.pid).unwrap().doc_text(&e.uri("new.fk")).is_none());

    // Closing the socket closes its documents; an idle server is stopped.
    drop(ws);
    let lsp = e.state.lsp.get(&e.pid).unwrap();
    wait_for(|| lsp.open_uris().is_empty()).await;
    let os_pid = lsp.server("fake").unwrap().os_pid().unwrap();
    lsp.sweep_idle(&e.state, Duration::ZERO).await;
    assert!(lsp.server("fake").is_none());
    wait_for(|| !alive(os_pid)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crashes_restart_with_backoff_then_stop() {
    let e = env_with(&[], None).await;
    e.enable().await;
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    let a = e.uri("a.fk");
    send(&mut ws, json!({ "t": "open", "uri": a, "text": e.text("a.fk") })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
    let lsp = e.state.lsp.get(&e.pid).unwrap();
    for round in 1..=3 {
        let first = lsp.server("fake").unwrap();
        let _ = req(&mut ws, &mut seen, 100 + round, "fake/crash", json!({}), Some("fake")).await;
        until(&mut ws, &mut seen, |v| v["t"] == "down").await;
        if round < 3 {
            // Back, with the open document handed over again.
            until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
            let second = lsp.server("fake").unwrap();
            assert!(!std::sync::Arc::ptr_eq(&first, &second));
            let r = req(&mut ws, &mut seen, 200 + round, "textDocument/hover", pos(&a, 1, 5), None).await;
            assert_eq!(r["result"]["contents"]["value"], "**beta** (v1)", "{r}");
        }
    }
    let (_, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
    assert_eq!(fake["state"], "crashed", "{fake}");
    assert!(fake["error"].as_str().unwrap().contains("exited with code 3"), "{fake}");
    // The user restarts it.
    let (code, st) = e.post(&format!("/api/projects/{}/lsp/servers/fake/restart", e.pid), json!({})).await;
    assert_eq!(code, 200, "{st}");
    until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
    // Stop by the user: stays off even with documents open.
    let os_pid = lsp.server("fake").unwrap().os_pid().unwrap();
    let (_, st) = e.post(&format!("/api/projects/{}/lsp/servers/fake/stop", e.pid), json!({})).await;
    let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
    assert_eq!(fake["state"], "stopped");
    wait_for(|| !alive(os_pid)).await;
    send(&mut ws, json!({ "t": "open", "uri": e.uri("b.fk"), "text": e.text("b.fk") })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(lsp.server("fake").is_none());
    // The log of the last process is still there.
    let (_, log) = e.get(&format!("/api/projects/{}/lsp/servers/fake/log", e.pid)).await;
    assert_eq!(log["running"], false);
    assert!(!log["lines"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_cannot_start_is_reported() {
    let e = env_with(&[("FAKE_LS_CRASH_ON_START", "1")], None).await;
    e.enable().await;
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    send(&mut ws, json!({ "t": "open", "uri": e.uri("a.fk"), "text": e.text("a.fk") })).await;
    let lsp = e.state.lsp.get(&e.pid).unwrap();
    // Three quick deaths: crashed, not restarted forever.
    for _ in 0..200 {
        let (_, st) = e.get(&format!("/api/projects/{}/lsp", e.pid)).await;
        let fake = st["servers"].as_array().unwrap().iter().find(|s| s["id"] == "fake").unwrap().clone();
        if fake["state"] == "failed" || fake["state"] == "crashed" {
            assert!(fake["error"].as_str().is_some(), "{fake}");
            let r = req(&mut ws, &mut seen, 1, "textDocument/hover", pos(&e.uri("a.fk"), 0, 0), None).await;
            assert!(r["error"]["message"].as_str().is_some(), "{r}");
            assert!(lsp.server("fake").is_none());
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never reported");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn symbol_search_says_when_no_running_server_offers_it() {
    let e = env_with(&[("FAKE_LS_NO_WORKSPACE_SYMBOL", "1")], None).await;
    e.enable().await;
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    send(&mut ws, json!({ "t": "open", "uri": e.uri("a.fk"), "text": e.text("a.fk") })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
    let ctx = crate::mcp::McpCtx { terminal_id: Some("t1".into()), project_id: Some(e.pid.clone()) };
    let tool = |n: &str| super::mcp_tools().into_iter().find(|t| t.name == n).unwrap();
    let crate::mcp::ToolOutput::Text(out) = (tool("code_symbols").handler)(e.state.clone(), ctx, json!({ "query": "bet" })).await.unwrap() else {
        panic!("text expected")
    };
    assert!(out.starts_with("None of the running language servers searches symbols"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disable_stops_servers_and_closes_sockets() {
    let e = env_with(&[], None).await;
    e.enable().await;
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    send(&mut ws, json!({ "t": "open", "uri": e.uri("a.fk"), "text": e.text("a.fk") })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "caps").await;
    let os_pid = e.state.lsp.get(&e.pid).unwrap().server("fake").unwrap().os_pid().unwrap();
    let (code, st) = e.post(&format!("/api/projects/{}/lsp/disable", e.pid), json!({})).await;
    assert_eq!((code, st["enabled"].as_bool()), (200, Some(false)));
    until(&mut ws, &mut seen, |v| v["t"] == "disabled").await;
    wait_for(|| !alive(os_pid)).await;
    assert!(e.state.lsp.get(&e.pid).is_none());
    assert!(e.connect().await.is_err());
    // The approval is per directory: a record for another root does not count.
    let p = e.state.projects.require(&e.pid).unwrap();
    e.state.lsp.trust.update(&e.state, &e.pid, |r| {
        r.enabled = true;
        r.root = "/somewhere/else".into();
    })
    .unwrap();
    assert!(!super::manager::enabled(&e.state, &p));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signing_the_device_out_closes_the_socket() {
    let e = env_with(&[], None).await;
    e.enable().await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = http.get(format!("http://{}/auth?token={}", e.addr, e.state.auth.master_token())).send().await.unwrap();
    let key = r.headers()["location"].to_str().unwrap().strip_prefix("/#wbk=").unwrap().to_string();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let origin = format!("http://{}", e.addr);
    let mut req = format!("ws://{}/api/projects/{}/lsp/ws?wbk={key}", e.addr, e.pid).into_client_request().unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    req.headers_mut().insert("origin", origin.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let mut seen = vec![];
    until(&mut ws, &mut seen, |v| v["t"] == "hello").await;
    let bearer = e.bearer();
    let devices: Value = http.get(format!("{origin}/api/auth/devices")).header("authorization", &bearer).send().await.unwrap().json().await.unwrap();
    let id = devices[0]["id"].as_str().unwrap().to_string();
    http.delete(format!("{origin}/api/auth/devices/{id}")).header("authorization", &bearer).send().await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut code = None;
    while let Ok(Some(m)) = tokio::time::timeout_at(deadline, ws.next()).await {
        if let Ok(Message::Close(f)) = m {
            code = f.map(|f| u16::from(f.code));
            break;
        }
    }
    assert_eq!(code, Some(crate::auth::CLOSE_SESSION_ENDED));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uri_like_text_stays_text_and_opens_nothing() {
    let e = env_with(&[], None).await;
    e.enable().await;
    let mut ws = e.connect().await.unwrap();
    let mut seen = vec![];
    let a = e.uri("a.fk");
    let lit = format!("file://{}", e.external.display());
    let src = super::uri::source_uri(&e.pid, &e.external.to_string_lossy());
    send(&mut ws, json!({ "t": "open", "uri": a, "text": "def alpha\n\n" })).await;
    until(&mut ws, &mut seen, |v| v["t"] == "caps").await;

    // A completion for a string literal that is a file URI: every text field comes back
    // exactly as the server wrote it (accepting it inserts what the user saw).
    let r = req(&mut ws, &mut seen, 1, "textDocument/completion", pos(&a, 1, 0), None).await;
    let item = r["result"]["items"].as_array().unwrap().iter().find(|i| i["kind"] == 21).cloned().unwrap_or_else(|| panic!("{r}"));
    for field in ["/label", "/detail", "/filterText", "/sortText", "/textEdit/newText", "/documentation/value", "/command/arguments/0"] {
        assert_eq!(item.pointer(field).and_then(Value::as_str), Some(lit.as_str()), "{field}: {item}");
    }
    // Resolving hands it back to the server unchanged, too.
    let r = req(&mut ws, &mut seen, 2, "completionItem/resolve", item, Some("fake")).await;
    assert_eq!(r["result"]["label"], lit.as_str(), "{r}");
    assert_eq!(r["result"]["textEdit"]["newText"], lit.as_str(), "{r}");
    // Text is not a location the server pointed to: the file stays unreadable.
    assert_eq!(e.source_status(&src).await, 403);

    // Diagnostics: the message is text, the related information is a location (which
    // may then be shown), and data is the server's own.
    send(&mut ws, json!({ "t": "change", "uri": a, "text": "def alpha\nLINK\n" })).await;
    let d = until(&mut ws, &mut seen, |v| v["t"] == "diagnostics" && v["uri"] == a.as_str() && !v["diagnostics"].as_array().unwrap().is_empty()).await;
    let d = &d["diagnostics"][0];
    assert_eq!(d["message"], lit.as_str(), "{d}");
    assert_eq!(d["relatedInformation"][0]["message"], lit.as_str(), "{d}");
    assert_eq!(d["relatedInformation"][0]["location"]["uri"], src.as_str(), "{d}");
    assert_eq!(d["data"]["uri"], e.server_uri("a.fk"), "{d}");
    assert_eq!(e.source_status(&src).await, 200);
    // Handed back in a code action context, the location maps to the server again.
    let lsp = e.state.lsp.get(&e.pid).unwrap();
    let server = lsp.ready_server("fake").unwrap();
    let mut back = json!({ "context": { "diagnostics": [d.clone()] } });
    server.params_to_server(&mut back, &lsp.allow.lock());
    assert_eq!(back["context"]["diagnostics"][0]["relatedInformation"][0]["location"]["uri"], lit.as_str());
    assert_eq!(back["context"]["diagnostics"][0]["message"], lit.as_str());
}

/// Ping and wait for the pong: whatever the server queued for this socket before has
/// been read into `seen`.
async fn barrier(ws: &mut Ws, seen: &mut Vec<Value>) {
    send(ws, json!({ "t": "ping" })).await;
    until(ws, seen, |v| v["t"] == "pong").await;
}

fn diag_lines(v: &Value) -> Vec<u64> {
    v["diagnostics"].as_array().map(|a| a.iter().filter_map(|d| d["range"]["start"]["line"].as_u64()).collect()).unwrap_or_default()
}

/// Two tabs with the same file open, one of them with unsaved edits.
async fn two_tabs_on_one_file(extra_env: &[(&str, &str)]) {
    let e = env_with(extra_env, None).await;
    e.enable().await;
    let (mut t1, mut t2) = (e.connect().await.unwrap(), e.connect().await.unwrap());
    let (mut s1, mut s2) = (vec![], vec![]);
    let a = e.uri("a.fk");
    let is_a = |v: &Value| v["t"] == "diagnostics" && v["uri"] == a.as_str();
    let disk = e.text("a.fk"); // ERROR on line 2
    let edited = format!("\n\n\n{disk}"); // ERROR on line 5
    let state_of = async |ws: &mut Ws, seen: &mut Vec<Value>, id: i64| req(ws, seen, id, "fake/state", json!({}), Some("fake")).await["result"].clone();

    // Tab 1 opens the file and inserts three lines at the top (unsaved).
    send(&mut t1, json!({ "t": "open", "uri": a, "text": disk })).await;
    until(&mut t1, &mut s1, |v| v["t"] == "caps").await;
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [2]).await;
    send(&mut t1, json!({ "t": "change", "uri": a, "text": edited })).await;
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [5]).await;

    // Tab 2 opens it from disk. The server keeps tab 1's text, and tab 2 is not given
    // diagnostics computed for it; what it got before it had the file open is cleared.
    send(&mut t2, json!({ "t": "open", "uri": a, "text": disk })).await;
    let o = until(&mut t2, &mut s2, |v| v["t"] == "opened" && v["uri"] == a.as_str()).await;
    assert_eq!(o["server"], "fake");
    let st = state_of(&mut t1, &mut s1, 1).await;
    assert_eq!(st["texts"][&e.server_uri("a.fk")], edited.as_str(), "opening does not replace another tab's text");
    assert_eq!(st["docs"][&e.server_uri("a.fk")], 2);
    barrier(&mut t2, &mut s2).await;
    let last = s2.iter().rev().find(|v| is_a(v)).cloned().unwrap_or_else(|| panic!("{s2:#?}"));
    assert!(diag_lines(&last).is_empty(), "tab 2 is left without tab 1's diagnostics: {last}");
    s2.clear();

    // Tab 1 keeps editing: its diagnostics reach tab 1 only.
    send(&mut t1, json!({ "t": "change", "uri": a, "text": format!("{edited}WARN\n") })).await;
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [5, 6]).await;
    barrier(&mut t2, &mut s2).await;
    assert!(!s2.iter().any(is_a), "{s2:#?}");

    // Tab 2 asks something: the server switches to tab 2's text first, answers from it,
    // and its diagnostics go to tab 2 only.
    let r = req(&mut t2, &mut s2, 1, "textDocument/hover", pos(&a, 1, 5), None).await;
    assert_eq!(r["result"]["contents"]["value"], "**beta** (v4)", "{r}");
    until(&mut t2, &mut s2, |v| is_a(v) && diag_lines(v) == [2]).await;
    barrier(&mut t1, &mut s1).await;
    assert!(!s1.iter().any(is_a), "{s1:#?}");

    // Tab 1 is used again: its text goes back, unchanged by tab 2.
    let r = req(&mut t1, &mut s1, 2, "textDocument/hover", pos(&a, 4, 5), None).await;
    assert_eq!(r["result"]["contents"]["value"], "**beta** (v5)", "{r}");
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [5, 6]).await;
    barrier(&mut t2, &mut s2).await;
    assert!(!s2.iter().any(is_a), "{s2:#?}");

    // Tab 1 closes the file (discarding its edits): tab 2's text takes over, and tab 2
    // gets its diagnostics.
    send(&mut t1, json!({ "t": "close", "uri": a })).await;
    until(&mut t2, &mut s2, |v| is_a(v) && diag_lines(v) == [2]).await;
    let st = state_of(&mut t2, &mut s2, 2).await;
    assert_eq!(st["texts"][&e.server_uri("a.fk")], disk.as_str());
    assert_eq!(st["docs"][&e.server_uri("a.fk")], 6);

    // A tab whose text becomes the server's again (a reload after a save elsewhere)
    // gets the current diagnostics without a new version.
    send(&mut t1, json!({ "t": "open", "uri": a, "text": edited })).await;
    let o = until(&mut t1, &mut s1, |v| v["t"] == "opened" && v["uri"] == a.as_str()).await;
    assert_eq!(o["server"], "fake");
    barrier(&mut t1, &mut s1).await;
    s1.clear();
    send(&mut t1, json!({ "t": "change", "uri": a, "text": disk })).await;
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [2]).await;
    let st = state_of(&mut t2, &mut s2, 3).await;
    assert_eq!(st["docs"][&e.server_uri("a.fk")], 6, "the same text is not sent again");

    // The socket of the tab in use goes away: the other tab's text takes over.
    let r = req(&mut t2, &mut s2, 4, "textDocument/hover", pos(&a, 1, 5), None).await;
    assert_eq!(r["result"]["contents"]["value"], "**beta** (v6)", "{r}");
    send(&mut t2, json!({ "t": "change", "uri": a, "text": format!("{disk}WARN\n") })).await;
    until(&mut t2, &mut s2, |v| is_a(v) && diag_lines(v) == [2, 3]).await;
    drop(t2);
    until(&mut t1, &mut s1, |v| is_a(v) && diag_lines(v) == [2]).await;
    let st = state_of(&mut t1, &mut s1, 5).await;
    assert_eq!(st["texts"][&e.server_uri("a.fk")], disk.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tabs_on_one_file_keep_their_own_text_and_diagnostics() {
    two_tabs_on_one_file(&[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_tabs_on_one_file_with_a_server_that_sends_no_versions() {
    two_tabs_on_one_file(&[("FAKE_LS_NO_VERSION", "1")]).await;
}

#[test]
fn the_fake_server_exists() {
    assert!(Path::new(&fake_ls()).is_file());
}
