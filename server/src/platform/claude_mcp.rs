//! Read-only overview of the MCP servers Claude Code will load for a project,
//! from the files on disk:
//!
//! * `~/.claude.json` — user servers (top-level `mcpServers`) and local ones
//!   (`projects[<root>].mcpServers`, plus the `.mcp.json` approvals stored there);
//! * `<root>/.mcp.json` — project servers;
//! * settings (`~/.claude/settings.json` < `<root>/.claude/settings.json` <
//!   `<root>/.claude/settings.local.json`, and the managed file) — `enabledMcpjsonServers`,
//!   `disabledMcpjsonServers`, `enableAllProjectMcpServers`, `deniedMcpServers`,
//!   `allowedMcpServers`, `enabledPlugins`;
//! * plugins — `~/.claude/plugins/installed_plugins.json` → each plugin's
//!   `.claude-plugin/plugin.json` `mcpServers` or `.mcp.json`.
//!
//! Only names, scopes, transports, endpoint *origins*, command *basenames* and
//! env/header *names* leave this module — never values, arguments or paths
//! inside URLs. claude.ai connectors are account-level and invisible here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::AppState;
use crate::config::contract_tilde;
use crate::error::{ApiError, ApiResult};

const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    pub name: String,
    /// user | local | project | plugin | managed
    pub scope: &'static str,
    /// File it came from (display form).
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// stdio | http | sse | ws
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub args: usize,
    pub env_names: Vec<String>,
    pub header_names: Vec<String>,
    /// enabled | disabled | needs-approval | denied
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNote {
    pub path: String,
    /// read | missing | error
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub servers: Vec<ServerEntry>,
    pub files: Vec<FileNote>,
    /// `~/.claude.json` records that claude.ai connectors were used.
    pub account_connectors_used: bool,
}

pub struct Locations {
    pub claude_json: PathBuf,
    pub claude_dir: PathBuf,
    pub managed_dir: PathBuf,
}

impl Locations {
    pub fn from_env() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let (claude_dir, claude_json) = match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(d) => {
                let d = PathBuf::from(d);
                (d.clone(), d.join(".claude.json"))
            }
            None => (home.join(".claude"), home.join(".claude.json")),
        };
        Self { claude_json, claude_dir, managed_dir: crate::util::os::path::claude_managed_dir() }
    }
}

/// [`read_json`] for a file of the project at `root` (repository content): never through a
/// link to another computer (Windows, `os::path::leaves_machine_below`), which reading
/// would connect to.
fn read_project_json(root: &Path, path: &Path, files: &mut Vec<FileNote>) -> Option<Value> {
    if crate::util::os::path::leaves_machine_below(root, path) {
        files.push(FileNote { path: contract_tilde(path), status: "error", error: Some("a link to a network path or a device: not read".into()) });
        return None;
    }
    read_json(path, files)
}

