//! docker compose files and Dockerfiles → runs, for **local development only**.
//!
//! A compose file that describes a remote host is skipped: one under `deploy/`,
//! `infra/`, `k8s/`… (`REMOTE_DIRS`, which also holds `.devcontainer/`), a
//! `prod`/`staging` variant (`docker-compose.prod.yml`), one a deploy script names
//! (except as the first `-f` of several: a base shared with local use), and the
//! root's default file when a deploy script runs `docker compose` without `-f` (it
//! deploys that file on the host). A default file with an override file next to
//! it (`compose.override.yml`, the local-development convention) is always local.
//!
//! A local compose file gives `compose up` (a server on the main service's
//! published port; `--build` when a service builds), `compose build`, and one
//! `compose logs <service>` task per service. A Dockerfile no local compose file
//! builds gives a `docker build` task. Nothing is started by itself.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, on_path, scoped, sh, source};
use crate::config::project::{Component, RunConfig, RunKind};

/// Directories whose compose files describe servers, not the developer's machine.
const REMOTE_DIRS: &[&str] = &[
    "deploy", "deployment", "deployments", ".deploy", "ops", "infra", "infrastructure", "k8s", "kubernetes", "helm",
    "production", "prod", "staging", "ansible", "terraform", "server-config", "provisioning",
    // Not remote, but not a stack to start from here either: dev containers, CI actions.
    ".devcontainer", ".github",
];
/// Variant names (`docker-compose.<variant>.yml`) that describe a remote environment.
const REMOTE_VARIANTS: &[&str] = &["prod", "production", "staging", "stage", "deploy", "release", "live", "remote", "swarm"];
/// Well-known database / broker ports: never the "main" port of a stack.
const INFRA_PORTS: &[u16] = &[5432, 3306, 6379, 27017, 9200, 9300, 5672, 15672, 11211, 1433, 9092, 2181, 4222, 8123, 9000, 26257, 5433];
/// Service names that are usually the stack's front door.
const FRONT_NAMES: &[&str] = &["web", "app", "frontend", "front", "api", "server", "nginx", "proxy", "caddy", "traefik", "ui", "site", "www", "gateway"];
const MAX_COMPOSE: usize = 6;
const MAX_LOGS: usize = 8;
const MAX_DOCKERFILES: usize = 8;

static COMPOSE_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:docker|podman)[- ]compose\b([^\n|;&]*)").unwrap());
static COPY_SRC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^\s*(?:COPY|ADD)\s+((?:--\S+\s+)*)(.+)$").unwrap());

/// `docker-compose.yml` → Some(""), `compose.dev.yaml` → Some("dev"), other names → None.
fn compose_variant(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".yml").or_else(|| name.strip_suffix(".yaml"))?;
    let rest = stem.strip_prefix("docker-compose").or_else(|| stem.strip_prefix("compose"))?;
    if rest.is_empty() {
        return Some(String::new());
    }
    rest.strip_prefix('.').or_else(|| rest.strip_prefix('-')).filter(|v| !v.is_empty()).map(str::to_string)
}

/// One compose service, as far as detection cares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Service {
    pub name: String,
    /// Published host ports, in file order.
    pub ports: Vec<u16>,
    /// Build context directory (absolute), when the service builds an image.
    pub build: Option<PathBuf>,
    /// The Dockerfile it builds (absolute): `build.dockerfile`, else `<context>/Dockerfile`.
    pub dockerfile: Option<PathBuf>,
    /// Only started with `--profile` (not by a plain `up`).
    pub profiled: bool,
}

