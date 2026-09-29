//! `devcontainer.json`: discovery, parsing and variable substitution.
//!
//! Supported (https://containers.dev/implementors/json_reference/): `name`, `image`,
//! `build {dockerfile, context, args, target, cacheFrom, options}` (and the legacy
//! top-level `dockerFile`/`context`), `dockerComposeFile` + `service`, `runServices`,
//! `workspaceFolder`, `workspaceMount`, `mounts` (string and object forms), `runArgs`,
//! `containerEnv`, `remoteEnv`, `remoteUser`, `containerUser`, `updateRemoteUserUID`,
//! `forwardPorts`, `appPort`, `portsAttributes`, `overrideCommand`, `shutdownAction`,
//! `init`, `privileged`, `capAdd`, `securityOpt`, `features` (CLI engine only),
//! `initializeCommand` (on the host) and the lifecycle commands `onCreateCommand`,
//! `updateContentCommand`, `postCreateCommand`, `postStartCommand`, `postAttachCommand`
//! (a string, an array or an object of parallel commands).
//!
//! Variables: `${localWorkspaceFolder}`, `${localWorkspaceFolderBasename}`,
//! `${containerWorkspaceFolder}`, `${containerWorkspaceFolderBasename}`,
//! `${localEnv:VAR[:default]}`, `${containerEnv:VAR}` (in `remoteEnv`, resolved against
//! the running container) and `${devcontainerId}`.
//!
//! A config is parsed twice: for display (and the approval hash) `${localEnv:…}` stays
//! as written, so host values never reach the browser; for execution it is resolved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::jsonc;

/// Largest `devcontainer.json` read.
pub const MAX_CONFIG: u64 = 1024 * 1024;

/// `devcontainer.json` files of a project, project-relative, in the order the spec
/// looks for them: `.devcontainer/devcontainer.json`, `.devcontainer.json`, then
/// `.devcontainer/<name>/devcontainer.json` by name. Symlinks leaving the project are
/// ignored.
pub fn discover(root: &Path) -> Vec<String> {
    let mut out = vec![];
    for rel in [".devcontainer/devcontainer.json", ".devcontainer.json"] {
        if contained_file(root, rel) {
            out.push(rel.to_string());
        }
    }
    if let Ok(rd) = std::fs::read_dir(root.join(".devcontainer")) {
        let mut names: Vec<String> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| !n.starts_with('.') && !n.contains(['\n', '\r']))
            .collect();
        names.sort();
        for n in names.into_iter().take(50) {
            let rel = format!(".devcontainer/{n}/devcontainer.json");
            if contained_file(root, &rel) {
                out.push(rel);
            }
        }
    }
    out
}

fn contained_file(root: &Path, rel: &str) -> bool {
    crate::util::paths::resolve_in_root(root, rel).is_ok_and(|p| p.is_file())
}

/// Whether `rel` is one of the project's configs (requests may only name those).
pub fn is_config(root: &Path, rel: &str) -> bool {
    discover(root).iter().any(|c| c == rel)
}

// ---------------------------------------------------------------- model

