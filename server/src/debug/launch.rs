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
use super::elf;
use super::servers::{self, Server};
use crate::app::AppState;
use super::channels::{self, Channel};
use crate::config::project::{ChannelPort, DebugLaunch, DebugRequest, RemoteTarget};
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
    /// A remote target (embedded): what Workbench starts and sends to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteView>,
    pub problems: Vec<String>,
}

/// What a remote-target configuration runs, for the Start view: nothing in it is
/// hidden from the person who is about to click Debug.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_label: Option<String>,
    pub server_available: bool,
    /// The server's command line as it will run (`{port}` still unexpanded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect: Option<String>,
    pub init: Vec<String>,
    pub reset: Vec<String>,
    pub download: bool,
    pub stop_at: String,
    /// `target extended-remote`: the stub runs the program (`attach` empty) or the
    /// configuration attaches to this target.
    pub extended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attach: Option<u32>,
    /// The output channels: `name (port)`.
    pub channels: Vec<String>,
    /// The SVD file as the configuration names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svd: Option<String>,
    /// The project works in its dev container: this configuration builds there and debugs here.
    pub in_container: bool,
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
    // Never through a link to another computer (Windows): configurations are listed unasked.
    let has = |name: &str| {
        let p = root.join(name);
        !crate::util::os::path::leaves_machine_below(root, &p) && p.is_file()
    };
    // A package path in the project: `.`, `./cmd/api` (on Windows also `.\cmd\api`).
    let package = crate::util::os::path::segments(program).next() == Some(".");
    if l.module.is_some() || program.ends_with(".py") {
        "python".into()
    } else if program.ends_with(".go") || (has("go.mod") && !has("Cargo.toml") && package) {
        "go".into()
    } else if has("Cargo.toml") {
        "rust".into()
    } else {
        "cpp".into()
    }
}

/// The adapter a launch configuration uses: the one it names, or the one for its
/// language. Errors name what to install or configure. Where gdb cannot attach to a
/// process (`util::os::support`: Windows), an attach picks another adapter for the
/// language and one that ends up with gdb is refused (a gdbserver `target` still goes).
pub async fn adapter_for(state: &AppState, l: &DebugLaunch, language: &str) -> Result<Adapter, ApiError> {
    use crate::util::os::support::{Feature, unsupported};
    let cfg = state.config.read().debug.clone();
    let gdb_attach = if l.request == DebugRequest::Attach && !l.extra.contains_key("target") { unsupported(Feature::GdbAttach) } else { None };
    let adapter = match l.adapter.as_deref().filter(|a| !a.trim().is_empty()) {
        Some(id) => adapters::find(&cfg, id.trim()).ok_or_else(|| {
            ApiError::not_configured(format!("launch configuration {:?} names adapter {id:?}, which is not a preset: define [debug.adapters.{id}] in config.toml", l.name))
        })?,
        None => adapters::for_language(state, language, gdb_attach.is_none()).await.ok_or_else(|| {
            ApiError::not_configured(format!("no debug adapter knows {language}: add one under [debug.adapters.<id>] in config.toml with languages = [\"{language}\"]"))
        })?,
    };
    match gdb_attach {
        Some(why) if adapter.kind == AdapterKind::Gdb => Err(ApiError::unsupported(Feature::GdbAttach.key(), why)),
        _ => Ok(adapter),
    }
}

// ---------------------------------------------------------------- remote targets

/// What the target does when the session is stopped (`on_stop`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnStop {
    Resume,
    Halt,
}

/// Where the program stops first when a remote configuration says `stop_on_entry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopAt {
    /// `main`, run to by a temporary hardware breakpoint.
    Main,
    /// The reset vector: the target stays halted after the reset.
    Reset,
    /// Any gdb location (`app_main`, `src/main.c:42`).
    Location(String),
}