/// Services of a compose file whose relative paths resolve against `dir`.
pub fn services(src: &str, dir: &Path) -> Vec<Service> {
    let Ok(y) = serde_norway::from_str::<serde_norway::Value>(src) else { return vec![] };
    let Some(map) = y.get("services").and_then(|s| s.as_mapping()) else { return vec![] };
    let mut out = vec![];
    for (k, v) in map {
        let Some(name) = k.as_str() else { continue };
        let mut s = Service { name: name.to_string(), ..Default::default() };
        for p in v.get("ports").and_then(|p| p.as_sequence()).into_iter().flatten() {
            let port = match p {
                serde_norway::Value::String(t) => published_port(t),
                serde_norway::Value::Mapping(_) => p.get("published").and_then(|x| match x {
                    serde_norway::Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
                    serde_norway::Value::String(t) => published_port(&format!("{t}:0")),
                    _ => None,
                }),
                _ => None, // a bare number is a container port on a random host port
            };
            if let Some(port) = port.filter(|p| *p > 0) {
                s.ports.push(port);
            }
        }
        s.build = match v.get("build") {
            Some(serde_norway::Value::String(ctx)) => Some(dir.join(ctx)),
            Some(b @ serde_norway::Value::Mapping(_)) => Some(dir.join(b.get("context").and_then(|c| c.as_str()).unwrap_or("."))),
            _ => None,
        }
        .map(|p| normalize(&p));
        if let Some(ctx) = &s.build {
            let file = v.get("build").and_then(|b| b.get("dockerfile")).and_then(|d| d.as_str()).unwrap_or("Dockerfile");
            s.dockerfile = Some(normalize(&ctx.join(file)));
        }
        s.profiled = v.get("profiles").and_then(|p| p.as_sequence()).is_some_and(|p| !p.is_empty());
        out.push(s);
    }
    out
}

/// `a/./b/../c` → `a/c` (lexically; the path may not exist).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// The host port of a short-syntax port mapping: `8080:80`, `127.0.0.1:8080:80`,
/// `${WEB_PORT:-8080}:80`, `8080-8081:80-81`, `[::1]:8080:80/tcp`. A container port
/// alone (`80`) is published on a random host port: None.
pub fn published_port(spec: &str) -> Option<u16> {
    static DEFAULTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{[A-Za-z_][A-Za-z0-9_]*:?-([^}]*)\}").unwrap());
    let s = DEFAULTED.replace_all(spec.trim().trim_matches(['"', '\'']), "$1").into_owned();
    if s.contains('$') {
        return None; // a variable without a default
    }
    let s = s.split('/').next().unwrap_or("");
    let s = match s.strip_prefix('[') {
        Some(v6) => v6.split_once("]:").map(|(_, r)| r).unwrap_or(""),
        None => s,
    };
    let parts: Vec<&str> = s.split(':').collect();
    let host = match parts.len() {
        3 => parts[1],
        2 if parts[0].contains('.') => return None, // `127.0.0.1:80`? an IP with a container port only
        2 => parts[0],
        _ => return None,
    };
    host.split('-').next()?.trim().parse::<u16>().ok()
}

/// `docker` or `podman`; compose through the v2 plugin unless only `docker-compose` exists.
fn engine() -> (&'static str, &'static str) {
    if !on_path("docker") && on_path("podman") {
        return ("podman", "podman compose");
    }
    let plugin = ["/usr/libexec/docker/cli-plugins", "/usr/lib/docker/cli-plugins", "/usr/local/lib/docker/cli-plugins"]
        .iter()
        .map(PathBuf::from)
        .chain(dirs::home_dir().map(|h| h.join(".docker/cli-plugins")))
        .any(|d| d.join("docker-compose").is_file());
    if on_path("docker") && !plugin && on_path("docker-compose") { ("docker", "docker-compose") } else { ("docker", "docker compose") }
}

