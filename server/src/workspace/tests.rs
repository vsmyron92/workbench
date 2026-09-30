//! The slice against a real AppState and router: REST, MCP tool handlers, the
//! sandboxed `/view` server and Mr. Mak registry compatibility. Temp dirs only.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::app::AppState;
use crate::config::{GlobalConfig, Paths};
use crate::mcp::{McpCtx, ToolOutput};

const MRMAK_REGISTRY: &str = r#"{
  "entities": [
    {
      "id": "my-dream-game",
      "title": "My Dream Game",
      "description": "A sample.",
      "type": "group",
      "category": "project",
      "created": "2026-09-15",
      "updated": "2026-09-15",
      "folder": "2026-09-15_my-dream-game",
      "steps": [ { "name": "Game", "path": "index.html" } ],
      "status": "active",
      "pinned": true,
      "sample": true,
      "defaultStep": 0
    },
    {
      "id": "escape",
      "title": "Hostile",
      "folder": "../../etc",
      "steps": [ { "name": "x", "path": "passwd" } ],
      "status": "active"
    }
  ]
}
"#;

struct Env {
    state: AppState,
    router: axum::Router,
    project: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

async fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    let ws = project.join("workspace");
    std::fs::create_dir_all(ws.join("2026-09-15_my-dream-game")).unwrap();
    std::fs::create_dir_all(ws.join("_shared")).unwrap();
    std::fs::write(ws.join("workspace.json"), MRMAK_REGISTRY).unwrap();
    std::fs::write(ws.join("2026-09-15_my-dream-game/index.html"), "<!doctype html><html><head><link href=\"../_shared/report.css\" rel=\"stylesheet\"></head><body><img src=\"hero.png\"></body></html>").unwrap();
    std::fs::write(ws.join("2026-09-15_my-dream-game/hero.png"), b"\x89PNG0123456789").unwrap();
    std::fs::write(ws.join("2026-09-15_my-dream-game/.env"), "SECRET=1").unwrap();
    std::fs::write(ws.join("_shared/report.css"), "body{}").unwrap();
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(project.join("docs/notes.md"), "# Notes\n![chart](chart.png)\n").unwrap();
    std::fs::write(project.join(".env"), "SECRET=1").unwrap();

    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![project.display().to_string()];
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let router = crate::app::build_router(state.clone());
    assert!(state.projects.get("proj").is_some());
    Env { state, router, project, _dir: dir }
}

impl Env {
    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut req = Request::builder().method(method).uri(uri).header("host", "127.0.0.1");
        if uri.starts_with("/api/") {
            req = req.header("authorization", format!("Bearer {}", self.state.auth.master_token()));
        }
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap().to_vec();
        (status, headers, bytes)
    }

    async fn json(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, _, b) = self.call(method, uri, body).await;
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    async fn tool(&self, name: &str, ctx: &McpCtx, args: Value) -> Result<Value, crate::error::ApiError> {
        let t = super::mcp_tools().into_iter().find(|t| t.name == name).unwrap();
        match (t.handler)(self.state.clone(), ctx.clone(), args).await? {
            ToolOutput::Json(v) => Ok(v),
            ToolOutput::Text(s) => Ok(Value::String(s)),
        }
    }
}

fn session() -> McpCtx {
    McpCtx { terminal_id: Some("t1".into()), project_id: Some("proj".into()) }
}