impl StopAt {
    fn parse(s: Option<&str>) -> StopAt {
        match s.map(str::trim).filter(|s| !s.is_empty()) {
            None => StopAt::Main,
            Some(x) if x.eq_ignore_ascii_case("main") => StopAt::Main,
            Some(x) if x.eq_ignore_ascii_case("reset") => StopAt::Reset,
            Some(x) => StopAt::Location(x.to_string()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            StopAt::Main => "main".into(),
            StopAt::Reset => "reset".into(),
            StopAt::Location(l) => l.clone(),
        }
    }
}

/// A remote-target configuration resolved: the debug server to start (if any), where
/// gdb connects and what it does there.
#[derive(Debug, Clone)]
pub struct RemotePlan {
    pub server: Option<Server>,
    /// The server's whole argument list (its own, then the configuration's) with
    /// `{root}`, `{program}`, toolchains and `${workspaceFolder}` expanded; the ports
    /// (`{port}`, `{port2}`…) are picked when the server starts.
    pub server_args: Vec<String>,
    /// `target remote` argument, when the configuration names one.
    pub connect: Option<String>,
    /// The fixed gdb port: the configuration's `port`, else the port of a loopback `connect`.
    pub port: Option<u16>,
    pub init: Vec<String>,
    pub reset: Vec<String>,
    pub download: bool,
    pub stop_at: StopAt,
    /// The chip's SVD file on this computer (the Peripherals view).
    pub svd: Option<PathBuf>,
    /// `[from, to]`: where the source was when built, and where it is here (gdb `substitute-path`).
    pub source_map: Vec<(String, String)>,
    /// Output channels: text the program streams to ports on this computer.
    pub channels: Vec<ChannelPlan>,
    /// `target extended-remote` (see `RemoteTarget::extended`).
    pub extended: bool,
    /// Extended: the target to attach to; none: the stub runs the program.
    pub attach: Option<u32>,
    /// Extended, running the program: its path on the remote.
    pub exec_file: Option<String>,
}

/// One output channel, its port possibly one of the server's free ports.
#[derive(Debug, Clone)]
pub struct ChannelPlan {
    pub name: String,
    pub port: ChannelPortRef,
    pub format: channels::Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPortRef {
    Fixed(u16),
    /// The n-th (0-based) free port of the server (`{port4}` is 3).
    Free(usize),
}

impl ChannelPlan {
    /// The channel with its port known: `ports` are the server's free ports.
    pub fn resolve(&self, ports: &[u16]) -> Option<Channel> {
        let port = match self.port {
            ChannelPortRef::Fixed(p) => p,
            ChannelPortRef::Free(i) => *ports.get(i)?,
        };
        Some(Channel { name: self.name.clone(), port, format: self.format })
    }

    /// How many free ports the channel needs (0 for a fixed one).
    fn ports_needed(&self) -> usize {
        match self.port {
            ChannelPortRef::Fixed(_) => 0,
            ChannelPortRef::Free(i) => i + 1,
        }
    }
}

impl RemotePlan {
    /// Free ports the server's arguments and the channels ask for.
    pub fn ports_needed(&self) -> usize {
        self.channels.iter().map(ChannelPlan::ports_needed).chain([servers::ports_needed(&self.server_args)]).max().unwrap_or(0)
    }

    /// An extended stub that runs the program itself (`gdbserver --multi`): a launch, not an
    /// attach. Nothing is downloaded or reset, and gdb's own `start` stops at `main`.
    pub fn runs_program(&self) -> bool {
        self.extended && self.attach.is_none()
    }
}

/// The port of a loopback `target remote` argument (`localhost:3333`, `tcp:127.0.0.1:2331`,
/// `[::1]:3333`); none for a serial device, a pipe or another computer.
pub fn connect_loopback_port(connect: &str) -> Option<u16> {
    let c = connect.trim();
    let c = ["tcp4:", "tcp6:", "tcp:"].iter().find_map(|p| c.strip_prefix(p)).unwrap_or(c);
    let (host, port) = c.rsplit_once(':')?;
    super::process::loopback_host(host)?;
    port.parse().ok()
}

const MAX_CHANNELS: usize = 8;

fn channel_plans(l: &DebugLaunch, r: &RemoteTarget, has_server: bool) -> Result<Vec<ChannelPlan>, ApiError> {
    if r.channels.len() > MAX_CHANNELS {
        return Err(ApiError::bad_request(format!("{:?}: at most {MAX_CHANNELS} output channels", l.name)));
    }
    let mut out = vec![];
    for (i, c) in r.channels.iter().enumerate() {
        let format = match (c.format.as_deref().map(str::trim).map(str::to_ascii_lowercase).as_deref(), c.itm_port) {
            (None | Some("") | Some("text"), None) => channels::Format::Text,
            (None | Some("") | Some("text"), Some(_)) => return Err(ApiError::bad_request(format!("{:?}: channel {}: `itm_port` is for `format = \"itm\"`", l.name, i + 1))),
            (Some("itm"), p) => match p.unwrap_or(0) {
                p @ 0..=31 => channels::Format::Itm(p),
                p => return Err(ApiError::bad_request(format!("{:?}: channel {}: ITM stimulus ports are 0 to 31, not {p}", l.name, i + 1))),
            },
            (Some(other), _) => return Err(ApiError::bad_request(format!("{:?}: channel {}: format {other:?} is not `text` or `itm`", l.name, i + 1))),
        };
        let port = match &c.port {
            ChannelPort::Number(0) => return Err(ApiError::bad_request(format!("{:?}: channel {}: `port` is a TCP port number, or {{port2}} … {{port9}}", l.name, i + 1))),
            ChannelPort::Number(p) => ChannelPortRef::Fixed(*p),
            ChannelPort::Reference(text) => match servers::port_index(text) {
                // `{port}` is the gdb stub's: a channel on it would read gdb's protocol.
                Some(n) if n >= 1 && has_server => ChannelPortRef::Free(n),
                Some(n) if n >= 1 => return Err(ApiError::bad_request(format!("{:?}: channel {}: {text} names a free port of a debug server, and the configuration starts none", l.name, i + 1))),
                _ => return Err(ApiError::bad_request(format!("{:?}: channel {}: `port` is a TCP port number, or {{port2}} … {{port9}}, not {text:?}", l.name, i + 1))),
            },
        };
        let name = c.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| match format {
            channels::Format::Text => "Target output".to_string(),
            channels::Format::Itm(_) => "SWO".to_string(),
        });
        one_line("name", &name)?;
        out.push(ChannelPlan { name, port, format });
    }
    Ok(out)
}

/// gdb gets these as one line of its command language: no line breaks.
fn one_line(what: &str, v: &str) -> Result<(), ApiError> {
    if v.trim().is_empty() || v.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!("`{what}` must be one non-empty line without control characters")));
    }
    Ok(())
}