pub fn detect(cx: &mut Ctx, files: &[PathBuf]) {
    let composes: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|f| Some((f.clone(), compose_variant(f.file_name()?.to_str()?)?)))
        .filter(|(_, v)| v != "override")
        .collect();
    let dockerfiles: Vec<PathBuf> = files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n == "Dockerfile" || n == "Containerfile"))
        .cloned()
        .collect();
    if composes.is_empty() && dockerfiles.is_empty() {
        return;
    }
    cx.tag("docker");
    let (docker, compose) = engine();
    let deploy = deploy_texts(cx);
    let mut built: Vec<PathBuf> = vec![];
    let mut added = 0;
    for (f, variant) in &composes {
        if added >= MAX_COMPOSE {
            break;
        }
        if describes_remote(cx, f, variant, &deploy) {
            continue;
        }
        let Some(dir) = f.parent() else { continue };
        let Some(src) = cx.read(f) else { continue };
        let mut svcs = services(&src, dir);
        if svcs.is_empty() {
            continue;
        }
        // The default file merges its override file; a variant whose services all
        // extend the default file's is an overlay (`-f base -f variant`).
        let base = composes.iter().find(|(b, v)| v.is_empty() && b.parent() == Some(dir)).map(|(b, _)| b.clone());
        let mut files_arg = String::new();
        if variant.is_empty() {
            let over = composes_override(cx, dir, f);
            if let Some(o) = over.and_then(|o| cx.read(&o)) {
                merge(&mut svcs, services(&o, dir));
            }
        } else {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match &base {
                Some(b) if !describes_remote(cx, b, "", &deploy) => {
                    let base_svcs = cx.read(b).map(|s| services(&s, dir)).unwrap_or_default();
                    if svcs.iter().all(|s| base_svcs.iter().any(|x| x.name == s.name)) {
                        let bn = b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        files_arg = format!(" -f {} -f {}", sh(&bn), sh(&name));
                        let mut merged = base_svcs;
                        merge(&mut merged, svcs);
                        svcs = merged;
                    } else {
                        files_arg = format!(" -f {}", sh(&name));
                    }
                }
                _ => files_arg = format!(" -f {}", sh(&name)),
            }
        }
        added += 1;
        let cwd = cx.rel(dir);
        let label = |base: &str| if variant.is_empty() { base.to_string() } else { format!("{base} · {variant}") };
        cx.pf.components.push(Component { name: scoped(&label("compose"), &cwd), path: cwd.clone(), kind: "compose".into(), version: None });
        let active: Vec<&Service> = svcs.iter().filter(|s| !s.profiled).collect();
        let builds = active.iter().any(|s| s.build.is_some());
        built.extend(svcs.iter().filter_map(|s| s.dockerfile.clone()));
        let port = main_port(&active);
        cx.add_run(RunConfig {
            name: scoped(&label("compose up"), &cwd),
            kind: RunKind::Server,
            command: format!("{compose}{files_arg} up{}", if builds { " --build" } else { "" }),
            cwd: cwd.clone(),
            port,
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: source(cx, f, ""),
            group: Some("dev".into()),
            ..Default::default()
        });
        if builds {
            cx.add_run(RunConfig {
                name: scoped(&label("compose build"), &cwd),
                kind: RunKind::Build,
                command: format!("{compose}{files_arg} build"),
                cwd: cwd.clone(),
                source: source(cx, f, ""),
                group: Some("build".into()),
                ..Default::default()
            });
        }
        if variant.is_empty() {
            for s in svcs.iter().take(MAX_LOGS) {
                cx.add_run(RunConfig {
                    name: scoped(&format!("compose logs {}", s.name), &cwd),
                    kind: RunKind::Task,
                    command: format!("{compose} logs -f --tail 200 {}", sh(&s.name)),
                    cwd: cwd.clone(),
                    source: source(cx, f, &format!("#services.{}", s.name)),
                    group: Some("tasks".into()),
                    ..Default::default()
                });
            }
        }
    }
    let mut builds = 0;
    for d in dockerfiles {
        if builds >= MAX_DOCKERFILES {
            break;
        }
        let Some(dir) = d.parent() else { continue };
        let rel = cx.rel(&d);
        if built.iter().any(|b| *b == d) || rel.split('/').any(|p| REMOTE_DIRS.contains(&p)) {
            continue;
        }
        let Some(src) = cx.read(&d) else { continue };
        let root = cx.root;
        let (cwd, file_arg) = if dir != root && copies_from_root(cx, &src, dir) {
            (".".to_string(), format!(" -f {}", sh(&rel)))
        } else {
            (cx.rel(dir), String::new())
        };
        let name_base = if dir == root { root.file_name() } else { dir.file_name() };
        let tag = image_name(&name_base.map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "app".into()));
        cx.add_run(RunConfig {
            name: scoped(&format!("{docker} build"), &cx.rel(dir)),
            kind: RunKind::Build,
            command: format!("{docker} build{file_arg} -t {tag}:dev ."),
            cwd,
            source: source(cx, &d, ""),
            group: Some("build".into()),
            ..Default::default()
        });
        builds += 1;
    }
}

