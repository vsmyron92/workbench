//! What starting a dev container will do, what of it is dangerous, and the approval
//! hash that covers it.
//!
//! `devcontainer.json`, its Dockerfile and its compose files are repository content:
//! building and starting executes code the repository chose on the host's Docker
//! (RUN steps, lifecycle commands) and can hand the container the host (`--privileged`,
//! `--network=host`, `/var/run/docker.sock`, `-v /:/host`…). Nothing starts without the
//! user's approval of exactly this plan; the hash changes whenever any of it does.

use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::config::{DevConfig, Lifecycle, Source};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Danger,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Risk {
    pub level: Level,
    /// The item as written (`--privileged`, a mount, a command).
    pub item: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Workbench drives `docker build` / `docker run` / `docker compose` itself.
    Docker,
    /// The devcontainer CLI (`devcontainer up`): features, and compose without the
    /// compose plugin.
    Cli,
}

/// What is installed.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Engines {
    pub docker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_error: Option<String>,
    pub compose: Option<String>,
    /// How the devcontainer CLI is run (a path, or `npx @devcontainers/cli`).
    pub cli: Option<String>,
    /// `[devcontainer] engine`: `auto`, `docker` or `cli`.
    pub preference: String,
}

/// A lifecycle hook, for display.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hook {
    pub key: &'static str,
    /// When it runs.
    pub when: &'static str,
    pub commands: Vec<String>,
}

/// The confirmation's content: everything that will run, and the risks.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub config: DevConfig,
    pub engine: Option<Engine>,
    pub engine_note: String,
    /// Why it cannot start now (no Docker, CLI needed…).
    pub problems: Vec<String>,
    pub risks: Vec<Risk>,
    pub hooks: Vec<Hook>,
    /// Ports published on 127.0.0.1 (built-in engine) or forwarded.
    pub ports: Vec<u16>,
    /// Files covered by the approval (project-relative).
    pub files: Vec<String>,
    pub hash: String,
}

pub fn hooks(c: &DevConfig) -> Vec<Hook> {
    let text = |l: &Lifecycle| {
        l.commands
            .iter()
            .map(|(label, cmd)| match label {
                Some(n) => format!("{n}: {}", cmd.text()),
                None => cmd.text(),
            })
            .collect::<Vec<_>>()
    };
    let mut out = vec![];
    if let Some(l) = &c.initialize_command {
        out.push(Hook { key: "initializeCommand", when: "on this computer, before the container starts", commands: text(l) });
    }
    for (key, l) in c.lifecycle() {
        let when = match key {
            "onCreateCommand" | "updateContentCommand" | "postCreateCommand" => "in the container, once after it is created",
            "postStartCommand" => "in the container, every time it starts",
            _ => "in the container, every time Workbench attaches",
        };
        out.push(Hook { key, when, commands: text(l) });
    }
    out
}

/// Pick the engine for `c` given what is installed.
pub fn choose_engine(c: &DevConfig, e: &Engines) -> (Option<Engine>, String, Vec<String>) {
    let mut problems = vec![];
    if e.docker.is_none() {
        problems.push(format!(
            "Docker is not available{}",
            e.docker_error.as_deref().map(|m| format!(": {m}")).unwrap_or_default()
        ));
    }
    if matches!(c.source, Source::None) {
        problems.push("the config names no image, Dockerfile or compose file".into());
    }
    let cli = e.cli.is_some();
    let needs_cli = !c.features.is_empty();
    let compose_ok = e.compose.is_some();
    let pref = e.preference.as_str();
    let (engine, note) = if needs_cli {
        if cli {
            (Some(Engine::Cli), "devcontainer CLI (the config uses features)".to_string())
        } else {
            problems.push(
                "this config uses features, which need the devcontainer CLI: install it (npm i -g @devcontainers/cli) or set [devcontainer] cli in config.toml"
                    .into(),
            );
            (None, "needs the devcontainer CLI".into())
        }
    } else if pref == "cli" && cli {
        (Some(Engine::Cli), "devcontainer CLI ([devcontainer] engine = \"cli\")".into())
    } else if c.is_compose() {
        if compose_ok && pref != "cli" {
            (Some(Engine::Docker), "docker compose (built in)".into())
        } else if cli {
            (Some(Engine::Cli), "devcontainer CLI (docker compose is not installed)".into())
        } else {
            problems.push("compose configs need the docker compose plugin or the devcontainer CLI".into());
            (None, "needs docker compose".into())
        }
    } else {
        (Some(Engine::Docker), "docker (built in)".into())
    };
    (engine, note, problems)
}