fn read_json(path: &Path, files: &mut Vec<FileNote>) -> Option<Value> {
    let note = |status, error| FileNote { path: contract_tilde(path), status, error };
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            files.push(note("missing", None));
            return None;
        }
        Err(e) => {
            files.push(note("error", Some(e.kind().to_string())));
            return None;
        }
    };
    if meta.len() > MAX_JSON_BYTES {
        files.push(note("error", Some("file too large".into())));
        return None;
    }
    // Fixed messages only: parse errors could quote file content.
    match std::fs::read(path).map(|b| serde_json::from_slice::<Value>(&b)) {
        Ok(Ok(v)) => {
            files.push(note("read", None));
            Some(v)
        }
        Ok(Err(_)) => {
            files.push(note("error", Some("not valid JSON".into())));
            None
        }
        Err(e) => {
            files.push(note("error", Some(e.kind().to_string())));
            None
        }
    }
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Settings relevant to MCP, merged across scopes (later scopes win for
/// scalars; lists are unioned; `enabledPlugins` is merged by key).
#[derive(Debug, Default)]
struct McpSettings {
    enabled_mcpjson: BTreeSet<String>,
    disabled_mcpjson: BTreeSet<String>,
    enable_all_project: bool,
    denied: Vec<Value>,
    allowed: Option<Vec<Value>>,
    enabled_plugins: BTreeMap<String, bool>,
}

impl McpSettings {
    fn absorb(&mut self, v: &Value) {
        self.enabled_mcpjson.extend(str_list(v.get("enabledMcpjsonServers")));
        self.disabled_mcpjson.extend(str_list(v.get("disabledMcpjsonServers")));
        if let Some(b) = v.get("enableAllProjectMcpServers").and_then(Value::as_bool) {
            self.enable_all_project = b;
        }
        if let Some(a) = v.get("deniedMcpServers").and_then(Value::as_array) {
            self.denied.extend(a.iter().cloned());
        }
        if let Some(a) = v.get("allowedMcpServers").and_then(Value::as_array) {
            self.allowed.get_or_insert_with(Vec::new).extend(a.iter().cloned());
        }
        if let Some(o) = v.get("enabledPlugins").and_then(Value::as_object) {
            for (k, b) in o {
                self.enabled_plugins.insert(k.clone(), b.as_bool().unwrap_or(false));
            }
        }
    }
}

/// `*` wildcard match (Claude's `serverUrl` patterns).
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn rule_matches(rule: &Value, name: &str, def: &Value) -> bool {
    if let Some(n) = rule.get("serverName").and_then(Value::as_str) {
        return n == name;
    }
    if let (Some(p), Some(u)) = (rule.get("serverUrl").and_then(Value::as_str), def.get("url").and_then(Value::as_str)) {
        return wildcard(p, u);
    }
    if let (Some(c), Some(cmd)) = (rule.get("serverCommand").and_then(Value::as_array), def.get("command").and_then(Value::as_str)) {
        let mut argv = vec![Value::String(cmd.to_string())];
        argv.extend(def.get("args").and_then(Value::as_array).cloned().unwrap_or_default());
        return *c == argv;
    }
    false
}

/// `scheme://host[:port]` of a URL, without path, query or credentials.
fn origin(url: &str) -> String {
    if url.contains("${") {
        return match reqwest::Url::parse(url) {
            Ok(u) if u.host_str().is_some_and(|h| !h.contains('$')) => format!("{} (uses variables)", u.origin().ascii_serialization()),
            _ => "(set from environment variables)".into(),
        };
    }
    match reqwest::Url::parse(url) {
        Ok(u) => u.origin().ascii_serialization(),
        Err(_) => "(invalid URL)".into(),
    }
}

fn keys(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

fn describe(name: &str, def: &Value, scope: &'static str, source: &Path, plugin: Option<&str>) -> ServerEntry {
    let url = def.get("url").and_then(Value::as_str);
    let command = def.get("command").and_then(Value::as_str);
    let transport = match def.get("type").and_then(Value::as_str) {
        Some("streamable-http") => "http".to_string(),
        Some(t) => t.to_string(),
        None if url.is_some() => "http".into(),
        None => "stdio".into(),
    };
    let mut header_names = keys(def.get("headers"));
    if def.get("headersHelper").is_some() {
        header_names.push("(headersHelper)".into());
    }
    ServerEntry {
        name: name.to_string(),
        scope,
        source: contract_tilde(source),
        plugin: plugin.map(str::to_string),
        transport,
        endpoint: url.map(origin),
        command: command.map(|c| Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| c.to_string())),
        args: def.get("args").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        env_names: keys(def.get("env")),
        header_names,
        status: "enabled",
        reason: None,
    }
}

fn server_map(v: Option<&Value>) -> Vec<(String, Value)> {
    v.and_then(Value::as_object)
        .map(|o| o.iter().filter(|(_, d)| d.is_object()).map(|(k, d)| (k.clone(), d.clone())).collect())
        .unwrap_or_default()
}

/// Plugin servers: `.claude-plugin/plugin.json` `mcpServers` (object, or a path
/// to a JSON file inside the plugin) or `<plugin>/.mcp.json`.
fn plugin_servers(install: &Path, files: &mut Vec<FileNote>) -> Vec<(String, Value, PathBuf)> {
    let manifest_path = install.join(".claude-plugin").join("plugin.json");
    let manifest = if manifest_path.is_file() { read_json(&manifest_path, files) } else { None };
    let (value, source) = match manifest.as_ref().and_then(|m| m.get("mcpServers")) {
        Some(Value::Object(_)) => (manifest.as_ref().and_then(|m| m.get("mcpServers")).cloned(), manifest_path),
        Some(Value::String(rel)) => {
            let p = install.join(rel.trim_start_matches("./"));
            // Stay inside the plugin directory.
            use crate::util::os::path::{canonicalize, starts_with};
            let inside = canonicalize(&p).ok().zip(canonicalize(install).ok()).is_some_and(|(p, root)| starts_with(&p, &root));
            if !inside {
                return vec![];
            }
            let v = read_json(&p, files);
            (v.map(|v| v.get("mcpServers").cloned().unwrap_or(v)), p)
        }
        _ => {
            let p = install.join(".mcp.json");
            if !p.is_file() {
                return vec![];
            }
            let v = read_json(&p, files);
            (v.map(|v| v.get("mcpServers").cloned().unwrap_or(v)), p)
        }
    };
    server_map(value.as_ref()).into_iter().map(|(n, d)| (n, d, source.clone())).collect()
}

/// Collect every server Claude Code would see for `root` (or only user-level ones).
pub fn scan(loc: &Locations, root: Option<&Path>) -> Overview {
    let mut ov = Overview::default();
    let files = &mut ov.files;

    let claude_json = read_json(&loc.claude_json, files).unwrap_or(Value::Null);
    ov.account_connectors_used = claude_json.get("claudeAiMcpEverConnected").and_then(Value::as_bool).unwrap_or(false);
    let project_entry = root.and_then(|r| {
        let projects = claude_json.get("projects")?.as_object()?;
        use crate::util::os::path::{canonicalize, same_dir, to_slash};
        // Claude Code writes Windows keys with `/` (`C:/Users/me/proj`), in the case of the
        // folder it was started in (`c:/users/me/proj` too): exact keys first, then any key
        // naming the same folder (Windows: without regard to case or separators).
        let key = to_slash(r);
        let canon = canonicalize(r).ok().map(|c| to_slash(&c));
        projects
            .get(key.as_str())
            .or_else(|| canon.as_deref().and_then(|c| projects.get(c)))
            .or_else(|| projects.get(format!("{}/", key.trim_end_matches('/')).as_str()))
            .or_else(|| projects.iter().find(|(k, _)| same_dir(k, &key) || canon.as_deref().is_some_and(|c| same_dir(k, c))).map(|(_, v)| v))
            .cloned()
    });

    let mut settings = McpSettings::default();
    // (file, the project it belongs to)
    let mut setting_files = vec![(loc.claude_dir.join("settings.json"), None)];
    if let Some(r) = root {
        setting_files.push((r.join(".claude").join("settings.json"), Some(r)));
        setting_files.push((r.join(".claude").join("settings.local.json"), Some(r)));
    }
    setting_files.push((loc.managed_dir.join("managed-settings.json"), None));
    for (f, project) in &setting_files {
        let v = match project {
            Some(r) => read_project_json(r, f, files),
            None => read_json(f, files),
        };
        if let Some(v) = v {
            settings.absorb(&v);
        }
    }
    if let Some(p) = &project_entry {
        settings.enabled_mcpjson.extend(str_list(p.get("enabledMcpjsonServers")));
        settings.disabled_mcpjson.extend(str_list(p.get("disabledMcpjsonServers")));
    }
    let disabled_local: BTreeSet<String> = project_entry.as_ref().map(|p| str_list(p.get("disabledMcpServers"))).unwrap_or_default().into_iter().collect();

    let mut raw: Vec<(ServerEntry, Value)> = vec![];
    for (n, d) in server_map(claude_json.get("mcpServers")) {
        raw.push((describe(&n, &d, "user", &loc.claude_json, None), d));
    }
    if let Some(p) = &project_entry {
        for (n, d) in server_map(p.get("mcpServers")) {
            raw.push((describe(&n, &d, "local", &loc.claude_json, None), d));
        }
    }
    if let Some(r) = root {
        let path = r.join(".mcp.json");
        if let Some(v) = read_project_json(r, &path, files) {
            for (n, d) in server_map(v.get("mcpServers")) {
                raw.push((describe(&n, &d, "project", &path, None), d));
            }
        }
    }
    let managed = loc.managed_dir.join("managed-mcp.json");
    if managed.is_file() {
        if let Some(v) = read_json(&managed, files) {
            for (n, d) in server_map(v.get("mcpServers")) {
                raw.push((describe(&n, &d, "managed", &managed, None), d));
            }
        }
    }

    // Plugins.
    let installed = read_json(&loc.claude_dir.join("plugins").join("installed_plugins.json"), files).unwrap_or(Value::Null);
    let plugins_root = crate::util::os::path::canonicalize(loc.claude_dir.join("plugins")).ok();
    if let Some(plugins) = installed.get("plugins").and_then(Value::as_object) {
        for (id, entries) in plugins {
            let Some(entries) = entries.as_array() else { continue };
            let entry = entries.iter().find(|e| {
                match e.get("projectPath").and_then(Value::as_str) {
                    None => true,
                    Some(pp) => root.is_some_and(|r| Path::new(pp) == r),
                }
            });
            let Some(install) = entry.and_then(|e| e.get("installPath")).and_then(Value::as_str) else { continue };
            let install = PathBuf::from(install);
            // Only read plugins installed under Claude's own plugin directory.
            let inside = crate::util::os::path::canonicalize(&install).ok().zip(plugins_root.clone()).is_some_and(|(p, r)| crate::util::os::path::starts_with(&p, &r));
            if !inside {
                continue;
            }
            let plugin_name = id.split('@').next().unwrap_or(id);
            for (n, d, source) in plugin_servers(&install, files) {
                let mut e = describe(&format!("plugin:{plugin_name}:{n}"), &d, "plugin", &source, Some(id));
                if settings.enabled_plugins.get(id) != Some(&true) {
                    e.status = "disabled";
                    e.reason = Some(format!("plugin {id} is not enabled"));
                }
                raw.push((e, d));
            }
        }
    }

    for (mut e, def) in raw {
        let bare = e.name.rsplit(':').next().unwrap_or(&e.name).to_string();
        if settings.denied.iter().any(|r| rule_matches(r, &bare, &def)) {
            e.status = "denied";
            e.reason = Some("listed in deniedMcpServers".into());
        } else if settings.allowed.as_ref().is_some_and(|a| !a.iter().any(|r| rule_matches(r, &bare, &def))) {
            e.status = "denied";
            e.reason = Some("not in allowedMcpServers".into());
        } else if e.scope == "project" {
            if settings.disabled_mcpjson.contains(&e.name) {
                e.status = "disabled";
                e.reason = Some("in disabledMcpjsonServers".into());
            } else if !(settings.enable_all_project || settings.enabled_mcpjson.contains(&e.name)) {
                e.status = "needs-approval";
                e.reason = Some("Claude asks before using project servers".into());
            }
        } else if matches!(e.scope, "user" | "local") && disabled_local.contains(&e.name) {
            e.status = "disabled";
            e.reason = Some("disabled for this project".into());
        }
        ov.servers.push(e);
    }
    ov
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServersQuery {
    project_id: Option<String>,
}

/// `GET /api/platform/mcp-servers?projectId=`
pub async fn route(State(state): State<AppState>, Query(q): Query<McpServersQuery>) -> ApiResult<Json<Value>> {
    let root = match q.project_id.as_deref().filter(|p| !p.is_empty()) {
        Some(pid) => Some(state.projects.require(pid)?.root.clone()),
        None => None,
    };
    let overview = tokio::task::spawn_blocking(move || scan(&Locations::from_env(), root.as_deref()))
        .await
        .map_err(|e| ApiError::internal(format!("scan failed: {e}")))?;
    let mut out: Map<String, Value> = serde_json::to_value(&overview)?.as_object().cloned().unwrap_or_default();
    out.insert(
        "workbench".into(),
        json!({
            "name": "workbench",
            "endpoint": format!("{}/mcp", state.local_base_url()),
            "note": "Injected into every Claude session Workbench starts (--mcp-config), authenticated with that session's agent token.",
            "tools": super::tools::tools_summary(&state),
        }),
    );
    out.insert(
        "notes".into(),
        json!([
            "claude.ai connectors (for example Atlassian Rovo or Gmail) belong to your Claude account and are not stored in local files, so they are not listed here.",
            "Values of environment variables and headers are never shown; only their names."
        ]),
    );
    Ok(Json(Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, v: &Value) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_vec_pretty(v).unwrap()).unwrap();
    }

    #[test]
    fn scans_all_scopes_without_leaking_values() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let root = crate::util::os::path::canonicalize(repo.path()).unwrap();
        let claude_dir = home.path().join(".claude");
        let loc = Locations {
            claude_json: home.path().join(".claude.json"),
            claude_dir: claude_dir.clone(),
            managed_dir: home.path().join("managed"),
        };
        write(
            &loc.claude_json,
            &json!({
                "mcpServers": {
                    "unity-mcp": { "command": "/opt/unity/bin/unity-mcp", "args": ["--token", "SECRETARG"], "env": { "UNITY_KEY": "SECRETENV" } },
                    "clion": { "type": "http", "url": "http://127.0.0.1:64342/sse?token=SECRETQ", "headers": { "Authorization": "Bearer SECRETH" } }
                },
                "projects": { crate::util::os::path::to_slash(&root): {
                    "mcpServers": { "local-one": { "type": "sse", "url": "https://user:pw@mcp.example.com/x" } },
                    "enabledMcpjsonServers": ["blender"],
                    "disabledMcpjsonServers": ["clion-proj"]
                }},
                "claudeAiMcpEverConnected": true,
                "oauthAccount": { "emailAddress": "SECRETMAIL" }
            }),
        );
        write(
            &root.join(".mcp.json"),
            &json!({ "mcpServers": {
                "blender": { "type": "stdio", "command": "uvx", "args": ["blender-mcp"] },
                "clion-proj": { "type": "http", "url": "http://localhost:1/x" },
                "newbie": { "command": "node", "args": ["srv.js"] }
            }}),
        );
        write(&root.join(".claude").join("settings.local.json"), &json!({ "deniedMcpServers": [{ "serverName": "newbie" }] }));
        write(&claude_dir.join("settings.json"), &json!({ "enabledPlugins": { "stripe@official": true, "off@official": false } }));
        let stripe = claude_dir.join("plugins/cache/official/stripe/1.0");
        let off = claude_dir.join("plugins/cache/official/off/1.0");
        write(&stripe.join(".mcp.json"), &json!({ "stripe": { "type": "http", "url": "https://mcp.stripe.com/v1?k=SECRETP" } }));
        write(&off.join(".claude-plugin/plugin.json"), &json!({ "name": "off", "mcpServers": { "offsrv": { "command": "off" } } }));
        write(
            &claude_dir.join("plugins/installed_plugins.json"),
            &json!({ "version": 2, "plugins": {
                "stripe@official": [{ "scope": "user", "installPath": stripe }],
                "off@official": [{ "scope": "user", "installPath": off }],
                "evil@x": [{ "scope": "user", "installPath": "/etc" }]
            }}),
        );

        let ov = scan(&loc, Some(&root));
        let text = serde_json::to_string(&ov).unwrap();
        for leak in ["SECRETARG", "SECRETENV", "SECRETQ", "SECRETH", "SECRETP", "SECRETMAIL", "user:pw", "/opt/unity"] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        let get = |n: &str| ov.servers.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("{n} missing: {text}"));
        let unity = get("unity-mcp");
        assert_eq!((unity.scope, unity.transport.as_str(), unity.command.as_deref(), unity.args), ("user", "stdio", Some("unity-mcp"), 2));
        assert_eq!(unity.env_names, vec!["UNITY_KEY"]);
        let clion = get("clion");
        assert_eq!(clion.endpoint.as_deref(), Some("http://127.0.0.1:64342"));
        assert_eq!(clion.header_names, vec!["Authorization"]);
        assert_eq!(get("local-one").endpoint.as_deref(), Some("https://mcp.example.com"));
        assert_eq!(get("local-one").scope, "local");
        assert_eq!(get("blender").status, "enabled");
        assert_eq!(get("clion-proj").status, "disabled");
        assert_eq!(get("newbie").status, "denied");
        assert_eq!(get("plugin:stripe:stripe").status, "enabled");
        assert_eq!(get("plugin:off:offsrv").status, "disabled");
        assert!(!ov.servers.iter().any(|s| s.name.contains("evil")));
        assert!(ov.account_connectors_used);
        // Without a project only user-level servers (and plugins) appear.
        let ov = scan(&loc, None);
        assert!(!ov.servers.iter().any(|s| s.scope == "project" || s.scope == "local"));
    }

    /// A `~/.claude.json` project key names the project's folder as this OS compares paths:
    /// on Windows in any case, with `/` or `\` and a trailing separator; on Linux exactly.
    #[test]
    fn project_keys_match_as_this_os_compares_folders() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let root = crate::util::os::path::canonicalize(repo.path()).unwrap();
        let loc = Locations {
            claude_json: home.path().join(".claude.json"),
            claude_dir: home.path().join(".claude"),
            managed_dir: home.path().join("managed"),
        };
        let upper = crate::util::os::path::to_slash(&root).to_ascii_uppercase();
        let lower_backslashed = format!("{}\\", root.display()).to_ascii_lowercase();
        for key in [upper, lower_backslashed] {
            write(&loc.claude_json, &json!({ "projects": { key.clone(): { "mcpServers": { "local-one": { "command": "x" } } } } }));
            let found = scan(&loc, Some(&root)).servers.iter().any(|s| s.name == "local-one" && s.scope == "local");
            assert_eq!(found, cfg!(windows), "{key}");
        }
    }

    #[test]
    fn project_servers_need_approval_by_default() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let loc = Locations {
            claude_json: home.path().join(".claude.json"),
            claude_dir: home.path().join(".claude"),
            managed_dir: home.path().join("managed"),
        };
        write(&repo.path().join(".mcp.json"), &json!({ "mcpServers": { "x": { "command": "x" } } }));
        let ov = scan(&loc, Some(repo.path()));
        assert_eq!(ov.servers[0].status, "needs-approval");
        assert!(ov.files.iter().any(|f| f.status == "missing"));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("https://*.example.com/*", "https://mcp.example.com/v1"));
        assert!(!wildcard("https://*.example.com/*", "https://evil.com/v1"));
        assert!(wildcard("exact", "exact"));
        assert_eq!(origin("https://${HOST}/mcp"), "(set from environment variables)");
    }
}