/// `docker-compose.override.yml` & co. next to the default file.
fn composes_override(cx: &Ctx, dir: &Path, default: &Path) -> Option<PathBuf> {
    let stem = if default.file_name().is_some_and(|n| n.to_string_lossy().starts_with("docker-compose")) { "docker-compose" } else { "compose" };
    ["yml", "yaml"].iter().map(|x| dir.join(format!("{stem}.override.{x}"))).find(|p| cx.is_file(p))
}

/// Merge overlay services into `base` (ports appended, build replaced).
fn merge(base: &mut Vec<Service>, over: Vec<Service>) {
    for o in over {
        match base.iter_mut().find(|b| b.name == o.name) {
            Some(b) => {
                for p in o.ports {
                    if !b.ports.contains(&p) {
                        b.ports.push(p);
                    }
                }
                if o.build.is_some() {
                    b.build = o.build;
                    b.dockerfile = o.dockerfile;
                }
                b.profiled = b.profiled || o.profiled;
            }
            None => base.push(o),
        }
    }
}

/// The stack's front door: a published port of a web-ish service, not a database.
fn main_port(svcs: &[&Service]) -> Option<u16> {
    let http = |s: &&&Service| s.ports.iter().copied().find(|p| !INFRA_PORTS.contains(p));
    svcs.iter()
        .filter(|s| FRONT_NAMES.contains(&s.name.as_str()))
        .find_map(|s| http(&s))
        .or_else(|| svcs.iter().find_map(|s| http(&s)))
}

/// Deploy scripts (shell files with `deploy`/`release` in their path) and the
/// lines of root task files that run compose over ssh: `(script dir, text)`.
fn deploy_texts(cx: &mut Ctx) -> Vec<(PathBuf, String)> {
    let candidates: Vec<PathBuf> = cx
        .files
        .iter()
        .filter(|f| {
            let rel = cx.rel(f).to_ascii_lowercase();
            let shell = f.extension().is_none_or(|e| matches!(e.to_str(), Some("sh" | "bash" | "zsh")));
            shell && (rel.contains("deploy") || rel.contains("release")) && !cx.is_sample(f)
        })
        .take(20)
        .cloned()
        .collect();
    let mut out = vec![];
    for f in candidates {
        if let (Some(dir), Some(t)) = (f.parent(), cx.read(&f)) {
            out.push((dir.to_path_buf(), t));
        }
    }
    for name in ["Makefile", "makefile", "justfile", "Justfile", "Taskfile.yml", "Taskfile.yaml"] {
        let p = cx.root.join(name);
        if cx.is_file(&p) {
            let lines: String = cx
                .read(&p)
                .unwrap_or_default()
                .lines()
                .filter(|l| l.contains("ssh") && l.contains("compose"))
                .collect::<Vec<_>>()
                .join("\n");
            if !lines.is_empty() {
                out.push((cx.root.to_path_buf(), lines));
            }
        }
    }
    out
}

