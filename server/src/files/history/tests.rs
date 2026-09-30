//! Local History against a real AppState: the router, the files watcher with an
//! external writer, agent hooks, labels, the MCP tool. Temp dirs only.

use std::time::Duration;

use axum::http::Method;
use serde_json::{Value, json};

use super::Kind;
use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, ToolOutput, call_api};

struct Env {
    state: AppState,
    pid: String,
    root: std::path::PathBuf,
    _dirs: [tempfile::TempDir; 3],
}

async fn setup() -> Env {
    setup_with(|_| {}).await
}

/// `setup`, with `prepare` adding files to the project before its watcher starts (they
/// have no history).
async fn setup_with(prepare: impl FnOnce(&std::path::Path)) -> Env {
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap().join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("ignored")).unwrap();
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n*.log\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    prepare(&root);
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _ = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    crate::files::watch::sync_all(&state).await;
    Env { state, pid, root, _dirs: [cfg, data, proj] }
}

impl Env {
    async fn api(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, crate::error::ApiError> {
        call_api(&self.state, method, &format!("/api/projects/{}{path}", self.pid), body, &McpCtx::default()).await
    }

    async fn history(&self, path: &str) -> Vec<Value> {
        let v = self.api(Method::GET, &format!("/files/history?path={path}"), None).await.unwrap();
        v["entries"].as_array().cloned().unwrap_or_default()
    }

    /// Poll until `path`'s history has `n` entries (the recorders are spawned).
    async fn wait_for(&self, path: &str, n: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let h = self.history(path).await;
            if h.len() >= n || tokio::time::Instant::now() > deadline {
                return h;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Let spawned recorders and the watcher (200 ms debounce) settle.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(900)).await;
    }
}

/// A Claude Code session of project `pid` (as the hook route finds it).
fn session(id: &str, pid: Option<&str>, cwd: &std::path::Path) -> crate::terminals::TerminalInfo {
    crate::terminals::TerminalInfo {
        id: id.into(),
        kind: crate::terminals::TerminalKind::Agent,
        title: format!("Session {id}"),
        project_id: pid.map(str::to_string),
        cwd: cwd.display().to_string(),
        argv: vec!["claude".into()],
        status: crate::terminals::TerminalStatus::Running,
        exit: None,
        created_at: 0,
        last_output_at: 0,
        cols: 80,
        rows: 24,
        open: true,
        pinned: false,
        color: None,
        order: 0,
        agent: None,
        meta: json!({}),
        lingering: 0,
    }
}

fn kinds(h: &[Value]) -> Vec<String> {
    h.iter().map(|e| e["kind"].as_str().unwrap_or("").to_string()).collect()
}

/// Open, save in Workbench, an external writer, an agent (hook first and hook
/// late), a deletion: every version is there once, labelled, with diffs.
#[tokio::test(flavor = "multi_thread")]
async fn saves_external_writes_and_agent_edits_are_recorded() {
    let env = setup().await;

    // Opening the file keeps the version the editor starts from.
    let read = env.api(Method::GET, "/files/read?path=src/main.rs", None).await.unwrap();
    let h = env.wait_for("src/main.rs", 1).await;
    assert_eq!(kinds(&h), ["base"]);
    assert_eq!(h[0]["hash"], read["etag"]);
    assert_eq!(h[0]["label"], "Opened in Workbench");

    // A save: one `save` entry; the watcher seeing the same bytes adds nothing.
    let body = json!({ "path": "src/main.rs", "content": "fn main() { println!(\"v2\"); }\n", "etag": read["etag"] });
    env.api(Method::PUT, "/files/write", Some(body)).await.unwrap();
    env.settle().await;
    let h = env.wait_for("src/main.rs", 2).await;
    assert_eq!(kinds(&h), ["save", "base"]);

    // An external writer (no hook): changed on disk.
    std::fs::write(env.root.join("src/main.rs"), "fn main() { println!(\"v3\"); }\n").unwrap();
    let h = env.wait_for("src/main.rs", 3).await;
    assert_eq!(kinds(&h), ["disk", "save", "base"]);

    // An agent whose PostToolUse hook names the file right after writing it.
    let abs = env.root.join("src/main.rs").display().to_string();
    let hook = json!({ "hook_event_name": "PostToolUse", "tool_name": "Edit", "tool_input": { "file_path": abs } });
    std::fs::write(env.root.join("src/main.rs"), "fn main() { println!(\"v4\"); }\n").unwrap();
    super::agent_edit(&env.state, &session("t-agent", Some(&env.pid), &env.root), &hook);
    let h = env.wait_for("src/main.rs", 4).await;
    env.settle().await;
    let h2 = env.history("src/main.rs").await;
    assert_eq!(h2.len(), h.len(), "the watcher must not add a duplicate: {h2:?}");
    assert_eq!(kinds(&h2), ["agent", "disk", "save", "base"]);
    assert_eq!(h2[0]["by"], "t-agent");

    // A hook that arrives after the watcher recorded the change re-labels it.
    std::fs::write(env.root.join("src/main.rs"), "fn main() { println!(\"v5\"); }\n").unwrap();
    let h = env.wait_for("src/main.rs", 5).await;
    assert_eq!(h[0]["kind"], "disk");
    super::agent_edit(&env.state, &session("t-late", Some(&env.pid), &env.root), &hook);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let h = loop {
        let h = env.history("src/main.rs").await;
        if h[0]["kind"] == "agent" || tokio::time::Instant::now() > deadline {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(kinds(&h), ["agent", "agent", "disk", "save", "base"]);
    assert_eq!(h[0]["by"], "t-late");
    assert_eq!(h[0]["who"], "Session t-late");

    // Hooks for other tools, events or files outside projects are ignored, and so
    // are hooks of sessions Workbench does not know.
    let t = session("t", Some(&env.pid), &env.root);
    super::agent_edit(&env.state, &t, &json!({ "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": { "command": "sed -i s/a/b/ x" } }));
    super::agent_edit(&env.state, &t, &json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": "/etc/hostname" } }));
    super::agent_hook(&env.state, "t-unknown", &hook);

    // Revision content and diffs.
    let save_id = h[3]["id"].as_u64().unwrap();
    let rev = env.api(Method::GET, &format!("/files/history/revision?id={save_id}"), None).await.unwrap();
    assert_eq!(rev["content"], "fn main() { println!(\"v2\"); }\n");
    assert_eq!(rev["previous"]["kind"], "base");
    let d = env.api(Method::GET, &format!("/files/history/diff?id={save_id}"), None).await.unwrap();
    let diff = d["diff"].as_str().unwrap();
    assert!(diff.contains("-fn main() {}\n+fn main() { println!(\"v2\"); }\n"), "{diff}");
    let d = env.api(Method::GET, &format!("/files/history/diff?id={save_id}&against=current"), None).await.unwrap();
    assert!(d["diff"].as_str().unwrap().contains("+fn main() { println!(\"v5\"); }"), "{d}");
    assert!(d["to"].is_null());

    // Deleting the file records it; the directory history lists changes, not bases.
    std::fs::remove_file(env.root.join("src/main.rs")).unwrap();
    let h = env.wait_for("src/main.rs", 6).await;
    assert_eq!(h[0]["kind"], "deleted");
    let dir = env.api(Method::GET, "/files/history/dir?path=src", None).await.unwrap();
    let dk: Vec<_> = dir["entries"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
    assert_eq!(dk, ["deleted", "agent", "agent", "disk", "save"]);

    // Labels apply to the files below them.
    let l = env.api(Method::POST, "/files/history/label", Some(json!({ "label": "Before refactoring" }))).await.unwrap();
    assert_eq!(l["kind"], "label");
    assert_eq!(env.history("src/main.rs").await[0]["label"], "Before refactoring");
    assert!(env.api(Method::POST, "/files/history/label", Some(json!({ "label": " " }))).await.is_err());

    let stats = env.api(Method::GET, "/files/history/stats", None).await.unwrap();
    assert_eq!(stats["files"], 1);
    assert_eq!(stats["retentionDays"], 7);
}

/// Sensitive, ignored, binary, `.git` and too large files are never snapshotted.
#[tokio::test(flavor = "multi_thread")]
async fn sensitive_ignored_and_binary_files_are_skipped() {
    let env = setup().await;
    let r = &env.root;
    std::fs::write(r.join(".env"), "API_TOKEN=supersecret\n").unwrap();
    std::fs::write(r.join("src/deploy_token"), "tok\n").unwrap();
    std::fs::write(r.join("ignored/x.txt"), "ignored\n").unwrap();
    std::fs::write(r.join("build.log"), "log\n").unwrap();
    std::fs::write(r.join("src/blob.bin"), b"\x00\x01\x02binary").unwrap();
    std::fs::write(r.join("src/big.txt"), vec![b'a'; 3 * 1024 * 1024]).unwrap();
    std::fs::write(r.join(".git/config"), "[core]\n").unwrap();
    std::fs::write(r.join("src/ok.txt"), "tracked\n").unwrap();
    // A save of a sensitive file through the API (the user revealed it) is not kept either.
    let body = json!({ "path": ".env", "content": "API_TOKEN=other\n", "etag": crate::files::sha256_hex(b"API_TOKEN=supersecret\n") });
    env.api(Method::PUT, "/files/write", Some(body)).await.unwrap();
    env.api(Method::GET, "/files/read?path=.env&allowSensitive=true", None).await.unwrap();
    assert_eq!(env.wait_for("src/ok.txt", 1).await.len(), 1);
    env.settle().await;

    let all = env.api(Method::GET, "/files/history/dir?path=", None).await.unwrap();
    let paths: Vec<_> = all["entries"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, ["src/ok.txt"]);
    let h = env.api(Method::GET, "/files/history?path=.env", None).await.unwrap();
    assert_eq!(h["untracked"], "sensitive");
    assert_eq!(env.api(Method::GET, "/files/history?path=ignored/x.txt", None).await.unwrap()["untracked"], "ignored");
    assert_eq!(env.api(Method::GET, "/files/history?path=src/blob.bin", None).await.unwrap()["untracked"], "binary");
    assert_eq!(env.api(Method::GET, "/files/history?path=src/big.txt", None).await.unwrap()["untracked"], "tooLarge");
    // Nothing of the secret reached the history folder.
    let dir = env.state.paths.data_dir.join("local-history");
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else if e.path().extension().is_some_and(|x| x == "zst") {
                let raw = zstd::stream::decode_all(std::fs::File::open(e.path()).unwrap()).unwrap();
                assert!(!String::from_utf8_lossy(&raw).contains("supersecret"));
            } else {
                assert!(!std::fs::read_to_string(e.path()).unwrap_or_default().contains("supersecret"));
            }
        }
    }
}

/// The MCP tool lists, diffs and shows revisions of the session's own project only.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_tool_is_read_only_and_confined() {
    let env = setup().await;
    std::fs::write(env.root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    env.wait_for("src/lib.rs", 1).await;
    std::fs::write(env.root.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    let h = env.wait_for("src/lib.rs", 2).await;
    let newest = h[0]["id"].as_u64().unwrap();

    let tools = super::tools::tools();
    let t = tools.iter().find(|t| t.name == "files_local_history").unwrap();
    assert!(!t.mutating);
    let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(env.pid.clone()) };
    let out = (t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "src/lib.rs" })).await.unwrap();
    let ToolOutput::Json(v) = out else { panic!("json expected") };
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    assert_eq!(v["entries"][0]["what"], "Changed on disk");

    let out = (t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "src/lib.rs", "revision": newest, "diff": true })).await.unwrap();
    let ToolOutput::Json(v) = out else { panic!("json expected") };
    assert!(v["diff"].as_str().unwrap().contains("+pub fn b() {}"), "{v}");

    // The whole project: recent changes.
    let out = (t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "" })).await.unwrap();
    let ToolOutput::Json(v) = out else { panic!("json expected") };
    assert_eq!(v["folder"], true);
    assert_eq!(v["entries"][0]["path"], "src/lib.rs");

    // Another project, a path outside, a sensitive file: refused.
    assert!((t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "x", "projectId": "other" })).await.is_err());
    assert!((t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "../outside" })).await.is_err());
    assert!((t.handler)(env.state.clone(), ctx.clone(), json!({ "path": "/etc/passwd" })).await.is_err());
    std::fs::write(env.root.join(".env"), "X=1\n").unwrap();
    assert!((t.handler)(env.state.clone(), ctx, json!({ "path": ".env" })).await.is_err());
}