/// `path` as the host resolves it: `..` removed lexically, then symlinks resolved for the
/// longest part that exists (Docker follows symlinks in bind sources: a link in the
/// repository to `/` would otherwise look like a folder of the project).
pub fn resolved(path: &str) -> std::path::PathBuf {
    let lexical = std::path::PathBuf::from(super::config::parse_path_rel(Path::new("/"), path));
    let mut existing = lexical.clone();
    let mut rest: Vec<std::ffi::OsString> = vec![];
    loop {
        if let Ok(c) = crate::util::os::path::canonicalize(&existing) {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

/// Whether `path` lies in the project, as the host resolves both.
fn inside(root: &Path, path: &str) -> bool {
    let root = crate::util::os::path::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    resolved(path).starts_with(root)
}

fn is_docker_socket(s: &str) -> bool {
    s.contains("docker.sock") || s.contains("/run/podman/podman.sock") || s.contains("containerd.sock")
}

/// A bind source on the host: inside the project it is ordinary, anything else is not.
fn bind_risk(root: &Path, source: &str, target: &str, item: &str, risks: &mut Vec<Risk>) {
    if source.is_empty() {
        return;
    }
    if is_docker_socket(source) {
        risks.push(Risk {
            level: Level::Danger,
            item: item.into(),
            message: "mounts the Docker socket: the container controls Docker, which is root on this computer".into(),
        });
    } else if source.contains("${localEnv") {
        risks.push(Risk { level: Level::Danger, item: item.into(), message: format!("mounts a host path from your environment into {target}") });
    } else if !source.starts_with('/') {
        // A named volume.
    } else if !inside(root, source) {
        let real = resolved(source);
        let what = if real == Path::new("/") {
            "the whole host filesystem".to_string()
        } else if real.display().to_string() != source {
            format!("{} (outside the project; {source} leads there)", real.display())
        } else {
            format!("{source} (outside the project)")
        };
        risks.push(Risk { level: Level::Danger, item: item.into(), message: format!("mounts {what} into the container at {target}") });
    } else if is_docker_socket(&resolved(source).display().to_string()) {
        risks.push(Risk { level: Level::Danger, item: item.into(), message: "mounts the Docker socket (through a link in the project)".into() });
    }
}

const HOST_NS: &[&str] = &["--network", "--net", "--pid", "--ipc", "--uts", "--userns", "--cgroupns"];
const DANGER_CAPS: &[&str] = &[
    "ALL", "SYS_ADMIN", "SYS_PTRACE", "SYS_MODULE", "SYS_RAWIO", "NET_ADMIN", "DAC_READ_SEARCH", "SYS_BOOT", "MAC_ADMIN", "BPF", "PERFMON",
];

fn cap_risk(cap: &str, item: &str, risks: &mut Vec<Risk>) {
    let c = cap.trim().to_ascii_uppercase();
    let c = c.strip_prefix("CAP_").unwrap_or(&c).to_string();
    let level = if DANGER_CAPS.contains(&c.as_str()) { Level::Danger } else { Level::Warning };
    risks.push(Risk { level, item: item.into(), message: format!("adds the {c} capability") });
}

fn secopt_risk(opt: &str, item: &str, risks: &mut Vec<Risk>) {
    let o = opt.to_ascii_lowercase();
    if o.contains("unconfined") || o.contains("label=disable") || o.contains("label:disable") || o.contains("no-new-privileges=false") {
        risks.push(Risk { level: Level::Danger, item: item.into(), message: format!("turns off a security restriction ({opt})") });
    } else {
        risks.push(Risk { level: Level::Info, item: item.into(), message: format!("security option {opt}") });
    }
}

/// Risks of `runArgs`, one flag (with its value) at a time.
pub fn run_arg_risks(root: &Path, args: &[String], risks: &mut Vec<Risk>) {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].trim();
        // `-v/:/host`: a short flag with its value attached.
        let attached = !a.starts_with("--")
            && a.len() > 2
            && a.is_char_boundary(2)
            && ["-v", "-p", "-e", "-u", "-l", "-h", "-m", "-w"].contains(&&a[..2]);
        let (flag, inline) = if attached {
            (&a[..2], Some(a[2..].trim_start_matches('=').to_string()))
        } else {
            match a.split_once('=') {
                Some((f, v)) if f.starts_with('-') => (f, Some(v.to_string())),
                _ => (a, None),
            }
        };
        let takes_value = [
            "--network", "--net", "--pid", "--ipc", "--uts", "--userns", "--cgroupns", "-v", "--volume", "--mount", "--device",
            "--cap-add", "--security-opt", "-p", "--publish", "--env-file", "--volumes-from", "-u", "--user", "--gpus",
            "--add-host", "--label", "-l", "-e", "--env", "--name", "--entrypoint", "--dns", "--hostname", "-h", "--memory",
            "-m", "--cpus", "--shm-size", "--ulimit", "--workdir", "-w", "--platform", "--runtime", "--sysctl",
        ]
        .contains(&flag);
        let value = if takes_value && inline.is_none() {
            i += 1;
            args.get(i).cloned()
        } else {
            inline
        };
        let item = match &value {
            Some(v) => format!("{flag} {v}"),
            None => flag.to_string(),
        };
        let v = value.clone().unwrap_or_default();
        match flag {
            "--privileged" => risks.push(Risk {
                level: Level::Danger,
                item,
                message: "runs the container privileged: full access to this computer's devices and kernel".into(),
            }),
            f if HOST_NS.contains(&f) && v == "host" => risks.push(Risk {
                level: Level::Danger,
                item,
                message: format!("shares this computer's {} namespace with the container", f.trim_start_matches('-')),
            }),
            f if HOST_NS.contains(&f) && v.starts_with("container:") => {
                risks.push(Risk { level: Level::Warning, item, message: "joins another container's namespace".into() })
            }
            "-v" | "--volume" => {
                let mut parts = v.splitn(3, ':');
                let (src, dst) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                if dst.is_empty() {
                    // An anonymous volume.
                } else {
                    bind_risk(root, src, dst, &item, risks);
                }
            }
            "--mount" => {
                if let Some(m) = super::config::parse_mount_str(&v) {
                    if m.kind == "bind" {
                        bind_risk(root, &m.source, &m.target, &item, risks);
                    }
                }
            }
            "--device" => risks.push(Risk { level: Level::Danger, item, message: "gives the container a host device".into() }),
            "--volumes-from" => risks.push(Risk { level: Level::Danger, item, message: "mounts another container's volumes".into() }),
            "--cap-add" => cap_risk(&v, &item, risks),
            "--security-opt" => secopt_risk(&v, &item, risks),
            "-p" | "--publish" => {
                let loopback = v.starts_with("127.") || v.starts_with("[::1]") || v.starts_with("localhost:");
                if !loopback {
                    risks.push(Risk { level: Level::Warning, item, message: "publishes a port on every network interface (reachable from your network)".into() })
                }
            }
            "-P" | "--publish-all" => {
                risks.push(Risk { level: Level::Warning, item, message: "publishes every exposed port on every network interface".into() })
            }
            "--env-file" => risks.push(Risk { level: Level::Warning, item, message: "reads environment variables from a file on this computer".into() }),
            "-u" | "--user" if v == "root" || v == "0" || v.starts_with("0:") => {
                risks.push(Risk { level: Level::Info, item, message: "runs as root in the container".into() })
            }
            "--runtime" => risks.push(Risk { level: Level::Warning, item, message: "uses another container runtime".into() }),
            "--sysctl" => risks.push(Risk { level: Level::Warning, item, message: "changes kernel parameters for the container".into() }),
            "--gpus" => risks.push(Risk { level: Level::Info, item, message: "gives the container GPUs".into() }),
            _ => {}
        }
        i += 1;
    }
}

/// How deep `include:` and `extends: {file}` are followed, and how many compose files are read.
const COMPOSE_MAX_DEPTH: usize = 8;
const COMPOSE_MAX_FILES: usize = 64;
/// Largest compose file read.
const COMPOSE_MAX_BYTES: u64 = 1024 * 1024;

/// What compose will read for a config: the files named in `dockerComposeFile`, the ones they
/// `include:` and `extends:` (followed the way compose resolves them), the `env_file`s,
/// `.env`s, secret/config files and the Dockerfiles builds use; and the risks of every
/// service reached.
///
/// Compose resolves relative paths in every `-f` file against the project directory (the
/// first file's folder), in an extended file against that file's folder, and in an
/// included file against its `project_directory` (default: its folder).
#[derive(Default)]
struct ComposeScan {
    risks: Vec<Risk>,
    /// Files compose reads, absolute, in the order found.
    files: Vec<String>,
    /// The `dockerComposeFile` files (their services are named without a file).
    top: Vec<String>,
    /// Parsed compose files (`None`: unreadable), each read once.
    docs: std::collections::HashMap<String, Option<Yaml>>,
    /// Files whose every service was scanned.
    whole: std::collections::HashSet<String>,
    /// (file, service) scanned.
    services: std::collections::HashSet<(String, String)>,
    capped: bool,
}

type Yaml = serde_norway::Value;

/// A YAML string, or the strings of a sequence (`env_file: a.env` / `[a.env, b.env]`, or
/// `[{path: a.env}]`).
fn yaml_paths(v: Option<&Yaml>) -> Vec<String> {
    match v {
        Some(Yaml::String(s)) => vec![s.clone()],
        Some(Yaml::Sequence(a)) => a
            .iter()
            .filter_map(|x| match x {
                Yaml::String(s) => Some(s.clone()),
                other => other.get("path").and_then(|p| p.as_str()).map(str::to_string),
            })
            .collect(),
        _ => vec![],
    }
}

impl ComposeScan {
    fn new(root: &Path, files: &[String]) -> ComposeScan {
        let mut s = ComposeScan { top: files.to_vec(), ..Default::default() };
        let base = files.first().and_then(|f| Path::new(f).parent()).unwrap_or(root).to_path_buf();
        // Compose reads `.env` in the project directory for its variables.
        s.cover_env(&base);
        for f in files {
            s.whole(root, f, &base, 0);
        }
        s
    }

    fn cover(&mut self, f: String) {
        if !self.files.contains(&f) {
            self.files.push(f);
        }
    }

    fn cover_env(&mut self, dir: &Path) {
        let env = dir.join(".env");
        if env.is_file() {
            self.cover(env.display().to_string());
        }
    }

    fn cap(&mut self, item: &str) {
        if !self.capped {
            self.capped = true;
            self.risks.push(Risk {
                level: Level::Danger,
                item: item.into(),
                message: format!("compose files nest deeper (or more of them) than Workbench follows ({COMPOSE_MAX_DEPTH} levels, {COMPOSE_MAX_FILES} files): the rest is not reviewed"),
            });
        }
    }

    /// `p` (a file compose reads, `what` for messages) against `base`; `None` when it
    /// cannot be known before compose runs (flagged).
    fn reference(&mut self, base: &Path, p: &str, item: &str, what: &str) -> Option<String> {
        let p = p.trim();
        if p.is_empty() {
            return None;
        }
        let (level, message) = if p.contains('$') {
            (Level::Danger, format!("{what} is taken from a variable (.env or the environment): which file is decided when compose runs"))
        } else if p.contains("://") || p.starts_with("git@") {
            (Level::Danger, format!("{what} is fetched from elsewhere when compose runs: it is not reviewed"))
        } else if p.starts_with('~') {
            (Level::Danger, format!("{what} is in your home folder, outside the project"))
        } else {
            return Some(super::config::parse_path_rel(base, p));
        };
        self.risks.push(Risk { level, item: item.into(), message });
        None
    }

    /// A file compose reads besides compose files (`env_file`, secrets): covered, and
    /// flagged when outside the project.
    fn host_file(&mut self, root: &Path, abs: String, item: &str, message: &str) {
        if !inside(root, &abs) {
            let real = resolved(&abs);
            self.risks.push(Risk { level: Level::Danger, item: item.into(), message: format!("{message} {} (outside the project)", real.display()) });
        }
        self.cover(abs);
    }

    /// The parsed compose file at `path` (read once, at most 1 MiB), covered either way.
    fn doc(&mut self, root: &Path, path: &str) -> Option<Yaml> {
        if let Some(d) = self.docs.get(path) {
            return d.clone();
        }
        if self.docs.len() >= COMPOSE_MAX_FILES {
            self.cap(&super::rel_display(root, path));
            return None;
        }
        self.cover(path.to_string());
        let rel = super::rel_display(root, path);
        if !inside(root, path) {
            self.risks.push(Risk { level: Level::Danger, item: rel.clone(), message: "the compose file is outside the project".into() });
        }
        let text = std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file() && m.len() <= COMPOSE_MAX_BYTES)
            .and_then(|_| std::fs::read_to_string(path).ok());
        let doc = match text {
            None => {
                self.risks.push(Risk { level: Level::Warning, item: rel, message: "the compose file cannot be read".into() });
                None
            }
            Some(t) => match serde_norway::from_str::<Yaml>(&t) {
                Ok(d) => Some(d),
                Err(_) => {
                    self.risks.push(Risk { level: Level::Warning, item: rel, message: "the compose file does not parse".into() });
                    None
                }
            },
        };
        self.docs.insert(path.to_string(), doc.clone());
        doc
    }

    /// Every service of `path` and what it includes (a `-f` file or an included one).
    fn whole(&mut self, root: &Path, path: &str, base: &Path, depth: usize) {
        if !self.whole.insert(path.to_string()) {
            return;
        }
        let Some(doc) = self.doc(root, path) else { return };
        let rel = super::rel_display(root, path);
        for inc in doc.get("include").and_then(|v| v.as_sequence()).into_iter().flatten() {
            let (paths, project_dir, env_files) = match inc {
                Yaml::String(p) => (vec![p.clone()], None, vec![]),
                other => (
                    yaml_paths(other.get("path")),
                    other.get("project_directory").and_then(|v| v.as_str()).map(str::to_string),
                    yaml_paths(other.get("env_file")),
                ),
            };
            if depth + 1 > COMPOSE_MAX_DEPTH {
                self.cap(&format!("{rel}: include"));
                continue;
            }
            let item = |p: &str| format!("{rel}: include {p}");
            let dir = match &project_dir {
                Some(d) => match self.reference(base, d, &format!("{rel}: include project_directory {d}"), "the included project directory") {
                    Some(d) => Some(std::path::PathBuf::from(d)),
                    None => continue,
                },
                None => None,
            };
            for e in &env_files {
                if let Some(abs) = self.reference(base, e, &item(e), "an include env_file") {
                    self.host_file(root, abs, &item(e), "reads variables from");
                }
            }
            for p in &paths {
                let Some(abs) = self.reference(base, p, &item(p), "an included compose file") else { continue };
                let inc_base = dir.clone().unwrap_or_else(|| Path::new(&abs).parent().unwrap_or(root).to_path_buf());
                if env_files.is_empty() {
                    self.cover_env(&inc_base);
                }
                self.whole(root, &abs, &inc_base, depth + 1);
            }
        }
        for key in ["secrets", "configs"] {
            let Some(defs) = doc.get(key).and_then(|v| v.as_mapping()) else { continue };
            for (name, def) in defs {
                let name = name.as_str().unwrap_or("?");
                if let Some(f) = def.get("file").and_then(|v| v.as_str()) {
                    let item = format!("{key}: {name}: file {f}");
                    if let Some(abs) = self.reference(base, f, &item, "a secret or config file") {
                        self.host_file(root, abs, &item, "mounts");
                    }
                }
                if let Some(v) = def.get("environment").and_then(|v| v.as_str()) {
                    self.risks.push(Risk {
                        level: Level::Warning,
                        item: format!("{key}: {name}: environment {v}"),
                        message: "passes the value of an environment variable on this computer into the container".into(),
                    });
                }
            }
        }
        let Some(services) = doc.get("services").and_then(|s| s.as_mapping()) else { return };
        for (name, svc) in services {
            let name = name.as_str().unwrap_or("?").to_string();
            self.service(root, path, &name, svc, base, depth);
        }
    }

    /// One service of `path` with the existing rules, and what it extends.
    fn service(&mut self, root: &Path, path: &str, name: &str, svc: &Yaml, base: &Path, depth: usize) {
        if !self.services.insert((path.to_string(), name.to_string())) {
            return;
        }
        let label = if self.top.iter().any(|t| t == path) { name.to_string() } else { format!("{name} ({})", super::rel_display(root, path)) };
        service_risks(root, &label, svc, base, &mut self.risks);
        for e in yaml_paths(svc.get("env_file")) {
            let item = format!("{label}: env_file {e}");
            if let Some(abs) = self.reference(base, &e, &item, "an env_file") {
                self.host_file(root, abs, &item, "reads environment variables from");
            }
        }
        if let Some(b) = svc.get("build") {
            let (ctx, df) = match b {
                Yaml::String(s) => (s.clone(), Some("Dockerfile".to_string())),
                other => (
                    other.get("context").and_then(|x| x.as_str()).unwrap_or(".").to_string(),
                    match other.get("dockerfile_inline") {
                        Some(_) => None,
                        None => Some(other.get("dockerfile").and_then(|x| x.as_str()).unwrap_or("Dockerfile").to_string()),
                    },
                ),
            };
            let item = format!("{label}: build {ctx}");
            if ctx.contains("://") || ctx.starts_with("git@") {
                self.risks.push(Risk { level: Level::Warning, item, message: "builds from a remote source: its Dockerfile is not reviewed".into() });
            } else if let Some(ctx) = self.reference(base, &ctx, &item, "the build context") {
                if !inside(root, &ctx) {
                    self.risks.push(Risk { level: Level::Danger, item, message: "the build context is outside the project: files from there are sent to the build".into() });
                }
                if let Some(df) = df {
                    self.cover(super::config::parse_path_rel(Path::new(&ctx), &df));
                }
            }
        }
        let (file, target) = match svc.get("extends") {
            Some(Yaml::String(s)) => (None, s.clone()),
            Some(e) => (e.get("file").and_then(|v| v.as_str()).map(str::to_string), e.get("service").and_then(|v| v.as_str()).unwrap_or("").to_string()),
            None => return,
        };
        if depth + 1 > COMPOSE_MAX_DEPTH {
            self.cap(&format!("{label}: extends"));
            return;
        }
        let (tpath, tbase) = match file {
            None => (path.to_string(), base.to_path_buf()),
            Some(f) => {
                let item = format!("{label}: extends {f}");
                let Some(abs) = self.reference(base, &f, &item, "the extended compose file") else { return };
                let dir = Path::new(&abs).parent().unwrap_or(root).to_path_buf();
                (abs, dir)
            }
        };
        let Some(doc) = self.doc(root, &tpath) else { return };
        match doc.get("services").and_then(|s| s.get(target.as_str())) {
            Some(t) => {
                let t = t.clone();
                self.service(root, &tpath, &target, &t, &tbase, depth + 1);
            }
            None => self.risks.push(Risk {
                level: Level::Warning,
                item: format!("{label}: extends {target}"),
                message: format!("{} has no service {target}", super::rel_display(root, &tpath)),
            }),
        }
    }
}