/// The program path for placeholders in a remote configuration, as the server sees it.
fn remote_vars(project: &Project, program: Option<&Path>) -> crate::apps::expand::Vars {
    let mut vars = crate::apps::expand::base_vars(project);
    if let Some(p) = program {
        vars.insert("program".into(), p.display().to_string());
    }
    vars
}

fn expand_server_arg(project: &Project, vars: &crate::apps::expand::Vars, arg: &str) -> String {
    crate::apps::expand::placeholders(arg, vars).replace("${workspaceFolder}", &project.root.display().to_string())
}

/// Resolve a launch configuration's `[debug.remote]` against the debug servers of
/// config.toml and the presets. Runs nothing.
pub fn remote_plan(state: &AppState, project: &Project, l: &DebugLaunch, r: &RemoteTarget, program: Option<&Path>) -> Result<RemotePlan, ApiError> {
    let id = r.server.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let server = match id {
        Some(id) => {
            let configured = state.config.read().debug.servers.clone();
            Some(servers::find(&configured, id).ok_or_else(|| {
                ApiError::not_configured(format!("launch configuration {:?} names debug server {id:?}, which is not a preset: define [debug.servers.{id}] in config.toml", l.name))
            })?)
        }
        None => None,
    };
    let connect = r.connect.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    if let Some(c) = &connect {
        one_line("connect", c)?;
    }
    if server.is_none() {
        if connect.is_none() {
            return Err(ApiError::bad_request(format!("{:?}: `remote` needs a `server` to start or a `connect` address of a stub that is already running", l.name)));
        }
        if !r.server_args.is_empty() {
            return Err(ApiError::bad_request(format!("{:?}: `server_args` without a `server`", l.name)));
        }
    }
    let port = r.port.filter(|p| *p != 0).or_else(|| connect.as_deref().and_then(connect_loopback_port));
    let vars = remote_vars(project, program);
    let own = server.as_ref().map(|s| s.args.as_slice()).unwrap_or_default();
    let on_stop = match r.on_stop.as_deref().map(str::trim) {
        None | Some("") | Some("resume") => OnStop::Resume,
        Some("halt") => OnStop::Halt,
        Some(other) => return Err(ApiError::bad_request(format!("{:?}: `on_stop` is `resume` or `halt`, not {other:?}", l.name))),
    };
    // What goes after the user's arguments: what needs the target their files define.
    let post: &[String] = match (&on_stop, &server) {
        (OnStop::Resume, Some(s)) => &s.post_args,
        _ => &[],
    };
    let server_args: Vec<String> = own.iter().chain(&r.server_args).chain(post).map(|a| expand_server_arg(project, &vars, a)).collect();
    let mut plan = RemotePlan {
        init: r.init.clone().or_else(|| server.as_ref().map(|s| s.init.clone())).unwrap_or_default(),
        reset: r.reset.clone().or_else(|| server.as_ref().map(|s| s.reset.clone())).unwrap_or_default(),
        download: r.download.or_else(|| server.as_ref().map(|s| s.download)).unwrap_or(false),
        stop_at: StopAt::parse(r.stop_at.as_deref()),
        source_map: {
            let mut pairs = vec![];
            for [from, to] in &r.source_map {
                one_line("source_map", from)?;
                one_line("source_map", to)?;
                pairs.push((from.trim().to_string(), expand_server_arg(project, &vars, to.trim())));
            }
            pairs
        },
        svd: match r.svd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => Some(host_program(project, s).map_err(|e| ApiError::bad_request(format!("{:?}: svd: {}", l.name, e.message)))?),
            None => None,
        },
        channels: channel_plans(l, r, server.is_some())?,
        extended: r.extended,
        attach: r.attach,
        exec_file: r.exec_file.as_deref().map(str::trim).filter(|f| !f.is_empty()).map(str::to_string),
        server,
        server_args,
        connect,
        port,
    };
    for (what, list) in [("init", &plan.init), ("reset", &plan.reset)] {
        for c in list {
            one_line(what, c)?;
        }
    }
    if let StopAt::Location(l) = &plan.stop_at {
        one_line("stop_at", l)?;
    }
    if plan.server.is_some() && servers::ports_needed(&plan.server_args) == 0 && plan.port.is_none() {
        return Err(ApiError::bad_request(format!(
            "{:?}: Workbench cannot tell when the debug server is ready: its arguments have no {{port}}, and the configuration names neither a `port` nor a loopback `connect`",
            l.name
        )));
    }
    for c in &plan.channels {
        if let ChannelPortRef::Free(i) = c.port {
            let name = if i == 0 { "{port}".to_string() } else { format!("{{port{}}}", i + 1) };
            if !plan.server_args.iter().any(|a| a.contains(&name)) {
                return Err(ApiError::bad_request(format!("{:?}: channel {:?} is on {name}, which no argument of the debug server uses: nothing would listen there", l.name, c.name)));
            }
        }
    }
    if plan.attach.is_some() && !plan.extended {
        return Err(ApiError::bad_request(format!("{:?}: `attach` names a target of an extended-remote stub: set `extended = true`", l.name)));
    }
    if plan.exec_file.is_some() && !plan.runs_program() {
        return Err(ApiError::bad_request(format!("{:?}: `exec_file` is the program's path on a stub that runs it: set `extended = true` and no `attach`", l.name)));
    }
    if let Some(f) = &plan.exec_file {
        one_line("exec_file", f)?;
    }
    if plan.runs_program() {
        // gdb's `run` starts the program on the remote: nothing to flash, nothing to reset.
        if program.is_none() {
            return Err(ApiError::bad_request(format!("{:?}: an extended-remote stub runs the `program`: name it (or set `attach` to attach to a target)", l.name)));
        }
        if r.download == Some(true) || r.reset.as_ref().is_some_and(|c| !c.is_empty()) || !matches!(plan.stop_at, StopAt::Main) {
            return Err(ApiError::bad_request(format!("{:?}: `download`, `reset` and `stop_at` apply to attaching: an extended-remote stub that runs the program stops at main with stop_on_entry", l.name)));
        }
        plan.download = false;
        plan.reset = vec![];
    } else if plan.download && program.is_none() {
        return Err(ApiError::bad_request(format!("{:?}: downloading to the target needs a `program`", l.name)));
    }
    Ok(plan)
}