/// `rename_all` on an enum renames only the variants (the `kind` tag): the struct
/// variants carry their own so their fields reach the web client camelCased too.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Source {
    Image {
        image: String,
    },
    #[serde(rename_all = "camelCase")]
    Dockerfile {
        /// Absolute path of the Dockerfile.
        dockerfile: String,
        /// Absolute path of the build context.
        context: String,
        args: BTreeMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        cache_from: Vec<String>,
        /// `build.options`: extra `docker build` flags.
        options: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Compose {
        /// Absolute paths of the compose files.
        files: Vec<String>,
        service: String,
        run_services: Vec<String>,
    },
    /// Neither an image, a Dockerfile nor compose files: not startable.
    None,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    /// `bind`, `volume` or `tmpfs`.
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    pub target: String,
    pub readonly: bool,
    /// The `--mount` value.
    pub spec: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Cmd {
    /// Run through `/bin/sh -c`.
    Shell(String),
    /// Run directly, no shell.
    Exec(Vec<String>),
}

impl Cmd {
    /// For display.
    pub fn text(&self) -> String {
        match self {
            Cmd::Shell(s) => s.clone(),
            Cmd::Exec(v) => v.iter().map(|a| super::sh_quote(a)).collect::<Vec<_>>().join(" "),
        }
    }
}

/// One lifecycle hook: one command, or several run in parallel (object form).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Lifecycle {
    pub commands: Vec<(Option<String>, Cmd)>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortSpec {
    /// A compose service (`db:5432`) or a host address (`127.0.0.1:3000:3000`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The host port asked for (`appPort: "8000:3000"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DevConfig {
    /// Project-relative path of the file.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub source: Source,
    /// Where the project opens inside the container.
    pub workspace_folder: String,
    /// The workspace bind mount (`None` for compose: the compose file mounts it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_mount: Option<Mount>,
    pub mounts: Vec<Mount>,
    pub run_args: Vec<String>,
    pub container_env: BTreeMap<String, String>,
    /// `None` unsets the variable.
    pub remote_env: BTreeMap<String, Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_user: Option<String>,
    pub update_remote_user_uid: bool,
    pub forward_ports: Vec<PortSpec>,
    pub app_ports: Vec<PortSpec>,
    pub override_command: bool,
    pub shutdown_action: String,
    pub init: bool,
    pub privileged: bool,
    pub cap_add: Vec<String>,
    pub security_opt: Vec<String>,
    /// Feature id → options.
    pub features: BTreeMap<String, Value>,
    pub initialize_command: Option<Lifecycle>,
    pub on_create_command: Option<Lifecycle>,
    pub update_content_command: Option<Lifecycle>,
    pub post_create_command: Option<Lifecycle>,
    pub post_start_command: Option<Lifecycle>,
    pub post_attach_command: Option<Lifecycle>,
    /// Host environment variables the config reads (`${localEnv:…}`).
    pub local_env: BTreeSet<String>,
    /// Keys Workbench does not act on, and other remarks.
    pub notes: Vec<String>,
}

impl DevConfig {
    pub fn is_compose(&self) -> bool {
        matches!(self.source, Source::Compose { .. })
    }

    /// Every lifecycle hook with its devcontainer.json key, in execution order.
    pub fn lifecycle(&self) -> Vec<(&'static str, &Lifecycle)> {
        [
            ("onCreateCommand", &self.on_create_command),
            ("updateContentCommand", &self.update_content_command),
            ("postCreateCommand", &self.post_create_command),
            ("postStartCommand", &self.post_start_command),
            ("postAttachCommand", &self.post_attach_command),
        ]
        .into_iter()
        .filter_map(|(k, l)| l.as_ref().map(|l| (k, l)))
        .collect()
    }
}

/// How `${localEnv:…}` is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalEnv {
    /// Left as written (display, approval hash).
    Keep,
    /// Replaced by this process's environment (execution).
    Resolve,
}

// ---------------------------------------------------------------- variables

/// The `${devcontainerId}` of a container with these id labels, computed like the
/// devcontainer CLI: sha256 of the sorted labels as JSON, in base 32, 52 digits.
pub fn devcontainer_id(local_folder: &str, config_file: &str) -> String {
    let mut labels = BTreeMap::new();
    labels.insert("devcontainer.config_file", config_file);
    labels.insert("devcontainer.local_folder", local_folder);
    let json = serde_json::to_string(&labels).unwrap_or_default();
    let hash = Sha256::digest(json.as_bytes());
    // 256 bits read as 52 five-bit digits (4 leading zero bits), most significant first.
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
    let bit = |i: usize| -> u8 {
        // Bit `i` of the 260-bit number (0 = most significant); the first 4 are zero.
        if i < 4 {
            return 0;
        }
        let j = i - 4;
        (hash[j / 8] >> (7 - (j % 8))) & 1
    };
    (0..52)
        .map(|d| {
            let v = (0..5).fold(0u8, |acc, k| (acc << 1) | bit(d * 5 + k));
            DIGITS[v as usize] as char
        })
        .collect()
}

