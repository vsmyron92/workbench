//! REST: `/api/workspace/**`. Every handler resolves the scope, runs the disk work on
//! the blocking pool (writes under the slice's write lock), and emits
//! `workspace.changed {scope, cardId?}` after a change.

use std::path::PathBuf;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use futures::StreamExt;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::store::{self, CardOut, CardPatch, NewCard, Scope, StepPatch, StepSource};
use super::trash::{self, TrashItem};
use super::{blocking, emit_changed, emit_trash};
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

type CardPath = UrlPath<(String, String)>;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeInfo {
    id: String,
    name: String,
    /// Cards in the scope (archived included).
    total: usize,
    /// Cards not archived.
    active: usize,
    /// Cards from the project's own `workspace/workspace.json`.
    repo_cards: usize,
    has_repo_registry: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub async fn scopes(State(state): State<AppState>) -> ApiResult<Json<Vec<ScopeInfo>>> {
    let st = state.clone();
    let out = blocking(move || {
        let cutoff = super::model::archive_cutoff_ms();
        Ok(store::all_scopes(&st)
            .into_iter()
            .map(|s| {
                let has_repo_registry = s.repo_dir().is_some();
                match store::load(&s) {
                    Ok(l) => ScopeInfo {
                        total: l.cards.len(),
                        active: l.cards.iter().filter(|c| !c.card.archived(cutoff)).count(),
                        repo_cards: l.cards.iter().filter(|c| c.origin == store::Origin::Repo).count(),
                        id: s.id,
                        name: s.name,
                        has_repo_registry,
                        error: None,
                    },
                    Err(e) => ScopeInfo { id: s.id, name: s.name, total: 0, active: 0, repo_cards: 0, has_repo_registry, error: Some(e.message) },
                }
            })
            .collect())
    })
    .await?;
    Ok(Json(out))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardList {
    /// The scope listed, or `all`.
    scope: String,
    cards: Vec<CardOut>,
    warnings: Vec<String>,
}

pub async fn list_cards(State(state): State<AppState>, UrlPath(scope): UrlPath<String>) -> ApiResult<Json<CardList>> {
    let s = store::scope(&state, &scope)?;
    let st = state.clone();
    let (cards, warnings) = blocking(move || store::scope_cards(&st, &s)).await?;
    Ok(Json(CardList { scope, cards, warnings }))
}

/// Every scope's cards in one list (the home panel's "All" view), freshest first.
pub async fn all_cards(State(state): State<AppState>) -> ApiResult<Json<CardList>> {
    let st = state.clone();
    let (cards, warnings) = blocking(move || {
        let mut cards = vec![];
        let mut warnings = vec![];
        // The Sandbox is a playground: its throwaway cards stay out of the All view.
        for s in store::all_scopes(&st).into_iter().filter(|s| s.id != store::SANDBOX) {
            match store::scope_cards(&st, &s) {
                Ok((c, w)) => {
                    cards.extend(c);
                    warnings.extend(w.into_iter().map(|w| format!("{}: {w}", s.name)));
                }
                Err(e) => warnings.push(format!("{}: {}", s.name, e.message)),
            }
        }
        cards.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.touched_at.cmp(&a.touched_at)));
        Ok((cards, warnings))
    })
    .await?;
    Ok(Json(CardList { scope: "all".into(), cards, warnings }))
}

