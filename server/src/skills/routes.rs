//! REST of the skills slice.
//!
//! * `GET /api/projects/{pid}/skills` → `{agents, skills, catalog, notes}`: every account whose CLI
//!   reads skills with what it costs, every skill found for the project, and Workbench's own set.
//! * `GET …/skills/{id}` → one skill with its text and files (`wb:<name>` for one of Workbench's);
//!   `GET …/skills/{id}/file?path=` → one file of its folder.
//! * `POST /api/skills/install {skill, account, force?}`, `POST /api/skills/remove {skill, account, force?}`:
//!   the user's, never an agent's: an in-process (MCP) call is refused with 403.
//!
//! The client never names a path: a skill is found by an id the scan made, a file by a name inside
//! its folder (`resolve_in_root`), and the folders written to are the accounts' own, from config.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::catalog::{self, State as InstallState};
use super::discover::{self, Account, Inputs};
use super::parse;
use crate::app::AppState;
use crate::auth::Caller;
use crate::config::contract_tilde;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub fn router() -> Router<AppState> {
    let b = "/api/projects/{pid}/skills";
    Router::new()
        .route(b, get(report))
        .route(&format!("{b}/{{id}}"), get(detail))
        .route(&format!("{b}/{{id}}/file"), get(file))
        .route("/api/skills/install", post(install))
        .route("/api/skills/remove", post(remove))
}

/// Skills are instructions an agent follows: adding or removing one is the user's decision.
fn user_only(caller: &Option<Extension<Caller>>) -> ApiResult<()> {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("skills are installed and removed by the user only")),
        _ => Ok(()),
    }
}

/// The accounts that read skills, with their folders, for the project at `root` (blocking: PATH lookups).
fn accounts_of(state: &AppState) -> Vec<Account> {
    let cfg = state.config.read().agents.clone();
    let home = state.skills.home();
    crate::terminals::agent_accounts(&cfg)
        .iter()
        .filter(|a| a.enabled)
        .filter_map(|a| Account::resolve(a, &home))
        // A CLI that is not here and has no folder is not an agent of this machine.
        .filter(|a| a.installed || a.dir.exists())
        .collect()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError::internal(e.to_string()))
}

async fn scan(state: &AppState, pid: &str) -> ApiResult<(Inputs, discover::Discovery)> {
    let project = state.projects.require(pid)?;
    let st = state.clone();
    blocking(move || {
        let inp = Inputs {
            home: st.skills.home(),
            git_root: discover::git_root_of(&project.root),
            root: project.root.clone(),
            accounts: accounts_of(&st),
            managed_claude: util::os::path::claude_managed_dir(),
            admin_codex: cfg!(unix).then(|| PathBuf::from("/etc/codex")),
            limits: discover::Limits::default(),
        };
        let d = discover::discover(&inp);
        (inp, d)
    })
    .await
}

async fn report(State(state): State<AppState>, Path(pid): Path<String>) -> ApiResult<Json<Value>> {
    let (inp, d) = scan(&state, &pid).await?;
    let accounts = inp.accounts.clone();
    let catalog = blocking(move || catalog::entries(&accounts)).await?;
    Ok(Json(json!({
        "projectId": pid,
        "generatedAt": util::now_ms(),
        "agents": d.accounts,
        "skills": d.skills,
        "catalog": catalog,
        "notes": d.notes,
    })))
}

fn file_json(rel: &str, size: u64, dir: bool) -> Value {
    json!({ "path": rel, "size": size, "dir": dir })
}

