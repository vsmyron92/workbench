//! Launch configurations: `[[debug]]` entries of the project config (explicit), and
//! derived ones (`derive`): Cargo targets, CMake executables, Python run
//! configurations, Go main packages. Explicit entries win on a name clash.
//!
//! `plan` turns one into what a session needs: the adapter (from config.toml only),
//! the launch/attach arguments, the working directory and environment, the build or
//! pre-launch step, and the dev container when the project uses it.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::adapters::{self, Adapter, AdapterKind};
use super::derive::{self, CargoTarget, CmakeTarget};
use crate::app::AppState;
use crate::config::project::{DebugLaunch, DebugRequest};
use crate::devcontainer::ExecTarget;
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;

#[derive(Debug, Clone, PartialEq)]
pub enum Build {
    Cargo(CargoTarget),
    Cmake(CmakeTarget),
}

#[derive(Debug, Clone)]
pub struct LaunchDef {
    pub launch: DebugLaunch,
    /// `config`, `cargo`, `cmake`, `python` or `go`.
    pub origin: &'static str,
    pub build: Option<Build>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfigView {
    pub name: String,
    pub origin: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub request: DebugRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_label: Option<String>,
    pub adapter_available: bool,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// What runs first: a run configuration, a command, or the derived build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_launch: Option<String>,
    pub stop_on_entry: bool,
    pub problems: Vec<String>,
}

fn build_command(b: &Build) -> String {
    match b {
        Build::Cargo(t) => {
            let args: Vec<String> = t.build_args().into_iter().filter(|a| !a.starts_with("--message-format")).collect();
            format!("cargo {}", args.join(" "))
        }
        Build::Cmake(t) => format!("cmake --build {} --target {}", t.build_dir, t.name),
    }
}

/// Every launch configuration of `project`, explicit first.
pub fn defs(project: &Project) -> Vec<LaunchDef> {
    let root = &project.root;
    let mut out: Vec<LaunchDef> = project.config.debugs.iter().filter(|d| !d.name.trim().is_empty()).map(|d| LaunchDef { launch: d.clone(), origin: "config", build: None }).collect();
    let taken = |out: &Vec<LaunchDef>, n: &str| out.iter().any(|d| d.launch.name == n);
    // Cargo projects: the root, and folders detection found (a monorepo's `app/server`).
    let mut cargo_dirs: Vec<String> = vec![String::new()];
    for c in &project.config.components {
        if (c.kind == "cargo-workspace" || c.kind == "cargo") && !cargo_dirs.contains(&c.path) && cargo_dirs.len() < 8 {
            cargo_dirs.push(c.path.trim_start_matches("./").trim_end_matches('/').to_string());
        }
    }
    cargo_dirs.dedup();
    let mut cargo: Vec<CargoTarget> = cargo_dirs.iter().flat_map(|d| derive::cargo_targets(root, if d == "." { "" } else { d })).collect();
    // Binaries first, then examples, unit tests and integration tests.
    cargo.sort_by_key(|t| match t.kind {
        derive::CargoKind::Bin => 0,
        derive::CargoKind::Example => 1,
        derive::CargoKind::Lib => 2,
        derive::CargoKind::Test => 3,
    });
    for t in cargo {
        let name = t.config_name();
        if taken(&out, &name) {
            continue;
        }
        // Tests run from their package directory, like `cargo test` does.
        let in_dir = |d: &str| if d.is_empty() { ".".to_string() } else { d.to_string() };
        let cwd = if matches!(t.kind, derive::CargoKind::Test | derive::CargoKind::Lib) { in_dir(&t.dir) } else { in_dir(&t.workspace) };
        let source = if t.dir.is_empty() { "Cargo.toml".to_string() } else { format!("{}/Cargo.toml", t.dir) };
        let launch = DebugLaunch { name, language: Some("rust".into()), cwd: Some(cwd), source: Some(source), ..Default::default() };
        out.push(LaunchDef { launch, origin: "cargo", build: Some(Build::Cargo(t)) });
    }
    for t in derive::cmake_targets(root) {
        let name = t.config_name();
        if taken(&out, &name) {
            continue;
        }
        let launch = DebugLaunch { name, language: Some("cpp".into()), source: Some("CMakeLists.txt".into()), ..Default::default() };
        out.push(LaunchDef { launch, origin: "cmake", build: Some(Build::Cmake(t)) });
    }
    let venv = derive::venv_python(root);
    for r in &project.config.runs {
        if r.group.as_deref() == Some("deploy") || r.group.as_deref() == Some("suggested") {
            continue;
        }
        let Some(p) = derive::python_from_command(&r.command) else { continue };
        let name = format!("Python: {}", r.name);
        if taken(&out, &name) {
            continue;
        }
        let mut extra = std::collections::BTreeMap::new();
        if let Some(py) = p.python.clone().or_else(|| venv.clone()) {
            extra.insert("python".to_string(), json!(py));
        }
        let launch = DebugLaunch {
            name,
            language: Some("python".into()),
            program: p.program,
            module: p.module,
            args: p.args,
            cwd: (r.cwd != ".").then(|| r.cwd.clone()),
            env: r.env.clone(),
            extra,
            source: Some(format!("run {}", r.name)),
            ..Default::default()
        };
        out.push(LaunchDef { launch, origin: "python", build: None });
    }
    for pkg in derive::go_mains(root) {
        let name = if pkg == "." { "Go: main package".to_string() } else { format!("Go: {}", pkg.trim_start_matches("./")) };
        if taken(&out, &name) {
            continue;
        }
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("mode".to_string(), json!("debug"));
        let launch = DebugLaunch { name, language: Some("go".into()), program: Some(pkg), extra, source: Some("go.mod".into()), ..Default::default() };
        out.push(LaunchDef { launch, origin: "go", build: None });
    }
    out
}

pub fn find(project: &Project, name: &str) -> Result<LaunchDef, ApiError> {
    defs(project)
        .into_iter()
        .find(|d| d.launch.name == name)
        .ok_or_else(|| ApiError::not_found(format!("no launch configuration {name:?} in {}", project.id)))
}

/// The language of a launch configuration: its `language`, else a guess.
pub fn language_of(l: &DebugLaunch, root: &Path) -> String {
    if let Some(x) = l.language.as_deref().filter(|x| !x.trim().is_empty()) {
        let x = x.trim().to_ascii_lowercase();
        return match x.as_str() {
            "c++" | "cxx" | "cc" => "cpp".into(),
            "golang" => "go".into(),
            "py" => "python".into(),
            _ => x,
        };
    }
    let program = l.program.as_deref().unwrap_or("");
    if l.module.is_some() || program.ends_with(".py") {
        "python".into()
    } else if program.ends_with(".go") || (root.join("go.mod").is_file() && !root.join("Cargo.toml").is_file() && (program.starts_with("./") || program == ".")) {
        "go".into()
    } else if root.join("Cargo.toml").is_file() {
        "rust".into()
    } else {
        "cpp".into()
    }
}

/// The adapter a launch configuration uses: the one it names, or the one for its
/// language. Errors name what to install or configure.
pub async fn adapter_for(state: &AppState, l: &DebugLaunch, language: &str) -> Result<Adapter, ApiError> {
    let cfg = state.config.read().debug.clone();
    match l.adapter.as_deref().filter(|a| !a.trim().is_empty()) {
        Some(id) => adapters::find(&cfg, id.trim()).ok_or_else(|| {
            ApiError::not_configured(format!("launch configuration {:?} names adapter {id:?}, which is not a preset: define [debug.adapters.{id}] in config.toml", l.name))
        }),
        None => adapters::for_language(state, language).await.ok_or_else(|| {
            ApiError::not_configured(format!("no debug adapter knows {language}: add one under [debug.adapters.<id>] in config.toml with languages = [\"{language}\"]"))
        }),
    }
}

fn run_named<'a>(project: &'a Project, name: &str) -> Option<&'a crate::config::project::RunConfig> {
    project.config.runs.iter().find(|r| r.name == name)
}

