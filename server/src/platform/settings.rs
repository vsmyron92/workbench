//! Settings: `config.toml` (structured patches and the raw text), secret
//! status, and the per-project config layers (`.workbench.toml` in the repo
//! and the machine overlay `projects/<id>.toml`).
//!
//! Every save validates by parsing, writes atomically, then applies live: the
//! in-memory config is swapped, the secret cache cleared and the projects
//! reloaded. Settings that only take effect on restart are reported back.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::config::{GlobalConfig, ProjectFile, SecretRef, contract_tilde, expand_tilde};
use crate::error::{ApiError, ApiResult};
use crate::secrets::SecretStatus;
use crate::util;
use crate::util::os::perm;

use super::{config_edit, restart_required, sha256_hex};

/// Largest config file we read or accept.
const MAX_CONFIG_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub ok: bool,
    /// SHA-256 of the file now on disk (pass back as `baseHash`).
    pub hash: String,
    /// Keys whose new values take effect only after a restart.
    pub restart_required: Vec<String>,
    pub warnings: Vec<String>,
    pub projects: usize,
}

// ---------------------------------------------------------------- TOML errors

/// 1-based line and column (in characters) of byte offset `at` in `text`.
pub fn line_col(text: &str, at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
    (line, col)
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    pub warnings: Vec<String>,
}

fn diagnose(text: &str, e: &toml::de::Error) -> Diagnostic {
    let mut d = Diagnostic { ok: false, message: Some(e.message().trim().to_string()), ..Default::default() };
    if let Some(span) = e.span() {
        let (l, c) = line_col(text, span.start);
        let (el, ec) = line_col(text, span.end.max(span.start + 1));
        d.line = Some(l);
        d.column = Some(c);
        d.end_line = Some(el);
        d.end_column = Some(ec);
    }
    d
}

fn toml_error(text: &str, e: &toml::de::Error) -> ApiError {
    let d = diagnose(text, e);
    let at = match (d.line, d.column) {
        (Some(l), Some(c)) => format!("line {l}, column {c}: "),
        _ => String::new(),
    };
    ApiError::bad_request(format!("{at}{}", d.message.unwrap_or_default()))
}

// ---------------------------------------------------------------- global config checks

fn valid_host_entry(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.contains("://")
        && !h.contains('/')
        && !h.chars().any(|c| c.is_whitespace() || c == '@')
}

pub fn valid_public_url(u: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(u).map_err(|e| format!("server.public_url is not a URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("server.public_url must start with http:// or https://".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("server.public_url must not contain credentials".into());
    }
    if url.host_str().is_none() {
        return Err("server.public_url has no host".into());
    }
    Ok(())
}

/// Hard errors (refuse to save) and warnings for a global config.
pub fn check_global(cfg: &GlobalConfig) -> (Vec<String>, Vec<String>) {
    let mut errors = vec![];
    let mut warnings = vec![];
    if cfg.server.bind.parse::<std::net::SocketAddr>().is_err() {
        errors.push(format!("server.bind {:?} is not an address like 127.0.0.1:7777", cfg.server.bind));
    }
    if let Some(u) = &cfg.server.public_url {
        if let Err(e) = valid_public_url(u) {
            errors.push(e);
        }
    }
    for h in &cfg.server.allowed_hosts {
        if !valid_host_entry(h) {
            errors.push(format!("server.allowed_hosts entry {h:?} must be a host name or host:port (no scheme or path)"));
        }
    }
    if let Some(tls) = &cfg.server.tls {
        for (what, p) in [("cert", &tls.cert), ("key", &tls.key)] {
            if !expand_tilde(p).is_file() {
                warnings.push(format!("server.tls.{what} {p} does not exist"));
            }
        }
    }
    if let Some(g) = &cfg.gitlab {
        if !cfg.secrets.contains_key(&g.token) {
            warnings.push(format!("gitlab.token names the secret {:?}, which is not defined under [secrets]", g.token));
        }
    }
    if let Some(g) = &cfg.github {
        let token = g.token.trim();
        if !token.is_empty() && !cfg.secrets.contains_key(token) {
            warnings.push(format!(
                "github.token names the secret {token:?}, which is not defined under [secrets]; GitHub is used without a token (public repositories, read-only)"
            ));
        }
        let host = g.host.trim().trim_end_matches('/');
        let authority = host.split_once("://").map(|(_, r)| r).unwrap_or(host);
        if authority.is_empty() || authority.contains(['@', '/', '?', '#']) || authority.contains(char::is_whitespace) {
            warnings.push(format!("github.host {:?} should be a host name like github.com or ghe.example.com", g.host));
        }
    }
    if let Some(a) = &cfg.atlassian {
        if !cfg.secrets.contains_key(&a.token) {
            warnings.push(format!("atlassian.token names the secret {:?}, which is not defined under [secrets]", a.token));
        }
        if !a.site.is_empty() && !a.site.starts_with("https://") {
            warnings.push("atlassian.site should look like https://<site>.atlassian.net".into());
        }
    }
    if let Some(e) = &cfg.agents.effort {
        if !["low", "medium", "high", "xhigh", "max"].contains(&e.as_str()) {
            warnings.push(format!("agents.effort {e:?} is not one of low, medium, high, xhigh, max"));
        }
    }
    if let Some(m) = &cfg.agents.permission_mode {
        if !["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"].contains(&m.as_str()) {
            warnings.push(format!("agents.permission_mode {m:?} is not a Claude Code permission mode"));
        }
    }
    if cfg.projects.roots.is_empty() && cfg.projects.include.is_empty() {
        warnings.push("no project roots or included directories: Workbench will show no projects".into());
    }
    if let Some(s) = cfg.push.subject.as_deref() {
        if !super::push::valid_subject(s) {
            errors.push(format!("push.subject {s:?} must be a mailto: address or an https:// URL"));
        }
    }
    for h in &cfg.push.extra_endpoint_hosts {
        if !super::push::endpoint::valid_extra_host(h) {
            errors.push(format!("push.extra_endpoint_hosts entry {h:?} must be a host name like push.example.com or *.example.com"));
        }
    }
    (errors, warnings)
}

fn parse_global(text: &str) -> ApiResult<(GlobalConfig, Vec<String>)> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(ApiError::bad_request("config is too large"));
    }
    let cfg: GlobalConfig = toml::from_str(text).map_err(|e| toml_error(text, &e))?;
    let (errors, warnings) = check_global(&cfg);
    if !errors.is_empty() {
        return Err(ApiError::bad_request(errors.join("; ")));
    }
    Ok((cfg, warnings))
}