pub struct Vars<'a> {
    pub local_folder: &'a str,
    pub container_folder: Option<&'a str>,
    pub devcontainer_id: &'a str,
    pub local_env: LocalEnv,
    /// For `${containerEnv:…}` (remoteEnv at exec time).
    pub container_env: Option<&'a BTreeMap<String, String>>,
}

/// The last name of a container path.
fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// The last name of a path on this computer (`${localWorkspaceFolderBasename}`, the default
/// `/workspaces/<name>`): its separators are this OS's (`\` too on Windows).
fn local_basename(p: &str) -> &str {
    crate::util::os::path::segments(p).filter(|s| !s.is_empty()).last().unwrap_or("")
}

/// Substitute variables in `s`. Unknown variables stay as written.
pub fn substitute(s: &str, v: &Vars, used_env: &mut BTreeSet<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let whole = &rest[start..start + 2 + end + 1];
        let replacement: Option<String> = match inner {
            "localWorkspaceFolder" => Some(v.local_folder.to_string()),
            "localWorkspaceFolderBasename" => Some(local_basename(v.local_folder).to_string()),
            "containerWorkspaceFolder" => v.container_folder.map(str::to_string),
            "containerWorkspaceFolderBasename" => v.container_folder.map(|f| basename(f).to_string()),
            "devcontainerId" => Some(v.devcontainer_id.to_string()),
            _ => {
                if let Some(spec) = inner.strip_prefix("localEnv:").or_else(|| inner.strip_prefix("env:")) {
                    let (name, default) = match spec.split_once(':') {
                        Some((n, d)) => (n, Some(d)),
                        None => (spec, None),
                    };
                    used_env.insert(name.to_string());
                    match v.local_env {
                        LocalEnv::Keep => None,
                        LocalEnv::Resolve => Some(
                            std::env::var(name).ok().filter(|x| !x.is_empty()).or(default.map(str::to_string)).unwrap_or_default(),
                        ),
                    }
                } else if let Some(spec) = inner.strip_prefix("containerEnv:") {
                    let (name, default) = match spec.split_once(':') {
                        Some((n, d)) => (n, Some(d)),
                        None => (spec, None),
                    };
                    v.container_env.map(|env| env.get(name).cloned().or(default.map(str::to_string)).unwrap_or_default())
                } else {
                    None
                }
            }
        };
        match replacement {
            Some(r) => out.push_str(&r),
            None => out.push_str(whole),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Substitute in every string of a JSON value.
fn substitute_value(val: &Value, v: &Vars, used: &mut BTreeSet<String>) -> Value {
    match val {
        Value::String(s) => Value::String(substitute(s, v, used)),
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute_value(x, v, used)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute_value(x, v, used))).collect()),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------- parsing

/// Keys acted on (or deliberately shown). Anything else is noted.
const KNOWN_KEYS: &[&str] = &[
    "name", "image", "build", "dockerFile", "context", "dockerComposeFile", "service", "runServices", "workspaceFolder",
    "workspaceMount", "mounts", "runArgs", "containerEnv", "remoteEnv", "remoteUser", "containerUser", "updateRemoteUserUID",
    "forwardPorts", "appPort", "portsAttributes", "otherPortsAttributes", "overrideCommand", "shutdownAction", "init",
    "privileged", "capAdd", "securityOpt", "features", "overrideFeatureInstallOrder", "initializeCommand", "onCreateCommand",
    "updateContentCommand", "postCreateCommand", "postStartCommand", "postAttachCommand", "customizations", "$schema",
    "waitFor", "userEnvProbe", "hostRequirements", "remoteUser",
];

/// Parse the config at `rel` (project-relative) of the project at `root`.
pub fn load(root: &Path, rel: &str, local_env: LocalEnv) -> Result<DevConfig, String> {
    let abs = crate::util::paths::resolve_in_root(root, rel).map_err(|e| e.message)?;
    let meta = std::fs::metadata(&abs).map_err(|e| format!("{rel}: {e}"))?;
    if meta.len() > MAX_CONFIG {
        return Err(format!("{rel} is larger than 1 MB"));
    }
    let text = std::fs::read_to_string(&abs).map_err(|e| format!("{rel}: {e}"))?;
    let json = jsonc::parse(&text).map_err(|e| format!("{rel} does not parse: {e}"))?;
    parse(root, rel, &abs, &json, local_env)
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty())
}