/// Whether the configuration stops at the program's entry: `stop_on_entry`, or a remote
/// target that says where (`stop_at`).
fn stops_at_entry(l: &DebugLaunch) -> bool {
    l.stop_on_entry || l.remote.as_ref().and_then(|r| r.stop_at.as_deref()).is_some_and(|s| !s.trim().is_empty())
}

/// The GDB for a remote target: the adapter the configuration names (it must be a gdb),
/// else the one for the program's architecture.
pub async fn remote_adapter(state: &AppState, project: &Project, l: &DebugLaunch) -> Result<Adapter, ApiError> {
    let cfg = state.config.read().debug.clone();
    let adapter = match l.adapter.as_deref().filter(|a| !a.trim().is_empty()) {
        Some(id) => adapters::find(&cfg, id.trim()).ok_or_else(|| {
            ApiError::not_configured(format!("launch configuration {:?} names adapter {id:?}, which is not a preset: define [debug.adapters.{id}] in config.toml", l.name))
        })?,
        None => {
            let arch = l.program.as_deref().filter(|p| !p.trim().is_empty()).and_then(|p| host_program(project, p).ok()).and_then(|p| elf::arch(&p));
            adapters::for_remote(state, arch).await
        }
    };
    if adapter.kind != AdapterKind::Gdb {
        return Err(ApiError::bad_request(format!(
            "{:?} debugs a remote target through gdb, and {} is not a gdb adapter (name a gdb: gdb-multiarch, arm-none-eabi-gdb… or leave `adapter` out)",
            l.name, adapter.label
        )));
    }
    Ok(adapter)
}

fn run_named<'a>(project: &'a Project, name: &str) -> Option<&'a crate::config::project::RunConfig> {
    project.config.runs.iter().find(|r| r.name == name)
}

/// A program path of a launch configuration on the host: placeholders
/// (`{root}`, `${workspaceFolder}`, toolchains) expanded; relative ones resolved in
/// the project (and kept inside it), on Windows also written with `\` (`build\app.exe`).
pub fn host_program(project: &Project, program: &str) -> Result<PathBuf, ApiError> {
    let vars = crate::apps::expand::base_vars(project);
    let p = crate::apps::expand::placeholders(program, &vars).replace("${workspaceFolder}", &project.root.display().to_string());
    if crate::util::os::path::is_absolute_str(&p) || crate::util::os::path::home_relative(&p).is_some() {
        return Ok(crate::config::expand_tilde(&p));
    }
    let rel = crate::util::os::path::segments(&p).collect::<Vec<_>>().join("/");
    crate::util::paths::resolve_in_root(&project.root, &rel)
}

