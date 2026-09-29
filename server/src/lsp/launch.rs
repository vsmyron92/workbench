//! Where and how a language server process starts: on the host, or inside the
//! project's running dev container through `docker exec` (`ExecTarget::wrap`) with the
//! workspace mount mapping paths both ways.

use std::path::Path;
use std::time::Duration;

use super::config::ServerSpec;
use super::server::Launch;
use super::trust::Mode;
use super::uri::{Origin, PathMap, Side};
use crate::app::AppState;
use crate::devcontainer::ExecTarget;
use crate::projects::Project;

/// Where a server would run right now, and its command there.
pub enum Placement {
    Host(std::path::PathBuf),
    Container(ExecTarget, String),
}

impl Placement {
    pub fn side(&self) -> &'static str {
        match self {
            Placement::Host(_) => "host",
            Placement::Container(..) => "container",
        }
    }
}

/// The container to use for `mode`, if any: `container` insists on a running one;
/// `auto` uses it when it runs and the project's terminals use it. Resolve it once and
/// pass it to `place_in` for every server.
pub async fn container_for(state: &AppState, pid: &str, mode: Mode) -> Result<Option<ExecTarget>, String> {
    match mode {
        Mode::Host => Ok(None),
        Mode::Auto => Ok(crate::devcontainer::exec_target(state, pid).await),
        Mode::Container => crate::devcontainer::running_target(state, pid).await.map(Some),
    }
}

/// Find the server's command where it would run. `Err` says why it cannot run.
pub async fn place(state: &AppState, project: &Project, spec: &ServerSpec, mode: Mode) -> Result<Placement, String> {
    let target = container_for(state, &project.id, mode).await;
    place_in(state, project, spec, mode, &target).await
}

/// `place` with the container already resolved (`container_for`).
pub async fn place_in(state: &AppState, project: &Project, spec: &ServerSpec, mode: Mode, target: &Result<Option<ExecTarget>, String>) -> Result<Placement, String> {
    match target {
        Err(e) => return Err(e.clone()),
        Ok(Some(target)) => match locate_in_container(state, &project.id, target, spec).await {
            Ok(path) => return Ok(Placement::Container(target.clone(), path)),
            Err(e) if mode == Mode::Container => return Err(e),
            // Auto: the host's, when the container lacks it.
            Err(_) => {}
        },
        Ok(None) => {}
    }
    state.lsp.avail.locate(&spec.command, spec.rustup_component.as_deref()).await.map(Placement::Host)
}

async fn locate_in_container(state: &AppState, pid: &str, target: &ExecTarget, spec: &ServerSpec) -> Result<String, String> {
    let (_, path) = crate::devcontainer::agent_command(state, pid, &spec.command).await?;
    let Some(component) = spec.rustup_component.as_deref() else { return Ok(path) };
    // A rustup proxy inside answers only when the toolchain has the component.
    let mut cmd = tokio::process::Command::new(&target.docker);
    cmd.arg("exec");
    if let Some(u) = &target.user {
        cmd.args(["-u", u]);
    }
    cmd.args([&target.container_id, "/bin/sh", "-lc", "if command -v rustup >/dev/null 2>&1; then rustup which \"$1\"; else command -v \"$1\"; fi", "probe", component]);
    match crate::util::proc::run_cmd(cmd, Duration::from_secs(15)).await {
        Ok(o) if o.ok() && o.stdout.trim().starts_with('/') => Ok(o.stdout.trim().to_string()),
        _ => Err(format!("{} is not installed in the dev container (rustup component add {component})", spec.command)),
    }
}

/// Preset defaults that depend on where the server was found. typescript-language-server
/// looks for TypeScript in the workspace root only: a monorepo whose `node_modules` is in
/// a subfolder (or a project without one) gets the TypeScript installed next to the
/// server as `tsserver.fallbackPath` (the workspace's own still wins). Explicit
/// `initialization_options` are left alone.
pub fn preset_defaults(spec: &mut ServerSpec, placement: &Placement) {
    if !spec.preset || spec.id != "typescript" {
        return;
    }
    let Placement::Host(bin) = placement else { return };
    let opts = spec.initialization_options.get_or_insert_with(|| serde_json::json!({}));
    let Some(obj) = opts.as_object_mut() else { return };
    let ts = obj.entry("tsserver").or_insert_with(|| serde_json::json!({}));
    if ts.get("path").is_some() || ts.get("fallbackPath").is_some() {
        return;
    }
    if let Some(lib) = typescript_near(bin) {
        ts["fallbackPath"] = serde_json::Value::String(lib.to_string_lossy().into_owned());
    }
}