fn bool_of(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}

fn strings_of(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    }
}

/// `path` relative to `base`, normalized lexically (`..` allowed: contexts often are).
fn join_norm(base: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `rel` against `dir`, lexically normalized, as a string.
pub fn parse_path_rel(dir: &Path, rel: &str) -> String {
    join_norm(dir, rel).display().to_string()
}

pub fn parse(root: &Path, rel: &str, abs: &Path, raw: &Value, local_env: LocalEnv) -> Result<DevConfig, String> {
    let obj = raw.as_object().ok_or_else(|| format!("{rel}: expected a JSON object"))?;
    let local_folder = root.display().to_string();
    let config_file = abs.display().to_string();
    let id = devcontainer_id(&local_folder, &config_file);
    let dir = abs.parent().unwrap_or(root).to_path_buf();
    let mut used_env = BTreeSet::new();
    let mut notes = vec![];

    // The workspace folder first: other values may name it.
    let pre = Vars { local_folder: &local_folder, container_folder: None, devcontainer_id: &id, local_env, container_env: None };
    let compose_files = strings_of(raw.get("dockerComposeFile"));
    let is_compose = !compose_files.is_empty();
    let default_folder = if is_compose { "/".to_string() } else { format!("/workspaces/{}", local_basename(&local_folder)) };
    let workspace_folder = str_of(raw, "workspaceFolder").map(|s| substitute(&s, &pre, &mut used_env)).unwrap_or(default_folder);
    let vars = Vars { container_folder: Some(&workspace_folder), ..pre };
    let raw = substitute_value(raw, &vars, &mut used_env);

    for k in obj.keys() {
        if !KNOWN_KEYS.contains(&k.as_str()) {
            notes.push(format!("{k} is not used by Workbench"));
        }
    }

    let build = raw.get("build").cloned().unwrap_or(Value::Null);
    let dockerfile = str_of(&build, "dockerfile").or_else(|| str_of(&raw, "dockerFile"));
    let source = if is_compose {
        let service = str_of(&raw, "service").ok_or_else(|| format!("{rel}: dockerComposeFile needs \"service\""))?;
        Source::Compose {
            files: compose_files.iter().map(|f| join_norm(&dir, &substitute(f, &vars, &mut used_env)).display().to_string()).collect(),
            service,
            run_services: strings_of(raw.get("runServices")),
        }
    } else if let Some(df) = dockerfile {
        let context = str_of(&build, "context").or_else(|| str_of(&raw, "context")).unwrap_or_else(|| ".".into());
        let args: BTreeMap<String, String> = build
            .get("args")
            .and_then(Value::as_object)
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
            .unwrap_or_default();
        Source::Dockerfile {
            dockerfile: join_norm(&dir, &df).display().to_string(),
            context: join_norm(&dir, &context).display().to_string(),
            args,
            target: str_of(&build, "target"),
            cache_from: strings_of(build.get("cacheFrom")),
            options: strings_of(build.get("options")),
        }
    } else if let Some(image) = str_of(&raw, "image") {
        Source::Image { image }
    } else {
        notes.push("no image, build.dockerfile or dockerComposeFile: nothing to start".into());
        Source::None
    };

    let workspace_mount = if is_compose {
        None
    } else {
        match str_of(&raw, "workspaceMount") {
            Some(m) => Some(parse_mount_str(&m).ok_or_else(|| format!("{rel}: workspaceMount {m:?} is not a mount"))?),
            None => {
                let target = format!("/workspaces/{}", local_basename(&local_folder));
                let spec = format!("type=bind,source={local_folder},target={target}");
                Some(Mount { kind: "bind".into(), source: local_folder.clone(), target, readonly: false, spec })
            }
        }
    };

    let mut mounts = vec![];
    if let Some(Value::Array(a)) = raw.get("mounts") {
        for m in a {
            let parsed = match m {
                Value::String(s) => parse_mount_str(s),
                Value::Object(_) => parse_mount_obj(m),
                _ => None,
            };
            match parsed {
                Some(m) => mounts.push(m),
                None => notes.push(format!("mount {m} is not understood and is skipped")),
            }
        }
    }

    let env_map = |k: &str| -> BTreeMap<String, String> {
        raw.get(k)
            .and_then(Value::as_object)
            .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
            .unwrap_or_default()
    };
    let container_env = env_map("containerEnv");
    let remote_env: BTreeMap<String, Option<String>> = raw
        .get("remoteEnv")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect())
        .unwrap_or_default();
    for k in container_env.keys().chain(remote_env.keys()) {
        if !valid_env_name(k) {
            return Err(format!("{rel}: {k:?} is not a valid environment variable name"));
        }
    }

    let labels: BTreeMap<String, String> = raw
        .get("portsAttributes")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.get("label").and_then(Value::as_str).map(|l| (k.clone(), l.to_string()))).collect())
        .unwrap_or_default();
    let label = |port: u16| labels.get(&port.to_string()).cloned();
    let mut forward_ports = vec![];
    if let Some(Value::Array(a)) = raw.get("forwardPorts") {
        for p in a {
            match parse_forward_port(p) {
                Some(mut spec) => {
                    spec.label = label(spec.port);
                    forward_ports.push(spec);
                }
                None => notes.push(format!("forwardPorts entry {p} is not a port")),
            }
        }
    }
    let mut app_ports = vec![];
    let app = match raw.get("appPort") {
        Some(Value::Array(a)) => a.clone(),
        Some(v) if !v.is_null() => vec![v.clone()],
        _ => vec![],
    };
    for p in &app {
        match parse_app_port(p) {
            Some(mut spec) => {
                spec.label = label(spec.port);
                app_ports.push(spec);
            }
            None => notes.push(format!("appPort entry {p} is not a port")),
        }
    }

    let features: BTreeMap<String, Value> = raw.get("features").and_then(Value::as_object).map(|o| o.clone().into_iter().collect()).unwrap_or_default();
    let lifecycle = |k: &str| raw.get(k).and_then(parse_lifecycle);

    Ok(DevConfig {
        path: rel.to_string(),
        name: str_of(&raw, "name"),
        source,
        workspace_folder,
        workspace_mount,
        mounts,
        run_args: strings_of(raw.get("runArgs")),
        container_env,
        remote_env,
        remote_user: str_of(&raw, "remoteUser"),
        container_user: str_of(&raw, "containerUser"),
        update_remote_user_uid: bool_of(&raw, "updateRemoteUserUID").unwrap_or(true),
        forward_ports,
        app_ports,
        override_command: bool_of(&raw, "overrideCommand").unwrap_or(!is_compose),
        shutdown_action: str_of(&raw, "shutdownAction").unwrap_or_else(|| if is_compose { "stopCompose".into() } else { "stopContainer".into() }),
        init: bool_of(&raw, "init").unwrap_or(false),
        privileged: bool_of(&raw, "privileged").unwrap_or(false),
        cap_add: strings_of(raw.get("capAdd")),
        security_opt: strings_of(raw.get("securityOpt")),
        features,
        initialize_command: lifecycle("initializeCommand"),
        on_create_command: lifecycle("onCreateCommand"),
        update_content_command: lifecycle("updateContentCommand"),
        post_create_command: lifecycle("postCreateCommand"),
        post_start_command: lifecycle("postStartCommand"),
        post_attach_command: lifecycle("postAttachCommand"),
        local_env: used_env,
        notes,
    })
}