/// Views for the UI, with problems found without running anything.
pub async fn views(state: &AppState, project: &Project) -> Vec<LaunchConfigView> {
    let mut out = vec![];
    let in_container = project.config.debugs.iter().any(|d| d.remote.is_some()) && crate::devcontainer::exec_target(state, &project.id).await.is_some();
    for d in defs(project) {
        let l = &d.launch;
        let language = language_of(l, &project.root);
        let mut problems = vec![];
        let adapter = match &l.remote {
            Some(_) => remote_adapter(state, project, l).await,
            None => adapter_for(state, l, &language).await,
        };
        let adapter = match adapter {
            Ok(a) => Some(a),
            Err(e) => {
                problems.push(e.message);
                None
            }
        };
        let remote = l.remote.as_ref().map(|r| {
            let program = l.program.as_deref().filter(|p| !p.trim().is_empty()).and_then(|p| host_program(project, p).ok());
            match remote_plan(state, project, l, r, program.as_deref()) {
                Ok(rp) => {
                    let av = rp.server.as_ref().map(|s| servers::probe(state, s));
                    if let Some(p) = av.as_ref().and_then(|a| a.problem.clone()) {
                        problems.push(format!("{}: {p}. {}", rp.server.as_ref().map(|s| s.label.as_str()).unwrap_or(""), rp.server.as_ref().map(|s| s.install_hint.as_str()).unwrap_or("")));
                    }
                    if let Some(path) = rp.svd.as_ref().filter(|p| !p.is_file()) {
                        problems.push(format!("the SVD file {} does not exist: the Peripherals view has nothing to show", path.display()));
                    }
                    RemoteView {
                        server: rp.server.as_ref().map(|s| s.id.clone()),
                        server_label: rp.server.as_ref().map(|s| s.label.clone()),
                        server_available: av.as_ref().is_some_and(|a| a.available),
                        command_line: rp.server.as_ref().map(|s| std::iter::once(s.command.clone()).chain(rp.server_args.iter().cloned()).collect::<Vec<_>>().join(" ")),
                        connect: rp.connect.clone(),
                        init: rp.init.clone(),
                        reset: rp.reset.clone(),
                        download: rp.download,
                        stop_at: rp.stop_at.describe(),
                        extended: rp.extended,
                        attach: rp.attach,
                        svd: r.svd.clone(),
                        in_container,
                        channels: rp
                            .channels
                            .iter()
                            .map(|c| match c.port {
                                ChannelPortRef::Fixed(p) => format!("{} (port {p})", c.name),
                                ChannelPortRef::Free(i) => format!("{} ({})", c.name, if i == 0 { "{port}".to_string() } else { format!("{{port{}}}", i + 1) }),
                            })
                            .collect(),
                    }
                }
                Err(e) => {
                    problems.push(e.message);
                    RemoteView { server: r.server.clone(), server_label: None, server_available: false, command_line: None, connect: r.connect.clone(), init: vec![], reset: vec![], download: false, stop_at: "main".into(), extended: r.extended, attach: r.attach, channels: vec![], svd: r.svd.clone(), in_container }
                }
            }
        });
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
        if (l.request == DebugRequest::Launch || l.remote.is_some()) && d.build.is_none() {
            match (&l.program, &l.module) {
                (None, None) if l.remote.is_some() => {}
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
            request: match &l.remote {
                Some(r) if r.extended && r.attach.is_none() => DebugRequest::Launch,
                Some(_) => DebugRequest::Attach,
                None => l.request,
            },
            adapter: adapter.as_ref().map(|a| a.id.clone()),
            adapter_label: adapter.as_ref().map(|a| a.label.clone()),
            adapter_available: available,
            language,
            program: l.program.clone(),
            module: l.module.clone(),
            args: l.args.clone(),
            cwd: l.cwd.clone(),
            pre_launch,
            stop_on_entry: stops_at_entry(l),
            remote,
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
    /// A remote target (embedded): the debug server to start and what gdb does once
    /// connected. `request` is then an attach.
    pub remote: Option<RemotePlan>,
    /// A remote target of a project that works in its dev container: the build step runs
    /// there (its toolchain is there) while gdb and the debug server run on this computer
    /// (the probe is plugged in here), so `target` stays none.
    pub build_target: Option<ExecTarget>,
    /// `[from, to]` pairs for gdb's `set substitute-path`: the workspace as the container
    /// built it, then the configuration's `source_map`.
    pub substitute_paths: Vec<(String, String)>,
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
    // A remote target is an attach to a gdb stub, never to a process.
    let remote_cfg = l.remote.clone();
    if let Some(r) = &remote_cfg {
        // gdb connects to a stub: an attach to its target (or a launch of the program on an
        // extended stub), never to a process of this computer.
        let runs = r.extended && r.attach.is_none();
        l.request = if runs { DebugRequest::Launch } else { DebugRequest::Attach };
        l.pid = r.attach.filter(|_| r.extended);
    }
    if l.request == DebugRequest::Attach && remote_cfg.is_none() {
        if let Some(p) = pid {
            if p <= 1 || p == std::process::id() {
                return Err(ApiError::bad_request("pick another process"));
            }
            l.pid = Some(p);
        }
    }
    let language = language_of(&l, &project.root);
    let adapter = match remote_cfg {
        Some(_) => remote_adapter(state, project, &l).await?,
        None => adapter_for(state, &l, &language).await?,
    };
    if l.request == DebugRequest::Attach && remote_cfg.is_none() && !attach_target_known(adapter.kind, &l) {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "pid_required",
            format!("{name:?} attaches to a process: pick one (or give the configuration a `pid`)"),
        ));
    }
    // A remote target is debugged from this computer even when the project works in its dev
    // container: the probe is plugged in here. Only the build step goes to the container.
    let build_target = match remote_cfg {
        Some(_) => crate::devcontainer::exec_target(state, &project.id).await,
        None => None,
    };
    let (target, inside_command) = if remote_cfg.is_some() { (None, None) } else { container(state, project, &adapter).await? };
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
    let remote = match &remote_cfg {
        Some(r) => Some(remote_plan(state, project, &l, r, program.as_deref())?),
        None => None,
    };
    if (l.request == DebugRequest::Launch || remote.is_some()) && d.build.is_none() && pre.is_none() && target.is_none() {
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
        stop_on_entry: stop_on_entry.unwrap_or(stops_at_entry(&l)),
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
        substitute_paths: substitute_paths(build_target.as_ref(), remote.as_ref()),
        build_target,
        remote,
        raw_arguments: None,
        connect: None,
    })
}