/// Whether a compose file describes a remote host (see the module docs).
fn describes_remote(cx: &Ctx, f: &Path, variant: &str, deploy: &[(PathBuf, String)]) -> bool {
    let rel = cx.rel(f);
    let mut dirs: Vec<&str> = rel.split('/').collect();
    dirs.pop();
    if dirs.iter().any(|d| REMOTE_DIRS.contains(&d.to_ascii_lowercase().as_str())) {
        return true;
    }
    let v = variant.to_ascii_lowercase();
    if v.split(['.', '-', '_']).any(|w| REMOTE_VARIANTS.contains(&w)) {
        return true;
    }
    let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(dir) = f.parent() else { return false };
    // An override file next to the default one is the local-development convention:
    // the pair is a local stack even when a deploy script reuses the base file.
    if variant.is_empty() && composes_override(cx, dir, f).is_some() {
        return false;
    }
    let is_root_default = variant.is_empty() && dir == cx.root;
    deploy.iter().any(|(script_dir, text)| {
        // Named by a deploy script (`scp compose.yml host:`, `-f compose.prod.yml`) —
        // except as the first of several `-f` files, a base shared with local use.
        let named = text.lines().any(|line| {
            if !mentions_exact(line, &name) {
                return false;
            }
            let files = compose_file_args(line);
            !(files.len() > 1 && files[0].rsplit('/').next() == Some(name.as_str()))
        });
        if named {
            return true;
        }
        // `docker compose up -d` without -f runs the default file of the directory it
        // runs in on the host: the root's, unless the script ships its own.
        let script_has_own = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"]
            .iter()
            .any(|n| script_dir != cx.root && cx.is_file(&script_dir.join(n)));
        is_root_default
            && !script_has_own
            && COMPOSE_CALL.captures_iter(text).any(|c| !c[1].contains("-f ") && !c[1].contains("--file") && !c[1].contains("-f="))
    })
}

/// The files a compose command line names with `-f` / `--file`, in order.
fn compose_file_args(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut it = line.split_whitespace();
    while let Some(t) = it.next() {
        let v = match t {
            "-f" | "--file" => it.next().map(str::to_string),
            _ => t.strip_prefix("--file=").or_else(|| t.strip_prefix("-f=")).map(str::to_string),
        };
        if let Some(v) = v {
            out.push(v.trim_matches(['"', '\'']).to_string());
        }
    }
    out
}

/// `name` as a whole file name in `text` (`docker-compose.yml`, not `docker-compose.yml.bak`).
fn mentions_exact(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let after = text[i + name.len()..].chars().next();
        let before = text[..i].chars().next_back();
        after.is_none_or(|c| !(c.is_alphanumeric() || c == '.' || c == '_' || c == '-'))
            && before.is_none_or(|c| !(c.is_alphanumeric() || c == '.' || c == '_' || c == '-'))
    })
}

/// Whether a Dockerfile's `COPY`/`ADD` sources exist only relative to the
/// repository root (it is built with the root as context: `docker build -f x/Dockerfile .`).
fn copies_from_root(cx: &Ctx, src: &str, dir: &Path) -> bool {
    let mut from_root = false;
    for c in COPY_SRC.captures_iter(src) {
        if c[1].contains("--from") {
            continue; // from another stage
        }
        let args: Vec<&str> = c[2].split_whitespace().collect();
        if args.len() < 2 || c[2].trim_start().starts_with('[') {
            continue;
        }
        for s in &args[..args.len() - 1] {
            let s = s.trim_end_matches('/');
            if s.is_empty() || s == "." || s.contains(['*', '$', '?']) || s.starts_with("http") || !crate::util::os::path::stays_inside(s) {
                continue;
            }
            if cx.exists(&dir.join(s)) {
                return false;
            }
            if cx.exists(&cx.root.join(s)) {
                from_root = true;
            }
        }
    }
    from_root
}

/// A valid lower-case image name from a directory name.
fn image_name(dir: &str) -> String {
    let s: String = dir
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    let s = s.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_string();
    if s.is_empty() { "app".into() } else { s }
}