/// `…/node_modules/typescript/lib` beside a server installed with npm (locally or globally).
fn typescript_near(bin: &Path) -> Option<std::path::PathBuf> {
    let real = bin.canonicalize().ok()?;
    for dir in real.ancestors().skip(1).take(8) {
        for cand in [dir.join("node_modules/typescript/lib"), dir.join("typescript/lib")] {
            if cand.join("tsserver.js").is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Plain `env` values with `~/` expanded (host only: a container has its own home).
fn host_env(spec: &ServerSpec) -> Vec<(String, Option<String>)> {
    spec.env.iter().map(|(k, v)| (k.clone(), Some(crate::config::expand_tilde(v).to_string_lossy().into_owned()))).collect()
}

pub fn launch(project: &Project, spec: &ServerSpec, placement: Placement) -> Result<Launch, String> {
    let command = if spec.command.contains('/') { crate::config::contract_tilde(&crate::config::expand_tilde(&spec.command)) } else { spec.command.clone() };
    let display = std::iter::once(command).chain(spec.args.iter().cloned()).collect::<Vec<_>>().join(" ");
    match placement {
        Placement::Host(path) => {
            // An npm shim (Windows) runs as node and its script, not through cmd.exe; another
            // batch file's cmd.exe never takes a program from the project (`child_env`).
            let r = crate::util::os::exe::classify(path);
            let own = crate::util::os::exe::child_env().iter().map(|(k, v)| (k.to_string(), Some(v.to_string())));
            Ok(Launch {
                program: r.program.to_string_lossy().into_owned(),
                args: r.prefix_args.into_iter().chain(spec.args.iter().cloned()).collect(),
                env: own.chain(host_env(spec)).collect(),
                cwd: project.root.clone(),
                map: PathMap::host(),
                origin: Origin::Host,
                side: "host",
                display,
                container: None,
                root_server: project.root.to_string_lossy().into_owned(),
            })
        }
        Placement::Container(target, path) => {
            let (src, dst) = target
                .map
                .clone()
                .ok_or_else(|| "the dev container has no workspace mount for this project, so paths cannot be mapped".to_string())?;
            let root_server = target.map_path(&project.root).ok_or_else(|| "the project is outside the dev container's workspace mount".to_string())?;
            let term = format!("lsp{}", &uuid::Uuid::new_v4().simple().to_string()[..20]);
            let env = container_env(spec, &target);
            let argv: Vec<String> = std::iter::once(path).chain(spec.args.iter().cloned()).collect();
            let display = argv.join(" ");
            let (docker_argv, docker_env) = target.wrap(&term, &argv, &project.root, &env);
            let docker_argv = without_tty(docker_argv, &target.container_id);
            let (program, args) = docker_argv.split_first().ok_or("empty docker command")?;
            Ok(Launch {
                program: program.clone(),
                args: args.to_vec(),
                env: docker_env,
                cwd: project.root.clone(),
                map: PathMap { side: Side::Container { host: src, container: dst } },
                origin: Origin::Container { docker: target.docker.clone(), container_id: target.container_id.clone(), user: target.user.clone() },
                side: "container",
                display: format!("{display} (in {})", target.container_name),
                container: Some((target.docker.clone(), target.container_id.clone(), term)),
                root_server,
            })
        }
    }
}

/// `env` for a server in the container: values naming host paths the container cannot
/// see (a host `CARGO_HOME`, a toolchain's source path) are left out; paths under the
/// workspace mount are mapped by `ExecTarget::wrap`.
fn container_env(spec: &ServerSpec, target: &ExecTarget) -> Vec<(String, Option<String>)> {
    spec.env
        .iter()
        .filter(|(_, v)| {
            let host_path = crate::util::os::path::is_absolute_str(v) || crate::util::os::path::home_relative(v).is_some();
            !host_path || target.map_path(&crate::config::expand_tilde(v)).is_some()
        })
        .map(|(k, v)| (k.clone(), Some(v.clone())))
        .collect()
}

/// Language servers speak JSON-RPC over stdio: a TTY would echo input and turn `\n`
/// into `\r\n`. Drop `-t` and the detach keys (a TTY option) from docker's own options,
/// which end at the container id.
fn without_tty(argv: Vec<String>, container_id: &str) -> Vec<String> {
    let end = argv.iter().position(|a| a == container_id).unwrap_or(argv.len());
    argv.into_iter()
        .enumerate()
        .filter(|(i, a)| *i >= end || (a != "-t" && !a.starts_with("--detach-keys")))
        .map(|(_, a)| a)
        .collect()
}

/// Files that mark a project a server is for, at the root or one level down.
pub fn has_root_marker(root: &Path, markers: &[String]) -> bool {
    if markers.is_empty() {
        return false;
    }
    let pats: Vec<globset::GlobMatcher> = markers.iter().filter_map(|m| globset::Glob::new(m).ok().map(|g| g.compile_matcher())).collect();
    let matches = |dir: &Path| -> bool {
        std::fs::read_dir(dir)
            .map(|rd| rd.flatten().take(500).any(|e| pats.iter().any(|p| p.is_match(e.file_name()))))
            .unwrap_or(false)
    };
    if matches(root) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .take(300)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    !n.starts_with('.') && !crate::files::HARD_IGNORE.contains(&n.as_ref())
                })
                .any(|e| matches(&e.path()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn target() -> ExecTarget {
        ExecTarget {
            project_id: "api".into(),
            container_id: "c0ffee".into(),
            container_name: "wbdc-api".into(),
            user: Some("vscode".into()),
            map: Some((PathBuf::from("/home/u/ws/api"), "/workspaces/api".into())),
            folder: "/workspaces/api".into(),
            remote_env: vec![],
            workbench_url: None,
            docker: "docker".into(),
            shell: "/bin/bash".into(),
            has_bash: true,
        }
    }

    fn project() -> Project {
        Project {
            id: "api".into(),
            name: "api".into(),
            root: PathBuf::from("/home/u/ws/api"),
            config: Default::default(),
            remote: None,
            warnings: vec![],
            repo_secret_names: Default::default(),
        }
    }

    #[test]
    fn container_launch_runs_docker_exec_without_a_tty_and_maps_paths() {
        let (spec, _) = super::super::config::LspConfig::default().specs();
        let ra = spec.iter().find(|s| s.id == "rust-analyzer").unwrap();
        let l = launch(&project(), ra, Placement::Container(target(), "/usr/local/cargo/bin/rust-analyzer".into())).unwrap();
        assert_eq!(l.program, "docker");
        assert_eq!(&l.args[..2], &["exec".to_string(), "-i".to_string()]);
        let cid = l.args.iter().position(|a| a == "c0ffee").unwrap();
        assert!(!l.args[..cid].iter().any(|a| a == "-t" || a.starts_with("--detach-keys")), "{:?}", l.args);
        assert!(l.args.windows(2).any(|w| w == ["-w", "/workspaces/api"]), "{:?}", l.args);
        assert_eq!(l.args.last().unwrap(), "/usr/local/cargo/bin/rust-analyzer");
        assert_eq!(l.root_server, "/workspaces/api");
        assert_eq!(l.map.to_server(Path::new("/home/u/ws/api/src/main.rs")).unwrap(), "/workspaces/api/src/main.rs");
        assert_eq!(l.map.to_host("/workspaces/api/src/lib.rs").unwrap(), PathBuf::from("/home/u/ws/api/src/lib.rs"));
        assert_eq!(l.side, "container");
        // The pid file lets kill_inside end the process in the container.
        assert!(l.env.iter().any(|(k, v)| k == "WB_PIDFILE" && v.as_deref().is_some_and(|p| p.contains(".workbench-exec-lsp"))));
        let (_, _, term) = l.container.unwrap();
        // Host paths the container cannot see stay out of its environment.
        let mut spec = ra.clone();
        spec.env = [("CARGO_HOME".to_string(), "/home/u/.cargo".to_string()), ("RA_LOG".to_string(), "info".to_string()), ("CACHE".to_string(), "/home/u/ws/api/.cache".to_string())].into();
        let env = container_env(&spec, &target());
        assert_eq!(env, vec![("CACHE".to_string(), Some("/home/u/ws/api/.cache".to_string())), ("RA_LOG".to_string(), Some("info".to_string()))]);
        assert!(term.starts_with("lsp") && term.len() <= 40 && term.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        // Arguments after the container id are the server's own, `-t` included.
        let argv = without_tty(vec!["docker".into(), "exec".into(), "-t".into(), "c0ffee".into(), "ls".into(), "-t".into()], "c0ffee");
        assert_eq!(argv, vec!["docker", "exec", "c0ffee", "ls", "-t"]);
    }

    #[test]
    fn host_launch_is_the_located_binary_in_the_project_root() {
        let (spec, _) = super::super::config::LspConfig::default().specs();
        let ts = spec.iter().find(|s| s.id == "typescript").unwrap();
        let l = launch(&project(), ts, Placement::Host(PathBuf::from("/opt/ls/bin/typescript-language-server"))).unwrap();
        assert_eq!(l.program, "/opt/ls/bin/typescript-language-server");
        assert_eq!(l.args, vec!["--stdio"]);
        assert_eq!(l.cwd, PathBuf::from("/home/u/ws/api"));
        assert_eq!(l.root_server, "/home/u/ws/api");
        assert!(l.container.is_none());
    }

    #[test]
    fn typescript_gets_the_library_beside_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path();
        std::fs::create_dir_all(prefix.join("node_modules/typescript/lib")).unwrap();
        std::fs::write(prefix.join("node_modules/typescript/lib/tsserver.js"), "").unwrap();
        std::fs::create_dir_all(prefix.join("node_modules/typescript-language-server/lib")).unwrap();
        std::fs::write(prefix.join("node_modules/typescript-language-server/lib/cli.mjs"), "").unwrap();
        std::fs::create_dir_all(prefix.join("node_modules/.bin")).unwrap();
        crate::util::os::fs::symlink("../typescript-language-server/lib/cli.mjs", prefix.join("node_modules/.bin/typescript-language-server")).unwrap();
        let (specs, _) = super::super::config::LspConfig::default().specs();
        let mut ts = specs.iter().find(|s| s.id == "typescript").unwrap().clone();
        preset_defaults(&mut ts, &Placement::Host(prefix.join("node_modules/.bin/typescript-language-server")));
        let lib = prefix.canonicalize().unwrap().join("node_modules/typescript/lib");
        assert_eq!(ts.initialization_options.unwrap()["tsserver"]["fallbackPath"], lib.display().to_string());
        // An explicit path is kept; other servers are untouched.
        let mut ts = specs.iter().find(|s| s.id == "typescript").unwrap().clone();
        ts.initialization_options = Some(serde_json::json!({ "tsserver": { "path": "/x" } }));
        preset_defaults(&mut ts, &Placement::Host(prefix.join("node_modules/.bin/typescript-language-server")));
        assert!(ts.initialization_options.unwrap()["tsserver"].get("fallbackPath").is_none());
        let mut py = specs.iter().find(|s| s.id == "pyright").unwrap().clone();
        preset_defaults(&mut py, &Placement::Host(prefix.join("node_modules/.bin/typescript-language-server")));
        assert!(py.initialization_options.is_none());
    }

    #[test]
    fn root_markers_are_found_at_the_root_or_one_level_down() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/x")).unwrap();
        std::fs::write(dir.path().join("web/package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("node_modules/x/Cargo.toml"), "").unwrap();
        std::fs::write(dir.path().join("App.csproj"), "").unwrap();
        assert!(has_root_marker(dir.path(), &["package.json".into()]));
        assert!(has_root_marker(dir.path(), &["*.csproj".into()]));
        assert!(!has_root_marker(dir.path(), &["Cargo.toml".into()]));
        assert!(!has_root_marker(dir.path(), &[]));
    }
}