/// The risk rules for one compose service; relative paths against `dir`.
fn service_risks(root: &Path, name: &str, svc: &Yaml, dir: &Path, risks: &mut Vec<Risk>) {
    let item = |k: &str, v: &str| format!("{name}: {k} {v}");
    let s = |k: &str| svc.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let list = |k: &str| -> Vec<String> {
        svc.get(k)
            .and_then(|v| v.as_sequence())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    if svc.get("privileged").and_then(|v| v.as_bool()) == Some(true) {
        risks.push(Risk { level: Level::Danger, item: item("privileged:", "true"), message: "runs privileged: full access to this computer".into() });
    }
    for k in ["network_mode", "pid", "ipc", "userns_mode", "uts", "cgroup"] {
        if s(k).as_deref() == Some("host") {
            risks.push(Risk { level: Level::Danger, item: item(&format!("{k}:"), "host"), message: format!("shares this computer's {k} namespace") });
        }
    }
    for c in list("cap_add") {
        cap_risk(&c, &item("cap_add:", &c), risks);
    }
    for o in list("security_opt") {
        secopt_risk(&o, &item("security_opt:", &o), risks);
    }
    if !list("devices").is_empty() {
        risks.push(Risk { level: Level::Danger, item: item("devices:", &list("devices").join(", ")), message: "gives the container host devices".into() });
    }
    if !list("volumes_from").is_empty() {
        risks.push(Risk { level: Level::Danger, item: item("volumes_from:", &list("volumes_from").join(", ")), message: "mounts another container's volumes".into() });
    }
    if let Some(vols) = svc.get("volumes").and_then(|v| v.as_sequence()) {
        for v in vols {
            let (src, dst, it) = match v {
                Yaml::String(x) => {
                    let mut p = x.splitn(3, ':');
                    (p.next().unwrap_or("").to_string(), p.next().unwrap_or("").to_string(), x.clone())
                }
                other => {
                    let g = |k: &str| other.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if g("type") != "bind" {
                        continue;
                    }
                    (g("source"), g("target"), format!("{} → {}", g("source"), g("target")))
                }
            };
            if dst.is_empty() {
                continue;
            }
            if src.contains('$') {
                // Compose fills it in from `.env` or the environment when it runs.
                risks.push(Risk {
                    level: Level::Danger,
                    item: item("volume", &it),
                    message: "mounts a host path taken from a variable (.env or the environment): where it points is decided when compose runs".into(),
                });
                continue;
            }
            let src_abs = if src.starts_with('.') {
                super::config::parse_path_rel(dir, &src)
            } else if let Some(r) = src.strip_prefix("~") {
                format!("${{localEnv:HOME}}{r}")
            } else {
                src.clone()
            };
            bind_risk(root, &src_abs, &dst, &item("volume", &it), risks);
        }
    }
    for p in list("ports") {
        let loopback = p.starts_with("127.") || p.starts_with("localhost:");
        if !loopback && p.contains(':') {
            risks.push(Risk { level: Level::Warning, item: item("ports:", &p), message: "published on every network interface".into() });
        }
    }
    if svc.get("build").is_some() {
        risks.push(Risk { level: Level::Warning, item: item("build", ""), message: "builds an image: runs its Dockerfile's RUN steps".into() });
    }
}

/// The folder of a local feature (`./name` next to the config).
fn local_feature_dir(root: &Path, c: &DevConfig, id: &str) -> Option<std::path::PathBuf> {
    if !(id.starts_with("./") || id.starts_with("../")) {
        return None;
    }
    let dir = root.join(&c.path);
    let dir = dir.parent()?;
    Some(std::path::PathBuf::from(super::config::parse_path_rel(dir, id)))
}

/// A feature installs as root while the image builds, and its metadata can add
/// privileges, mounts and entrypoints the config does not show. Local features are
/// read; well-known official ones that take the host are named.
fn feature_risks(root: &Path, c: &DevConfig, id: &str, r: &mut Vec<Risk>) {
    let item = format!("feature {id}");
    let official = id.starts_with("ghcr.io/devcontainers/features/");
    if id.contains("docker-in-docker") {
        r.push(Risk { level: Level::Danger, item: item.clone(), message: "docker-in-docker runs the container privileged".into() });
    }
    if id.contains("docker-outside-of-docker") || id.contains("docker-from-docker") {
        r.push(Risk { level: Level::Danger, item: item.clone(), message: "mounts this computer's Docker socket into the container (root on this computer)".into() });
    }
    let Some(dir) = local_feature_dir(root, c, id) else {
        let (level, message) = if official {
            (Level::Info, "installs an official feature (its install script runs as root while building)")
        } else {
            (Level::Warning, "installs a third-party feature: its install script runs as root while building, and it may add mounts, capabilities or privileges that are not listed here")
        };
        r.push(Risk { level, item, message: message.into() });
        return;
    };
    let rel = super::rel_display(root, &dir.display().to_string());
    if !inside(root, &dir.display().to_string()) {
        r.push(Risk { level: Level::Danger, item: item.clone(), message: format!("the feature's folder {rel} is outside the project") });
    }
    r.push(Risk { level: Level::Warning, item: item.clone(), message: format!("installs a feature from the repository ({rel}): its install script runs as root while building") });
    let meta = std::fs::read_to_string(dir.join("devcontainer-feature.json")).ok().and_then(|t| super::jsonc::parse(&t).ok());
    let Some(meta) = meta else { return };
    if meta.get("privileged").and_then(|v| v.as_bool()) == Some(true) {
        r.push(Risk { level: Level::Danger, item: format!("{item}: privileged"), message: "the feature runs the container privileged".into() });
    }
    for cap in meta.get("capAdd").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        cap_risk(cap, &format!("{item}: capAdd {cap}"), r);
    }
    for o in meta.get("securityOpt").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
        secopt_risk(o, &format!("{item}: securityOpt {o}"), r);
    }
    for m in meta.get("mounts").and_then(|v| v.as_array()).into_iter().flatten() {
        let (source, target, spec) = match m {
            serde_json::Value::String(s) => match super::config::parse_mount_str(s) {
                Some(p) => (p.source, p.target, s.clone()),
                None => continue,
            },
            other => (
                other.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                other.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                other.to_string(),
            ),
        };
        bind_risk(root, &source, &target, &format!("{item}: mount {spec}"), r);
    }
    if meta.get("entrypoint").is_some() {
        r.push(Risk { level: Level::Warning, item: format!("{item}: entrypoint"), message: "the feature runs its own entrypoint every time the container starts".into() });
    }
    for key in ["onCreateCommand", "updateContentCommand", "postCreateCommand", "postStartCommand", "postAttachCommand"] {
        if let Some(v) = meta.get(key) {
            r.push(Risk { level: Level::Warning, item: format!("{item}: {key} {v}"), message: "runs in the container (from the feature)".into() });
        }
    }
}