async fn detail(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    if let Some(name) = id.strip_prefix("wb:") {
        state.projects.require(&pid)?;
        let skill = catalog::find(name).ok_or_else(|| ApiError::not_found(format!("Workbench has no skill called {name:?}")))?;
        let st = state.clone();
        let name = skill.name.clone();
        let entry = blocking(move || catalog::entries(&accounts_of(&st)).into_iter().find(|e| e.name == name)).await?;
        let files: Vec<Value> = skill.files.iter().filter(|f| f.path != "SKILL.md").map(|f| file_json(&f.path, f.data.len() as u64, false)).collect();
        return Ok(Json(json!({ "catalog": entry, "text": skill.skill_md(), "truncated": false, "files": files, "filesTruncated": false })));
    }
    let (_, d) = scan(&state, &pid).await?;
    let info = d.skills.into_iter().find(|s| s.id == id).ok_or_else(|| ApiError::not_found("no such skill (it may have been moved or removed: reload the list)"))?;
    let located = d.located.into_iter().find(|l| l.id == id);
    // A skill built into Claude Code has no files: only what the session listed.
    let Some(l) = located else {
        return Ok(Json(json!({ "skill": info, "text": null, "truncated": false, "files": [], "filesTruncated": false })));
    };
    // A skill that is a link out of its repository, plugin or sync folder is listed, and nothing of it is read.
    if !l.readable {
        return Ok(Json(json!({ "skill": info, "text": null, "truncated": false, "files": [], "filesTruncated": false })));
    }
    let root = state.projects.require(&pid)?.root.clone();
    let (text, truncated, files, files_truncated) = blocking(move || {
        let refused = l.under_project && util::os::path::leaves_machine_below(&root, &l.file);
        let (text, truncated) = if refused { (String::new(), false) } else { discover::read_capped(&l.file, parse::MAX_BYTES).unwrap_or_default() };
        let (files, ft) = l.dir.as_deref().map(|d| discover::folder_files(d, discover::MAX_FILES)).unwrap_or_default();
        (text, truncated, files, ft)
    })
    .await?;
    // A skill file that is not text shows nothing of itself (the report says so).
    let text = (!text.contains('\0')).then_some(text);
    let files: Vec<Value> = files.iter().map(|(p, s, d)| file_json(p, *s, *d)).collect();
    Ok(Json(json!({ "skill": info, "text": text, "truncated": truncated, "files": files, "filesTruncated": files_truncated })))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn file(State(state): State<AppState>, Path((pid, id)): Path<(String, String)>, Query(q): Query<FileQuery>) -> ApiResult<Json<Value>> {
    let rel = q.path.trim().to_string();
    if let Some(name) = id.strip_prefix("wb:") {
        state.projects.require(&pid)?;
        let skill = catalog::find(name).ok_or_else(|| ApiError::not_found(format!("Workbench has no skill called {name:?}")))?;
        let f = skill.file(&rel).ok_or_else(|| ApiError::not_found("no such file in this skill"))?;
        return Ok(Json(json!({ "path": rel, "text": String::from_utf8_lossy(&f.data), "truncated": false, "binary": false })));
    }
    let (_, d) = scan(&state, &pid).await?;
    let l = d.located.into_iter().find(|l| l.id == id).ok_or_else(|| ApiError::not_found("no such skill (reload the list)"))?;
    if !l.readable {
        return Err(ApiError::forbidden("this skill is a link that leaves its folder: Workbench does not read through it"));
    }
    let dir = l.dir.clone().ok_or_else(|| ApiError::not_found("this skill is a single file"))?;
    let root = state.projects.require(&pid)?.root.clone();
    blocking(move || {
        // Inside the skill's folder, links that leave it refused.
        let path = util::paths::resolve_in_root(&dir, &rel)?;
        if l.under_project && util::os::path::leaves_machine_below(&root, &path) {
            return Err(ApiError::forbidden("this path leads to another computer"));
        }
        let (text, truncated) = discover::read_text(&path, parse::MAX_BYTES).map_err(|e| ApiError::not_found(format!("cannot read it ({})", e.kind())))?;
        // A binary file is not text: say so and send nothing.
        Ok(Json(json!({ "path": rel, "binary": text.is_none(), "text": text, "truncated": truncated })))
    })
    .await?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallBody {
    skill: String,
    account: String,
    #[serde(default)]
    force: bool,
}

/// The account a request names, when Workbench's skills are offered for it.
fn target(state: &AppState, id: &str) -> ApiResult<Account> {
    let accounts = accounts_of(state);
    catalog::targets(&accounts).find(|a| a.id == id).cloned().ok_or_else(|| ApiError::bad_request(format!("{id:?} is not an agent account Workbench's skills are installed for (Claude Code and Codex accounts that are set up)")))
}

async fn install(State(state): State<AppState>, caller: Option<Extension<Caller>>, Json(body): Json<InstallBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let skill = catalog::find(&body.skill).ok_or_else(|| ApiError::not_found(format!("Workbench has no skill called {:?}", body.skill)))?;
    let st = state.clone();
    let (account, force) = (body.account, body.force);
    let (st_after, path) = blocking(move || -> ApiResult<(InstallState, String)> {
        let a = target(&st, &account)?;
        let dir = a.skills_dir();
        let after = catalog::install(&skill, &dir, force)?;
        Ok((after, contract_tilde(&dir.join(&skill.name))))
    })
    .await??;
    state.events.emit("skills.changed", None, json!({}));
    Ok(Json(json!({ "state": st_after, "path": path })))
}

async fn remove(State(state): State<AppState>, caller: Option<Extension<Caller>>, Json(body): Json<InstallBody>) -> ApiResult<Json<Value>> {
    user_only(&caller)?;
    let st = state.clone();
    let (name, account, force) = (body.skill, body.account, body.force);
    blocking(move || -> ApiResult<()> {
        let a = target(&st, &account)?;
        catalog::remove(&name, &a.skills_dir(), force)
    })
    .await??;
    state.events.emit("skills.changed", None, json!({}));
    Ok(Json(json!({ "ok": true })))
}