/// gdb's `set substitute-path` pairs for a remote target: the workspace of a dev container the
/// program was built in (its path there → its path here), then the configuration's own.
pub fn substitute_paths(build: Option<&ExecTarget>, remote: Option<&RemotePlan>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = build.and_then(|t| t.map.as_ref()).map(|(host, container)| vec![(container.trim_end_matches('/').to_string(), host.display().to_string())]).unwrap_or_default();
    pairs.extend(remote.map(|r| r.source_map.clone()).unwrap_or_default());
    pairs
}

/// `set substitute-path` commands as gdb's `-iex` arguments (one per pair: quoted, so a space
/// in a path stays in it).
pub fn substitute_path_args(pairs: &[(String, String)]) -> Vec<String> {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    pairs.iter().flat_map(|(from, to)| ["-iex".to_string(), format!("set substitute-path {} {}", quote(from), quote(to))]).collect()
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
        remote: None,
        build_target: None,
        substitute_paths: vec![],
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
/// path (after the build resolved it); `remote_target` is what gdb connects to (the
/// debug server's port is only known once it runs).
pub fn arguments(plan: &Plan, program: Option<&str>, terminal: bool, remote_target: Option<&str>) -> Value {
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
            if let Some(t) = remote_target {
                m.insert("target".into(), json!(t));
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
            overlay_error: None,
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
            remote: None,
            build_target: None,
            substitute_paths: vec![],
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

    /// In a Go module a package path is Go's: `./cmd/api`, and on Windows `.\cmd\api`.
    #[test]
    fn go_packages_are_go_in_either_spelling() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("go.mod"), "module example.com/app\n").unwrap();
        let lang = |program: &str| language_of(&DebugLaunch { program: Some(program.into()), ..Default::default() }, d.path());
        for go in [".", "./", "./cmd/api", "cmd/api/main.go"] {
            assert_eq!(lang(go), "go", "{go}");
        }
        for other in ["", "..", "../x", "build/app", ".x"] {
            assert_eq!(lang(other), "cpp", "{other}");
        }
        // `\` separates only on Windows: elsewhere `.\cmd\api` is one odd file name.
        assert_eq!(lang(r".\cmd\api"), if cfg!(windows) { "go" } else { "cpp" });
        assert_eq!(lang(r".\"), if cfg!(windows) { "go" } else { "cpp" });
        // A Cargo project stays Rust.
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        assert_eq!(lang(r".\cmd\api"), "rust");
        assert_eq!(lang("./cmd/api"), "rust");
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
        let gdb = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), launch.clone(), d.path()), Some("/w/app"), true, None);
        assert_eq!(gdb["program"], "/w/app");
        assert_eq!(gdb["stopAtBeginningOfMainSubprogram"], true);
        assert_eq!(gdb["env"]["RUST_LOG"], "debug");
        assert_eq!(gdb["setupCommands"], json!(["x"]), "extra is merged last");
        assert_eq!(gdb["request"], "launch");
        let lldb = arguments(&plan(adapters::find(&cfg, "lldb-dap").unwrap(), launch.clone(), d.path()), Some("/w/app"), true, None);
        assert_eq!(lldb["env"], json!(["RUST_LOG=debug"]));
        assert_eq!(lldb["runInTerminal"], true);
        let mut py = launch.clone();
        py.module = Some("api.main".into());
        py.extra.insert("python".into(), json!("/usr/bin/python3"));
        let dp = arguments(&plan(adapters::find(&cfg, "debugpy").unwrap(), py, d.path()), None, true, None);
        assert_eq!(dp["module"], "api.main");
        assert_eq!(dp["console"], "integratedTerminal");
        assert_eq!(dp["python"], "/usr/bin/python3");
        assert!(dp.get("program").is_none());
        let mut attach = launch;
        attach.request = DebugRequest::Attach;
        attach.pid = Some(4242);
        let at = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), attach.clone(), d.path()), None, false, None);
        assert_eq!((at["pid"].clone(), at["request"].clone()), (json!(4242), json!("attach")));
        let at = arguments(&plan(adapters::find(&cfg, "debugpy").unwrap(), attach.clone(), d.path()), None, false, None);
        assert_eq!(at["processId"], 4242);
        // No pid: none is invented (gdb would "attach" to pid 0 and debug nothing).
        attach.pid = None;
        let at = arguments(&plan(adapters::find(&cfg, "gdb").unwrap(), attach, d.path()), None, false, None);
        assert!(at.get("pid").is_none() && at.get("processId").is_none(), "{at}");
    }

    #[test]
    fn a_loopback_connect_names_the_port_of_our_server() {
        for (connect, port) in [
            ("localhost:3333", Some(3333)),
            ("127.0.0.1:2331", Some(2331)),
            ("tcp:localhost:3333", Some(3333)),
            ("tcp4:127.0.0.1:50000", Some(50000)),
            ("[::1]:3333", Some(3333)),
            (" localhost:3333 ", Some(3333)),
            // Another computer, a serial device, a pipe, a bad port: ours to start nothing on.
            ("192.168.1.9:3333", None),
            ("board.local:3333", None),
            ("/dev/ttyACM0", None),
            ("COM3", None),
            ("| ssh board gdbserver - prog", None),
            ("localhost:notaport", None),
            ("localhost:99999", None),
        ] {
            assert_eq!(connect_loopback_port(connect), port, "{connect}");
        }
    }

    #[tokio::test]
    async fn remote_plans_take_the_servers_defaults_and_the_configurations_overrides() {
        let t = crate::platform::testutil::app().await;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("fw.elf"), b"").unwrap();
        let p = project(d.path(), "");
        let plan = |toml_text: &str| {
            let l: DebugLaunch = toml::from_str(toml_text).unwrap();
            let program = l.program.as_deref().map(|x| d.path().join(x));
            remote_plan(&t.state, &p, &l, l.remote.as_ref().unwrap(), program.as_deref())
        };
        // OpenOCD's defaults: halt, download, halt again; run to `main` when asked to stop.
        let r = plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\nserver_args = [\"-f\", \"{root}/board.cfg\", \"-c\", \"x {program}\", \"-c\", \"y ${workspaceFolder}\"]").unwrap();
        assert_eq!((r.init.clone(), r.reset.clone(), r.download, r.stop_at.clone()), (vec![], vec!["monitor reset halt".to_string()], true, StopAt::Main));
        let root = d.path().display().to_string();
        let fw = d.path().join("fw.elf").display().to_string();
        let all = &r.server_args;
        assert!(all[..6].iter().any(|a| a == "gdb_port {port}"), "the preset's arguments come first, the ports left for the server: {all:?}");
        assert_eq!(all[6..12], ["-f".to_string(), format!("{root}/board.cfg"), "-c".into(), format!("x {fw}"), "-c".into(), format!("y {root}")], "{{root}}, {{program}} and ${{workspaceFolder}} expand");
        // After the user's arguments (their files define the target): let the core run when gdb detaches.
        assert_eq!(all[12..], ["-c".to_string(), "foreach t [target names] { $t configure -event gdb-detach { resume } }".into()], "{all:?}");
        // `on_stop = "halt"` leaves it halted; `resume` is the default spelled out; others have no such hook.
        let tail = |extra: &str| plan(&format!("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\n{extra}")).unwrap().server_args.len();
        assert_eq!((tail("on_stop = \"halt\""), tail("on_stop = \"resume\""), tail("onStop = \"halt\"")), (6, 8, 6));
        assert_eq!(plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"st-util\"").unwrap().server_args, ["-p", "{port}"]);
        assert!(plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\non_stop = \"explode\"").unwrap_err().message.contains("`on_stop` is `resume` or `halt`"));
        // Overrides: an explicit empty list, no download, another place to stop.
        let r = plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\nreset = []\ndownload = false\nstop_at = \"app_main\"").unwrap();
        assert_eq!((r.reset, r.download, r.stop_at), (vec![], false, StopAt::Location("app_main".into())));
        assert_eq!(plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"qemu-arm\"").unwrap().download, false, "QEMU loads the image itself");
        assert_eq!(plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"qemu-arm\"\nstop_at = \" Reset \"").unwrap().stop_at, StopAt::Reset);
        // A loopback `connect` fixes the port the server must listen on.
        let r = plan("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"jlink\"\nconnect = \"localhost:2331\"").unwrap();
        assert_eq!((r.port, r.connect.as_deref()), (Some(2331), Some("localhost:2331")));
        // Mistakes.
        let err = |toml_text: &str| plan(toml_text).unwrap_err().message;
        assert!(err("name = \"a\"\n[remote]\nserver = \"mystery\"").contains("[debug.servers.mystery]"));
        assert!(err("name = \"a\"\n[remote]\ndownload = true").contains("`server` to start or a `connect`"));
        assert!(err("name = \"a\"\n[remote]\nserver = \"openocd\"").contains("needs a `program`"));
        assert!(err("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\ninit = \"a\\nb\"").contains("without control characters"));
        assert!(err("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nconnect = \"a\\nb\"").contains("`connect`"));
        assert!(err("name = \"a\"\nprogram = \"fw.elf\"\n[remote]\nserver = \"openocd\"\nstop_at = \"main\\rx\"").contains("`stop_at`"));
    }

    #[test]
    fn source_paths_are_translated_for_gdb_with_quoting() {
        let pairs = vec![("/workspaces/my proj".to_string(), "/home/u/my proj".to_string()), (r#"/ci/"x""#.to_string(), r"/tmp\y".to_string())];
        assert_eq!(
            substitute_path_args(&pairs),
            [
                "-iex",
                r#"set substitute-path "/workspaces/my proj" "/home/u/my proj""#,
                "-iex",
                r#"set substitute-path "/ci/\"x\"" "/tmp\\y""#,
            ]
        );
        assert!(substitute_path_args(&[]).is_empty());
        // The workspace of a dev container the program was built in comes first, then the configuration's own pairs.
        let target = ExecTarget {
            project_id: "p".into(),
            container_id: "c".into(),
            container_name: "c".into(),
            user: None,
            map: Some((PathBuf::from("/home/u/proj"), "/workspaces/proj/".into())),
            folder: "/workspaces/proj".into(),
            remote_env: vec![],
            workbench_url: None,
            docker: "docker".into(),
            shell: "/bin/sh".into(),
            has_bash: false,
        };
        let remote = RemotePlan {
            server: None,
            server_args: vec![],
            connect: Some("localhost:1".into()),
            port: None,
            init: vec![],
            reset: vec![],
            download: false,
            stop_at: StopAt::Main,
            source_map: vec![("/ci/build".into(), "/home/u/proj".into())],
            svd: None,
            channels: vec![],
            extended: false,
            attach: None,
            exec_file: None,
        };
        assert_eq!(
            substitute_paths(Some(&target), Some(&remote)),
            [("/workspaces/proj".to_string(), "/home/u/proj".to_string()), ("/ci/build".to_string(), "/home/u/proj".to_string())]
        );
        assert_eq!(substitute_paths(None, Some(&remote)), [("/ci/build".to_string(), "/home/u/proj".to_string())]);
        assert!(substitute_paths(None, None).is_empty());
    }

    #[test]
    fn a_remote_attach_names_its_target_and_never_a_pid() {
        let d = tempfile::tempdir().unwrap();
        let cfg = adapters::DebugConfig::default();
        let launch = DebugLaunch { name: "board".into(), request: DebugRequest::Attach, program: Some("fw.elf".into()), extra: [("setupCommands".to_string(), json!(["x"]))].into(), ..Default::default() };
        let mut pl = plan(adapters::find(&cfg, "gdb-multiarch").unwrap(), launch, d.path());
        pl.launched = false;
        let a = arguments(&pl, Some("/w/fw.elf"), false, Some("127.0.0.1:3333"));
        assert_eq!((a["request"].clone(), a["target"].clone(), a["program"].clone()), (json!("attach"), json!("127.0.0.1:3333"), json!("/w/fw.elf")));
        assert!(a.get("pid").is_none() && a.get("processId").is_none(), "{a}");
        assert_eq!(a["setupCommands"], json!(["x"]), "extra is merged last, as ever");
        // A plain attach has no `target`.
        assert!(arguments(&pl, Some("/w/fw.elf"), false, None).get("target").is_none());
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

    /// gdb attaches to a process only where the OS allows it (`util::os::support`): on
    /// Windows an attach by language passes over gdb and one that names it is refused;
    /// launches and gdbserver targets keep gdb everywhere.
    #[tokio::test]
    async fn gdb_attaches_only_where_the_os_allows_it() {
        use crate::util::os::support::{Feature, unsupported};
        let t = crate::platform::testutil::app().await;
        let launch = |adapter: Option<&str>, request: DebugRequest, extra: &[(&str, Value)]| DebugLaunch {
            name: "x".into(),
            adapter: adapter.map(str::to_string),
            request,
            extra: extra.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            ..Default::default()
        };
        let kind = |r: Result<Adapter, ApiError>| r.map(|a| a.kind).map_err(|e| (e.code, e.feature));
        let named = kind(adapter_for(&t.state, &launch(Some("gdb"), DebugRequest::Attach, &[]), "cpp").await);
        match unsupported(Feature::GdbAttach) {
            None => assert_eq!(named, Ok(AdapterKind::Gdb)),
            Some(_) => {
                assert_eq!(named, Err(("unsupported_platform", Some("gdbAttach"))));
                let by_language = kind(adapter_for(&t.state, &launch(None, DebugRequest::Attach, &[]), "cpp").await);
                assert!(by_language.is_ok_and(|k| k != AdapterKind::Gdb), "an attach by language picked gdb");
                // A language only gdb knows: the reason, not "no debug adapter knows fortran".
                let only_gdb = kind(adapter_for(&t.state, &launch(None, DebugRequest::Attach, &[]), "fortran").await);
                assert_eq!(only_gdb, Err(("unsupported_platform", Some("gdbAttach"))));
            }
        }
        let gdb_launch = kind(adapter_for(&t.state, &launch(Some("gdb"), DebugRequest::Launch, &[]), "cpp").await);
        assert_eq!(gdb_launch, Ok(AdapterKind::Gdb));
        let gdbserver = kind(adapter_for(&t.state, &launch(Some("gdb"), DebugRequest::Attach, &[("target", json!("localhost:1234"))]), "cpp").await);
        assert_eq!(gdbserver, Ok(AdapterKind::Gdb));
    }
}