/// Every risk of `c` with `engine`.
pub fn risks(root: &Path, c: &DevConfig, engine: Option<Engine>) -> Vec<Risk> {
    let mut r = vec![];
    if c.privileged {
        r.push(Risk { level: Level::Danger, item: "\"privileged\": true".into(), message: "runs the container privileged: full access to this computer's devices and kernel".into() });
    }
    for cap in &c.cap_add {
        cap_risk(cap, &format!("capAdd {cap}"), &mut r);
    }
    for o in &c.security_opt {
        secopt_risk(o, &format!("securityOpt {o}"), &mut r);
    }
    run_arg_risks(root, &c.run_args, &mut r);
    if let Some(m) = &c.workspace_mount {
        if m.kind == "bind" && m.source != root.display().to_string() {
            bind_risk(root, &m.source, &m.target, &format!("workspaceMount {}", m.spec), &mut r);
        }
    }
    for m in &c.mounts {
        if m.kind == "bind" {
            bind_risk(root, &m.source, &m.target, &format!("mount {}", m.spec), &mut r);
        }
    }
    if let Some(l) = &c.initialize_command {
        for (_, cmd) in &l.commands {
            r.push(Risk { level: Level::Danger, item: format!("initializeCommand: {}", cmd.text()), message: "runs on this computer (not in the container)".into() });
        }
    }
    for (key, l) in c.lifecycle() {
        for (_, cmd) in &l.commands {
            r.push(Risk {
                level: Level::Warning,
                item: format!("{key}: {}", cmd.text()),
                message: "runs in the container, which can change the project's files".into(),
            });
        }
    }
    match &c.source {
        Source::Image { image } => r.push(Risk { level: Level::Info, item: format!("image {image}"), message: "pulls and runs this image".into() }),
        Source::Dockerfile { dockerfile, context, options, .. } => {
            let rel = super::rel_display(root, dockerfile);
            r.push(Risk { level: Level::Warning, item: format!("build {rel}"), message: "builds the Dockerfile: runs its RUN steps".into() });
            if !inside(root, dockerfile) {
                r.push(Risk { level: Level::Danger, item: rel, message: "the Dockerfile is outside the project".into() });
            }
            if !inside(root, context) {
                r.push(Risk {
                    level: Level::Danger,
                    item: format!("context {}", super::rel_display(root, context)),
                    message: "the build context is outside the project: files from there are sent to the build".into(),
                });
            }
            if !options.is_empty() {
                r.push(Risk { level: Level::Warning, item: format!("build.options {}", options.join(" ")), message: "extra docker build flags".into() });
            }
        }
        Source::Compose { files, .. } => r.extend(ComposeScan::new(root, files).risks),
        Source::None => {}
    }
    for (id, _) in &c.features {
        feature_risks(root, c, id, &mut r);
    }
    if !c.local_env.is_empty() {
        r.push(Risk {
            level: Level::Warning,
            item: c.local_env.iter().map(|v| format!("${{localEnv:{v}}}")).collect::<Vec<_>>().join(", "),
            message: "passes values of environment variables on this computer into the container".into(),
        });
    }
    let publishes_everywhere = engine == Some(Engine::Cli) && !c.app_ports.is_empty();
    for p in &c.app_ports {
        let item = format!("appPort {}", p.port);
        if publishes_everywhere || p.host.as_deref().is_some_and(|h| !h.starts_with("127.")) {
            r.push(Risk { level: Level::Warning, item, message: "published on every network interface by the devcontainer CLI".into() });
        }
    }
    for u in [c.container_user.as_deref(), c.remote_user.as_deref()].into_iter().flatten() {
        if u == "root" || u == "0" {
            r.push(Risk { level: Level::Info, item: format!("user {u}"), message: "commands run as root in the container".into() });
            break;
        }
    }
    r.sort_by_key(|x| x.level);
    r.dedup();
    r
}