/// The first recorded change of a committed file keeps the committed version before it.
#[tokio::test(flavor = "multi_thread")]
async fn first_change_of_a_committed_file_keeps_head() {
    if !crate::util::which("git") {
        return;
    }
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = crate::util::os::path::canonicalize(proj.path()).unwrap().join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(root.join("src/unix.rs"), "pub fn u() {}\n").unwrap();
    // CRLF on disk, LF in the repository: what `core.autocrlf` (Git for Windows' default) leaves.
    std::fs::write(root.join("src/crlf.rs"), "pub fn x() {}\r\npub fn y() {}\r\n").unwrap();
    std::fs::write(root.join("src/same.rs"), "pub fn s() {}\r\n").unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=T", "-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    // Checkouts keep LF, whatever the machine's settings (Git for Windows' say CRLF).
    git(&["config", "core.autocrlf", "false"]);
    git(&["config", "core.eol", "lf"]);
    git(&["-c", "core.autocrlf=true", "-c", "core.safecrlf=false", "add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![root.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _ = crate::app::build_router(state.clone());
    let pid = state.projects.list()[0].id.clone();
    crate::files::watch::sync_all(&state).await;
    let env = Env { state, pid, root: root.clone(), _dirs: [cfg, data, proj] };

    // An agent edits the file nobody opened: its hook names it.
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    let hook = json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": root.join("src/lib.rs").display().to_string() } });
    super::agent_edit(&env.state, &session("t-agent", Some(&env.pid), &root), &hook);
    let h = env.wait_for("src/lib.rs", 2).await;
    assert_eq!(kinds(&h), ["agent", "base"]);
    assert_eq!(h[1]["label"], "Last commit (HEAD)");
    let d = env.api(Method::GET, &format!("/files/history/diff?id={}", h[0]["id"]), None).await.unwrap();
    assert!(d["diff"].as_str().unwrap().contains("+pub fn b() {}"), "{d}");
    // Nothing more once the file has history.
    std::fs::write(root.join("src/lib.rs"), "pub fn c() {}\n").unwrap();
    let h = env.wait_for("src/lib.rs", 3).await;
    assert_eq!(kinds(&h), ["disk", "agent", "base"]);

    // With LF checkouts, a file rewritten with CRLF keeps HEAD as committed.
    std::fs::write(root.join("src/unix.rs"), "pub fn u() {}\r\n").unwrap();
    let h = env.wait_for("src/unix.rs", 2).await;
    assert_eq!(kinds(&h), ["disk", "base"]);
    let base = env.api(Method::GET, &format!("/files/history/revision?id={}", h[1]["id"]), None).await.unwrap();
    assert_eq!(base["content"], "pub fn u() {}\n");

    // A CRLF checkout keeps HEAD with its line ends on Windows: the diff shows the line
    // added… Elsewhere HEAD is kept as committed, as it always was.
    let crlf = crate::util::os::fs::NATIVE_CRLF;
    git(&["config", "core.autocrlf", "true"]);
    std::fs::write(root.join("src/crlf.rs"), "pub fn x() {}\r\npub fn y() {}\r\npub fn z() {}\r\n").unwrap();
    let hook = json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": root.join("src").join("crlf.rs").display().to_string() } });
    super::agent_edit(&env.state, &session("t-agent", Some(&env.pid), &root), &hook);
    let h = env.wait_for("src/crlf.rs", 2).await;
    assert_eq!(kinds(&h), ["agent", "base"]);
    let base = env.api(Method::GET, &format!("/files/history/revision?id={}", h[1]["id"]), None).await.unwrap();
    assert_eq!(base["content"], if crlf { "pub fn x() {}\r\npub fn y() {}\r\n" } else { "pub fn x() {}\npub fn y() {}\n" });
    let d = env.api(Method::GET, &format!("/files/history/diff?id={}", h[0]["id"]), None).await.unwrap();
    let diff = d["diff"].as_str().unwrap();
    assert!(diff.contains("+pub fn z() {}\r\n") && diff.contains("-pub fn x() {}") != crlf, "{diff}");
    // …and a file that only went through the checkout has no older version to keep.
    std::fs::write(root.join("src/same.rs"), "pub fn s() {}\r\n").unwrap();
    env.wait_for("src/same.rs", 1).await;
    env.settle().await;
    assert_eq!(kinds(&env.history("src/same.rs").await), if crlf { &["disk"][..] } else { &["disk", "base"][..] });
}

/// Git's rules for the line ends of a checkout (`convert.c`), from its settings and the
/// file's attributes.
#[test]
fn head_takes_the_line_ends_of_the_checkout() {
    use super::{Eol, EolAttrs, EolConfig, to_worktree};
    let config = |out: &str| EolConfig::parse(out, false);
    let attrs = |text: &str, eol: &str| EolAttrs { text: text.into(), eol: eol.into(), crlf: "unspecified".into() };
    let none = attrs("unspecified", "unspecified");

    // Settings: the last value wins; a key alone is true.
    assert_eq!(config(""), EolConfig { autocrlf: None, eol_crlf: false });
    assert_eq!(EolConfig::parse("", true), EolConfig { autocrlf: None, eol_crlf: true });
    assert_eq!(config("core.autocrlf\nTrue\0"), EolConfig { autocrlf: Some(true), eol_crlf: false });
    assert_eq!(config("core.autocrlf\0").autocrlf, Some(true));
    assert_eq!(config("core.autocrlf\ntrue\0core.autocrlf\ninput\0").autocrlf, Some(false));
    assert_eq!(config("core.autocrlf\nfalse\0core.eol\ncrlf\0"), EolConfig { autocrlf: None, eol_crlf: true });
    assert!(!EolConfig::parse("core.eol\nlf\0", true).eol_crlf);

    // A stock Linux repository: as committed, whatever the attributes short of eol=crlf.
    let stock = config("");
    assert_eq!(stock.eol(&none), Eol::AsCommitted);
    assert_eq!(stock.eol(&attrs("set", "unspecified")), Eol::AsCommitted);
    assert_eq!(stock.eol(&attrs("auto", "unspecified")), Eol::AsCommitted);
    assert_eq!(stock.eol(&attrs("unspecified", "crlf")), Eol::Crlf);
    assert_eq!(stock.eol(&attrs("auto", "crlf")), Eol::AutoCrlf);
    assert_eq!(stock.eol(&attrs("unset", "crlf")), Eol::AsCommitted);
    // core.autocrlf=true (Git for Windows): CRLF unless the attributes say otherwise.
    let autocrlf = config("core.autocrlf\ntrue\0");
    assert_eq!(autocrlf.eol(&none), Eol::AutoCrlf);
    assert_eq!(autocrlf.eol(&attrs("set", "unspecified")), Eol::Crlf);
    assert_eq!(autocrlf.eol(&attrs("unset", "unspecified")), Eol::AsCommitted);
    assert_eq!(autocrlf.eol(&attrs("unspecified", "lf")), Eol::AsCommitted);
    assert_eq!(config("core.autocrlf\ninput\0").eol(&none), Eol::AsCommitted);
    // No autocrlf where the native line end is CRLF: text files get it.
    let native = EolConfig::parse("", true);
    assert_eq!(native.eol(&none), Eol::AsCommitted);
    assert_eq!(native.eol(&attrs("auto", "unspecified")), Eol::AutoCrlf);
    // The older `crlf` attribute counts when `text` is not given.
    let legacy = EolAttrs { text: "unspecified".into(), eol: "unspecified".into(), crlf: "unset".into() };
    assert_eq!(autocrlf.eol(&legacy), Eol::AsCommitted);

    let parsed = EolAttrs::parse(b"a b.rs\0text\0auto\0a b.rs\0eol\0crlf\0a b.rs\0crlf\0unspecified\0");
    assert_eq!((parsed.text.as_str(), parsed.eol.as_str(), parsed.crlf.as_str()), ("auto", "crlf", "unspecified"));

    // Lone LFs become CRLF; auto leaves a file that has a CR, or a NUL, alone.
    assert_eq!(&*to_worktree(b"a\nb\n", Eol::Crlf), b"a\r\nb\r\n");
    assert_eq!(&*to_worktree(b"a\r\nb\n", Eol::Crlf), b"a\r\nb\r\n");
    assert_eq!(&*to_worktree(b"a\nb", Eol::AutoCrlf), b"a\r\nb");
    assert_eq!(&*to_worktree(b"a\r\nb\n", Eol::AutoCrlf), b"a\r\nb\n");
    assert_eq!(&*to_worktree(b"a\0\nb\n", Eol::AutoCrlf), b"a\0\nb\n");
    assert_eq!(&*to_worktree(b"a\nb\n", Eol::AsCommitted), b"a\nb\n");
}

/// Git operations that rewrite the working tree get a label first.
#[tokio::test(flavor = "multi_thread")]
async fn git_ops_put_automatic_labels() {
    let env = setup().await;
    super::start(&env.state);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let ev = |op: &str, line: &str| json!({ "opId": "op1", "op": op, "line": line });
    env.state.events.emit("git.op", Some(&env.pid), ev("fetch", "$ git fetch --progress"));
    env.state.events.emit("git.op", Some(&env.pid), ev("pull", "$ git pull --ff-only"));
    env.state.events.emit("git.op", Some(&env.pid), ev("pull", "Updating 123..456"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v = env.api(Method::GET, "/files/history/dir?path=", None).await.unwrap();
        let labels: Vec<_> = v["entries"].as_array().unwrap().iter().filter(|e| e["kind"] == "auto").map(|e| e["label"].clone()).collect();
        if !labels.is_empty() || tokio::time::Instant::now() > deadline {
            assert_eq!(labels, [json!("Before git pull")]);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = Kind::Auto;
}

/// Files written into a folder that did not exist (`mkdir -p x && cat > x/y`, an
/// agent's apply_patch) are recorded although the watcher only saw the folder, and
/// their deletion with the folder is recorded too.
#[tokio::test(flavor = "multi_thread")]
async fn files_in_a_new_folder_are_recorded() {
    let env = setup().await;
    let r = &env.root;
    std::fs::create_dir_all(r.join("newmod/deep")).unwrap();
    std::fs::write(r.join("newmod/mod.rs"), "pub mod deep;\n").unwrap();
    std::fs::write(r.join("newmod/deep/a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(r.join("newmod/.env"), "TOKEN=x\n").unwrap();
    std::fs::write(r.join("newmod/data.log"), "ignored\n").unwrap();
    std::fs::create_dir_all(r.join("newmod/node_modules/p")).unwrap();
    std::fs::write(r.join("newmod/node_modules/p/i.js"), "x\n").unwrap();
    let h = env.wait_for("newmod/mod.rs", 1).await;
    assert_eq!(kinds(&h), ["disk"], "{h:?}");
    assert_eq!(kinds(&env.wait_for("newmod/deep/a.rs", 1).await), ["disk"]);
    env.settle().await;
    let all = env.api(Method::GET, "/files/history/dir?path=newmod", None).await.unwrap();
    let mut paths: Vec<_> = all["entries"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap().to_string()).collect();
    paths.sort();
    assert_eq!(paths, ["newmod/deep/a.rs", "newmod/mod.rs"]);

    std::fs::remove_dir_all(r.join("newmod")).unwrap();
    let h = env.wait_for("newmod/mod.rs", 2).await;
    assert_eq!(kinds(&h), ["deleted", "disk"]);
    assert_eq!(kinds(&env.wait_for("newmod/deep/a.rs", 2).await), ["deleted", "disk"]);
}

/// Linux: `a\b.txt` is one file, not `b.txt` in the folder `a` (which exists too). A change
/// on disk, a save, an agent's edit and the MCP tool keep its history under its own name
/// and with its own content, and the other file keeps only its own version.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_backslash_in_a_name_is_one_file() {
    const NAME: &str = r"a\b.txt";
    const QUERY: &str = "a%5Cb.txt";
    let env = setup().await;
    std::fs::create_dir_all(env.root.join("a")).unwrap();
    std::fs::write(env.root.join("a/b.txt"), "other\n").unwrap();
    assert_eq!(env.wait_for("a/b.txt", 1).await.len(), 1);

    std::fs::write(env.root.join(NAME), "v1\n").unwrap();
    let h = env.wait_for(QUERY, 1).await;
    assert_eq!(kinds(&h), ["disk"], "{h:?}");
    assert_eq!(h[0]["path"], NAME);
    let read = env.api(Method::GET, &format!("/files/read?path={QUERY}"), None).await.unwrap();
    assert_eq!(read["content"], "v1\n");
    env.api(Method::PUT, "/files/write", Some(json!({ "path": NAME, "content": "v2\n", "etag": read["etag"] }))).await.unwrap();
    assert_eq!(kinds(&env.wait_for(QUERY, 2).await), ["save", "disk"]);

    let abs = env.root.join(NAME).display().to_string();
    std::fs::write(env.root.join(NAME), "v3\n").unwrap();
    let hook = json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": abs } });
    super::agent_edit(&env.state, &session("t-agent", Some(&env.pid), &env.root), &hook);
    env.wait_for(QUERY, 3).await;
    env.settle().await;
    let h = env.history(QUERY).await;
    assert_eq!(kinds(&h), ["agent", "save", "disk"], "{h:?}");
    let rev = env.api(Method::GET, &format!("/files/history/revision?id={}", h[0]["id"]), None).await.unwrap();
    assert_eq!(rev["content"], "v3\n");

    let tools = super::tools::tools();
    let t = tools.iter().find(|t| t.name == "files_local_history").unwrap();
    let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(env.pid.clone()) };
    let ToolOutput::Json(v) = (t.handler)(env.state.clone(), ctx, json!({ "path": abs })).await.unwrap() else { panic!("json expected") };
    assert_eq!((v["path"].as_str(), v["entries"].as_array().map(Vec::len)), (Some(NAME), Some(3)), "{v}");

    let other = env.history("a/b.txt").await;
    assert_eq!(other.len(), 1, "{other:?}");
    let rev = env.api(Method::GET, &format!("/files/history/revision?id={}", other[0]["id"]), None).await.unwrap();
    assert_eq!(rev["content"], "other\n");
}

/// Linux: a new folder named `n\d` is looked into under its own name. The file written
/// into it before its watch existed is recorded as `n\d/f.rs`, and `n/d/f.rs` (there
/// before the watcher, never changed) gets no version from that look.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_new_folder_with_a_backslash_is_looked_into_by_its_own_name() {
    const NAME: &str = r"n\d/f.rs";
    const QUERY: &str = "n%5Cd/f.rs";
    let env = setup_with(|root| {
        std::fs::create_dir_all(root.join("n/d")).unwrap();
        std::fs::write(root.join("n/d/f.rs"), "pub fn other() {}\n").unwrap();
    })
    .await;
    std::fs::create_dir_all(env.root.join(r"n\d")).unwrap();
    std::fs::write(env.root.join(NAME), "pub fn f() {}\n").unwrap();
    let h = env.wait_for(QUERY, 1).await;
    assert_eq!(kinds(&h), ["disk"], "{h:?}");
    assert_eq!(h[0]["path"], NAME);
    let rev = env.api(Method::GET, &format!("/files/history/revision?id={}", h[0]["id"]), None).await.unwrap();
    assert_eq!(rev["content"], "pub fn f() {}\n");
    env.settle().await;
    let other = env.history("n/d/f.rs").await;
    assert!(other.is_empty(), "{other:?}");

    // The folder's own watch sees the file go.
    std::fs::remove_dir_all(env.root.join(r"n\d")).unwrap();
    assert_eq!(kinds(&env.wait_for(QUERY, 2).await), ["deleted", "disk"]);
    env.settle().await;
    let other = env.history("n/d/f.rs").await;
    assert!(other.is_empty(), "{other:?}");
}

/// A folder with more files than one batch takes (a clone, an unpacked archive) is
/// not snapshotted file by file.
#[test]
fn new_folders_stay_within_the_batch_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let root = crate::util::os::path::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir_all(root.join("small/sub")).unwrap();
    std::fs::write(root.join("small/a.txt"), "a").unwrap();
    std::fs::write(root.join("small/sub/b.txt"), "b").unwrap();
    std::fs::create_dir_all(root.join("big")).unwrap();
    for i in 0..(super::MAX_BATCH + 1) {
        std::fs::write(root.join(format!("big/f{i}.txt")), "x").unwrap();
    }
    // A link is never followed (skipped where Windows does not allow creating one).
    crate::files::symlink_or_skip("/etc", root.join("small/etc"));
    let file: crate::config::ProjectFile = toml::from_str("schema = 1\n[project]\nid = \"p\"\nname = \"p\"\nroot = \".\"\n").unwrap();
    let project = crate::projects::Project {
        id: "p".into(),
        name: "p".into(),
        root: root.clone(),
        config: file,
        remote: None,
        warnings: vec![],
        repo_secret_names: Default::default(),
        overlay_error: None,
    };
    let mut paths = vec!["small".to_string(), "small/a.txt".to_string()];
    super::files_in_new_dirs(&project, &["big".into(), "small".into(), "gone".into()], &mut paths);
    paths.sort();
    assert_eq!(paths, ["small", "small/a.txt", "small/sub/b.txt"]);
}

/// A session's hook only touches its own project's history.
#[tokio::test(flavor = "multi_thread")]
async fn agent_hooks_are_confined_to_the_session_project() {
    let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let base = crate::util::os::path::canonicalize(proj.path()).unwrap();
    let (mine, other) = (base.join("mine"), base.join("other"));
    for r in [&mine, &other] {
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(r.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    }
    let mut config = GlobalConfig::default();
    config.projects.roots = vec![];
    config.projects.include = vec![mine.display().to_string(), other.display().to_string()];
    config.notify.desktop = false;
    let paths = Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
    let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let _ = crate::app::build_router(state.clone());
    let id_of = |root: &std::path::Path| state.projects.list().iter().find(|p| p.root == root).unwrap().id.clone();
    let (mine_id, other_id) = (id_of(&mine), id_of(&other));
    let env = Env { state, pid: other_id.clone(), root: other.clone(), _dirs: [cfg, data, proj] };

    let hook = |file: &std::path::Path| json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": file.display().to_string() } });
    // A session of `mine` names the other project's file (absolute, or climbing out).
    let s = session("t-mine", Some(&mine_id), &mine);
    super::agent_edit(&env.state, &s, &hook(&other.join("src/lib.rs")));
    super::agent_edit(&env.state, &s, &json!({ "hook_event_name": "PostToolUse", "tool_name": "Edit", "tool_input": { "file_path": "../other/src/lib.rs" } }));
    // A session without a project names it.
    super::agent_edit(&env.state, &session("t-home", None, &base), &hook(&other.join("src/lib.rs")));
    env.settle().await;
    assert!(env.history("src/lib.rs").await.is_empty());

    // Its own file, by absolute and by relative path, is attributed.
    super::agent_edit(&env.state, &s, &hook(&mine.join("src/lib.rs")));
    let v = env.state.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let h = loop {
        let out = call_api(&v, Method::GET, &format!("/api/projects/{mine_id}/files/history?path=src/lib.rs"), None, &McpCtx::default()).await.unwrap();
        let h = out["entries"].as_array().cloned().unwrap_or_default();
        if !h.is_empty() || tokio::time::Instant::now() > deadline {
            break h;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(kinds(&h), ["agent"]);
    assert_eq!(h[0]["by"], "t-mine");
    assert!(env.history("src/lib.rs").await.is_empty());
}

/// Container paths map through the workspace mount, whatever the workspace folder
/// (`/` for compose, or a folder below the mount). POSIX paths on both sides: dev
/// containers are not supported on Windows.
#[cfg(unix)]
#[test]
fn container_paths_map_through_the_workspace_mount() {
    use std::path::{Path, PathBuf};
    let mount = (PathBuf::from("/home/u/repo"), "/workspaces/repo".to_string());
    let cwd = Path::new("/home/u/repo/app");
    let map = |f: &str| super::host_file(f, cwd, Some(Some(&mount)));
    assert_eq!(map("/workspaces/repo/app/package.json"), Some(PathBuf::from("/home/u/repo/app/package.json")));
    assert_eq!(map("/workspaces/repo/src/main.rs"), Some(PathBuf::from("/home/u/repo/src/main.rs")));
    assert_eq!(map("/workspaces/repo"), Some(PathBuf::from("/home/u/repo")));
    // Outside the mount: not guessed at (not even when a host path looks the same).
    assert_eq!(map("/workspaces/repo2/x"), None);
    assert_eq!(map("/home/u/repo/src/main.rs"), None);
    assert_eq!(map("/etc/passwd"), None);
    // Relative: the session's working directory.
    assert_eq!(map("src/x.rs"), Some(PathBuf::from("/home/u/repo/app/src/x.rs")));
    // The mount unknown: nothing for absolute container paths.
    assert_eq!(super::host_file("/workspaces/repo/a", cwd, Some(None)), None);
    // A host session: as named.
    assert_eq!(super::host_file("/home/u/repo/a", cwd, None), Some(PathBuf::from("/home/u/repo/a")));
}

/// A host path an agent names lands in its project, never outside it.
#[test]
fn agent_paths_stay_inside_the_project() {
    use std::path::Path;
    let root = tempfile::tempdir().unwrap();
    let root = crate::util::os::path::canonicalize(root.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    assert_eq!(super::rel_in_project(&root, &root.join("src/a.rs")).as_deref(), Some("src/a.rs"));
    assert_eq!(super::rel_in_project(&root, &root.join("src").join("a.rs")).as_deref(), Some("src/a.rs"));
    assert_eq!(super::rel_in_project(&root, &root.join("src/../../x")), None);
    assert_eq!(super::rel_in_project(&root, &root), None);
    assert_eq!(super::rel_in_project(&root, Path::new("/etc/passwd")), None);
    // Linux keeps the name as written: another case is another file, and a `\` is part
    // of a name.
    #[cfg(unix)]
    {
        assert_eq!(super::rel_in_project(&root, &root.join("SRC/a.rs")).as_deref(), Some("SRC/a.rs"));
        assert_eq!(super::rel_in_project(&root, &root.join(r"src\a.rs")).as_deref(), Some(r"src\a.rs"));
        assert_eq!(super::rel_in_project(&root, &root.join(r"..\x")).as_deref(), Some(r"..\x"));
    }
    // Windows: the root spelled in another case is the same folder.
    #[cfg(windows)]
    {
        let lower = std::path::PathBuf::from(root.display().to_string().to_ascii_lowercase());
        assert_eq!(super::rel_in_project(&root, &lower.join("src").join("a.rs")).as_deref(), Some("src/a.rs"));
        assert_eq!(super::rel_in_project(&root, Path::new(r"C:\Windows\win.ini")), None);
        // So is a file: its key is the case on disk, one history whatever case the agent wrote.
        std::fs::create_dir(root.join("src").join("Deep")).unwrap();
        std::fs::write(root.join("src").join("Deep").join("Main.rs"), "fn main() {}\n").unwrap();
        for spelled in [root.join("SRC").join("deep").join("MAIN.RS"), lower.join("src").join("deep").join("main.rs"), root.join(r"src\Deep/main.RS")] {
            assert_eq!(super::rel_in_project(&root, &spelled).as_deref(), Some("src/Deep/Main.rs"), "{}", spelled.display());
        }
        // A deleted one: its folder as on disk, its name as written.
        assert_eq!(super::rel_in_project(&root, &root.join("SRC").join("DEEP").join("Gone.rs")).as_deref(), Some("src/Deep/Gone.rs"));
    }
}

/// Review Changes: the files a session edited, what they were before its first
/// edit, and whether someone changed them after it.
#[tokio::test(flavor = "multi_thread")]
async fn a_sessions_changes_are_listed_per_file() {
    let env = setup().await;
    env.api(Method::GET, "/files/read?path=src/main.rs", None).await.unwrap();
    env.wait_for("src/main.rs", 1).await;
    let edit = |rel: &str| json!({ "hook_event_name": "PostToolUse", "tool_name": "Write", "tool_input": { "file_path": env.root.join(rel).display().to_string() } });
    let a = session("t-a", Some(&env.pid), &env.root);
    // Session A edits main.rs twice and creates notes.md; session B edits main.rs later.
    std::fs::write(env.root.join("src/main.rs"), "fn main() { a1(); }\n").unwrap();
    super::agent_edit(&env.state, &a, &edit("src/main.rs"));
    env.wait_for("src/main.rs", 2).await;
    std::fs::write(env.root.join("src/main.rs"), "fn main() { a2(); }\n").unwrap();
    super::agent_edit(&env.state, &a, &edit("src/main.rs"));
    env.wait_for("src/main.rs", 3).await;
    std::fs::write(env.root.join("src/notes.md"), "# notes\n").unwrap();
    super::agent_edit(&env.state, &a, &edit("src/notes.md"));
    env.wait_for("src/notes.md", 1).await;
    std::fs::write(env.root.join("src/main.rs"), "fn main() { b(); }\n").unwrap();
    super::agent_edit(&env.state, &session("t-b", Some(&env.pid), &env.root), &edit("src/main.rs"));
    env.wait_for("src/main.rs", 4).await;

    let v = env.api(Method::GET, "/files/history/session?by=t-a", None).await.unwrap();
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.iter().map(|f| f["path"].as_str().unwrap()).collect::<Vec<_>>(), ["src/main.rs", "src/notes.md"]);
    assert_eq!(v["who"], "Session t-a");
    let main = &files[0];
    assert_eq!(main["edits"], 2);
    assert_eq!(main["before"]["kind"], "base", "the version the session started from");
    assert_eq!(main["changedSince"], true, "session B edited it after");
    let before = env.api(Method::GET, &format!("/files/history/revision?id={}", main["before"]["id"]), None).await.unwrap();
    assert_eq!(before["content"], "fn main() {}\n");
    let notes = &files[1];
    assert!(notes["before"].is_null(), "a file the session created");
    assert_eq!(notes["changedSince"], false);

    let b = env.api(Method::GET, "/files/history/session?by=t-b", None).await.unwrap();
    assert_eq!(b["files"].as_array().unwrap().len(), 1);
    assert_eq!(b["files"][0]["before"]["by"], "t-a", "B started from A's last version");
    let none = env.api(Method::GET, "/files/history/session?by=t-none", None).await.unwrap();
    assert!(none["files"].as_array().unwrap().is_empty());
    assert!(env.api(Method::GET, "/files/history/session?by=", None).await.is_err());
}

/// A file deleted before a session writes it again counts as created by that session.
#[test]
fn a_file_deleted_before_the_session_has_no_earlier_version() {
    use super::store::{NewRevision, Store};
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.record(NewRevision::content("a.md", Kind::Disk, 1, b"one\n")).unwrap();
    store.record(NewRevision { path: "a.md", kind: Kind::Deleted, ts: 2, content: None, label: None, by: None, who: None }).unwrap();
    let mut r = NewRevision::content("a.md", Kind::Agent, 3, b"two\n");
    r.by = Some("t".into());
    store.record(r).unwrap();
    let files = store.session_files("t");
    assert_eq!(files.len(), 1);
    assert!(files[0].before.is_none());
    assert_eq!(files[0].edits, 1);
    assert!(store.session_files("other").is_empty());
}