#[tokio::test]
async fn agents_create_cards_write_files_and_add_steps() {
    let env = setup().await;
    let mut events = env.state.events.subscribe();
    let created = env
        .tool("workspace_create_card", &session(), json!({ "title": "Load report", "description": "Peak loads", "category": "Analytics" }))
        .await
        .unwrap();
    assert_eq!(created["cardId"], "load-report");
    assert_eq!(created["scope"], "proj", "a session's cards go to its own project");
    let folder = std::path::PathBuf::from(created["folder"].as_str().unwrap());
    assert!(folder.is_dir() && folder.starts_with(env.state.paths.data_dir.join("workspace/proj")));
    let ev = events.recv().await.unwrap();
    assert_eq!(ev.kind, "workspace.changed");
    assert_eq!(ev.data["scope"], "proj");
    assert_eq!(ev.project_id.as_deref(), Some("proj"));

    let html = "<!DOCTYPE html><html data-wb-report=\"document\"><head><link rel=\"stylesheet\" href=\"../_shared/report.css\"></head><body><h1>Loads</h1><img src=\"img/chart.png\" alt=\"chart\"></body></html>";
    env.tool("workspace_write_file", &session(), json!({ "cardId": "load-report", "path": "report.html", "content": html })).await.unwrap();
    env.tool("workspace_write_file", &session(), json!({ "cardId": "load-report", "path": "img/chart.png", "content": "iVBORw0KGgo=", "encoding": "base64" }))
        .await
        .unwrap();
    assert!(env.tool("workspace_write_file", &session(), json!({ "cardId": "load-report", "path": "../x.html", "content": "x" })).await.is_err());
    assert!(env.tool("workspace_write_file", &session(), json!({ "cardId": "load-report", "path": ".env", "content": "x" })).await.is_err());

    let step = env.tool("workspace_add_step", &session(), json!({ "cardId": "load-report", "name": "Report", "path": "report.html" })).await.unwrap();
    assert_eq!(step["step"]["kind"], "html");
    assert_eq!(step["step"]["exists"], true);
    // An absolute path in the project is copied into the card.
    let notes = env.project.join("docs/notes.md").display().to_string();
    let step = env.tool("workspace_add_step", &session(), json!({ "cardId": "load-report", "name": "Notes", "path": notes })).await.unwrap();
    assert_eq!(step["step"]["path"], "notes.md");
    assert!(folder.join("notes.md").is_file());
    // …but not the project's secrets, nor anything outside it.
    let env_file = env.project.join(".env").display().to_string();
    assert!(env.tool("workspace_add_step", &session(), json!({ "cardId": "load-report", "name": "x", "path": env_file })).await.is_err());
    assert!(env.tool("workspace_add_step", &session(), json!({ "cardId": "load-report", "name": "x", "path": "/etc/hostname" })).await.is_err());
    let missing = env.tool("workspace_add_step", &session(), json!({ "cardId": "load-report", "name": "Later", "path": "later.md" })).await.unwrap();
    assert!(missing["warning"].as_str().unwrap().contains("does not exist"));

    // Confinement: another project's session cannot touch this scope.
    let other = McpCtx { terminal_id: Some("t2".into()), project_id: Some("other".into()) };
    let err = env.tool("workspace_create_card", &other, json!({ "title": "x", "description": "", "scope": "proj" })).await.unwrap_err();
    assert_eq!(err.code, "forbidden");
    // Home is everyone's.
    let home = env.tool("workspace_create_card", &other, json!({ "title": "Scratch", "description": "", "scope": "home" })).await.unwrap();
    assert_eq!(home["scope"], "home");

    let listed = env.tool("workspace_list_cards", &session(), json!({})).await.unwrap();
    let ids: Vec<&str> = listed["cards"].as_array().unwrap().iter().map(|c| c["cardId"].as_str().unwrap()).collect();
    assert_eq!(ids, ["repo:my-dream-game", "load-report"], "pinned first; the hostile entry is skipped");
    assert!(listed["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("unusable folder")));

    let updated = env.tool("workspace_update_card", &session(), json!({ "cardId": "load-report", "status": "done", "pinned": true })).await.unwrap();
    assert_eq!(updated["status"], "done");
    let opened = env.tool("workspace_open_card", &session(), json!({ "cardId": "load-report", "step": 1 })).await.unwrap();
    assert!(opened.as_str().unwrap().contains("Load report"));
}

#[tokio::test]
async fn rest_card_lifecycle() {
    let env = setup().await;
    let (s, scopes) = env.json(Method::GET, "/api/workspace/scopes", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(scopes[0]["id"], "home");
    let proj = scopes.as_array().unwrap().iter().find(|x| x["id"] == "proj").unwrap();
    assert_eq!(proj["repoCards"], 1);
    assert_eq!(proj["hasRepoRegistry"], true);

    let (s, card) = env.json(Method::POST, "/api/workspace/home/cards", Some(json!({ "title": "Ideas", "description": "d" }))).await;
    assert_eq!(s, StatusCode::OK, "{card}");
    assert_eq!(card["id"], "ideas");
    assert_eq!(card["origin"], "workbench");
    assert!(card["base"].as_str().unwrap().starts_with("/view/"));
    let (s, _) = env.json(Method::POST, "/api/workspace/nope/cards", Some(json!({ "title": "x" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Markdown: create via content PUT (no revision = new file), then edit with conflicts.
    let (s, w) = env.json(Method::PUT, "/api/workspace/home/cards/ideas/content", Some(json!({ "path": "plan.md", "text": "# Plan" }))).await;
    assert_eq!(s, StatusCode::OK, "{w}");
    let (s, card) = env.json(Method::POST, "/api/workspace/home/cards/ideas/steps", Some(json!({ "name": "Plan", "path": "plan.md" }))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(card["steps"][0]["kind"], "markdown");
    let (_, c) = env.json(Method::GET, "/api/workspace/home/cards/ideas/content?path=plan.md", None).await;
    let rev = c["revision"].as_str().unwrap().to_string();
    assert_eq!(c["editable"], true);
    let (s, _) = env.json(Method::PUT, "/api/workspace/home/cards/ideas/content", Some(json!({ "path": "plan.md", "text": "# Plan 2", "revision": rev }))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, e) = env.json(Method::PUT, "/api/workspace/home/cards/ideas/content", Some(json!({ "path": "plan.md", "text": "# Plan 3", "revision": rev }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(e["error"]["code"], "conflict");

    // Upload with a step, then list the folder.
    let token = env.state.auth.master_token().to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/workspace/home/cards/ideas/upload?name=shot.png&step=true")
        .header("host", "127.0.0.1")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(vec![1u8; 5000]))
        .unwrap();
    let resp = env.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (_, files) = env.json(Method::GET, "/api/workspace/home/cards/ideas/files", None).await;
    let names: Vec<&str> = files["entries"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["plan.md", "shot.png"]);
    let (_, card) = env.json(Method::GET, "/api/workspace/home/cards/ideas", None).await;
    assert_eq!(card["steps"][1]["kind"], "image");
    assert_eq!(card["thumb"], "shot.png");

    // Reorder, default step, pin, then a stale step delete.
    let (s, card) = env.json(Method::PATCH, "/api/workspace/home/cards/ideas", Some(json!({ "defaultStep": 0, "pinned": true, "category": "Road Map" }))).await;
    assert_eq!(s, StatusCode::OK, "{card}");
    assert_eq!((card["defaultIndex"].as_u64(), card["category"].as_str()), (Some(0), Some("road-map")));
    let (s, card) = env.json(Method::PATCH, "/api/workspace/home/cards/ideas/steps/0", Some(json!({ "position": 1, "expectPath": "plan.md" }))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(card["steps"][1]["path"], "plan.md");
    assert_eq!(card["defaultIndex"], 1, "the default follows its step");
    let (s, _) = env.json(Method::DELETE, "/api/workspace/home/cards/ideas/steps/0?path=plan.md", None).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (_, all) = env.json(Method::GET, "/api/workspace/cards", None).await;
    assert_eq!(all["cards"].as_array().unwrap().len(), 2);

    let (s, _) = env.json(Method::DELETE, "/api/workspace/home/cards/ideas", None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = env.json(Method::GET, "/api/workspace/home/cards/ideas", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // The trash is outside the watched `data_dir/workspace`.
    assert_eq!(std::fs::read_dir(env.state.paths.data_dir.join("workspace-trash/home")).unwrap().count(), 1);
    assert!(!env.state.paths.data_dir.join("workspace/.trash").exists());
}

#[tokio::test]
async fn cards_carry_when_their_grant_expires() {
    let env = setup().await;
    let (_, card) = env.json(Method::GET, "/api/workspace/proj/cards/repo:my-dream-game", None).await;
    let exp = card["grantExpiresAt"].as_i64().unwrap();
    let left = exp - crate::util::now_ms();
    assert!(left > 3600 * 1000 && left <= super::view::GRANT_TTL_MS, "{left}");
}

/// An agent owns its card folder (Codex gets it as a writable sandbox root), so it
/// can plant a dangling symlink there. Copying a project file into the card must
/// never write through it.
#[tokio::test]
async fn imports_never_write_through_a_planted_symlink() {
    let env = setup().await;
    let created = env.tool("workspace_create_card", &session(), json!({ "title": "Planted", "description": "" })).await.unwrap();
    let folder = std::path::PathBuf::from(created["folder"].as_str().unwrap());
    let outside = env._dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    crate::util::os::fs::symlink(outside.join("notes.md"), folder.join("notes.md")).unwrap();
    crate::util::os::fs::symlink(outside.join("newdir"), folder.join("docs")).unwrap();

    let notes = env.project.join("docs/notes.md").display().to_string();
    let step = env.tool("workspace_add_step", &session(), json!({ "cardId": "planted", "name": "Notes", "path": notes })).await.unwrap();
    assert_eq!(step["step"]["path"], "notes (2).md");
    assert_eq!(step["step"]["exists"], true);
    assert!(!outside.join("notes.md").exists(), "wrote outside the card folder");
    assert!(folder.join("notes.md").symlink_metadata().unwrap().file_type().is_symlink());

    let docs = env.project.join("docs").display().to_string();
    let step = env.tool("workspace_add_step", &session(), json!({ "cardId": "planted", "name": "Docs", "path": docs })).await.unwrap();
    assert_eq!(step["step"]["path"], "docs (2)");
    assert!(folder.join("docs (2)/notes.md").is_file());
    assert!(!outside.join("newdir").exists());
}

#[tokio::test]
async fn large_folders_list_in_order_and_page() {
    let env = setup().await;
    let (_, card) = env.json(Method::POST, "/api/workspace/home/cards", Some(json!({ "title": "Big gallery" }))).await;
    let shots = std::path::PathBuf::from(card["folderPath"].as_str().unwrap()).join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    // Created newest-name-first, so directory order is not name order.
    for i in (0..2600).rev() {
        std::fs::write(shots.join(format!("img{i:05}.png")), b"png").unwrap();
    }
    std::fs::create_dir_all(shots.join("zz-sub")).unwrap();
    let (s, _) = env.json(Method::POST, "/api/workspace/home/cards/big-gallery/steps", Some(json!({ "path": "shots" }))).await;
    assert_eq!(s, StatusCode::OK);

    let (_, page) = env.json(Method::GET, "/api/workspace/home/cards/big-gallery/files?path=shots", None).await;
    let names: Vec<&str> = page["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), super::store::MAX_LIST);
    assert_eq!(&names[..3], ["zz-sub", "img00000.png", "img00001.png"], "folders first, then by name");
    assert_eq!((page["truncated"].as_bool(), page["total"].as_u64()), (Some(true), Some(2601)));

    let (_, next) = env.json(Method::GET, "/api/workspace/home/cards/big-gallery/files?path=shots&offset=2000", None).await;
    let names: Vec<&str> = next["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!((names.len(), names[0], *names.last().unwrap()), (601, "img01999.png", "img02599.png"));
    assert_eq!(next["truncated"], false);

    let (_, card) = env.json(Method::GET, "/api/workspace/home/cards/big-gallery", None).await;
    assert_eq!(card["thumb"], "shots/img00000.png", "the gallery's first image, not a random one");
}

/// A repository directory called `home` must not vanish behind the Home scope.
#[tokio::test]
async fn a_project_named_home_keeps_its_own_scope() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("x/home");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("workspace/2026-09-15_my-dream-game")).unwrap();
    std::fs::write(repo.join("workspace/workspace.json"), MRMAK_REGISTRY).unwrap();
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![repo.display().to_string()];
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let router = crate::app::build_router(state.clone());
    assert!(state.projects.get("home").is_none());
    let p = state.projects.find_by_path(&repo.canonicalize().unwrap()).unwrap();
    assert_eq!(p.id, "home-2");
    let req = Request::builder()
        .uri("/api/workspace/scopes")
        .header("host", "127.0.0.1")
        .header("authorization", format!("Bearer {}", state.auth.master_token()))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let scopes: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let mine = scopes.as_array().unwrap().iter().find(|s| s["id"] == "home-2").expect("the project's own scope");
    assert_eq!(mine["repoCards"], 1);
    let home = scopes.as_array().unwrap().iter().find(|s| s["id"] == "home").unwrap();
    assert_eq!(home["repoCards"], 0);
}

#[tokio::test]
async fn legacy_trash_moves_out_of_the_watched_tree() {
    let env = setup().await;
    let root = env.state.paths.data_dir.join("workspace");
    std::fs::create_dir_all(root.join(".trash/home/old~1")).unwrap();
    std::fs::create_dir_all(root.join(".backups/home/card")).unwrap();
    super::store::migrate_legacy_dirs(&env.state);
    assert!(!root.join(".trash").exists() && !root.join(".backups").exists());
    assert!(env.state.paths.data_dir.join("workspace-trash/home/old~1").is_dir());
    assert!(env.state.paths.data_dir.join("workspace-backups/home/card").is_dir());
}

#[tokio::test]
async fn mrmak_cards_are_merged_and_only_status_pin_are_written() {
    let env = setup().await;
    let (_, list) = env.json(Method::GET, "/api/workspace/proj/cards", None).await;
    let cards = list["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1);
    let c = &cards[0];
    assert_eq!((c["id"].as_str(), c["origin"].as_str(), c["editable"].as_bool()), (Some("repo:my-dream-game"), Some("repo"), Some(false)));
    assert_eq!(c["thumb"], "hero.png", "the report's first image");
    assert_eq!(c["archived"], false, "samples never auto-archive");

    let (s, _) = env.json(Method::PATCH, "/api/workspace/proj/cards/repo:my-dream-game", Some(json!({ "title": "Mine now" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env.json(Method::POST, "/api/workspace/proj/cards/repo:my-dream-game/steps", Some(json!({ "name": "x", "path": "hero.png" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = env.json(Method::DELETE, "/api/workspace/proj/cards/repo:my-dream-game", None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, card) = env.json(Method::PATCH, "/api/workspace/proj/cards/repo:my-dream-game", Some(json!({ "status": "done", "pinned": false }))).await;
    assert_eq!(s, StatusCode::OK, "{card}");
    let text = std::fs::read_to_string(env.project.join("workspace/workspace.json")).unwrap();
    assert!(text.contains("\"status\": \"done\"") && text.contains("\"pinned\": false"));
    assert!(text.contains("\"folder\": \"../../etc\""), "entries we cannot use are preserved");
    // Key order is Mr. Mak's, not alphabetical.
    assert!(text.find("\"id\"").unwrap() < text.find("\"title\"").unwrap());
    assert!(text.find("\"steps\"").unwrap() < text.find("\"status\"").unwrap());
}

#[tokio::test]
async fn view_serves_sandboxed_content_inside_the_grant_only() {
    let env = setup().await;
    let (_, card) = env.json(Method::GET, "/api/workspace/proj/cards/repo:my-dream-game", None).await;
    let base = card["base"].as_str().unwrap().to_string();
    let (s, grant) = env.json(Method::POST, "/api/workspace/proj/cards/repo:my-dream-game/grant", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(grant["base"], base, "grants are reused");

    let (s, h, body) = env.call(Method::GET, &format!("{base}index.html"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h["content-security-policy"], super::view::CSP);
    assert!(!super::view::CSP.contains("allow-same-origin"));
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "no-referrer");
    assert_eq!(h["cache-control"], "no-store");
    let text = String::from_utf8(body.clone()).unwrap();
    assert!(text.starts_with("<!doctype html><html><head><meta name=\"color-scheme\""), "{text}");
    assert_eq!(h["content-length"].to_str().unwrap().parse::<usize>().unwrap(), body.len());

    // Shared assets resolve from the card (../_shared/…).
    let shared = base.trim_end_matches('/').rsplit_once('/').unwrap().0.to_string() + "/_shared/report.css";
    let (s, h, _) = env.call(Method::GET, &shared, None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-type"].to_str().unwrap().starts_with("text/css"));

    // Ranges on other files.
    let req = Request::builder().uri(format!("{base}hero.png")).header("host", "127.0.0.1").header("range", "bytes=2-5").body(Body::empty()).unwrap();
    let resp = env.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resp.headers()["content-range"], "bytes 2-5/14");
    assert_eq!(resp.headers()["content-security-policy"], super::view::CSP);

    // Outside the grant, private files, bad grants: refused, still sandboxed.
    let root = base.trim_end_matches('/').rsplit_once('/').unwrap().0.to_string();
    for (path, want) in [
        (format!("{base}.env"), StatusCode::FORBIDDEN),
        (format!("{base}..%2F..%2Fworkspace.json"), StatusCode::FORBIDDEN),
        (format!("{root}/workspace.json"), StatusCode::NOT_FOUND),
        (format!("{base}missing.png"), StatusCode::NOT_FOUND),
        ("/view/not-a-grant/2026-09-15_my-dream-game/index.html".to_string(), StatusCode::NOT_FOUND),
        ("/view/".to_string(), StatusCode::NOT_FOUND),
    ] {
        let (s, h, _) = env.call(Method::GET, &path, None).await;
        assert_eq!(s, want, "{path}");
        assert_eq!(h["content-security-policy"], super::view::CSP, "{path}");
    }
    // A symlink out of the card folder is not followed. Its target is a folder of the
    // test's own that exists on every OS (`/etc` does not on Windows, where the link led
    // nowhere and the request was only not found).
    let outside = env.project.with_file_name("etc");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("hostname"), "outside-the-card").unwrap();
    let card = env.project.join("workspace/2026-09-15_my-dream-game");
    crate::util::os::fs::symlink(&outside, card.join("etc")).unwrap();
    assert_eq!(std::fs::read_to_string(card.join("etc").join("hostname")).unwrap(), "outside-the-card");
    let (s, h, body) = env.call(Method::GET, &format!("{base}etc/hostname"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(h["content-security-policy"], super::view::CSP);
    assert!(!String::from_utf8_lossy(&body).contains("outside-the-card"));
    // A link that leads nowhere serves nothing either.
    crate::util::os::fs::symlink(env.project.with_file_name("missing"), card.join("gone")).unwrap();
    let (s, _, _) = env.call(Method::GET, &format!("{base}gone/hostname"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// Delete a card, find it in the trash, restore it (files and steps back), then
/// delete it for good and empty the trash, over REST, with `workspace.trash` events.
#[tokio::test]
async fn trash_round_trip() {
    let env = setup().await;
    let (s, card) = env.json(Method::POST, "/api/workspace/proj/cards", Some(json!({ "title": "Trash me", "category": "report" }))).await;
    assert_eq!(s, StatusCode::OK, "{card}");
    let id = card["id"].as_str().unwrap().to_string();
    let folder = std::path::PathBuf::from(card["folderPath"].as_str().unwrap());
    std::fs::write(folder.join("notes.md"), "# kept\n").unwrap();
    let (s, _) = env.json(Method::POST, &format!("/api/workspace/proj/cards/{id}/steps"), Some(json!({ "name": "Notes", "path": "notes.md" }))).await;
    assert_eq!(s, StatusCode::OK);

    let mut events = env.state.events.subscribe();
    let (s, _) = env.json(Method::DELETE, &format!("/api/workspace/proj/cards/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let mut kinds = vec![];
    while let Ok(ev) = events.try_recv() {
        kinds.push(ev.kind.clone());
    }
    assert!(kinds.contains(&"workspace.trash".to_string()), "{kinds:?}");

    let (s, list) = env.json(Method::GET, "/api/workspace/proj/trash", None).await;
    assert_eq!(s, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!((items[0]["title"].as_str(), items[0]["files"].as_u64(), items[0]["restorable"].as_bool()), (Some("Trash me"), Some(1), Some(true)));
    let item = items[0]["id"].as_str().unwrap().to_string();
    let (_, all) = env.json(Method::GET, "/api/workspace/all/trash", None).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);
    let (s, _) = env.json(Method::GET, "/api/workspace/nope/trash", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, restored) = env.json(Method::POST, &format!("/api/workspace/proj/trash/{item}/restore"), None).await;
    assert_eq!(s, StatusCode::OK, "{restored}");
    assert_eq!(restored["id"], id.as_str());
    assert_eq!(restored["steps"][0]["exists"], true);
    let (_, list) = env.json(Method::GET, "/api/workspace/proj/trash", None).await;
    assert!(list["items"].as_array().unwrap().is_empty());
    let (s, _) = env.json(Method::POST, &format!("/api/workspace/proj/trash/{item}/restore"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Delete for good, one item; then empty everything.
    env.json(Method::DELETE, &format!("/api/workspace/proj/cards/{id}"), None).await;
    let (_, list) = env.json(Method::GET, "/api/workspace/proj/trash", None).await;
    let item = list["items"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = env.json(Method::DELETE, &format!("/api/workspace/proj/trash/{item}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = env.json(Method::DELETE, "/api/workspace/proj/trash/..%2F..%2Fworkspace", None).await;
    assert!(s.is_client_error());
    for title in ["One", "Two"] {
        let (_, c) = env.json(Method::POST, "/api/workspace/home/cards", Some(json!({ "title": title }))).await;
        env.json(Method::DELETE, &format!("/api/workspace/home/cards/{}", c["id"].as_str().unwrap()), None).await;
    }
    let (s, r) = env.json(Method::DELETE, "/api/workspace/all/trash", None).await;
    assert_eq!((s, r["removed"].as_u64()), (StatusCode::OK, Some(2)));
    let (_, all) = env.json(Method::GET, "/api/workspace/all/trash", None).await;
    assert!(all["items"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_first_start_puts_the_examples_into_home_once() {
    let env = setup().await;
    let home = || super::store::scope(&env.state, "home").unwrap();
    assert_eq!(super::examples::seed(&home()).unwrap(), 4);
    let (_, list) = env.json(Method::GET, "/api/workspace/home/cards", None).await;
    let cards = list["cards"].as_array().unwrap();
    let ids: Vec<&str> = cards.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["welcome-to-workbench", "workbench-tour", "hand-work-to-an-agent", "connect-your-services"]);
    assert_eq!((&cards[0]["pinned"], &cards[0]["thumb"]), (&json!(true), &json!("cover.svg")));
    for c in cards {
        assert_eq!((c["sample"].as_bool(), c["archived"].as_bool(), c["editable"].as_bool()), (Some(true), Some(false), Some(true)), "{c}");
        assert!(c["steps"].as_array().unwrap().iter().all(|s| s["exists"] == true), "{c}");
    }
    assert_eq!(cards[1]["steps"][1]["kind"], "gallery");
    assert_eq!(cards[1]["thumb"], "screens/workbench-overview.png", "the tour report's first image");

    // The report and its shared styles are served through the card's grant.
    let base = cards[1]["base"].as_str().unwrap();
    let (s, _, body) = env.call(Method::GET, &format!("{base}report.html"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains("../_shared/report.css"));
    let (s, _, _) = env.call(Method::GET, &format!("{base}screens/phone.png"), None).await;
    assert_eq!(s, StatusCode::OK);

    // Home has a registry now: deleted examples stay deleted.
    for id in ["workbench-tour", "hand-work-to-an-agent", "connect-your-services", "welcome-to-workbench"] {
        let (s, _) = env.json(Method::DELETE, &format!("/api/workspace/home/cards/{id}"), None).await;
        assert_eq!(s, StatusCode::OK);
    }
    assert_eq!(super::examples::seed(&home()).unwrap(), 0);
    let (_, list) = env.json(Method::GET, "/api/workspace/home/cards", None).await;
    assert!(list["cards"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn examples_never_join_cards_that_are_already_there() {
    let env = setup().await;
    let (s, _) = env.json(Method::POST, "/api/workspace/home/cards", Some(json!({ "title": "Mine" }))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(super::examples::seed(&super::store::scope(&env.state, "home").unwrap()).unwrap(), 0);
    let (_, list) = env.json(Method::GET, "/api/workspace/home/cards", None).await;
    assert_eq!(list["cards"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn the_sandbox_is_a_scope_of_its_own_that_reset_empties() {
    let env = setup().await;
    let sandbox = || super::store::scope(&env.state, "wb-sandbox").unwrap();
    assert_eq!(super::examples::seed(&sandbox()).unwrap(), 1);
    assert_eq!(super::examples::seed(&sandbox()).unwrap(), 0, "only on a first start");

    let (_, scopes) = env.json(Method::GET, "/api/workspace/scopes", None).await;
    let ids: Vec<&str> = scopes.as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(&ids[..2], ["home", "wb-sandbox"]);
    assert!(env.state.projects.get("wb-sandbox").is_none() && env.state.projects.list().iter().all(|p| p.id != "wb-sandbox"));

    let (_, list) = env.json(Method::GET, "/api/workspace/wb-sandbox/cards", None).await;
    let guide = &list["cards"][0];
    assert_eq!((guide["id"].as_str(), guide["scope"].as_str()), (Some("sandbox-playground"), Some("wb-sandbox")));
    assert!(guide["steps"].as_array().unwrap().iter().all(|s| s["exists"] == true), "{guide}");

    // The playground report is served like any card's, and the sandbox stays out of All.
    let base = guide["base"].as_str().unwrap();
    let (s, _, body) = env.call(Method::GET, &format!("{base}playground.html"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains("../_shared/report.css"));
    let (_, all) = env.json(Method::GET, "/api/workspace/cards", None).await;
    assert!(all["cards"].as_array().unwrap().iter().all(|c| c["scope"] != "wb-sandbox"), "{all}");

    // Reset trashes every card, keeps them restorable and brings the guide back.
    let (s, mine) = env.json(Method::POST, "/api/workspace/wb-sandbox/cards", Some(json!({ "title": "Experiment" }))).await;
    assert_eq!(s, StatusCode::OK, "{mine}");
    let (s, r) = env.json(Method::POST, "/api/workspace/wb-sandbox/reset", None).await;
    assert_eq!((s, r["removed"].as_u64()), (StatusCode::OK, Some(2)), "{r}");
    let (_, list) = env.json(Method::GET, "/api/workspace/wb-sandbox/cards", None).await;
    let ids: Vec<&str> = list["cards"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["sandbox-playground"]);
    let (_, trash) = env.json(Method::GET, "/api/workspace/wb-sandbox/trash", None).await;
    assert_eq!(trash["items"].as_array().unwrap().len(), 2, "{trash}");

    // Only the sandbox can be reset.
    for scope in ["home", "proj"] {
        let (s, _) = env.json(Method::POST, &format!("/api/workspace/{scope}/reset"), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{scope}");
    }
}