/// Files the approval covers besides the config itself (Dockerfile, compose files and
/// what they include, extend and read, the Dockerfiles compose builds, local features),
/// absolute.
pub fn covered_files(root: &Path, c: &DevConfig) -> Vec<String> {
    let mut v = vec![];
    // A local feature's files (its metadata and install script) are built into the image.
    for id in c.features.keys() {
        if let Some(dir) = local_feature_dir(root, c, id) {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let mut files: Vec<String> = rd
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .map(|e| e.path().display().to_string())
                    .collect();
                files.sort();
                v.extend(files.into_iter().take(64));
            }
        }
    }
    match &c.source {
        Source::Dockerfile { dockerfile, .. } => v.push(dockerfile.clone()),
        // Compose files, what they include and extend, env files, secrets and the
        // Dockerfiles they build.
        Source::Compose { files, .. } => v.extend(ComposeScan::new(root, files).files),
        _ => {}
    }
    v
}

/// sha256 over the plan as displayed (config, engine) and the contents of the files it
/// covers, so any change to what would run asks for approval again.
pub fn approval_hash(root: &Path, config_text: &[u8], c: &DevConfig, engine: Option<Engine>, files: &[String]) -> String {
    let mut h = Sha256::new();
    h.update(b"workbench-devcontainer-v1\0");
    h.update(serde_json::to_vec(c).unwrap_or_default());
    h.update(serde_json::to_vec(&engine).unwrap_or_default());
    h.update(config_text);
    for f in files {
        h.update(b"\0file\0");
        h.update(f.as_bytes());
        // Only regular files in the project are read; others are hashed by path (and
        // flagged). A larger file is hashed by its size and modification time.
        let meta = std::fs::metadata(f).ok().filter(|m| m.is_file() && inside(root, f));
        if let Some(m) = meta {
            if m.len() <= 4 * 1024 * 1024 {
                if let Ok(bytes) = std::fs::read(f) {
                    h.update(&bytes);
                }
            } else {
                h.update(m.len().to_le_bytes());
                h.update(format!("{:?}", m.modified().ok()).as_bytes());
            }
        }
    }
    hex::encode(h.finalize())
}

pub fn ports(c: &DevConfig) -> Vec<u16> {
    let mut v: Vec<u16> = c.forward_ports.iter().filter(|p| p.host.is_none()).map(|p| p.port).chain(c.app_ports.iter().map(|p| p.port)).collect();
    v.sort();
    v.dedup();
    v
}

/// The plan for a display-mode config.
pub fn build(root: &Path, c: DevConfig, config_text: &[u8], engines: &Engines) -> Plan {
    let (engine, engine_note, problems) = choose_engine(&c, engines);
    let risks = risks(root, &c, engine);
    let mut files = covered_files(root, &c);
    files.retain(|f| !f.is_empty());
    let hash = approval_hash(root, config_text, &c, engine, &files);
    Plan {
        hooks: hooks(&c),
        ports: ports(&c),
        files: std::iter::once(c.path.clone()).chain(files.iter().map(|f| super::rel_display(root, f))).collect(),
        config: c,
        engine,
        engine_note,
        problems,
        risks,
        hash,
    }
}