pub fn valid_env_name(k: &str) -> bool {
    let mut c = k.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

fn parse_lifecycle(v: &Value) -> Option<Lifecycle> {
    let one = |v: &Value| -> Option<Cmd> {
        match v {
            Value::String(s) if !s.trim().is_empty() => Some(Cmd::Shell(s.clone())),
            Value::Array(a) => {
                let argv: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
                (!argv.is_empty()).then_some(Cmd::Exec(argv))
            }
            _ => None,
        }
    };
    let commands: Vec<(Option<String>, Cmd)> = match v {
        Value::Object(o) => o.iter().filter_map(|(k, x)| one(x).map(|c| (Some(k.clone()), c))).collect(),
        other => one(other).map(|c| vec![(None, c)]).unwrap_or_default(),
    };
    (!commands.is_empty()).then_some(Lifecycle { commands })
}

/// `source=/a,target=/b,type=bind,consistency=cached` (also `src`, `dst`,
/// `destination`, `readonly`/`ro`).
pub fn parse_mount_str(s: &str) -> Option<Mount> {
    let (mut kind, mut source, mut target, mut readonly) = (String::from("volume"), String::new(), String::new(), false);
    for part in s.split(',') {
        let (k, v) = part.split_once('=').map(|(k, v)| (k.trim(), v.trim())).unwrap_or((part.trim(), ""));
        match k {
            "type" => kind = v.to_string(),
            "source" | "src" => source = v.to_string(),
            "target" | "dst" | "destination" => target = v.to_string(),
            "readonly" | "ro" => readonly = v.is_empty() || v == "true" || v == "1",
            _ => {}
        }
    }
    if target.is_empty() {
        return None;
    }
    Some(Mount { kind, source, target, readonly, spec: s.to_string() })
}

fn parse_mount_obj(v: &Value) -> Option<Mount> {
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("volume").to_string();
    let source = v.get("source").and_then(Value::as_str).unwrap_or("").to_string();
    let target = v.get("target").and_then(Value::as_str)?.to_string();
    let mut spec = format!("type={kind}");
    if !source.is_empty() {
        spec.push_str(&format!(",source={source}"));
    }
    spec.push_str(&format!(",target={target}"));
    Some(Mount { kind, source, target, readonly: false, spec })
}

fn parse_forward_port(v: &Value) -> Option<PortSpec> {
    match v {
        Value::Number(n) => {
            let port = u16::try_from(n.as_u64()?).ok().filter(|p| *p > 0)?;
            Some(PortSpec { host: None, host_port: None, port, label: None })
        }
        Value::String(s) => {
            let (host, port) = s.rsplit_once(':')?;
            let port: u16 = port.trim().parse().ok().filter(|p| *p > 0)?;
            let host = host.trim();
            let host = (!host.is_empty() && host != "localhost" && host != "127.0.0.1").then(|| host.to_string());
            Some(PortSpec { host, host_port: None, port, label: None })
        }
        _ => None,
    }
}

fn parse_app_port(v: &Value) -> Option<PortSpec> {
    match v {
        Value::Number(_) => parse_forward_port(v),
        Value::String(s) => {
            let parts: Vec<&str> = s.split(':').collect();
            let num = |x: &str| x.trim().split('/').next().and_then(|p| p.parse::<u16>().ok()).filter(|p| *p > 0);
            match parts.as_slice() {
                [p] => Some(PortSpec { host: None, host_port: num(p), port: num(p)?, label: None }),
                [h, c] => Some(PortSpec { host: None, host_port: num(h), port: num(c)?, label: None }),
                [ip, h, c] => Some(PortSpec { host: Some(ip.to_string()).filter(|i| !i.is_empty()), host_port: num(h), port: num(c)?, label: None }),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devcontainer_id_matches_the_cli() {
        // Value from @devcontainers/cli 0.89 for these labels (getDevContainerId).
        let id = devcontainer_id("/home/u/app", "/home/u/app/.devcontainer/devcontainer.json");
        assert_eq!(id, "1nes5920mmbiqp8b5fijl7tu4vaqscdt4a75v9cup3fsoks3nmn7");
        assert_ne!(id, devcontainer_id("/home/u/app2", "/home/u/app/.devcontainer/devcontainer.json"));
    }

    #[test]
    fn variables() {
        let mut used = BTreeSet::new();
        let v = Vars {
            local_folder: "/home/u/shop",
            container_folder: Some("/workspaces/shop"),
            devcontainer_id: "abc",
            local_env: LocalEnv::Keep,
            container_env: None,
        };
        assert_eq!(
            substitute("${localWorkspaceFolder}:${localWorkspaceFolderBasename}:${containerWorkspaceFolderBasename}:${devcontainerId}", &v, &mut used),
            "/home/u/shop:shop:shop:abc"
        );
        assert_eq!(substitute("${localEnv:HOME}/.ssh", &v, &mut used), "${localEnv:HOME}/.ssh");
        assert!(used.contains("HOME"));
        // A folder on this computer is named with its OS's separators (`C:\…\shop` on Windows).
        let folder = std::env::temp_dir().join("shop").display().to_string();
        let local = Vars { local_folder: &folder, container_folder: None, devcontainer_id: "abc", local_env: LocalEnv::Keep, container_env: None };
        assert_eq!(substitute("/workspaces/${localWorkspaceFolderBasename}", &local, &mut used), "/workspaces/shop");
        let env: BTreeMap<String, String> = [("PATH".to_string(), "/usr/bin".to_string())].into();
        let v2 = Vars { container_env: Some(&env), local_env: LocalEnv::Resolve, ..v };
        assert_eq!(substitute("${containerEnv:PATH}:/x", &v2, &mut used), "/usr/bin:/x");
        assert_eq!(substitute("${containerEnv:NOPE:dflt}", &v2, &mut used), "dflt");
        // SAFETY: tests in this module do not read this variable concurrently.
        assert_eq!(substitute("${localEnv:WB_DEVC_SURELY_UNSET:fallback}", &v2, &mut used), "fallback");
        assert_eq!(substitute("${unknown} ${", &v2, &mut used), "${unknown} ${");
    }

    #[test]
    fn mounts_and_ports() {
        let m = parse_mount_str("source=${localEnv:HOME}/.ssh,target=/home/vscode/.ssh,type=bind,readonly").unwrap();
        assert_eq!((m.kind.as_str(), m.target.as_str(), m.readonly), ("bind", "/home/vscode/.ssh", true));
        let m = parse_mount_obj(&serde_json::json!({"source": "cache", "target": "/cache", "type": "volume"})).unwrap();
        assert_eq!(m.spec, "type=volume,source=cache,target=/cache");
        assert_eq!(parse_forward_port(&serde_json::json!(3000)).unwrap().port, 3000);
        let db = parse_forward_port(&serde_json::json!("db:5432")).unwrap();
        assert_eq!((db.host.as_deref(), db.port), (Some("db"), 5432));
        let a = parse_app_port(&serde_json::json!("8000:3000")).unwrap();
        assert_eq!((a.host_port, a.port), (Some(8000), 3000));
        let a = parse_app_port(&serde_json::json!("0.0.0.0:8000:3000")).unwrap();
        assert_eq!(a.host.as_deref(), Some("0.0.0.0"));
        assert!(parse_forward_port(&serde_json::json!(0)).is_none());
    }
}