pub async fn get_card(State(state): State<AppState>, UrlPath((scope, id)): CardPath) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    let st = state.clone();
    Ok(Json(blocking(move || store::one_card(&st, &s, &id)).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBody {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    icon: Option<String>,
}

pub async fn create(State(state): State<AppState>, UrlPath(scope): UrlPath<String>, Json(b): Json<CreateBody>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    let out = create_card(&state, s, NewCard { title: b.title, description: b.description, category: b.category, icon: b.icon }).await?;
    Ok(Json(out))
}

/// Create a card and return it (shared with the MCP tool).
pub async fn create_card(state: &AppState, s: Scope, input: NewCard) -> ApiResult<CardOut> {
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let (out, scope_id, project) = blocking(move || {
        let id = store::create_card(&s, input)?;
        Ok((store::one_card(&st, &s, &id)?, s.id.clone(), s.project_id().map(str::to_string)))
    })
    .await?;
    emit_changed(state, &scope_id, project.as_deref(), Some(&out.id));
    Ok(out)
}

/// `null` → `Some(None)`; absent → `None`.
fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchBody {
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub pinned: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub default_step: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub icon: Option<Option<String>>,
}

pub async fn patch(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Json(b): Json<PatchBody>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    Ok(Json(patch_card(&state, s, id, b).await?))
}

pub async fn patch_card(state: &AppState, s: Scope, id: String, b: PatchBody) -> ApiResult<CardOut> {
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let (out, scope_id, project) = blocking(move || {
        let loc = store::find(&s, &id)?;
        store::patch_card(
            &loc,
            CardPatch { title: b.title, description: b.description, category: b.category, status: b.status, pinned: b.pinned, default_step: b.default_step, icon: b.icon },
        )?;
        Ok((store::one_card(&st, &s, &id)?, s.id.clone(), s.project_id().map(str::to_string)))
    })
    .await?;
    emit_changed(state, &scope_id, project.as_deref(), Some(&out.id));
    Ok(out)
}

/// Bump a card's `updated` without changing anything else.
pub async fn touch_card(state: &AppState, s: Scope, id: String) -> ApiResult<CardOut> {
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let (out, scope_id, project) = blocking(move || {
        let loc = store::find(&s, &id)?;
        store::touch(&loc)?;
        Ok((store::one_card(&st, &s, &id)?, s.id.clone(), s.project_id().map(str::to_string)))
    })
    .await?;
    emit_changed(state, &scope_id, project.as_deref(), Some(&out.id));
    Ok(out)
}

pub async fn delete(State(state): State<AppState>, UrlPath((scope, id)): CardPath) -> ApiResult<Json<Value>> {
    let s = store::scope(&state, &scope)?;
    let _w = state.workspace.write_lock.lock().await;
    let trash = store::trash_dir(&state);
    let project = s.project_id().map(str::to_string);
    let cid = id.clone();
    let item = blocking(move || {
        let loc = store::find(&s, &cid)?;
        store::delete_card(&trash, &s, &loc)
    })
    .await?;
    emit_changed(&state, &scope, project.as_deref(), Some(&id));
    emit_trash(&state, &scope, project.as_deref());
    // `trashItem` restores it (`POST …/trash/{item}/restore`).
    Ok(Json(json!({ "ok": true, "trashItem": item })))
}

/// Empty the Sandbox: every card goes to the trash (Restore works) and the guide card
/// comes back.
pub async fn reset(State(state): State<AppState>, UrlPath(scope): UrlPath<String>) -> ApiResult<Json<Value>> {
    if scope != store::SANDBOX {
        return Err(ApiError::bad_request("only the sandbox can be reset"));
    }
    let s = store::scope(&state, &scope)?;
    let _w = state.workspace.write_lock.lock().await;
    let trash = store::trash_dir(&state);
    let removed = blocking(move || {
        let mut n = 0;
        for loc in store::load(&s)?.cards {
            store::delete_card(&trash, &s, &loc)?;
            n += 1;
        }
        super::examples::reseed(&s)?;
        Ok(n)
    })
    .await?;
    emit_changed(&state, &scope, None, None);
    emit_trash(&state, &scope, None);
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepBody {
    #[serde(default)]
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub viewer: Option<String>,
}

pub async fn add_step(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Json(b): Json<StepBody>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    // The user may add files from any project (dragged from the files tree).
    let roots: Vec<PathBuf> = state.projects.list().iter().map(|p| p.root.clone()).collect();
    let (out, _) = add_step_to(&state, s, id, b, roots).await?;
    Ok(Json(out))
}

/// Add a step; a path outside the card folder but inside `import_roots` is copied in
/// first. Returns the card and the new step's index.
pub async fn add_step_to(state: &AppState, s: Scope, id: String, b: StepBody, import_roots: Vec<PathBuf>) -> ApiResult<(CardOut, usize)> {
    let viewer = store::check_viewer(b.viewer.as_deref())?;
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let (out, index, scope_id, project) = blocking(move || {
        let loc = store::find(&s, &id)?;
        if !loc.editable() {
            return Err(ApiError::forbidden("cards from the repository's workspace/workspace.json are read-only here"));
        }
        let dir = loc.dir()?;
        let rel = match store::classify_step_path(&dir, &b.path, &import_roots)? {
            StepSource::Inside(rel) => rel,
            StepSource::Import(src) => store::import_into(&dir, &src)?,
        };
        let index = store::add_step(&loc, &b.name, &rel, viewer.as_deref())?;
        Ok((store::one_card(&st, &s, &id)?, index, s.id.clone(), s.project_id().map(str::to_string)))
    })
    .await?;
    emit_changed(state, &scope_id, project.as_deref(), Some(&out.id));
    Ok((out, index))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepPatchBody {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    viewer: Option<Option<String>>,
    position: Option<usize>,
    /// The step's path as the client saw it (409 when the steps moved meanwhile).
    expect_path: Option<String>,
}

pub async fn patch_step(State(state): State<AppState>, UrlPath((scope, id, index)): UrlPath<(String, String, usize)>, Json(b): Json<StepPatchBody>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    let viewer = match &b.viewer {
        Some(v) => Some(store::check_viewer(v.as_deref())?),
        None => None,
    };
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let project = s.project_id().map(str::to_string);
    let out = blocking(move || {
        let loc = store::find(&s, &id)?;
        store::patch_step(&loc, index, b.expect_path.as_deref(), StepPatch { name: b.name, viewer, position: b.position })?;
        store::one_card(&st, &s, &id)
    })
    .await?;
    emit_changed(&state, &scope, project.as_deref(), Some(&out.id));
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ExpectQuery {
    path: Option<String>,
}

pub async fn delete_step(State(state): State<AppState>, UrlPath((scope, id, index)): UrlPath<(String, String, usize)>, Query(q): Query<ExpectQuery>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope)?;
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let project = s.project_id().map(str::to_string);
    let out = blocking(move || {
        let loc = store::find(&s, &id)?;
        store::delete_step(&loc, index, q.path.as_deref())?;
        store::one_card(&st, &s, &id)
    })
    .await?;
    emit_changed(&state, &scope, project.as_deref(), Some(&out.id));
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
pub struct FilesQuery {
    #[serde(default)]
    path: String,
    /// Skip this many entries (the next page of a large folder).
    #[serde(default)]
    offset: usize,
}

pub async fn files(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Query(q): Query<FilesQuery>) -> ApiResult<Json<Value>> {
    let s = store::scope(&state, &scope)?;
    let out = blocking(move || {
        let loc = store::find(&s, &id)?;
        let l = store::list_files_page(&loc.dir()?, &q.path, q.offset)?;
        Ok(json!({ "path": store::check_rel(&q.path)?, "entries": l.entries, "truncated": l.truncated, "total": l.total, "offset": q.offset }))
    })
    .await?;
    Ok(Json(out))
}

pub async fn read_content(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Query(q): Query<PathQuery>) -> ApiResult<Json<store::TextContent>> {
    let s = store::scope(&state, &scope)?;
    let out = blocking(move || {
        let loc = store::find(&s, &id)?;
        store::read_text(&loc.dir()?, &q.path, loc.editable())
    })
    .await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteBody {
    path: String,
    text: String,
    /// sha256 the edit is based on (from GET content); absent for a new file.
    #[serde(default)]
    revision: Option<String>,
    /// Save over whatever is on disk (the previous version is still backed up).
    #[serde(default)]
    force: bool,
}

pub async fn write_content(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Json(b): Json<WriteBody>) -> ApiResult<Json<Value>> {
    let s = store::scope(&state, &scope)?;
    let _w = state.workspace.write_lock.lock().await;
    let backups_root = store::backups_dir(&state);
    let project = s.project_id().map(str::to_string);
    let scope_id = s.id.clone();
    let cid = id.clone();
    let (path, revision, size) = blocking(move || {
        let loc = store::find(&s, &cid)?;
        if !loc.editable() {
            return Err(ApiError::forbidden("files of repository cards are edited in the project, not here"));
        }
        let backups = backups_root.join(&s.id).join(super::model::card_slug(&cid));
        let r = store::write_markdown(&loc.dir()?, &b.path, &b.text, b.revision.as_deref(), b.force, &backups)?;
        // Editing a card counts as touching it.
        store::touch(&loc)?;
        Ok(r)
    })
    .await?;
    emit_changed(&state, &scope_id, project.as_deref(), Some(&id));
    Ok(Json(json!({ "path": path, "revision": revision, "size": size })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadQuery {
    name: String,
    #[serde(default)]
    dir: String,
    /// Also add the uploaded file as a step.
    #[serde(default)]
    step: bool,
}

/// Raw body → a file in the card folder (never replacing one: `name (2).ext`).
pub async fn upload(State(state): State<AppState>, UrlPath((scope, id)): CardPath, Query(q): Query<UploadQuery>, body: Body) -> ApiResult<Json<Value>> {
    let s = store::scope(&state, &scope)?;
    let name = store::check_file_name(&q.name)?;
    store::check_rel(&q.dir)?;
    let (s, loc) = blocking(move || {
        let loc = store::find(&s, &id)?;
        Ok((s, loc))
    })
    .await?;
    if !loc.editable() {
        return Err(ApiError::forbidden("cards from the repository's workspace/workspace.json are read-only here"));
    }
    let dir = loc.dir()?;
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!(".upload-{}.part", crate::util::random_token(8)));
    let size = match receive(&tmp, body).await {
        Ok(n) => n,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let project = s.project_id().map(str::to_string);
    let scope_id = s.id.clone();
    let public_id = loc.public_id();
    let tmp2 = tmp.clone();
    let result = blocking(move || {
        let rel = store::finish_upload(&dir, &tmp2, &q.dir, &name)?;
        let index = if q.step {
            Some(store::add_step(&loc, "", &rel, None)?)
        } else {
            store::touch(&loc)?;
            None
        };
        Ok((rel, index, store::one_card(&st, &s, &loc.public_id())?))
    })
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    let (rel, index, card) = result?;
    emit_changed(&state, &scope_id, project.as_deref(), Some(&public_id));
    Ok(Json(json!({ "path": rel, "size": size, "stepIndex": index, "card": card })))
}

async fn receive(tmp: &std::path::Path, body: Body) -> ApiResult<u64> {
    let mut f = tokio::fs::OpenOptions::new().write(true).create_new(true).open(tmp).await?;
    let mut stream = body.into_data_stream();
    let mut n: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request(format!("upload interrupted: {e}")))?;
        n += chunk.len() as u64;
        if n > store::MAX_UPLOAD_BYTES {
            return Err(ApiError::bad_request(format!("uploads are limited to {} GB", store::MAX_UPLOAD_BYTES >> 30)));
        }
        f.write_all(&chunk).await?;
    }
    f.sync_all().await?;
    Ok(n)
}

pub async fn grant(State(state): State<AppState>, UrlPath((scope, id)): CardPath) -> ApiResult<Json<Value>> {
    let s = store::scope(&state, &scope)?;
    let loc = blocking(move || store::find(&s, &id)).await?;
    let (token, expires) = state.workspace.grants.mint(&loc.base, &loc.card.folder);
    Ok(Json(json!({ "base": format!("/view/{token}/{}/", store::encode_segment(&loc.card.folder)), "expiresAt": expires })))
}

// ---------------------------------------------------------------- trash

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashList {
    /// The scope listed, or `all`.
    scope: String,
    items: Vec<TrashItem>,
}

/// `GET /api/workspace/{scope}/trash` (`all`: every scope's), newest first.
pub async fn trash_list(State(state): State<AppState>, UrlPath(scope): UrlPath<String>) -> ApiResult<Json<TrashList>> {
    let st = state.clone();
    let sc = scope.clone();
    let items = blocking(move || {
        let root = trash::root(&st);
        let scopes = if sc == "all" { trash::all_trash_scopes(&st) } else { vec![trash::trash_scope(&st, &sc)?] };
        let mut items: Vec<TrashItem> = scopes.iter().flat_map(|ts| trash::list(&root, ts)).collect();
        items.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        Ok(items)
    })
    .await?;
    Ok(Json(TrashList { scope, items }))
}

/// `POST /api/workspace/{scope}/trash/{item}/restore` → the restored card.
pub async fn trash_restore(State(state): State<AppState>, UrlPath((scope, item)): UrlPath<(String, String)>) -> ApiResult<Json<CardOut>> {
    let s = store::scope(&state, &scope).map_err(|e| ApiError::new(e.status, e.code, format!("{} (a removed project's cards cannot be restored)", e.message)))?;
    let project = s.project_id().map(str::to_string);
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let out = blocking(move || {
        let id = trash::restore(&trash::root(&st), &s, &item)?;
        store::one_card(&st, &s, &id)
    })
    .await?;
    emit_changed(&state, &scope, project.as_deref(), Some(&out.id));
    emit_trash(&state, &scope, project.as_deref());
    Ok(Json(out))
}

/// `DELETE /api/workspace/{scope}/trash/{item}`: gone for good.
pub async fn trash_delete(State(state): State<AppState>, UrlPath((scope, item)): UrlPath<(String, String)>) -> ApiResult<Json<Value>> {
    let ts = trash::trash_scope(&state, &scope)?;
    let project = ts.scope.as_ref().and_then(|s| s.project_id()).map(str::to_string);
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let id = ts.id.clone();
    blocking(move || trash::purge(&trash::root(&st), &id, &item)).await?;
    emit_trash(&state, &ts.id, project.as_deref());
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/workspace/{scope}/trash` (`all`: every scope's): empty the trash.
pub async fn trash_empty(State(state): State<AppState>, UrlPath(scope): UrlPath<String>) -> ApiResult<Json<Value>> {
    let scopes = if scope == "all" { trash::all_trash_scopes(&state) } else { vec![trash::trash_scope(&state, &scope)?] };
    let _w = state.workspace.write_lock.lock().await;
    let st = state.clone();
    let ids: Vec<(String, Option<String>)> = scopes.iter().map(|ts| (ts.id.clone(), ts.scope.as_ref().and_then(|s| s.project_id()).map(str::to_string))).collect();
    let for_disk = ids.clone();
    let removed = blocking(move || {
        let root = trash::root(&st);
        let mut n = 0;
        for (id, _) in &for_disk {
            n += trash::empty(&root, id)?;
        }
        Ok(n)
    })
    .await?;
    for (id, project) in &ids {
        emit_trash(&state, id, project.as_deref());
    }
    Ok(Json(json!({ "ok": true, "removed": removed })))
}
