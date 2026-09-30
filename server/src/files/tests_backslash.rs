//! Linux keeps a `\` as part of a file name: `a\b.txt` is one file beside the folder `a`,
//! and `d\x` one folder. Every route names them as they are and none reaches `a/b.txt` or
//! `d/x/inner.txt`, which exist here to be reached by mistake. (Windows cannot name such
//! files: `\` separates there, see `util::paths`.)

use std::path::PathBuf;

use axum::http::Method;
use serde_json::{Value, json};

use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, call_api};

struct Env {
    state: AppState,
    pid: String,
    root: PathBuf,
    _dirs: [tempfile::TempDir; 3],
}

async fn setup() -> Env {
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap().join("app");
    for (file, text) in [(r"a\b.txt", "needle one\n"), ("a/b.txt", "needle decoy\n"), (r"d\x/inner.txt", "needle inner\n"), ("d/x/inner.txt", "needle decoy\n")] {
        let p = root.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _ = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    Env { state, pid, root, _dirs: [cfg, data, proj] }
}

impl Env {
    async fn api(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let url = format!("/api/projects/{}{path}", self.pid);
        call_api(&self.state, method, &url, body, &McpCtx::default()).await.unwrap_or_else(|e| panic!("{path}: {}", e.message))
    }

    async fn op(&self, op: &str, path: &str, to: &str) -> Value {
        self.api(Method::POST, "/files/op", Some(json!({ "op": op, "path": path, "to": to }))).await
    }

    fn disk(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }
}

/// `path` for a query string (`\` is `%5C`).
fn q(path: &str) -> String {
    urlencoding::encode(path).into_owned()
}

/// A listing's entries as `(name, path, kind)`, sorted.
fn entries(listing: &Value) -> Vec<(String, String, String)> {
    let s = |e: &Value, k: &str| e[k].as_str().unwrap_or_default().to_string();
    let mut v: Vec<_> = listing["entries"].as_array().unwrap().iter().map(|e| (s(e, "name"), s(e, "path"), s(e, "kind"))).collect();
    v.sort();
    v
}

fn entry(name: &str, path: &str, kind: &str) -> (String, String, String) {
    (name.into(), path.into(), kind.into())
}

async fn next_ui_open(rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<crate::events::Event>>) -> Value {
    loop {
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        if ev.kind == "ui.open" {
            return ev.data.clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backslash_is_part_of_a_name() {
    let env = setup().await;

    // Listing: the file and the folder under their own names, the folder's entries below it.
    let l = env.api(Method::GET, "/files/list?path=", None).await;
    assert_eq!(entries(&l), [entry("a", "a", "dir"), entry(r"a\b.txt", r"a\b.txt", "file"), entry("d", "d", "dir"), entry(r"d\x", r"d\x", "dir")]);
    let l = env.api(Method::GET, &format!("/files/list?path={}", q(r"d\x")), None).await;
    assert_eq!(l["path"], r"d\x");
    assert_eq!(entries(&l), [entry("inner.txt", r"d\x/inner.txt", "file")]);

    // Open: the file itself, answered under its name.
    let a = env.api(Method::GET, &format!("/files/read?path={}", q(r"a\b.txt")), None).await;
    assert_eq!((a["path"].as_str(), a["content"].as_str()), (Some(r"a\b.txt"), Some("needle one\n")));
    let inner = env.api(Method::GET, &format!("/files/read?path={}", q(r"d\x/inner.txt")), None).await;
    assert_eq!((inner["path"].as_str(), inner["content"].as_str()), (Some(r"d\x/inner.txt"), Some("needle inner\n")));

    // Save: into that file.
    let w = env.api(Method::PUT, "/files/write", Some(json!({ "path": r"a\b.txt", "content": "needle saved\n", "etag": a["etag"] }))).await;
    assert_eq!(w["path"], r"a\b.txt");
    assert_eq!(env.disk(r"a\b.txt").as_deref(), Some("needle saved\n"));

    // Rename the file and the folder: the new names, not paths below `a` or `d`.
    assert_eq!(env.op("rename", r"a\b.txt", r"a\c.txt").await["path"], r"a\c.txt");
    assert_eq!(env.disk(r"a\c.txt").as_deref(), Some("needle saved\n"));
    assert!(!env.root.join(r"a\b.txt").exists() && !env.root.join("a/c.txt").exists());
    assert_eq!(env.op("rename", r"d\x", r"d\y").await["path"], r"d\y");
    assert_eq!(env.disk(r"d\y/inner.txt").as_deref(), Some("needle inner\n"));
    assert!(!env.root.join(r"d\x").exists() && !env.root.join("d/y").exists());

    // Search and quick open: every file once, under its own name.
    let s = env.api(Method::GET, "/search?q=needle", None).await;
    let hits: Vec<(&str, &str)> = s["matches"].as_array().unwrap().iter().map(|h| (h["path"].as_str().unwrap(), h["preview"].as_str().unwrap())).collect();
    assert_eq!(hits, [("a/b.txt", "needle decoy"), (r"a\c.txt", "needle saved"), ("d/x/inner.txt", "needle decoy"), (r"d\y/inner.txt", "needle inner")]);
    let f = env.api(Method::GET, "/files/find?q=txt", None).await;
    let mut found: Vec<&str> = f["results"].as_array().unwrap().iter().map(|h| h["path"].as_str().unwrap()).collect();
    found.sort();
    assert_eq!(found, ["a/b.txt", r"a\c.txt", "d/x/inner.txt", r"d\y/inner.txt"]);

    // Replace in files, from a search result: that file's lines, and that file changed.
    let body = |dry: bool, expected: Value| json!({ "q": "needle", "replacement": "pin", "paths": [r"a\c.txt"], "dryRun": dry, "expected": expected });
    let d = env.api(Method::POST, "/search/replace", Some(body(true, json!({})))).await;
    assert_eq!((d["files"][0]["path"].as_str(), d["files"][0]["lines"][0]["before"].as_str()), (Some(r"a\c.txt"), Some("needle saved")));
    let done = env.api(Method::POST, "/search/replace", Some(body(false, json!({ r"a\c.txt": d["files"][0]["etag"] })))).await;
    assert_eq!(done["replaced"][0]["path"], r"a\c.txt");
    assert_eq!(env.disk(r"a\c.txt").as_deref(), Some("pin saved\n"));

    // An agent's absolute path opens the file under its name.
    let mut rx = env.state.events.subscribe();
    let tools = super::tools::tools();
    let open = tools.iter().find(|t| t.name == "workbench_open_file").unwrap();
    (open.handler)(env.state.clone(), McpCtx::default(), json!({ "path": env.root.join(r"a\c.txt").display().to_string() })).await.unwrap();
    assert_eq!(next_ui_open(&mut rx).await["params"]["path"], r"a\c.txt");

    // Nothing reached the other files.
    assert_eq!((env.disk("a/b.txt").as_deref(), env.disk("d/x/inner.txt").as_deref()), (Some("needle decoy\n"), Some("needle decoy\n")));

    // What would climb out on Windows is one name here, inside the project.
    let w = env.api(Method::PUT, "/files/write", Some(json!({ "path": r"..\up.txt", "content": "x", "etag": null }))).await;
    assert_eq!(w["path"], r"..\up.txt");
    assert_eq!(env.op("mkdir", r"..\..\up", "").await["path"], r"..\..\up");
    assert!(env.root.join(r"..\up.txt").is_file() && env.root.join(r"..\..\up").is_dir());
    let parent = env.root.parent().unwrap();
    assert!(!parent.join("up.txt").exists() && !parent.join("up").exists() && !parent.parent().unwrap().join("up").exists());
}