// ---------------------------------------------------------------- apply

fn read_optional(path: &Path) -> ApiResult<Option<String>> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() as usize > MAX_CONFIG_BYTES => {
            return Err(ApiError::bad_request(format!("{} is larger than 2 MB", contract_tilde(path))));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => {}
    }
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(format!("read {}: {e}", contract_tilde(path)))),
    }
}

/// The config file's text, or a rendering of the in-memory config when the file is missing.
fn current_text(state: &AppState) -> ApiResult<(String, bool)> {
    match read_optional(&state.paths.config_file())? {
        Some(t) => Ok((t, true)),
        None => {
            let cfg = state.config.read().clone();
            Ok((config_edit::render(&cfg).map_err(ApiError::from)?, false))
        }
    }
}

/// What a structured change (a Settings form, the pairing dialog) is merged into.
pub(super) struct EditBase {
    /// config.toml's text, or a rendering of the in-memory config when there is no file.
    pub text: String,
    /// The hash the write is conditional on (see [`merge_base`]).
    pub hash: String,
    /// What `text` means. The file, not the in-memory config, is the starting
    /// point: config.toml may have been edited outside Workbench since it was
    /// last applied, and a form must not revert those edits to stale values.
    pub cfg: GlobalConfig,
}

/// The on-disk config a structured change starts from. A file that does not
/// parse is an error: replacing it would throw away the user's work in progress.
pub(super) fn edit_base(state: &AppState) -> ApiResult<EditBase> {
    let (text, exists) = current_text(state)?;
    let hash = merge_base(&text, exists);
    if !exists {
        let cfg = state.config.read().clone();
        return Ok(EditBase { text, hash, cfg });
    }
    match toml::from_str::<GlobalConfig>(&text) {
        Ok(cfg) => Ok(EditBase { text, hash, cfg }),
        Err(e) => {
            let d = diagnose(&text, &e);
            let at = match (d.line, d.column) {
                (Some(l), Some(c)) => format!(" at line {l}, column {c}"),
                _ => String::new(),
            };
            Err(ApiError::bad_request(format!(
                "config.toml has an error{at} ({}). Fix it under Settings → config.toml, then save again.",
                d.message.unwrap_or_default()
            )))
        }
    }
}

/// Write `text` (already validated to mean `cfg`) and apply it.
pub async fn apply_config(
    state: &AppState,
    cfg: GlobalConfig,
    text: String,
    base_hash: Option<&str>,
    mut warnings: Vec<String>,
) -> ApiResult<ApplyResult> {
    let _serial = state.platform.save_lock.lock().await;
    {
        // Hold the config lock across check-and-write so core routes that save
        // the config (add/remove project) cannot interleave with this write.
        let mut live = state.config.write();
        if let Some(base) = base_hash {
            let on_disk = read_optional(&state.paths.config_file())?.unwrap_or_default();
            if sha256_hex(&on_disk) != base {
                return Err(ApiError::conflict("config.toml changed since it was loaded; reload and try again"));
            }
        }
        util::fs::write_atomic(&state.paths.config_file(), text.as_bytes(), 0o600)?;
        *live = cfg.clone();
    }
    state.secrets.clear_cache();
    state.projects.reload(state).await;
    let projects = state.projects.list();
    for p in &projects {
        for w in &p.warnings {
            warnings.push(format!("{}: {w}", p.name));
        }
    }
    state.events.emit("settings.changed", None, json!({}));
    Ok(ApplyResult {
        ok: true,
        hash: sha256_hex(&text),
        restart_required: restart_required(&state.platform.boot(), &cfg.server),
        warnings,
        projects: projects.len(),
    })
}

/// Apply a structured change: `patch` maps top-level keys to new values.
/// Object sections are merged field by field; `secrets` and `extra_roots` are replaced.
pub fn patched(current: &GlobalConfig, patch: &Map<String, Value>) -> ApiResult<GlobalConfig> {
    const KEYS: &[&str] = &["server", "projects", "agents", "gitlab", "github", "atlassian", "notify", "push", "extra_roots", "secrets"];
    let mut v = serde_json::to_value(current)?;
    let obj = v.as_object_mut().ok_or_else(|| ApiError::internal("config is not an object"))?;
    for (k, val) in patch {
        if !KEYS.contains(&k.as_str()) {
            return Err(ApiError::bad_request(format!("unknown settings section {k:?}")));
        }
        match (obj.get_mut(k), val) {
            (Some(Value::Object(existing)), Value::Object(fields)) if k != "secrets" => {
                for (fk, fv) in fields {
                    existing.insert(fk.clone(), fv.clone());
                }
            }
            _ => {
                obj.insert(k.clone(), val.clone());
            }
        }
    }
    serde_json::from_value(v).map_err(|e| ApiError::bad_request(format!("invalid settings: {e}")))
}

// ---------------------------------------------------------------- routes: global