/// A program path of a launch configuration on the host: placeholders
/// (`{root}`, `${workspaceFolder}`, toolchains) expanded; relative ones resolved in
/// the project (and kept inside it).
pub fn host_program(project: &Project, program: &str) -> Result<PathBuf, ApiError> {
    let vars = crate::apps::expand::base_vars(project);
    let p = crate::apps::expand::placeholders(program, &vars).replace("${workspaceFolder}", &project.root.display().to_string());
    if crate::util::os::path::is_absolute_str(&p) || crate::util::os::path::home_relative(&p).is_some() {
        return Ok(crate::config::expand_tilde(&p));
    }
    crate::util::paths::resolve_in_root(&project.root, &p)
}

/// Views for the UI, with problems found without running anything.
pub async fn views(state: &AppState, project: &Project) -> Vec<LaunchConfigView> {
    let mut out = vec![];
    for d in defs(project) {
        let l = &d.launch;
        let language = language_of(l, &project.root);
        let mut problems = vec![];
        let adapter = match adapter_for(state, l, &language).await {
            Ok(a) => Some(a),
            Err(e) => {
                problems.push(e.message);
                None
            }
        };
        let mut available = false;
        if let Some(a) = &adapter {
            let av = adapters::probe(state, a).await;
            available = av.available;
            if let Some(p) = av.problem {
                problems.push(format!("{}: {p}", a.label));
            }
        }
        let pre_launch = match (&d.build, &l.pre_launch) {
            (Some(b), _) => Some(build_command(b)),
            (None, Some(p)) => Some(p.clone()),
            _ => None,
        };
        if let Some(p) = l.pre_launch.as_deref() {
            if let Some(r) = run_named(project, p) {
                if crate::apps::runs::needs_confirmation(r) {
                    problems.push(format!("pre-launch run {p:?} deploys or reaches another host: Workbench does not start it before debugging"));
                }
            }
        }
        if l.request == DebugRequest::Launch && d.build.is_none() {
            match (&l.program, &l.module) {
                (None, None) => problems.push("no `program` to launch".into()),
                (Some(p), _) if l.pre_launch.is_none() && d.origin == "config" && !crate::util::os::path::is_absolute_str(p) && !p.contains('{') => {
                    if let Ok(path) = host_program(project, p) {
                        if !path.exists() {
                            problems.push(format!("{p} does not exist yet: build it first, or set pre_launch"));
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(LaunchConfigView {
            name: l.name.clone(),
            origin: d.origin,
            source: l.source.clone().or_else(|| (d.origin == "config").then(|| "machine overlay".to_string())),
            request: l.request,
            adapter: adapter.as_ref().map(|a| a.id.clone()),
            adapter_label: adapter.as_ref().map(|a| a.label.clone()),
            adapter_available: available,
            language,
            program: l.program.clone(),
            module: l.module.clone(),
            args: l.args.clone(),
            cwd: l.cwd.clone(),
            pre_launch,
            stop_on_entry: l.stop_on_entry,
            problems,
        });
    }
    out
}

// ---------------------------------------------------------------- plans

/// What runs before the adapter starts.
#[derive(Debug, Clone)]
pub enum PreLaunch {
    /// A run configuration of the project (waited for until it exits or is ready).
    Run(String),
    /// A command (`bash -lc`) in a Workbench terminal.
    Command(String),
    /// The derived build that also tells which executable to debug.
    Build(Build),
}

impl PreLaunch {
    pub fn describe(&self) -> String {
        match self {
            PreLaunch::Run(r) => format!("run configuration {r}"),
            PreLaunch::Command(c) => c.clone(),
            PreLaunch::Build(b) => build_command(b),
        }
    }
}

#[derive(Clone)]
pub struct Plan {
    pub name: String,
    /// The launch configuration, for Rerun (none for an attach picked by pid).
    pub config: Option<String>,
    pub adapter: Adapter,
    pub request: DebugRequest,
    pub launch: DebugLaunch,
    pub pre: Option<PreLaunch>,
    /// Host program path (none: derived from the build, or a module/attach).
    pub program: Option<PathBuf>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub secrets: Vec<Secret>,
    pub target: Option<ExecTarget>,
    /// The adapter's executable inside the container.
    pub inside_command: Option<String>,
    pub pid: Option<u32>,
    pub stop_on_entry: bool,
    /// `startDebugging`: the child's configuration, sent as-is.
    pub raw_arguments: Option<Value>,
    /// A child session talks to its parent's adapter over a new loopback connection
    /// (debugpy's `configuration.connect`, a TCP adapter's port) instead of starting
    /// an adapter.
    pub connect: Option<(String, u16)>,
    /// Workbench launched the program (or the program whose child this session
    /// debugs): Stop terminates it rather than detaching.
    pub launched: bool,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan").field("name", &self.name).field("adapter", &self.adapter.id).field("request", &self.request).finish()
    }
}

async fn container(state: &AppState, project: &Project, adapter: &Adapter) -> Result<(Option<ExecTarget>, Option<String>), ApiError> {
    let Some(t) = crate::devcontainer::exec_target(state, &project.id).await else { return Ok((None, None)) };
    match crate::devcontainer::agent_command(state, &project.id, &adapter.command).await {
        Ok((t, path)) => Ok((Some(t), Some(path))),
        Err(msg) => Err(ApiError::not_configured(format!(
            "this project's terminals and runs use its dev container ({}), and {msg}: install {} there (e.g. in the Dockerfile), or turn \"Run terminals and runs in the container\" off",
            t.container_name, adapter.label
        ))),
    }
}

/// Launch-argument keys by which an attach configuration names its target without a
/// pid: a debug server to connect to or to wait for, a remote target, a core file,
/// commands, a process by name.
const ATTACH_TARGET_KEYS: &[&str] =
    &["pid", "processId", "connect", "listen", "host", "port", "target", "waitFor", "attachCommands", "gdb-remote-port", "gdb-remote-hostname", "coreFile", "processName", "mode"];

/// Whether an attach configuration says what to attach to. Without it the UI asks
/// for a process (`pid_required`): gdb 17 would answer an attach to pid 0 with success
/// and debug nothing.
pub fn attach_target_known(kind: AdapterKind, l: &DebugLaunch) -> bool {
    if l.pid.is_some() || ATTACH_TARGET_KEYS.iter().any(|k| l.extra.contains_key(*k)) {
        return true;
    }
    // lldb-dap and CodeLLDB attach to a process by its program's name.
    matches!(kind, AdapterKind::Lldb | AdapterKind::Codelldb) && l.program.as_deref().is_some_and(|p| !p.trim().is_empty())
}

/// The plan to start launch configuration `name` (`pid`: the process an attach
/// configuration without one attaches to, picked by the user).
pub async fn plan_config(state: &AppState, project: &Project, name: &str, stop_on_entry: Option<bool>, pid: Option<u32>) -> Result<Plan, ApiError> {
    let d = find(project, name)?;
    let mut l = d.launch.clone();
    if l.request == DebugRequest::Attach {
        if let Some(p) = pid {
            if p <= 1 || p == std::process::id() {
                return Err(ApiError::bad_request("pick another process"));
            }
            l.pid = Some(p);
        }
    }
    let language = language_of(&l, &project.root);
    let adapter = adapter_for(state, &l, &language).await?;
    if l.request == DebugRequest::Attach && !attach_target_known(adapter.kind, &l) {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "pid_required",
            format!("{name:?} attaches to a process: pick one (or give the configuration a `pid`)"),
        ));
    }
    let (target, inside_command) = container(state, project, &adapter).await?;
    if target.is_none() {
        let av = adapters::probe(state, &adapter).await;
        if !av.available {
            return Err(ApiError::not_configured(format!("{}: {}. {}", adapter.label, av.problem.unwrap_or_default(), adapter.install_hint)));
        }
    }
    let cwd = match l.cwd.as_deref() {
        Some(c) if !c.trim().is_empty() && c != "." => crate::apps::runs::resolve_cwd(project, c)?,
        _ => project.root.clone(),
    };
    if !cwd.is_dir() {
        return Err(ApiError::bad_request(format!("{name}: working directory {} does not exist", cwd.display())));
    }
    let vars = crate::apps::expand::base_vars(project);
    let (env, secrets) = crate::apps::expand::run_env(state, project, &l.env, &vars)?;
    let env: Vec<(String, String)> = env.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect();
    let pre = match (&d.build, l.pre_launch.as_deref().map(str::trim).filter(|p| !p.is_empty())) {
        (Some(b), _) => Some(PreLaunch::Build(b.clone())),
        (None, Some(p)) => match run_named(project, p) {
            Some(r) => {
                if crate::apps::runs::needs_confirmation(r) {
                    return Err(ApiError::forbidden(format!(
                        "the pre-launch run {p:?} deploys or reaches another host; start it yourself from Apps, then debug without pre_launch"
                    )));
                }
                Some(PreLaunch::Run(p.to_string()))
            }
            None => Some(PreLaunch::Command(p.to_string())),
        },
        _ => None,
    };
    let program = match &l.program {
        Some(p) if !p.trim().is_empty() => Some(host_program(project, p)?),
        _ => None,
    };
    // A relative interpreter (`.venv/bin/python`) is the run's or the project's.
    if let Some(py) = l.extra.get("python").and_then(Value::as_str).map(str::to_string) {
        if crate::util::os::path::has_separator(&py) && !crate::util::os::path::is_absolute_str(&py) {
            let in_cwd = cwd.join(&py);
            let abs = if in_cwd.exists() { in_cwd } else { project.root.join(&py) };
            l.extra.insert("python".into(), json!(abs.display().to_string()));
        }
    }
    if l.request == DebugRequest::Launch && d.build.is_none() && pre.is_none() && target.is_none() {
        if let Some(p) = &program {
            if adapter.kind != AdapterKind::Delve && !p.exists() {
                return Err(ApiError::bad_request(format!(
                    "{} does not exist: build it first, or give the launch configuration a pre_launch step",
                    p.display()
                )));
            }
        }
    }
    if l.request == DebugRequest::Launch && d.build.is_none() && l.program.is_none() && l.module.is_none() {
        return Err(ApiError::bad_request(format!("launch configuration {name:?} has no program")));
    }
    Ok(Plan {
        name: l.name.clone(),
        config: Some(l.name.clone()),
        adapter,
        request: l.request,
        stop_on_entry: stop_on_entry.unwrap_or(l.stop_on_entry),
        pid: l.pid,
        launched: l.request == DebugRequest::Launch,
        launch: l,
        pre,
        program,
        cwd,
        env,
        secrets,
        target,
        inside_command,
        raw_arguments: None,
        connect: None,
    })
}

/// The plan to attach to process `pid`.
pub async fn plan_attach(state: &AppState, project: &Project, pid: u32, adapter: Option<&str>, language: Option<&str>, program: Option<&str>) -> Result<Plan, ApiError> {
    let launch = DebugLaunch {
        name: format!("Attach to {pid}"),
        adapter: adapter.map(str::to_string),
        request: DebugRequest::Attach,
        language: language.map(str::to_string),
        program: program.map(str::to_string),
        pid: Some(pid),
        ..Default::default()
    };
    let language = language_of(&launch, &project.root);
    let language = if language == "rust" && launch.language.is_none() { "cpp".to_string() } else { language };
    let adapter = adapter_for(state, &launch, &language).await?;
    let (target, inside_command) = container(state, project, &adapter).await?;
    if target.is_some() {
        return Err(ApiError::bad_request("attaching inside a dev container is not supported yet: attach from a shell in the container"));
    }
    let av = adapters::probe(state, &adapter).await;
    if !av.available {
        return Err(ApiError::not_configured(format!("{}: {}. {}", adapter.label, av.problem.unwrap_or_default(), adapter.install_hint)));
    }
    let program = match program {
        Some(p) if !p.trim().is_empty() => Some(host_program(project, p)?),
        _ => None,
    };
    Ok(Plan {
        name: launch.name.clone(),
        config: None,
        adapter,
        request: DebugRequest::Attach,
        launch,
        pre: None,
        program,
        cwd: project.root.clone(),
        env: vec![],
        secrets: vec![],
        target,
        inside_command,
        pid: Some(pid),
        stop_on_entry: false,
        raw_arguments: None,
        connect: None,
        launched: false,
    })
}

/// Paths as the adapter sees them: the host path, or its container path.
pub fn adapter_path(target: Option<&ExecTarget>, host: &Path) -> String {
    match target.and_then(|t| t.map_path(host)) {
        Some(p) => p,
        None => host.display().to_string(),
    }
}

/// Launch or attach arguments for `plan`'s adapter. `program` is the adapter-side
/// path (after the build resolved it).
pub fn arguments(plan: &Plan, program: Option<&str>, terminal: bool) -> Value {
    if let Some(raw) = &plan.raw_arguments {
        return raw.clone();
    }
    let kind = plan.adapter.kind;
    let l = &plan.launch;
    let mut m = Map::new();
    m.insert("name".into(), json!(plan.name));
    m.insert("type".into(), json!(plan.adapter.adapter_id));
    m.insert("request".into(), json!(if plan.request == DebugRequest::Attach { "attach" } else { "launch" }));
    let cwd = adapter_path(plan.target.as_ref(), &plan.cwd);
    let target = plan.target.as_ref();
    let env_obj: Map<String, Value> = plan.env.iter().map(|(k, v)| (k.clone(), json!(target.map(|t| t.map_value(v)).unwrap_or_else(|| v.clone())))).collect();
    match plan.request {
        DebugRequest::Attach => {
            // No pid: the configuration names its target in `extra` (a server to
            // connect to, a remote target, a program by name).
            if let Some(pid) = plan.pid {
                match kind {
                    AdapterKind::Debugpy => {
                        m.insert("processId".into(), json!(pid));
                    }
                    AdapterKind::Delve => {
                        m.insert("mode".into(), json!("local"));
                        m.insert("processId".into(), json!(pid));
                    }
                    AdapterKind::Generic => {
                        m.insert("pid".into(), json!(pid));
                        m.insert("processId".into(), json!(pid));
                    }
                    _ => {
                        m.insert("pid".into(), json!(pid));
                    }
                }
            }
            if let Some(p) = program {
                m.insert("program".into(), json!(p));
            }
        }
        DebugRequest::Launch => {
            if let Some(p) = program {
                m.insert("program".into(), json!(p));
            }
            if let (AdapterKind::Debugpy, Some(module)) = (kind, &l.module) {
                m.remove("program");
                m.insert("module".into(), json!(module));
            }
            m.insert("args".into(), json!(l.args));
            m.insert("cwd".into(), json!(cwd));
            match kind {
                AdapterKind::Gdb => {
                    m.insert("env".into(), Value::Object(env_obj));
                    // `starti` stops in the dynamic loader; `start` stops at main, what
                    // "stop on entry" means for a C, C++ or Rust program.
                    m.insert("stopAtBeginningOfMainSubprogram".into(), json!(plan.stop_on_entry));
                }
                AdapterKind::Lldb => {
                    let env: Vec<String> = env_obj.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect();
                    m.insert("env".into(), json!(env));
                    m.insert("stopOnEntry".into(), json!(plan.stop_on_entry));
                    m.insert("runInTerminal".into(), json!(terminal));
                }
                AdapterKind::Codelldb => {
                    m.insert("env".into(), Value::Object(env_obj));
                    m.insert("stopOnEntry".into(), json!(plan.stop_on_entry));
                    m.insert("terminal".into(), json!(if terminal { "integrated" } else { "console" }));
                }
                AdapterKind::Debugpy => {
                    m.insert("env".into(), Value::Object(env_obj));
                    m.insert("stopOnEntry".into(), json!(plan.stop_on_entry));
                    m.insert("console".into(), json!(if terminal { "integratedTerminal" } else { "internalConsole" }));
                    m.insert("justMyCode".into(), json!(true));
                    // Subprocesses are debugged through child sessions that connect
                    // to the adapter on a loopback port, which Workbench cannot reach
                    // inside a dev container: there they run without the debugger
                    // (instead of waiting for it forever).
                    if plan.target.is_some() {
                        m.insert("subProcess".into(), json!(false));
                    }
                    if let Some(py) = l.extra.get("python").and_then(Value::as_str) {
                        let py = if crate::util::os::path::is_absolute_str(py) { adapter_path(target, Path::new(py)) } else { py.to_string() };
                        m.insert("python".into(), json!(py));
                    }
                }
                AdapterKind::Delve => {
                    m.insert("env".into(), Value::Object(env_obj));
                    m.insert("stopOnEntry".into(), json!(plan.stop_on_entry));
                    // A built executable is debugged as is; a package is built by dlv.
                    let is_exec = plan.program.as_deref().is_some_and(|p| p.is_file() && p.extension().is_none_or(|e| e != "go"));
                    m.insert("mode".into(), json!(if is_exec { "exec" } else { "debug" }));
                }
                AdapterKind::Generic => {
                    m.insert("env".into(), Value::Object(env_obj));
                    m.insert("stopOnEntry".into(), json!(plan.stop_on_entry));
                }
            }
        }
    }
    for (k, v) in &plan.adapter.launch_defaults {
        m.entry(k.clone()).or_insert_with(|| v.clone());
    }
    for (k, v) in &l.extra {
        if k == "python" && kind == AdapterKind::Debugpy {
            continue; // resolved above
        }
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::project::ProjectFile;

    fn project(root: &Path, toml_text: &str) -> Project {
        let config: ProjectFile = toml::from_str(toml_text).unwrap();
        Project {
            id: "p".into(),
            name: "p".into(),
            root: root.to_path_buf(),
            config,
            remote: None,
            warnings: vec![],
            repo_secret_names: Default::default(),
        }
    }

    fn plan(adapter: Adapter, launch: DebugLaunch, cwd: &Path) -> Plan {
        Plan {
            name: launch.name.clone(),
            config: Some(launch.name.clone()),
            adapter,
            request: launch.request,
            stop_on_entry: launch.stop_on_entry,
            pid: launch.pid,
            launch,
            pre: None,
            program: None,
            cwd: cwd.to_path_buf(),
            env: vec![("RUST_LOG".into(), "debug".into())],
            secrets: vec![],
            target: None,
            inside_command: None,
            raw_arguments: None,
            connect: None,
            launched: true,
        }
    }

    #[test]
    fn explicit_configs_come_first_and_win_name_clashes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "fn main() {}").unwrap();
        let p = project(
            d.path(),
            r#"
            [[debug]]
            name = "Cargo: bin app"
            adapter = "lldb-dap"
            program = "target/debug/app"
            [[run]]
            name = "api"
            command = "uv run python -m api.main"
            [[run]]
            name = "ship"
            command = "python3 deploy.py"
            group = "deploy"
            "#,
        );
        let all = defs(&p);
        let names: Vec<&str> = all.iter().map(|d| d.launch.name.as_str()).collect();
        assert_eq!(names, vec!["Cargo: bin app", "Python: api"]);
        assert_eq!(all[0].origin, "config");
        assert_eq!(all[1].launch.module.as_deref(), Some("api.main"));
        assert_eq!(language_of(&all[1].launch, d.path()), "python");
        assert_eq!(language_of(&DebugLaunch { program: Some("build/app".into()), ..Default::default() }, d.path()), "rust");
        assert_eq!(language_of(&DebugLaunch { language: Some("C++".into()), ..Default::default() }, d.path()), "cpp");
    }

    #[test]
    fn arguments_follow_each_adapter_dialect() {
        let d = tempfile::tempdir().unwrap();
        let cfg = adapters::DebugConfig::default();
        let launch = DebugLaunch {
            name: "server".into(),
            args: vec!["--port".into(), "1".into()],
            stop_on_entry: true,
            extra: [("setupCommands".to_string(), json!(["x"]))].into(),
            ..Default::default()
        };
        let gdb = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), launch.clone(), d.path()), Some("/w/app"), true);
        assert_eq!(gdb["program"], "/w/app");
        assert_eq!(gdb["stopAtBeginningOfMainSubprogram"], true);
        assert_eq!(gdb["env"]["RUST_LOG"], "debug");
        assert_eq!(gdb["setupCommands"], json!(["x"]), "extra is merged last");
        assert_eq!(gdb["request"], "launch");
        let lldb = arguments(&plan(adapters::find(&cfg, "lldb-dap").unwrap(), launch.clone(), d.path()), Some("/w/app"), true);
        assert_eq!(lldb["env"], json!(["RUST_LOG=debug"]));
        assert_eq!(lldb["runInTerminal"], true);
        let mut py = launch.clone();
        py.module = Some("api.main".into());
        py.extra.insert("python".into(), json!("/usr/bin/python3"));
        let dp = arguments(&plan(adapters::find(&cfg, "debugpy").unwrap(), py, d.path()), None, true);
        assert_eq!(dp["module"], "api.main");
        assert_eq!(dp["console"], "integratedTerminal");
        assert_eq!(dp["python"], "/usr/bin/python3");
        assert!(dp.get("program").is_none());
        let mut attach = launch;
        attach.request = DebugRequest::Attach;
        attach.pid = Some(4242);
        let at = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), attach.clone(), d.path()), None, false);
        assert_eq!((at["pid"].clone(), at["request"].clone()), (json!(4242), json!("attach")));
        let at = arguments(&plan(adapters::find(&cfg, "debugpy").unwrap(), attach.clone(), d.path()), None, false);
        assert_eq!(at["processId"], 4242);
        // No pid: none is invented (gdb would "attach" to pid 0 and debug nothing).
        attach.pid = None;
        let at = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), attach, d.path()), None, false);
        assert!(at.get("pid").is_none() && at.get("processId").is_none(), "{at}");
    }

    #[test]
    fn attach_configurations_need_a_target() {
        let attach = |extra: &[(&str, Value)], program: Option<&str>| DebugLaunch {
            name: "a".into(),
            request: DebugRequest::Attach,
            program: program.map(str::to_string),
            extra: extra.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            ..Default::default()
        };
        assert!(!attach_target_known(AdapterKind::Gdb, &attach(&[], Some("hello"))), "gdb needs a pid");
        assert!(!attach_target_known(AdapterKind::Debugpy, &attach(&[], None)));
        assert!(attach_target_known(AdapterKind::Debugpy, &attach(&[("connect", json!({"port": 5678}))], None)));
        assert!(attach_target_known(AdapterKind::Gdb, &attach(&[("target", json!("localhost:1234"))], None)));
        assert!(attach_target_known(AdapterKind::Lldb, &attach(&[], Some("server"))), "lldb attaches by name");
        let mut with_pid = attach(&[], None);
        with_pid.pid = Some(42);
        assert!(attach_target_known(AdapterKind::Gdb, &with_pid));
    }
}