/// `GET /api/settings`. `config` is what config.toml says (forms edit the file,
/// and `hash` is the file's), falling back to the running config when the file
/// is missing or does not parse.
pub async fn get_settings(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (text, exists) = current_text(&state)?;
    let cfg = exists
        .then(|| toml::from_str::<GlobalConfig>(&text).ok())
        .flatten()
        .unwrap_or_else(|| state.config.read().clone());
    Ok(Json(json!({
        "config": cfg,
        "hash": merge_base(&text, exists),
        "paths": {
            "configDir": contract_tilde(&state.paths.config_dir),
            "configFile": contract_tilde(&state.paths.config_file()),
            "dataDir": contract_tilde(&state.paths.data_dir),
            "projectsDir": contract_tilde(&state.paths.config_dir.join("projects")),
        },
        "version": env!("CARGO_PKG_VERSION"),
        "bind": state.bind_addr.to_string(),
        "startedAt": state.started_at,
        "restartRequired": restart_required(&state.platform.boot(), &cfg.server),
        "tlsActive": state.platform.tls_active(),
        "notifySend": util::os::desktop::notify_send().is_some(),
        "mcpEndpoint": format!("{}/mcp", state.local_base_url()),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchBody {
    patch: Map<String, Value>,
    base_hash: Option<String>,
}

/// `PATCH /api/settings {patch, baseHash?}` — structured edit that keeps the file's comments.
pub async fn patch_settings(State(state): State<AppState>, Json(body): Json<PatchBody>) -> ApiResult<Json<ApplyResult>> {
    let base = edit_base(&state)?;
    if body.base_hash.as_deref().is_some_and(|b| b != base.hash) {
        return Err(ApiError::conflict("config.toml changed since it was loaded; reload and try again"));
    }
    let cfg = patched(&base.cfg, &body.patch)?;
    let (errors, warnings) = check_global(&cfg);
    if !errors.is_empty() {
        return Err(ApiError::bad_request(errors.join("; ")));
    }
    let text = config_edit::update_text(&base.text, &cfg).map_err(ApiError::from)?;
    // Write only if the file is still the one the change was merged into.
    Ok(Json(apply_config(&state, cfg, text, Some(&base.hash), warnings).await?))
}

/// Hash of the on-disk text a structured change is merged into (`""` when there is no file).
fn merge_base(text: &str, exists: bool) -> String {
    sha256_hex(if exists { text } else { "" })
}

/// `GET /api/settings/raw` → `{text, path, hash, exists}`
pub async fn get_raw(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let (text, exists) = current_text(&state)?;
    Ok(Json(json!({
        "hash": if exists { sha256_hex(&text) } else { sha256_hex("") },
        "text": text,
        "path": contract_tilde(&state.paths.config_file()),
        "exists": exists,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBody {
    text: String,
    base_hash: Option<String>,
}

/// `PUT /api/settings/raw {text, baseHash?}` — save the file verbatim and apply it.
pub async fn put_raw(State(state): State<AppState>, Json(body): Json<TextBody>) -> ApiResult<Json<ApplyResult>> {
    let (cfg, warnings) = parse_global(&body.text)?;
    Ok(Json(apply_config(&state, cfg, body.text, body.base_hash.as_deref(), warnings).await?))
}

#[derive(Deserialize)]
pub struct ValidateBody {
    /// `global` (config.toml) or `project` (.workbench.toml / overlay)
    kind: String,
    text: String,
}

/// `POST /api/settings/validate {kind, text}` → parse diagnostics for editors.
pub async fn validate(Json(body): Json<ValidateBody>) -> Json<Diagnostic> {
    let text = &body.text;
    if text.len() > MAX_CONFIG_BYTES {
        return Json(Diagnostic { ok: false, message: Some("file is too large".into()), ..Default::default() });
    }
    Json(match body.kind.as_str() {
        "project" => match toml::from_str::<ProjectFile>(text) {
            Ok(p) => Diagnostic { ok: true, warnings: project_warnings(&p, None), ..Default::default() },
            Err(e) => diagnose(text, &e),
        },
        _ => match toml::from_str::<GlobalConfig>(text) {
            Ok(c) => {
                let (errors, warnings) = check_global(&c);
                if errors.is_empty() {
                    Diagnostic { ok: true, warnings, ..Default::default() }
                } else {
                    Diagnostic { ok: false, message: Some(errors.join("; ")), warnings, ..Default::default() }
                }
            }
            Err(e) => diagnose(text, &e),
        },
    })
}

// ---------------------------------------------------------------- edits outside Workbench

/// Apply `config.toml` edits made outside Workbench (an editor, a setup hint followed
/// by hand) like a raw save: a valid file replaces the running config, clears the
/// secret cache, reloads the projects and emits `settings.changed`, so "edit
/// config.toml, then Retry" works without a restart. A file that does not parse or
/// fails the hard checks is reported once and the running config stays. The file is
/// never written here.
///
/// The config directory is watched, not the file: atomic saves replace the inode.
/// The debouncer lives in the task, so it stops with the runtime.
pub fn watch_config(state: &AppState) {
    use notify_debouncer_full::DebounceEventResult;
    use notify_debouncer_full::notify::RecursiveMode;
    let dir = state.paths.config_dir.clone();
    let name = state.paths.config_file().file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let deb = crate::util::os::watch::debouncer(Duration::from_millis(400), move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        if events.iter().any(|e| e.event.paths.iter().any(|p| p.file_name() == Some(name.as_os_str()))) {
            let _ = tx.send(());
        }
    });
    let mut deb = match deb {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("no file watcher for config.toml ({e}); edits outside Settings apply after a restart");
            return;
        }
    };
    if let Err(e) = deb.watch(&dir, RecursiveMode::NonRecursive) {
        tracing::warn!("cannot watch {}: {e}; edits outside Settings apply after a restart", contract_tilde(&dir));
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let _deb = deb;
        while rx.recv().await.is_some() {
            while rx.try_recv().is_ok() {}
            apply_external_edit(&state).await;
        }
    });
}

/// One round of [`watch_config`].
pub(super) async fn apply_external_edit(state: &AppState) {
    // Serialized with Settings saves and project add/remove, which write the file.
    let _serial = state.platform.save_lock.lock().await;
    let text = match read_optional(&state.paths.config_file()) {
        Ok(Some(t)) => t,
        Ok(None) => return, // removed: keep running on what was loaded
        Err(e) => {
            tracing::warn!("config.toml: {}", e.message);
            return;
        }
    };
    let (cfg, warnings) = match parse_global(&text) {
        Ok(v) => v,
        Err(e) => {
            let hash = sha256_hex(&text);
            let mut reported = state.platform.config_reported.lock();
            if reported.as_deref() != Some(hash.as_str()) {
                *reported = Some(hash);
                state.events.notify(
                    "warning",
                    &format!("config.toml was changed but not applied: {}. Workbench keeps the last good settings.", e.message),
                );
            }
            return;
        }
    };
    *state.platform.config_reported.lock() = None;
    if *state.config.read() == cfg {
        return; // a save made here, or an edit that changes nothing
    }
    *state.config.write() = cfg.clone();
    state.secrets.clear_cache();
    state.projects.reload(state).await;
    state.events.emit("settings.changed", None, json!({}));
    for w in &warnings {
        tracing::warn!("config.toml: {w}");
    }
    let restart = restart_required(&state.platform.boot(), &cfg.server);
    let mut message = "config.toml changed on disk; the new settings are in use".to_string();
    if !restart.is_empty() {
        message.push_str(&format!(" ({} after a restart)", restart.join(" and ")));
    }
    tracing::info!("{message}");
    state.events.notify("info", &message);
}

// ---------------------------------------------------------------- secrets

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRow {
    #[serde(flatten)]
    status: SecretStatus,
    /// global | project
    scope: &'static str,
    /// Config fields that name this secret.
    used_by: Vec<String>,
    /// A file reference readable by other users (chmod 600 fixes it).
    fixable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingSecret {
    name: String,
    project_id: Option<String>,
    used_by: Vec<String>,
}

static SECRET_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{secret:([A-Za-z0-9_.\-]+)\}").unwrap());

/// `(secret name, where it is used)` for one project's merged config.
fn project_secret_uses(p: &ProjectFile) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut push = |name: &str, what: String| {
        if !name.is_empty() {
            out.push((name.to_string(), what));
        }
    };
    if let Some(g) = p.repo.as_ref().and_then(|r| r.gitlab.as_ref()) {
        push(&g.token, "repo.gitlab.token".into());
    }
    if let Some(g) = p.repo.as_ref().and_then(|r| r.github.as_ref()) {
        push(g.token.trim(), "repo.github.token".into());
    }
    if let Some(c) = &p.links.confluence {
        push(&c.token, "links.confluence.token".into());
    }
    if let Some(j) = &p.links.jira {
        push(&j.token, "links.jira.token".into());
    }
    for e in &p.envs {
        if let Some(a) = &e.auth {
            push(&a.password, format!("env {} basic auth", e.name));
        }
    }
    for r in &p.runs {
        for v in r.env.values() {
            for c in SECRET_PLACEHOLDER.captures_iter(v) {
                push(&c[1], format!("run {} env", r.name));
            }
        }
    }
    for v in p.agent.env.values() {
        for c in SECRET_PLACEHOLDER.captures_iter(v) {
            push(&c[1], "agent env".into());
        }
    }
    out
}

/// A file reference's permissions as shown (`perm::describe`) and whether others can read it.
fn file_mode(r: &SecretRef) -> Option<(String, bool)> {
    match r {
        SecretRef::File(p) => {
            let path = expand_tilde(p);
            Some((perm::describe(&path).ok()?, perm::privacy(&path).ok()?.is_exposed()))
        }
        _ => None,
    }
}

/// Resolve a reference's status off the async workers, bounded in time
/// (keyring and command references can block).
async fn status_of(state: &AppState, name: &str, r: &SecretRef, project_id: Option<&str>) -> SecretStatus {
    let (st, n, rf, pid) = (state.clone(), name.to_string(), r.clone(), project_id.map(str::to_string));
    let task = tokio::task::spawn_blocking(move || st.secrets.status(&n, &rf, pid.as_deref()));
    match tokio::time::timeout(Duration::from_secs(8), task).await {
        Ok(Ok(s)) => s,
        _ => {
            let (source, location) = match r {
                SecretRef::File(p) => ("file", p.clone()),
                SecretRef::Env(k) => ("env", k.clone()),
                SecretRef::Keyring(k) => ("keyring", k.clone()),
                SecretRef::Dotenv { path, key } => ("dotenv", format!("{path}#{key}")),
                SecretRef::Command(a) => ("command", a.first().cloned().unwrap_or_default()),
            };
            SecretStatus {
                name: name.to_string(),
                source: source.into(),
                location,
                resolved: false,
                error: Some("timed out while resolving".into()),
                warnings: vec![],
                project_id: project_id.map(str::to_string),
            }
        }
    }
}

async fn secret_row(state: &AppState, name: &str, r: &SecretRef, project_id: Option<&str>, used_by: Vec<String>) -> SecretRow {
    let status = status_of(state, name, r, project_id).await;
    let mode = file_mode(r);
    SecretRow {
        status,
        scope: if project_id.is_some() { "project" } else { "global" },
        used_by,
        fixable: mode.as_ref().is_some_and(|(_, exposed)| *exposed),
        mode: mode.map(|(shown, _)| shown),
    }
}

/// `GET /api/settings/secrets` → `{secrets: SecretRow[], missing: MissingSecret[]}`. Never values.
pub async fn get_secrets(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let cfg = state.config.read().clone();
    // Where every secret name is used.
    let mut global_uses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut missing: Vec<MissingSecret> = vec![];
    if let Some(g) = &cfg.gitlab {
        global_uses.entry(g.token.clone()).or_default().push("gitlab.token".into());
    }
    if let Some(g) = cfg.github.as_ref().filter(|g| !g.token.trim().is_empty()) {
        global_uses.entry(g.token.trim().to_string()).or_default().push("github.token".into());
    }
    if let Some(a) = &cfg.atlassian {
        global_uses.entry(a.token.clone()).or_default().push("atlassian.token".into());
    }
    let projects = state.projects.list();
    let mut project_rows: Vec<(String, String, SecretRef, Vec<String>)> = vec![];
    for p in &projects {
        let mut uses: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, what) in project_secret_uses(&p.config) {
            uses.entry(name).or_default().push(what);
        }
        for (name, what) in &uses {
            if p.config.secrets.contains_key(name) {
                continue;
            }
            // Names from repository config resolve only from the project's overlay.
            if cfg.secrets.contains_key(name) && !p.repo_secret_names.contains(name) {
                global_uses.entry(name.clone()).or_default().extend(what.iter().map(|w| format!("{}: {w}", p.name)));
            } else {
                missing.push(MissingSecret { name: name.clone(), project_id: Some(p.id.clone()), used_by: what.clone() });
            }
        }
        for (name, r) in &p.config.secrets {
            project_rows.push((p.id.clone(), name.clone(), r.clone(), uses.get(name).cloned().unwrap_or_default()));
        }
    }
    for (name, uses) in &global_uses {
        if !cfg.secrets.contains_key(name) {
            missing.push(MissingSecret { name: name.clone(), project_id: None, used_by: uses.clone() });
        }
    }

    let global_futs = cfg.secrets.iter().map(|(name, r)| {
        let used = global_uses.get(name).cloned().unwrap_or_default();
        secret_row(&state, name, r, None, used)
    });
    let mut rows = futures::future::join_all(global_futs).await;
    let project_futs = project_rows.iter().map(|(pid, name, r, used)| secret_row(&state, name, r, Some(pid), used.clone()));
    rows.extend(futures::future::join_all(project_futs).await);
    Ok(Json(json!({ "secrets": rows, "missing": missing })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretQuery {
    project_id: Option<String>,
}

/// `POST /api/settings/secrets/{name}/chmod?projectId=` — make a secret file private (0600).
pub async fn chmod_secret(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<SecretQuery>,
) -> ApiResult<Json<SecretRow>> {
    let r = match q.project_id.as_deref() {
        Some(pid) => state.projects.require(pid)?.config.secrets.get(&name).cloned(),
        None => state.config.read().secrets.get(&name).cloned(),
    }
    .ok_or_else(|| ApiError::not_found(format!("no secret named {name:?}")))?;
    let SecretRef::File(p) = &r else {
        return Err(ApiError::bad_request("only file references have permissions to fix"));
    };
    let path = expand_tilde(p);
    let meta = std::fs::metadata(&path).map_err(|e| ApiError::not_found(format!("{p}: {e}")))?;
    if !meta.is_file() {
        return Err(ApiError::bad_request(format!("{p} is not a regular file")));
    }
    if !perm::owned_by_me(&path).map_err(|e| ApiError::not_found(format!("{p}: {e}")))? {
        return Err(ApiError::forbidden(format!("{p} belongs to another user")));
    }
    perm::apply(&path, 0o600).map_err(|e| ApiError::internal(format!("chmod {p}: {e}")))?;
    tracing::info!("secret file {p} set to mode 600");
    Ok(Json(secret_row(&state, &name, &r, q.project_id.as_deref(), vec![]).await))
}

// ---------------------------------------------------------------- per-project layers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Repo,
    Overlay,
}

pub fn project_warnings(p: &ProjectFile, layer: Option<Layer>) -> Vec<String> {
    let mut w = vec![];
    if !p.project.id.is_empty() {
        w.push("project.id is assigned by Workbench; the value here is ignored".into());
    }
    if !p.project.root.is_empty() {
        w.push("project.root is the repository directory; the value here is ignored".into());
    }
    if layer == Some(Layer::Repo) && !p.secrets.is_empty() {
        w.push("secret references are machine-specific: keep [secrets] in the machine overlay, not the committed .workbench.toml".into());
    }
    for r in &p.runs {
        if r.command.trim().is_empty() {
            w.push(format!("run {:?} has no command", r.name));
        }
    }
    for e in &p.envs {
        if e.url.trim().is_empty() {
            w.push(format!("env {:?} has no url", e.name));
        }
    }
    w
}

fn to_toml(p: &ProjectFile) -> String {
    toml::to_string_pretty(p).unwrap_or_else(|e| format!("# cannot render as TOML: {e}\n"))
}

/// `GET /api/settings/projects/{pid}` → detected, repo file, overlay file, merged.
pub async fn get_project(State(state): State<AppState>, UrlPath(pid): UrlPath<String>) -> ApiResult<Json<Value>> {
    let p = state.projects.require(&pid)?;
    let root = p.root.clone();
    let detected = tokio::task::spawn_blocking(move || crate::apps::detect(&root))
        .await
        .map_err(|e| ApiError::internal(format!("detection failed: {e}")))?;
    let repo_path = p.root.join(".workbench.toml");
    let overlay_path = state.paths.project_overlay(&pid);
    // One that links to another computer is not read (the project's warnings say so).
    let repo_file = if crate::config::project::repo_layer_linked_away(&p.root) { None } else { read_optional(&repo_path)? };
    let overlay_file = read_optional(&overlay_path)?;
    let hash = |t: &Option<String>| sha256_hex(t.as_deref().unwrap_or(""));
    Ok(Json(json!({
        "projectId": p.id,
        "name": p.name,
        "root": contract_tilde(&p.root),
        "repoPath": contract_tilde(&repo_path),
        "overlayPath": contract_tilde(&overlay_path),
        "detectedToml": to_toml(&detected),
        "detected": detected,
        "repoHash": hash(&repo_file),
        "repoFile": repo_file,
        "overlayHash": hash(&overlay_file),
        "overlayFile": overlay_file,
        "mergedToml": to_toml(&p.config),
        "merged": p.config,
        "warnings": p.warnings,
    })))
}

async fn put_layer(state: AppState, pid: String, layer: Layer, body: TextBody) -> ApiResult<Json<Value>> {
    let _serial = state.platform.save_lock.lock().await;
    let p = state.projects.require(&pid)?;
    let path = match layer {
        Layer::Repo if crate::config::project::repo_layer_linked_away(&p.root) => {
            return Err(ApiError::forbidden(format!(".workbench.toml is {}", crate::config::project::REPO_LAYER_LINKED_AWAY)));
        }
        Layer::Repo => p.root.join(".workbench.toml"),
        Layer::Overlay => state.paths.project_overlay(&pid),
    };
    if body.text.len() > MAX_CONFIG_BYTES {
        return Err(ApiError::bad_request("file is too large"));
    }
    let current = read_optional(&path)?;
    if let Some(base) = body.base_hash.as_deref() {
        if sha256_hex(current.as_deref().unwrap_or("")) != base {
            return Err(ApiError::conflict(format!("{} changed on disk; reload and try again", contract_tilde(&path))));
        }
    }
    let mut warnings = vec![];
    if body.text.trim().is_empty() {
        if current.is_some() {
            std::fs::remove_file(&path).map_err(|e| ApiError::internal(format!("remove {}: {e}", contract_tilde(&path))))?;
        }
    } else {
        let parsed: ProjectFile = toml::from_str(&body.text).map_err(|e| toml_error(&body.text, &e))?;
        warnings = project_warnings(&parsed, Some(layer));
        let mode = match layer {
            Layer::Repo => 0o644,
            Layer::Overlay => 0o600,
        };
        util::fs::write_atomic(&path, body.text.as_bytes(), mode)?;
    }
    state.projects.reload(&state).await;
    let project_warnings = state.projects.get(&pid).map(|p| p.warnings.clone()).unwrap_or_default();
    Ok(Json(json!({
        "ok": true,
        "hash": sha256_hex(if body.text.trim().is_empty() { "" } else { &body.text }),
        "warnings": warnings,
        "projectWarnings": project_warnings,
    })))
}

/// `PUT /api/settings/projects/{pid}/overlay {text, baseHash?}` (empty text removes the file)
pub async fn put_overlay(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Json(body): Json<TextBody>) -> ApiResult<Json<Value>> {
    put_layer(state, pid, Layer::Overlay, body).await
}

/// `PUT /api/settings/projects/{pid}/repo {text, baseHash?}` — `<root>/.workbench.toml`
pub async fn put_repo(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Json(body): Json<TextBody>) -> ApiResult<Json<Value>> {
    put_layer(state, pid, Layer::Repo, body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::testutil;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn line_and_column_of_offsets() {
        let t = "a = 1\nb = é\nc";
        assert_eq!(line_col(t, 0), (1, 1));
        assert_eq!(line_col(t, 6), (2, 1));
        assert_eq!(line_col(t, 10), (2, 5));
        assert_eq!(line_col(t, 999), (3, 2));
    }

    #[test]
    fn diagnostics_locate_toml_errors() {
        let text = "[server]\nbind = \"127.0.0.1:7777\"\nallowed_hosts = [1\n";
        let e = toml::from_str::<GlobalConfig>(text).unwrap_err();
        let d = diagnose(text, &e);
        assert!(!d.ok);
        assert_eq!(d.line, Some(3), "{d:?}");
    }

    #[test]
    fn global_checks() {
        let mut cfg = GlobalConfig::default();
        assert!(check_global(&cfg).0.is_empty());
        cfg.server.bind = "nope".into();
        cfg.server.public_url = Some("ftp://x".into());
        cfg.server.allowed_hosts = vec!["https://box".into(), "box.tailnet.ts.net".into(), "192.168.1.5:7777".into()];
        let (errors, _) = check_global(&cfg);
        assert_eq!(errors.len(), 3, "{errors:?}");
        cfg = GlobalConfig::default();
        cfg.gitlab = Some(crate::config::global::GitlabConfig { host: "gitlab.com".into(), token: "gl".into() });
        let (errors, warnings) = check_global(&cfg);
        assert!(errors.is_empty());
        assert!(warnings.iter().any(|w| w.contains("\"gl\"")), "{warnings:?}");
        // GitHub: an undefined token secret is reported; no token is anonymous mode.
        cfg = GlobalConfig::default();
        cfg.github = Some(crate::config::global::GithubConfig { host: "github.com".into(), token: "gh".into() });
        let (errors, warnings) = check_global(&cfg);
        assert!(errors.is_empty());
        assert!(warnings.iter().any(|w| w.contains("github.token") && w.contains("\"gh\"")), "{warnings:?}");
        cfg.github = Some(crate::config::global::GithubConfig { host: "https://ghe.example/x".into(), token: String::new() });
        let (_, warnings) = check_global(&cfg);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("github.host"), "{warnings:?}");
    }

    #[test]
    fn patches_merge_sections() {
        let cfg = GlobalConfig::default();
        let patch = json!({
            "agents": { "model": "opus", "effort": null },
            "server": { "allowed_hosts": ["box.ts.net"] },
            "secrets": { "gitlab": { "file": "~/.gitlab_token" } },
        });
        let out = patched(&cfg, patch.as_object().unwrap()).unwrap();
        assert_eq!(out.agents.model.as_deref(), Some("opus"));
        assert_eq!(out.agents.effort, None);
        assert_eq!(out.agents.command, "claude");
        assert_eq!(out.server.bind, "127.0.0.1:7777");
        assert_eq!(out.server.allowed_hosts, vec!["box.ts.net"]);
        assert_eq!(out.secrets["gitlab"], SecretRef::File("~/.gitlab_token".into()));
        assert!(patched(&cfg, json!({"nope": 1}).as_object().unwrap()).is_err());
        let gh = patched(&cfg, json!({ "github": { "host": "github.com", "token": "github" } }).as_object().unwrap()).unwrap();
        assert_eq!(gh.github.map(|g| g.token), Some("github".to_string()));
        // Written into the file comment-preserving, like the other sections.
        let text = "# mine\n[server]\nbind = \"127.0.0.1:7777\" # port note\n";
        let old: GlobalConfig = toml::from_str(text).unwrap();
        let new = patched(&old, json!({ "github": { "host": "ghe.example", "token": "" } }).as_object().unwrap()).unwrap();
        let out = config_edit::update_text(text, &new).unwrap();
        assert!(out.contains("# port note") && out.contains("[github]"), "{out}");
        assert_eq!(toml::from_str::<GlobalConfig>(&out).unwrap(), new);
        assert!(patched(&cfg, json!({"server": {"bind": 5}}).as_object().unwrap()).is_err());
    }

    #[test]
    fn project_secret_uses_are_found() {
        let p: ProjectFile = toml::from_str(
            r#"
            [repo.gitlab]
            path = "a/b"
            token = "gitlab"
            [repo.github]
            path = "o/r"
            token = "gh-work"
            [[run]]
            name = "api"
            command = "cargo run"
            env = { STRIPE = "${secret:stripe}", X = "plain" }
            [[env]]
            name = "staging"
            url = "https://staging.example"
            auth = { user = "u", password = "staging-basic" }
            "#,
        )
        .unwrap();
        let uses = project_secret_uses(&p);
        let names: Vec<&str> = uses.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["gitlab", "gh-work", "staging-basic", "stripe"]);
    }

    async fn call(app: &testutil::TestApp, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "127.0.0.1:7999")
            .header("authorization", format!("Bearer {}", app.state.auth.master_token()));
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    #[tokio::test]
    async fn raw_round_trip_applies_and_reloads_projects() {
        let app = testutil::app().await;
        let repos = tempfile::tempdir().unwrap();
        let repo = repos.path().join("demo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let (s, raw) = call(&app, "GET", "/api/settings/raw", None).await;
        assert_eq!(s, StatusCode::OK);
        let hash = raw["hash"].as_str().unwrap().to_string();
        let text = format!(
            "# edited by hand\n{}\n[projects]\nroots = []\ninclude = [{:?}]\n",
            "[server]\nbind = \"127.0.0.1:7999\"",
            repo.display().to_string()
        );
        // Invalid TOML is rejected with a location.
        let (s, v) = call(&app, "PUT", "/api/settings/raw", Some(json!({ "text": "[server\n", "baseHash": hash }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(v["error"]["message"].as_str().unwrap().contains("line 1"), "{v}");
        // A stale base hash is a conflict.
        let (s, _) = call(&app, "PUT", "/api/settings/raw", Some(json!({ "text": text, "baseHash": "stale" }))).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, v) = call(&app, "PUT", "/api/settings/raw", Some(json!({ "text": text, "baseHash": hash }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["projects"], 1);
        assert_eq!(app.state.projects.list()[0].name, "demo");
        let on_disk = std::fs::read_to_string(app.state.paths.config_file()).unwrap();
        assert!(on_disk.starts_with("# edited by hand"));

        // A structured patch keeps the comment and changes one value.
        let (s, v) = call(
            &app,
            "PATCH",
            "/api/settings",
            Some(json!({ "patch": { "server": { "bind": "0.0.0.0:7999" } }, "baseHash": v["hash"] })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["restartRequired"], json!(["server.bind"]));
        let on_disk = std::fs::read_to_string(app.state.paths.config_file()).unwrap();
        assert!(on_disk.starts_with("# edited by hand"), "{on_disk}");
        assert!(on_disk.contains("0.0.0.0:7999"));

        // Project layers: write the overlay, see it merged, then clear it.
        let pid = app.state.projects.list()[0].id.clone();
        let (s, v) = call(
            &app,
            "PUT",
            &format!("/api/settings/projects/{pid}/overlay"),
            Some(json!({ "text": "[[run]]\nname = \"api\"\ncommand = \"cargo run\"\n" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (_, v) = call(&app, "GET", &format!("/api/settings/projects/{pid}"), None).await;
        assert_eq!(v["merged"]["run"][0]["name"], "api");
        assert!(v["overlayFile"].as_str().unwrap().contains("cargo run"));
        assert!(v["repoFile"].is_null());
        let (s, _) = call(
            &app,
            "PUT",
            &format!("/api/settings/projects/{pid}/repo"),
            Some(json!({ "text": "[[run]]\nname = \n" })),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, _) = call(&app, "PUT", &format!("/api/settings/projects/{pid}/overlay"), Some(json!({ "text": "" }))).await;
        assert_eq!(s, StatusCode::OK);
        assert!(!app.state.paths.project_overlay(&pid).exists());
    }

    /// config.toml edited outside Workbench (nothing reloads it): a later save
    /// from an unrelated form must keep the hand edit, not revert it to the
    /// stale in-memory value.
    #[tokio::test]
    async fn structured_saves_keep_hand_edits() {
        let app = testutil::app().await;
        let path = app.state.paths.config_file();
        let original = std::fs::read_to_string(&path).unwrap();
        let edited = original.replace("allowed_hosts = []", "allowed_hosts = [\"box.tailnet.ts.net\"] # added by hand");
        assert_ne!(edited, original, "{original}");
        std::fs::write(&path, &edited).unwrap();

        // Settings shows the file as it is, with its hash.
        let (_, v) = call(&app, "GET", "/api/settings", None).await;
        assert_eq!(v["config"]["server"]["allowed_hosts"], json!(["box.tailnet.ts.net"]), "{v}");
        let (s, v) =
            call(&app, "PATCH", "/api/settings", Some(json!({ "patch": { "agents": { "effort": "high" } }, "baseHash": v["hash"] }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("allowed_hosts = [\"box.tailnet.ts.net\"] # added by hand"), "{on_disk}");
        assert!(on_disk.contains("effort = \"high\""), "{on_disk}");
        // Saving applied the whole file, hand edit included.
        assert_eq!(app.state.config.read().server.allowed_hosts, vec!["box.tailnet.ts.net"]);

        // The pairing dialog's "allow host" keeps hand edits elsewhere too.
        let edited = on_disk.replace("effort = \"high\"", "effort = \"max\" # by hand");
        std::fs::write(&path, &edited).unwrap();
        let (s, v) = call(&app, "PUT", "/api/platform/remote", Some(json!({ "addAllowedHost": "phone.tailnet.ts.net" }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("effort = \"max\" # by hand"), "{on_disk}");
        assert_eq!(v["remote"]["allowedHosts"], json!(["box.tailnet.ts.net", "phone.tailnet.ts.net"]));
        assert_eq!(app.state.config.read().agents.effort.as_deref(), Some("max"));

        // A file broken by hand is not silently replaced by a structured save.
        let broken = format!("{on_disk}\n[agents\n");
        std::fs::write(&path, &broken).unwrap();
        let (_, v) = call(&app, "GET", "/api/settings", None).await;
        let (s, v) =
            call(&app, "PATCH", "/api/settings", Some(json!({ "patch": { "agents": { "effort": "low" } }, "baseHash": v["hash"] }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert!(v["error"]["message"].as_str().unwrap().contains("config.toml"), "{v}");
        let (s, _) = call(&app, "PUT", "/api/platform/remote", Some(json!({ "addAllowedHost": "x.ts.net" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    /// Following a setup hint by hand (edit config.toml, then Retry) works without a
    /// restart; a broken edit is reported and changes nothing.
    #[tokio::test]
    async fn edits_outside_workbench_apply_live() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("p");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let app = testutil::app().await;
        let state = &app.state;
        let file = state.paths.config_file();
        let mut events = state.events.subscribe();
        let text = format!(
            "[projects]\nroots = []\ninclude = [{:?}]\n\n[notify]\ndesktop = false\n\n[github]\ntoken = \"github\"\n\n[secrets]\ngithub = {{ env = \"WB_TEST_UNSET\" }}\n",
            repo.display().to_string()
        );
        std::fs::write(&file, &text).unwrap();
        apply_external_edit(state).await;
        assert_eq!(state.config.read().github.as_ref().map(|g| g.token.clone()), Some("github".to_string()));
        assert_eq!(state.projects.list().len(), 1);
        let mut kinds = vec![];
        while let Ok(ev) = events.try_recv() {
            kinds.push(ev.kind.clone());
        }
        assert!(kinds.contains(&"settings.changed".to_string()) && kinds.contains(&"projects.changed".to_string()), "{kinds:?}");
        // Unchanged meaning: nothing happens.
        apply_external_edit(state).await;
        assert!(events.try_recv().is_err());
        // Broken: reported once, the running config stays.
        std::fs::write(&file, "[github\n").unwrap();
        apply_external_edit(state).await;
        apply_external_edit(state).await;
        let ev = events.try_recv().unwrap();
        assert_eq!((ev.kind.as_str(), ev.data["level"].as_str()), ("ui.notify", Some("warning")));
        assert!(events.try_recv().is_err(), "reported twice");
        assert!(state.config.read().github.is_some());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[github\n");

        // And the watcher does it by itself.
        std::fs::write(&file, text.replace("token = \"github\"", "token = \"gh2\"")).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while state.config.read().github.as_ref().map(|g| g.token.as_str()) != Some("gh2") {
            assert!(std::time::Instant::now() < deadline, "config.toml edit was not picked up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn secrets_report_status_without_values() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("tok");
        std::fs::write(&f, "supersecretvalue123").unwrap();
        perm::expose(&f, 0o664);
        let mut cfg = GlobalConfig::default();
        cfg.projects.roots.clear();
        cfg.notify.desktop = false;
        cfg.secrets.insert("gitlab".into(), SecretRef::File(f.display().to_string()));
        cfg.secrets.insert("env-missing".into(), SecretRef::Env("WORKBENCH_TEST_SURELY_UNSET".into()));
        cfg.gitlab = Some(crate::config::global::GitlabConfig { host: "gitlab.com".into(), token: "gitlab".into() });
        cfg.atlassian = Some(crate::config::global::AtlassianConfig { site: String::new(), email: String::new(), token: "atl".into() });
        cfg.secrets.insert("github".into(), SecretRef::Env("WORKBENCH_TEST_SURELY_UNSET_GH".into()));
        cfg.github = Some(crate::config::global::GithubConfig { host: "github.com".into(), token: "github".into() });
        let app = testutil::app_with(cfg).await;
        let (s, v) = call(&app, "GET", "/api/settings/secrets", None).await;
        assert_eq!(s, StatusCode::OK);
        let text = v.to_string();
        assert!(!text.contains("supersecretvalue123"));
        let rows = v["secrets"].as_array().unwrap();
        let gl = rows.iter().find(|r| r["name"] == "gitlab").unwrap();
        assert_eq!(gl["resolved"], true);
        assert_eq!(gl["fixable"], true);
        assert_eq!(gl["mode"], if cfg!(unix) { "664" } else { "shared" });
        assert_eq!(gl["usedBy"], json!(["gitlab.token"]));
        let gh = rows.iter().find(|r| r["name"] == "github").unwrap();
        assert_eq!(gh["usedBy"], json!(["github.token"]));
        let env = rows.iter().find(|r| r["name"] == "env-missing").unwrap();
        assert_eq!(env["resolved"], false);
        assert_eq!(v["missing"][0]["name"], "atl");

        let (s, v) = call(&app, "POST", "/api/settings/secrets/gitlab/chmod", None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["fixable"], false);
        perm::assert_mode(&f, 0o600);
        let (s, _) = call(&app, "POST", "/api/settings/secrets/env-missing/chmod", None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
}
