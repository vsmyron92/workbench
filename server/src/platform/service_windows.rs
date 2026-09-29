//! `workbench service install [--enable] | uninstall | status | stop` on Windows: Workbench
//! starts at sign-in from the per-user `Run` value, through the launcher `workbenchw.exe`
//! (docs/windows-port.md §2). `service.rs` is the systemd variant.
//!
//! * `%LOCALAPPDATA%\workbench\service.json` (`service-<name>.json`) holds the current
//!   `WORKBENCH_CONFIG_DIR` / `WORKBENCH_DATA_DIR` / `WORKBENCH_LOG` when set. Not `PATH`: a
//!   program started at sign-in has the user's.
//! * The Start Menu shortcut `Workbench.lnk` runs `workbenchw.exe open`: it starts the
//!   service when it is not running, then opens a signed-in window like `workbench open`.
//! * `--enable` sets the value `Workbench` of `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//!   to `"<folder>\workbenchw.exe"` and starts the service now. Over a running service it
//!   starts a new supervisor (`service run --replace <old data dir>`) that stops the old one:
//!   stopping the old server ends its terminals, and this command with them when it runs in
//!   one. `--dry-run` prints everything and changes nothing.
//! * `workbenchw.exe` is a GUI program (no console at sign-in) that starts the supervisor,
//!   `workbench service run`: it runs `workbench serve` without a console window, appends its
//!   output to `service.log` next to `service.json`, restarts it 5 s after a failure
//!   (systemd's `RestartSec=5`) and gives up after 5 failures within 60 s. It starts nothing
//!   while a server already serves the data dir.
//! * `stop` sets the stop events of the server and of the supervisor
//!   (`os::proc::request_stop`) and waits until the server's port is free. `uninstall` stops
//!   the service last, after removing its files.
//!
//! Files carry a marker (the shortcut in its description); `uninstall` removes only what
//! has it, and the `Run` value only when it runs a `workbenchw.exe`.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::util;
use crate::util::os::autostart;
use crate::util::os::proc::{self, Event};

pub const ABOUT: &str = "Start Workbench at sign-in (workbenchw.exe), with a Start Menu shortcut";

const MARKER: &str = "Written by `workbench service install`";
/// Environment carried into the service when set.
const CARRIED_VARS: &[&str] = &["WORKBENCH_CONFIG_DIR", "WORKBENCH_DATA_DIR", "WORKBENCH_LOG"];
const DEFAULT_NAME: &str = "workbench";
const LAUNCHER: &str = "workbenchw.exe";
const DESCRIPTION: &str = "Agents, editor, git, CI and docs in one window.";
/// systemd's `RestartSec=5`.
const RESTART_DELAY: Duration = Duration::from_secs(5);
/// The supervisor gives up after this many failures within `FAILURE_WINDOW`.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// How long a stop may take before the supervisor ends the server (systemd's
/// `TimeoutStopSec=30`).
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// How much longer `stop` waits for a supervised server: time for the supervisor to end it
/// (systemd's stop, too, waits through the timeout and the kill).
const KILL_GRACE: Duration = Duration::from_secs(10);
/// How long `install --enable` and the Start Menu shortcut wait for a server to come up.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// A larger `service.log` is moved to `service.log.old` when the supervisor starts.
const LOG_LIMIT: u64 = 10 << 20;
const POLL: Duration = Duration::from_millis(200);

#[derive(Args, Debug)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Write the service's settings and a Start Menu shortcut (`workbenchw open`).
    Install {
        /// Also start Workbench now and at every sign-in (the `Run` entry of HKCU).
        #[arg(long)]
        enable: bool,
        /// Print what would be written and started, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Entry, shortcut and settings name (another instance with its own WORKBENCH_CONFIG_DIR/DATA_DIR needs its own).
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
    },
    /// Stop the service, then remove the sign-in entry, the shortcut and the settings.
    Uninstall {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
    },
    /// Show what is installed, whether Workbench starts at sign-in, and whether it runs.
    Status {
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
    },
    /// Stop the running Workbench and the service's restarts, and wait until its port is free.
    Stop {
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
    },
    /// The supervisor `workbenchw.exe` starts: runs `workbench serve`, restarting it after a failure.
    #[command(hide = true)]
    Run {
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
        /// Stop the service running on this data dir first (`install --enable` restarting it).
        #[arg(long)]
        replace: Option<PathBuf>,
    },
    /// `workbenchw.exe open`: start the service when it does not run, then open a signed-in window.
    #[command(hide = true)]
    Open {
        #[arg(long, default_value = DEFAULT_NAME)]
        name: String,
    },
}

/// Where things go and what runs: the process environment in `cli`, scratch folders and
/// registry keys in tests.
#[derive(Debug, Clone)]
pub struct Env {
    /// `workbench.exe`, which the supervisor runs.
    pub exe: PathBuf,
    /// `workbenchw.exe`, next to it: what the `Run` value and the shortcut start.
    pub launcher: PathBuf,
    /// `%LOCALAPPDATA%\workbench`: `service.json` and `service.log`.
    pub state_dir: PathBuf,
    /// The Start Menu's Programs folder.
    pub programs_dir: PathBuf,
    /// Keys under HKCU: `Run` and Task Manager's state for its values.
    pub run_key: String,
    pub approved_key: String,
    /// `CARRIED_VARS` that are set.
    pub vars: Vec<(String, String)>,
    /// This process runs elevated through UAC while the user's session does not
    /// (`autostart::elevated`): it starts no service, which would run as administrator too,
    /// its agents included.
    pub elevated: bool,
    pub start_timeout: Duration,
    /// The supervisor's stop timeout, after which it ends the server; `stop` waits
    /// `kill_grace` longer.
    pub stop_timeout: Duration,
    pub kill_grace: Duration,
}

impl Env {
    pub fn from_process() -> anyhow::Result<Self> {
        let exe = proc::current_exe().context("cannot find this executable")?;
        let programs_dir = autostart::programs_dir()
            .ok()
            .or_else(|| dirs::data_dir().map(|d| d.join(r"Microsoft\Windows\Start Menu\Programs")))
            .context("cannot find the Start Menu folder")?;
        Ok(Self {
            launcher: exe.with_file_name(LAUNCHER),
            exe,
            state_dir: util::os::path::data_home().context("no local application data folder")?.join("workbench"),
            programs_dir,
            run_key: autostart::RUN_KEY.into(),
            approved_key: autostart::APPROVED_KEY.into(),
            vars: carried_vars(),
            elevated: autostart::elevated(),
            start_timeout: START_TIMEOUT,
            stop_timeout: STOP_TIMEOUT,
            kill_grace: KILL_GRACE,
        })
    }

    fn settings_file(&self, name: &str) -> PathBuf {
        self.state_dir.join(format!("{}.json", file_stem(name)))
    }
    fn log_file(&self, name: &str) -> PathBuf {
        self.state_dir.join(format!("{}.log", file_stem(name)))
    }
    fn shortcut_file(&self, name: &str) -> PathBuf {
        self.programs_dir.join(format!("{}.lnk", entry_name(name)))
    }
    fn run_value_path(&self, name: &str) -> String {
        format!(r"HKCU\{}\{}", self.run_key, entry_name(name))
    }
    /// The folder of the programs: the shortcut's working folder.
    fn program_dir(&self) -> &Path {
        self.launcher.parent().unwrap_or(Path::new("."))
    }
}

/// `CARRIED_VARS` of this process that are set, directories made absolute.
fn carried_vars() -> Vec<(String, String)> {
    CARRIED_VARS
        .iter()
        .filter_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()).map(|v| (k.to_string(), v)))
        .map(|(k, v)| {
            // Directories go in absolute, whatever the current folder of this shell was, and
            // normalised (GetFullPathNameW: `\` separators, no `.` or `..`), as the data dir
            // names the service's events.
            if k != "WORKBENCH_LOG" {
                let p = crate::config::expand_tilde(&v);
                let p = std::path::absolute(&p).unwrap_or(p);
                (k, p.to_string_lossy().into_owned())
            } else {
                (k, v)
            }
        })
        .collect()
}

/// The data dir a server started with `vars` uses (as `config::Paths::from_env`).
fn data_dir_of(vars: &[(String, String)]) -> anyhow::Result<PathBuf> {
    match vars.iter().find(|(k, _)| k == "WORKBENCH_DATA_DIR") {
        Some((_, v)) => Ok(PathBuf::from(v)),
        None => Ok(util::os::path::data_home().context("no local application data folder")?.join("workbench")),
    }
}

fn file_stem(name: &str) -> String {
    if name == DEFAULT_NAME { "service".into() } else { format!("service-{name}") }
}

/// The `Run` value's and the shortcut's name.
fn entry_name(name: &str) -> String {
    if name == DEFAULT_NAME { "Workbench".into() } else { format!("Workbench-{name}") }
}

fn name_args(name: &str) -> Vec<&str> {
    if name == DEFAULT_NAME { vec![] } else { vec!["--name", name] }
}

/// `workbench service run [--name …]`: the supervisor.
fn run_args(name: &str) -> Vec<&str> {
    [vec!["service", "run"], name_args(name)].concat()
}

fn check_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("--name must be letters, digits, - or _ (got {name:?})");
    }
    Ok(())
}

/// The supervisor's stop event, next to the server's (`os::proc::stop_event_name`): held
/// while a supervisor runs for the data dir.
fn service_event(data_dir: &Path) -> String {
    format!("{}-service", proc::stop_event_name(data_dir))
}

/// What `service.json` says; `env` keeps only `CARRIED_VARS`.
#[derive(Serialize, Deserialize)]
struct Settings {
    note: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

pub fn settings_text(env: &Env) -> anyhow::Result<String> {
    let s = Settings {
        note: format!("{MARKER}; `workbench service uninstall` removes it. The service runs `workbench serve` with this environment."),
        env: env.vars.iter().cloned().collect(),
    };
    Ok(serde_json::to_string_pretty(&s)? + "\n")
}

/// The environment of the service `name`: its `service.json`, or this process's when it
/// has none.
fn service_vars(env: &Env, name: &str) -> anyhow::Result<Vec<(String, String)>> {
    let path = env.settings_file(name);
    let Some(s) = util::fs::read_json::<Settings>(&path)? else { return Ok(env.vars.clone()) };
    Ok(s.env.into_iter().filter(|(k, v)| CARRIED_VARS.contains(&k.as_str()) && !v.is_empty()).collect())
}

fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.lines().take(3).any(|l| l.contains(MARKER)))
}

fn shortcut_is_ours(path: &Path) -> bool {
    autostart::shortcut_mentions(path, MARKER)
}

/// A `Run` value that starts a `workbenchw.exe`, wherever it is, is Workbench's.
fn run_value_is_ours(v: &str) -> bool {
    v.to_ascii_lowercase().contains(&LAUNCHER.to_ascii_lowercase())
}

pub fn run_value_text(env: &Env, name: &str) -> anyhow::Result<String> {
    Ok(autostart::command_line(&env.launcher, &name_args(name))?)
}

fn shortcut_args(name: &str) -> String {
    [name_args(name), vec!["open"]].concat().join(" ")
}

fn shortcut_description() -> String {
    format!("{DESCRIPTION} {MARKER}.")
}

/// Ours, and starting this `workbenchw.exe` with the right arguments.
fn shortcut_matches(env: &Env, name: &str, path: &Path) -> bool {
    shortcut_is_ours(path)
        && autostart::shortcut_mentions(path, &env.program_dir().to_string_lossy())
        && autostart::shortcut_mentions(path, &shortcut_args(name))
}

/// What runtime.json says about the server of `data_dir`.
#[derive(Default)]
struct Runtime {
    pid: Option<u32>,
    port: Option<u16>,
    url: Option<String>,
}

impl Runtime {
    fn of(data_dir: &Path) -> Runtime {
        let Ok(Some(rt)) = util::fs::read_json::<serde_json::Value>(&data_dir.join("runtime.json")) else { return Runtime::default() };
        Runtime {
            pid: rt["pid"].as_u64().and_then(|p| u32::try_from(p).ok()),
            port: rt["port"].as_u64().and_then(|p| u16::try_from(p).ok()),
            url: rt["url"].as_str().map(str::to_string),
        }
    }

    fn live_pid(&self) -> Option<u32> {
        self.pid.filter(|p| proc::own_pid_alive(*p))
    }

    /// ` (pid N)` while that process runs.
    fn pid_text(&self) -> String {
        self.live_pid().map(|p| format!(" (pid {p})")).unwrap_or_default()
    }
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), POLL).is_ok()
}

/// Polls `done` until it holds or `timeout` has passed.
fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Starts the supervisor (`workbench service run`) apart from this process, then waits for
/// the server to come up.
fn start(env: &Env, name: &str, data_dir: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let apart = autostart::start_detached(&env.exe, &run_args(name), dirs::home_dir().as_deref())
        .with_context(|| format!("cannot start {}", env.exe.display()))?;
    if !apart {
        writeln!(out, "note: this terminal keeps what it starts in a job that may end with it; if Workbench stops when it closes, start it from the Start Menu")?;
    }
    wait_started(env, name, data_dir, out)
}

/// Replaces the service running on `old` (a data dir) with a new supervisor, which stops the
/// old one itself (`service run --replace`), then waits for the new server. Stopping the
/// old server ends its terminals, and with them this command when it runs in one: the
/// replacement must not depend on it. `Ok(false)`, and nothing changed, when this process's
/// job keeps what it starts: the replacement would end with that terminal too.
fn restart(env: &Env, name: &str, old: &Path, data_dir: &Path, out: &mut dyn Write) -> anyhow::Result<bool> {
    let old_pid = Runtime::of(old).live_pid();
    let old_arg = old.to_string_lossy();
    let args = [run_args(name), vec!["--replace", &old_arg]].concat();
    if !autostart::start_apart(&env.exe, &args, dirs::home_dir().as_deref()).with_context(|| format!("cannot start {}", env.exe.display()))? {
        return Ok(false);
    }
    writeln!(out, "restarting Workbench to load the new settings (open Workbench tabs reconnect)")?;
    out.flush()?;
    // The old server by its pid: on the same data dir, the new one takes over its events.
    if let Some(pid) = old_pid
        && !wait_until(env.stop_timeout + env.kill_grace, || !proc::own_pid_alive(pid))
    {
        writeln!(out, "the previous Workbench (pid {pid}) is still stopping; see {}", env.log_file(name).display())?;
        return Ok(true);
    }
    wait_started(env, name, data_dir, out)?;
    Ok(true)
}

/// Waits for the server of `data_dir` to come up, and says so.
fn wait_started(env: &Env, name: &str, data_dir: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    if wait_until(env.start_timeout, || proc::server_running(data_dir)) {
        let url = Runtime::of(data_dir).url.map(|u| format!(" at {u}")).unwrap_or_default();
        writeln!(out, "started Workbench{url}")?;
    } else {
        writeln!(out, "started the service, but the server is not up yet; see {}", env.log_file(name).display())?;
    }
    Ok(())
}

/// Asks the supervisor and the server of `data_dir` to stop and waits until both are gone
/// and the port is free. `Ok(false)` when neither was running.
fn stop_instance(env: &Env, data_dir: &Path) -> anyhow::Result<bool> {
    let before = Runtime::of(data_dir);
    let supervisor = Event::set(&service_event(data_dir)).context("cannot reach the service")?;
    let server = proc::request_stop(data_dir).context("cannot reach Workbench")?;
    if !supervisor && !server {
        // A server that could not create its stop event still answers on its port. (Without
        // one, runtime.json may be stale: a server ended at sign-out leaves it, and its pid
        // can be another program's by now.)
        if let (Some(pid), Some(port)) = (before.live_pid(), before.port.filter(|p| port_open(*p))) {
            bail!(
                "Workbench (pid {pid}) answers on port {port} but does not take stop requests (its log says why); stop it where it runs, or in Task Manager"
            );
        }
        return Ok(false);
    }
    // runtime.json is the running server's when it held its stop event.
    let down = || {
        !proc::server_running(data_dir)
            && !Event::exists(&service_event(data_dir))
            && (!server || before.pid.is_none_or(|p| !proc::own_pid_alive(p)))
            && (!server || before.port.is_none_or(|p| !port_open(p)))
    };
    // A supervisor ends a server that has not stopped after `stop_timeout`: wait for that too.
    let timeout = if supervisor { env.stop_timeout + env.kill_grace } else { env.stop_timeout };
    if !wait_until(timeout, down) {
        bail!("Workbench is still running after {} s{}", timeout.as_secs(), before.pid_text());
    }
    Ok(true)
}

pub fn install(env: &Env, name: &str, enable: bool, dry_run: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    check_name(name)?;
    if !env.launcher.is_file() {
        bail!(
            "{LAUNCHER} is not next to {}: it comes with Workbench's Windows release, and both programs stay in one folder",
            env.exe.display()
        );
    }
    let settings = env.settings_file(name);
    let text = settings_text(env)?;
    let lnk = env.shortcut_file(name);
    let run_path = env.run_value_path(name);
    let run_value = run_value_text(env, name)?;
    let entry = entry_name(name);
    if settings.exists() && !is_ours(&settings) {
        bail!("{} exists and was not written by Workbench; remove or rename it first", settings.display());
    }
    if lnk.exists() && !shortcut_is_ours(&lnk) {
        bail!("{} exists and was not written by Workbench; remove or rename it first", lnk.display());
    }
    let current = autostart::get_string(&env.run_key, &entry).with_context(|| format!("cannot read {run_path}"))?;
    if let Some(v) = current.as_deref().filter(|v| !run_value_is_ours(v)) {
        bail!("{run_path} is set to {v} by another program; remove it first (Task Manager › Startup apps shows it)");
    }
    // A sign-in entry is written with --enable, and kept up to date when it is there.
    let set_run = enable || current.is_some();
    let data_dir = data_dir_of(&env.vars)?;
    let shortcut_line = format!("{} {}", autostart::command_line(&env.launcher, &[])?, shortcut_args(name));
    if dry_run {
        writeln!(out, "would write {}:\n{text}", settings.display())?;
        writeln!(out, "would write {}: {shortcut_line}\n", lnk.display())?;
        if set_run {
            writeln!(out, "would set {run_path} to {run_value}")?;
        }
        if enable && env.elevated {
            writeln!(out, "would start nothing now: this terminal runs as administrator")?;
        } else if enable {
            writeln!(out, "would start: {}", autostart::command_line(&env.exe, &run_args(name))?)?;
        }
        return Ok(());
    }
    // Whether the service runs, on this data dir or on the one of the settings being
    // replaced, and whether a server started by hand serves this data dir: it holds the
    // port, so the service would only fail next to it.
    let previous = data_dir_of(&service_vars(env, name).unwrap_or_else(|_| env.vars.clone()))?;
    let supervised = [&data_dir, &previous].into_iter().find(|d| Event::exists(&service_event(d))).cloned();
    let outside = proc::server_running(&data_dir) && !Event::exists(&service_event(&data_dir));
    let pid = Runtime::of(&data_dir).pid_text();
    if enable && outside {
        bail!("Workbench is already running{pid} outside the service; stop it first (`workbench service stop` does), then run this again");
    }
    util::fs::write_atomic(&settings, text.as_bytes(), 0o644)?;
    writeln!(out, "wrote {}", settings.display())?;
    let shortcut =
        autostart::Shortcut { target: &env.launcher, args: &shortcut_args(name), workdir: env.program_dir(), description: &shortcut_description() };
    autostart::create_shortcut(&lnk, &shortcut).with_context(|| format!("cannot write {}", lnk.display()))?;
    writeln!(out, "wrote {}: {shortcut_line}", lnk.display())?;
    if set_run {
        autostart::set_string(&env.run_key, &entry, &run_value).with_context(|| format!("cannot set {run_path}"))?;
        writeln!(out, "set {run_path} to {run_value}")?;
    }
    let turned_off = set_run && autostart::startup_disabled(&env.approved_key, &entry);
    if enable && env.elevated {
        writeln!(out, "\nnot started now: this terminal runs as administrator, and Workbench and its agents would too.")?;
        if supervised.is_some() {
            writeln!(out, "Workbench runs with the previous settings. To load these, from a terminal that is not elevated:")?;
            writeln!(out, "  workbench service stop, then open {entry} from the Start Menu")?;
        } else {
            writeln!(out, "It starts at the next sign-in; to start it now, open {entry} from the Start Menu.")?;
        }
        if turned_off {
            writeln!(out, "note: {entry} is turned off in Task Manager › Startup apps, so it does not start at sign-in; turn it on there")?;
        }
    } else if enable {
        // A running supervisor keeps its old environment: a new one replaces it.
        let started = match &supervised {
            Some(old) => restart(env, name, old, &data_dir, out)?,
            None => {
                start(env, name, &data_dir, out)?;
                true
            }
        };
        if !started {
            writeln!(out, "\nnot restarted: this terminal keeps what it starts in its job, so a service")?;
            writeln!(out, "started from here would end with it. Workbench runs with the previous settings. To load these:")?;
            writeln!(out, "  workbench service stop, then open {entry} from the Start Menu")?;
        }
        if turned_off {
            writeln!(out, "note: {entry} is turned off in Task Manager › Startup apps, so it does not start at sign-in; turn it on there")?;
        } else {
            writeln!(out, "Workbench now starts at sign-in")?;
        }
        if started {
            writeln!(out, "open it with the Start Menu's {entry} or `workbench open`; log: {}", env.log_file(name).display())?;
        }
    } else if supervised.is_some() {
        writeln!(out, "\nWorkbench runs with the previous settings. To load these:")?;
        writeln!(out, "  workbench service stop, then open {entry} from the Start Menu")?;
        writeln!(out, "(or run `workbench service install --enable`)")?;
    } else {
        writeln!(out, "\nnothing was started.")?;
        if outside {
            writeln!(out, "Workbench is running outside the service{pid}: stop it first.")?;
        }
        if current.is_some() {
            writeln!(out, "Workbench starts at the next sign-in. To start it now, open {entry} from the Start Menu")?;
            writeln!(out, "(or run `workbench service install --enable`)")?;
        } else {
            writeln!(out, "To run Workbench now and at every sign-in:")?;
            writeln!(out, "  workbench service install --enable")?;
            writeln!(out, "(the Start Menu's {entry} starts it when it is not running)")?;
        }
    }
    Ok(())
}

pub fn uninstall(env: &Env, name: &str, dry_run: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    check_name(name)?;
    let entry = entry_name(name);
    let run_path = env.run_value_path(name);
    let run = autostart::get_string(&env.run_key, &entry).with_context(|| format!("cannot read {run_path}"))?;
    let run_ours = run.as_deref().is_some_and(run_value_is_ours);
    let files = [(env.settings_file(name), false), (env.shortcut_file(name), true)];
    let ours = |(f, lnk): &(PathBuf, bool)| if *lnk { shortcut_is_ours(f) } else { is_ours(f) };
    // The service's own data dir, even when its settings cannot be read any more.
    let data_dir = data_dir_of(&service_vars(env, name).unwrap_or_else(|_| env.vars.clone()))?;
    let supervised = Event::exists(&service_event(&data_dir));
    if dry_run {
        match &run {
            Some(v) if run_ours => writeln!(out, "would remove {run_path} ({v})")?,
            Some(v) => writeln!(out, "would keep {run_path} ({v}, not written by Workbench)")?,
            None => {}
        }
        for f in files.iter().filter(|(f, _)| f.exists()) {
            if ours(f) {
                writeln!(out, "would remove {}", f.0.display())?;
            } else {
                writeln!(out, "would keep {} (not written by Workbench)", f.0.display())?;
            }
        }
        if supervised {
            writeln!(out, "would stop Workbench (the service runs it)")?;
        }
        return Ok(());
    }
    let mut removed = 0;
    match &run {
        Some(_) if run_ours => {
            autostart::delete_value(&env.run_key, &entry).with_context(|| format!("cannot remove {run_path}"))?;
            writeln!(out, "removed {run_path}")?;
            removed += 1;
        }
        Some(v) => writeln!(out, "kept {run_path} ({v}, not written by Workbench)")?,
        None => {}
    }
    if run_ours || run.is_none() {
        // Task Manager's on/off state for the entry goes with it.
        let _ = autostart::delete_value(&env.approved_key, &entry);
    }
    for f in &files {
        if !f.0.exists() {
            continue;
        }
        if !ours(f) {
            writeln!(out, "kept {} (not written by Workbench)", f.0.display())?;
            continue;
        }
        std::fs::remove_file(&f.0).with_context(|| format!("remove {}", f.0.display()))?;
        writeln!(out, "removed {}", f.0.display())?;
        removed += 1;
    }
    if removed == 0 {
        writeln!(out, "nothing to remove")?;
    }
    // Last: stopping the server ends its terminals, and this command with them when it runs
    // in one.
    if supervised {
        writeln!(out, "stopping Workbench")?;
        out.flush()?;
        match stop_instance(env, &data_dir) {
            Ok(_) => writeln!(out, "stopped Workbench")?,
            Err(e) => writeln!(out, "could not stop Workbench: {e:#}")?,
        }
    }
    Ok(())
}

pub fn status(env: &Env, name: &str, out: &mut dyn Write) -> anyhow::Result<()> {
    check_name(name)?;
    let entry = entry_name(name);
    let settings = env.settings_file(name);
    let state = match std::fs::read_to_string(&settings) {
        Err(_) => "not installed",
        Ok(t) if t == settings_text(env)? => "installed",
        Ok(_) if is_ours(&settings) => "installed (differs from this environment's; run install again)",
        Ok(_) => "present, not written by Workbench",
    };
    writeln!(out, "{:9} {}: {state}", "settings", settings.display())?;
    let lnk = env.shortcut_file(name);
    let state = if !lnk.exists() {
        "not installed"
    } else if shortcut_matches(env, name, &lnk) {
        "installed"
    } else if shortcut_is_ours(&lnk) {
        "installed (differs from this binary's; run install again)"
    } else {
        "present, not written by Workbench"
    };
    writeln!(out, "{:9} {}: {state}", "shortcut", lnk.display())?;
    let expected = run_value_text(env, name)?;
    let state = match autostart::get_string(&env.run_key, &entry) {
        Err(e) => format!("cannot read: {e}"),
        Ok(None) => "not set: Workbench does not start at sign-in".into(),
        Ok(Some(v)) if !run_value_is_ours(&v) => format!("set by another program: {v}"),
        Ok(Some(_)) if autostart::startup_disabled(&env.approved_key, &entry) => "turned off in Task Manager › Startup apps".into(),
        Ok(Some(v)) if v == expected => "on: Workbench starts at sign-in".into(),
        Ok(Some(v)) => format!("on, but starts {v} (differs from this binary's; run install again)"),
    };
    writeln!(out, "{:9} {}: {state}", "sign-in", env.run_value_path(name))?;
    let data_dir = data_dir_of(&service_vars(env, name).unwrap_or_else(|_| env.vars.clone()))?;
    let supervised = Event::exists(&service_event(&data_dir));
    let rt = Runtime::of(&data_dir);
    let state = match (proc::server_running(&data_dir), supervised) {
        (true, true) => format!("running{}, under the service", rt.pid_text()),
        (true, false) => format!("running{}, started outside the service", rt.pid_text()),
        (false, true) => "not running; the service is about to restart it".into(),
        (false, false) => "not running".into(),
    };
    writeln!(out, "{:9} {state}", "server")?;
    writeln!(out, "{:9} {}", "log", env.log_file(name).display())?;
    Ok(())
}

pub fn stop(env: &Env, name: &str, out: &mut dyn Write) -> anyhow::Result<()> {
    check_name(name)?;
    let data_dir = data_dir_of(&service_vars(env, name)?)?;
    let pid = Runtime::of(&data_dir).pid_text();
    if stop_instance(env, &data_dir)? {
        writeln!(out, "stopped Workbench{pid}; the Start Menu's {} starts it again", entry_name(name))?;
    } else {
        writeln!(out, "Workbench is not running")?;
    }
    Ok(())
}

/// `workbenchw open`: starts the service when no server serves its data dir, then runs
/// `workbench open` in the service's environment.
pub fn open(env: &Env, name: &str) -> anyhow::Result<()> {
    check_name(name)?;
    let vars = service_vars(env, name)?;
    let data_dir = data_dir_of(&vars)?;
    if !proc::server_running(&data_dir) {
        if !Event::exists(&service_event(&data_dir)) {
            if env.elevated {
                bail!("Workbench is not running, and started from here it would run as administrator; open it from the Start Menu instead");
            }
            autostart::start_detached(&env.exe, &run_args(name), dirs::home_dir().as_deref())
                .with_context(|| format!("cannot start {}", env.exe.display()))?;
        }
        if !wait_until(env.start_timeout, || proc::server_running(&data_dir)) {
            bail!("Workbench did not start within {} s; see {}", env.start_timeout.as_secs(), env.log_file(name).display());
        }
    }
    let mut cmd = Command::new(&env.exe);
    cmd.arg("open").stdin(Stdio::null());
    with_vars(&mut cmd, &vars);
    let st = cmd.status().with_context(|| format!("cannot run {}", env.exe.display()))?;
    if !st.success() {
        bail!("`workbench open` failed ({})", proc::exit_text(&st));
    }
    Ok(())
}

/// The service's environment on `cmd`: exactly `vars` of `CARRIED_VARS`.
fn with_vars(cmd: &mut Command, vars: &[(String, String)]) {
    for k in CARRIED_VARS {
        cmd.env_remove(k);
    }
    cmd.envs(vars.iter().map(|(k, v)| (k, v)));
}

// ---------------------------------------------------------------- the supervisor

/// Failures of the server, for the restart budget: `max` within `window` end the retries.
struct Failures {
    at: VecDeque<Instant>,
    max: usize,
    window: Duration,
}

impl Failures {
    fn new(max: usize, window: Duration) -> Self {
        Failures { at: VecDeque::new(), max, window }
    }

    /// Records a failure at `now`; true when that makes `max` within the window.
    fn record(&mut self, now: Instant) -> bool {
        self.at.push_back(now);
        while self.at.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            self.at.pop_front();
        }
        self.at.len() >= self.max
    }
}

/// How supervising ended.
#[derive(Debug, PartialEq)]
enum Outcome {
    /// `workbench service stop` (the supervisor's event).
    Stopped,
    /// The server exited by itself with code 0 (its own stop event, Ctrl-Break).
    Exited,
    /// Another server serves the data dir (started by hand, or another copy).
    Held,
    /// `MAX_FAILURES` failures within `FAILURE_WINDOW`.
    GaveUp,
}

struct Supervisor<'a> {
    data_dir: PathBuf,
    /// A fresh `workbench serve` command.
    command: Box<dyn Fn() -> std::io::Result<Command> + 'a>,
    delay: Duration,
    failures: Failures,
    stop_timeout: Duration,
}

/// A line of the supervisor in the service log.
fn log_line(log: &mut dyn Write, msg: &str) {
    let _ = writeln!(log, "{}  service: {msg}", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ"));
    let _ = log.flush();
}

/// Runs the server until it is asked to stop (`me`, the supervisor's event), exits with
/// code 0, or fails too often; restarts it `delay` after a failure. Starts nothing while a
/// server serves the data dir already.
fn supervise(sup: &mut Supervisor, me: &Event, log: &mut dyn Write) -> Outcome {
    loop {
        if proc::server_running(&sup.data_dir) {
            let pid = Runtime::of(&sup.data_dir).pid_text();
            log_line(log, &format!("a Workbench server{pid} already serves {}; not starting another", sup.data_dir.display()));
            return Outcome::Held;
        }
        let failed = match (sup.command)().and_then(|mut c| c.spawn()) {
            Ok(mut child) => {
                log_line(log, &format!("started workbench serve (pid {})", child.id()));
                match wait_child(sup, &mut child, me) {
                    Ok((_, true)) => {
                        log_line(log, "stopped on request");
                        return Outcome::Stopped;
                    }
                    Ok((st, false)) if st.success() => {
                        log_line(log, "workbench serve exited (exit code 0); not restarting");
                        return Outcome::Exited;
                    }
                    Ok((st, false)) => format!("workbench serve ended: {}", proc::exit_text(&st)),
                    Err(e) => format!("cannot wait for workbench serve: {e}"),
                }
            }
            Err(e) => format!("cannot start workbench serve: {e}"),
        };
        log_line(log, &failed);
        if proc::server_running(&sup.data_dir) {
            log_line(log, &format!("another Workbench server serves {}; not restarting", sup.data_dir.display()));
            return Outcome::Held;
        }
        if sup.failures.record(Instant::now()) {
            log_line(log, &format!("{} failures within {} s: giving up", sup.failures.max, sup.failures.window.as_secs()));
            return Outcome::GaveUp;
        }
        log_line(log, &format!("restarting in {} s", sup.delay.as_secs_f32()));
        if me.wait(sup.delay).unwrap_or(false) {
            log_line(log, "stopped on request");
            return Outcome::Stopped;
        }
    }
}

/// Waits for `child` to exit; true with its status when a stop was requested meanwhile.
/// The server is then asked to stop until it does (the request may come while it is still
/// starting, before it listens for one), and ended after `stop_timeout`.
fn wait_child(sup: &Supervisor, child: &mut std::process::Child, me: &Event) -> std::io::Result<(ExitStatus, bool)> {
    let mut stopping: Option<Instant> = None;
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok((st, stopping.is_some()));
        }
        match stopping {
            None => {
                if me.wait(POLL)? {
                    stopping = Some(Instant::now());
                    let _ = proc::request_stop(&sup.data_dir);
                }
            }
            Some(since) => {
                if since.elapsed() > sup.stop_timeout {
                    let _ = child.kill();
                } else {
                    let _ = proc::request_stop(&sup.data_dir);
                }
                std::thread::sleep(POLL);
            }
        }
    }
}

/// `service.log` for appending, set aside first when it has grown past `LOG_LIMIT`.
fn open_log(path: &Path) -> anyhow::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    util::os::perm::open_append(path, 0o600).with_context(|| format!("open {}", path.display()))
}

/// `workbench service run`: the supervisor `workbenchw.exe` starts. With `replace` (a data
/// dir), it first stops the service running there: `install --enable` hands a restart over.
pub fn run(env: &Env, name: &str, replace: Option<&Path>) -> anyhow::Result<()> {
    check_name(name)?;
    let vars = service_vars(env, name)?;
    let data_dir = data_dir_of(&vars)?;
    // Created here as the server creates it: an existing data dir has one canonical path,
    // which names the events of this process, of the server and of the other commands alike.
    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    util::fs::set_mode(&data_dir, 0o700);
    // Before claiming the event, which the old supervisor holds on the same data dir.
    let replaced = replace.map(|old| stop_instance(env, old));
    let Some(me) = Event::create(&service_event(&data_dir)).context("cannot create the service's stop event")? else {
        // Another supervisor runs for the data dir (the shortcut clicked twice): its log, too,
        // is left alone.
        return Ok(());
    };
    let log_path = env.log_file(name);
    let mut log = open_log(&log_path)?;
    match replaced {
        Some(Ok(true)) => log_line(&mut log, "stopped the previous service, to start with new settings"),
        Some(Err(e)) => log_line(&mut log, &format!("cannot stop the previous service: {e:#}")),
        _ => {}
    }
    let exe = env.exe.clone();
    let child_log = log.try_clone()?;
    let command = move || -> std::io::Result<Command> {
        let mut cmd = Command::new(&exe);
        cmd.arg("serve").stdin(Stdio::null()).stdout(child_log.try_clone()?).stderr(child_log.try_clone()?);
        autostart::no_console_window(&mut cmd);
        with_vars(&mut cmd, &vars);
        if let Some(home) = dirs::home_dir() {
            cmd.current_dir(home);
        }
        Ok(cmd)
    };
    let mut sup = Supervisor {
        data_dir,
        command: Box::new(command),
        delay: RESTART_DELAY,
        failures: Failures::new(MAX_FAILURES, FAILURE_WINDOW),
        stop_timeout: env.stop_timeout,
    };
    if supervise(&mut sup, &me, &mut log) == Outcome::GaveUp {
        // Let a new start (the Start Menu, `install --enable`) run while the box is up.
        drop(me);
        let text = format!(
            "Workbench failed {MAX_FAILURES} times within a minute and is not restarted any more.\n\nIts log: {}",
            log_path.display()
        );
        autostart::message_box("Workbench", &text, true);
        bail!("gave up after {MAX_FAILURES} failures");
    }
    Ok(())
}

/// `workbench service …`
pub fn cli(args: ServiceArgs) -> anyhow::Result<()> {
    let env = Env::from_process()?;
    let mut out = std::io::stdout();
    match args.action {
        ServiceAction::Install { enable, dry_run, name } => install(&env, &name, enable, dry_run, &mut out),
        ServiceAction::Uninstall { dry_run, name } => uninstall(&env, &name, dry_run, &mut out),
        ServiceAction::Status { name } => status(&env, &name, &mut out),
        ServiceAction::Stop { name } => stop(&env, &name, &mut out),
        ServiceAction::Run { name, replace } => run(&env, &name, replace.as_deref()),
        ServiceAction::Open { name } => open(&env, &name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        env: Env,
        /// The scratch key under HKCU\Software, deleted on drop.
        key: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = autostart::delete_tree(&self.key);
        }
    }

    fn system32(exe: &str) -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32").join(exe)
    }

    /// Scratch folders, a scratch registry key standing in for HKCU's `Run` and
    /// `StartupApproved\Run`, and a harmless program in place of workbench.exe.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let key = format!(r"Software\Workbench-test-{}", util::random_token(8).replace(['-', '_'], "x"));
        let bin = dir.path().join("Work Bench");
        std::fs::create_dir_all(&bin).unwrap();
        let launcher = bin.join(LAUNCHER);
        std::fs::write(&launcher, b"").unwrap();
        std::fs::create_dir_all(dir.path().join("Programs")).unwrap();
        let env = Env {
            // Refuses the arguments and exits: never a real server.
            exe: system32("whoami.exe"),
            launcher,
            state_dir: dir.path().join("state"),
            programs_dir: dir.path().join("Programs"),
            run_key: format!(r"{key}\Run"),
            approved_key: format!(r"{key}\StartupApproved\Run"),
            vars: vec![
                ("WORKBENCH_CONFIG_DIR".into(), dir.path().join("cfg").display().to_string()),
                ("WORKBENCH_DATA_DIR".into(), dir.path().join("data").display().to_string()),
            ],
            // CI runners run tests as administrator; the elevated path has its own test.
            elevated: false,
            start_timeout: Duration::ZERO,
            stop_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_secs(1),
        };
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        Fixture { dir, env, key }
    }

    fn text(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap()
    }

    fn run_value(f: &Fixture) -> Option<String> {
        autostart::get_string(&f.env.run_key, "Workbench").unwrap()
    }

    #[test]
    fn install_writes_settings_and_shortcut_without_starting() {
        let f = fixture();
        let mut out = vec![];
        install(&f.env, "workbench", false, false, &mut out).unwrap();
        let out = text(out);
        let settings = std::fs::read_to_string(f.env.state_dir.join("service.json")).unwrap();
        assert!(settings.lines().nth(1).unwrap().contains(MARKER), "{settings}");
        let parsed: Settings = serde_json::from_str(&settings).unwrap();
        assert_eq!(parsed.env["WORKBENCH_DATA_DIR"], f.dir.path().join("data").display().to_string());
        assert!(!parsed.env.contains_key("PATH"));
        let lnk = f.env.programs_dir.join("Workbench.lnk");
        assert!(shortcut_matches(&f.env, "workbench", &lnk), "{out}");
        assert_eq!(run_value(&f), None, "no sign-in entry without --enable");
        assert!(out.contains("nothing was started"), "{out}");
    }

    #[test]
    fn dry_run_changes_nothing() {
        let f = fixture();
        let mut out = vec![];
        install(&f.env, "workbench", true, true, &mut out).unwrap();
        let out = text(out);
        assert!(out.contains("would write") && out.contains("would set") && out.contains("would start"), "{out}");
        assert!(out.contains(&format!("\"{}\"", f.env.launcher.display())), "{out}");
        assert!(!f.env.state_dir.exists() && !f.env.programs_dir.join("Workbench.lnk").exists());
        assert_eq!(run_value(&f), None);
    }

    #[test]
    fn enable_sets_the_sign_in_entry_and_starts() {
        let f = fixture();
        let mut out = vec![];
        install(&f.env, "workbench", true, false, &mut out).unwrap();
        let out = text(out);
        assert_eq!(run_value(&f), Some(format!("\"{}\"", f.env.launcher.display())));
        // whoami.exe stands in for the supervisor: no server comes up.
        assert!(out.contains("not up yet"), "{out}");
        // Turned off in Task Manager: said so.
        autostart::set_binary(&f.env.approved_key, "Workbench", &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        let mut out = vec![];
        install(&f.env, "workbench", true, false, &mut out).unwrap();
        assert!(text(out).contains("turned off in Task Manager"));
        let mut out = vec![];
        status(&f.env, "workbench", &mut out).unwrap();
        assert!(text(out).contains("turned off in Task Manager"));
    }

    #[test]
    fn enable_refuses_while_a_server_runs_outside_the_service() {
        let f = fixture();
        let data = f.dir.path().join("data");
        let _server = Event::create(&proc::stop_event_name(&data)).unwrap().unwrap();
        let err = install(&f.env, "workbench", true, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("already running"), "{err}");
        assert_eq!(run_value(&f), None);
        // Without --enable the hint says to stop it first.
        let mut out = vec![];
        install(&f.env, "workbench", false, false, &mut out).unwrap();
        let out = text(out);
        assert!(out.find("running outside the service").unwrap() < out.find("--enable").unwrap(), "{out}");
    }

    #[test]
    fn reinstalling_hands_the_restart_over_and_keeps_the_entry() {
        let f = fixture();
        install(&f.env, "workbench", true, false, &mut vec![]).unwrap();
        // Without --enable, an entry that is there stays and is kept up to date.
        let mut out = vec![];
        install(&f.env, "workbench", false, false, &mut out).unwrap();
        assert!(text(out).contains("starts at the next sign-in"));
        assert!(run_value(&f).is_some());
        // The service runs on the data dir of the settings being replaced (a stand-in holds
        // its event). A replacement supervisor stops it (whoami.exe stands in for that one), or
        // nothing happens when this test's job keeps what it starts; never this command itself,
        // which a terminal of the old server would not survive.
        let old = f.dir.path().join("data");
        let mut env = f.env.clone();
        env.vars[1].1 = f.dir.path().join("data2").display().to_string();
        let supervisor = Event::create(&service_event(&old)).unwrap().unwrap();
        let mut out = vec![];
        install(&env, "workbench", true, false, &mut out).unwrap();
        let out = text(out);
        assert!(out.contains("restarting Workbench") || out.contains("not restarted"), "{out}");
        assert!(!supervisor.wait(Duration::ZERO).unwrap(), "not stopped by `install`");
        assert!(run_value(&f).is_some());
    }

    #[test]
    fn stop_waits_for_the_supervisor_to_end_a_hung_server() {
        let f = fixture();
        let data = f.dir.path().join("data");
        // A supervisor stand-in that ends its server just after its stop timeout.
        let mut env = f.env.clone();
        env.stop_timeout = Duration::from_millis(300);
        let server = Event::create(&proc::stop_event_name(&data)).unwrap().unwrap();
        let service = Event::create(&service_event(&data)).unwrap().unwrap();
        let waiter = std::thread::spawn(move || {
            assert!(service.wait(Duration::from_secs(10)).unwrap());
            std::thread::sleep(Duration::from_millis(500));
            drop(server);
        });
        stop(&env, "workbench", &mut vec![]).unwrap();
        waiter.join().unwrap();
    }

    #[test]
    fn an_administrator_terminal_starts_nothing() {
        let mut f = fixture();
        f.env.elevated = true;
        // A stand-in for the supervisor: `install` must not stop it (it could not restart it).
        let supervisor = Event::create(&service_event(&f.dir.path().join("data"))).unwrap().unwrap();
        let mut out = vec![];
        install(&f.env, "workbench", true, false, &mut out).unwrap();
        let out = text(out);
        assert!(out.contains("runs as administrator") && !out.contains("restarting") && !out.contains("not up yet"), "{out}");
        assert!(!supervisor.wait(Duration::ZERO).unwrap(), "not asked to stop");
        assert!(run_value(&f).is_some(), "the entry is written: sign-in starts it unelevated");
        drop(supervisor);
        let err = open(&f.env, "workbench").unwrap_err().to_string();
        assert!(err.contains("administrator"), "{err}");
    }

    #[test]
    fn install_refuses_what_is_not_ours() {
        let f = fixture();
        autostart::set_string(&f.env.run_key, "Workbench", r"C:\Other\thing.exe").unwrap();
        let err = install(&f.env, "workbench", true, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("another program"), "{err}");
        autostart::delete_value(&f.env.run_key, "Workbench").unwrap();
        std::fs::create_dir_all(&f.env.state_dir).unwrap();
        std::fs::write(f.env.state_dir.join("service.json"), "{}").unwrap();
        let err = install(&f.env, "workbench", false, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("not written by Workbench"), "{err}");
    }

    #[test]
    fn uninstall_removes_only_ours() {
        let f = fixture();
        install(&f.env, "workbench", true, false, &mut vec![]).unwrap();
        autostart::set_binary(&f.env.approved_key, "Workbench", &[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        // Someone's own shortcut with our name is left alone.
        let lnk = f.env.programs_dir.join("Workbench.lnk");
        std::fs::write(&lnk, b"not a link of ours").unwrap();
        let mut out = vec![];
        uninstall(&f.env, "workbench", true, &mut out).unwrap();
        let dry = text(out);
        assert!(dry.contains("would remove HKCU") && dry.contains("would keep"), "{dry}");
        assert!(run_value(&f).is_some(), "a dry run changes nothing");
        let mut out = vec![];
        uninstall(&f.env, "workbench", false, &mut out).unwrap();
        let out = text(out);
        assert_eq!(run_value(&f), None, "{out}");
        assert!(!autostart::startup_disabled(&f.env.approved_key, "Workbench"), "Task Manager's state went with it");
        assert!(!f.env.state_dir.join("service.json").exists());
        assert!(lnk.exists() && out.contains("kept"), "{out}");
        let mut out = vec![];
        uninstall(&f.env, "workbench", false, &mut out).unwrap();
        assert!(text(out).contains("kept"));
    }

    #[test]
    fn status_reports_each_part() {
        let f = fixture();
        let mut out = vec![];
        status(&f.env, "workbench", &mut out).unwrap();
        let out = text(out);
        assert!(out.contains("not installed") && out.contains("not set") && out.contains("not running"), "{out}");
        install(&f.env, "workbench", true, false, &mut vec![]).unwrap();
        let mut out = vec![];
        status(&f.env, "workbench", &mut out).unwrap();
        let out = text(out);
        assert_eq!(out.lines().filter(|l| l.ends_with(": installed")).count(), 2, "{out}");
        assert!(out.contains("on: Workbench starts at sign-in"), "{out}");
        // Another copy of the programs: everything differs.
        let mut other = f.env.clone();
        other.launcher = f.dir.path().join("elsewhere").join(LAUNCHER);
        other.vars.pop();
        let mut out = vec![];
        status(&other, "workbench", &mut out).unwrap();
        let out = text(out);
        assert_eq!(out.matches("differs").count(), 3, "{out}");
        // A server of the service's data dir, run by hand.
        let _server = Event::create(&proc::stop_event_name(&f.dir.path().join("data"))).unwrap().unwrap();
        let mut out = vec![];
        status(&f.env, "workbench", &mut out).unwrap();
        assert!(text(out).contains("started outside the service"));
    }

    #[test]
    fn names_pick_their_own_entries() {
        let f = fixture();
        install(&f.env, "dev_2", true, false, &mut vec![]).unwrap();
        assert!(f.env.state_dir.join("service-dev_2.json").is_file());
        assert!(f.env.programs_dir.join("Workbench-dev_2.lnk").is_file());
        let v = autostart::get_string(&f.env.run_key, "Workbench-dev_2").unwrap().unwrap();
        assert_eq!(v, format!("\"{}\" --name dev_2", f.env.launcher.display()));
        assert_eq!(shortcut_args("dev_2"), "--name dev_2 open");
        assert_eq!(run_args("dev_2"), ["service", "run", "--name", "dev_2"]);
        for bad in ["", "../x", "a b", "x\"y", "ü"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stop_stops_the_server_and_the_service() {
        let f = fixture();
        let data = f.dir.path().join("data");
        let mut out = vec![];
        stop(&f.env, "workbench", &mut out).unwrap();
        assert!(text(out).contains("not running"));
        // Stand-ins that exit when their events are set, as the server and the supervisor do.
        let server = Event::create(&proc::stop_event_name(&data)).unwrap().unwrap();
        let service = Event::create(&service_event(&data)).unwrap().unwrap();
        let waiter = std::thread::spawn(move || {
            assert!(server.wait(Duration::from_secs(10)).unwrap());
            assert!(service.wait(Duration::from_secs(10)).unwrap());
        });
        let mut out = vec![];
        stop(&f.env, "workbench", &mut out).unwrap();
        assert!(text(out).contains("stopped Workbench"));
        waiter.join().unwrap();
    }

    fn cmd_exit(code: u8) -> Box<dyn Fn() -> std::io::Result<Command>> {
        Box::new(move || {
            let mut c = Command::new(system32("cmd.exe"));
            c.args(["/c", &format!("exit {code}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            Ok(c)
        })
    }

    fn supervisor(data_dir: &Path, command: Box<dyn Fn() -> std::io::Result<Command>>) -> Supervisor<'static> {
        Supervisor {
            data_dir: data_dir.to_path_buf(),
            command,
            delay: Duration::from_millis(10),
            failures: Failures::new(MAX_FAILURES, FAILURE_WINDOW),
            stop_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn the_supervisor_restarts_then_gives_up() {
        let dir = tempfile::tempdir().unwrap();
        let me = Event::create(&service_event(dir.path())).unwrap().unwrap();
        let mut log = vec![];
        assert_eq!(supervise(&mut supervisor(dir.path(), cmd_exit(3)), &me, &mut log), Outcome::GaveUp);
        let log = text(log);
        assert_eq!(log.matches("started workbench serve").count(), MAX_FAILURES, "{log}");
        assert_eq!(log.matches("exit code 3").count(), MAX_FAILURES, "{log}");
        // A clean exit is not restarted.
        let mut log = vec![];
        assert_eq!(supervise(&mut supervisor(dir.path(), cmd_exit(0)), &me, &mut log), Outcome::Exited);
        assert_eq!(text(log).matches("started workbench serve").count(), 1);
    }

    #[test]
    fn the_supervisor_leaves_a_server_started_by_hand_alone() {
        let dir = tempfile::tempdir().unwrap();
        let me = Event::create(&service_event(dir.path())).unwrap().unwrap();
        let _server = Event::create(&proc::stop_event_name(dir.path())).unwrap().unwrap();
        let mut log = vec![];
        assert_eq!(supervise(&mut supervisor(dir.path(), cmd_exit(1)), &me, &mut log), Outcome::Held);
        assert!(!text(log).contains("started workbench serve"));
    }

    #[test]
    fn the_supervisor_stops_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let name = service_event(dir.path());
        let me = Event::create(&name).unwrap().unwrap();
        // A server that runs until ended: the request reaches it through the (absent) stop
        // event, so the supervisor ends it after its stop timeout.
        let long = Box::new(|| {
            let mut c = Command::new(system32("ping.exe"));
            c.args(["-n", "60", "127.0.0.1"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            Ok(c)
        });
        let mut sup = supervisor(dir.path(), long);
        sup.stop_timeout = Duration::from_millis(500);
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            assert!(Event::set(&name).unwrap());
        });
        let mut log = vec![];
        assert_eq!(supervise(&mut sup, &me, &mut log), Outcome::Stopped);
        setter.join().unwrap();
    }

    #[test]
    fn the_failure_budget_is_per_window() {
        let t0 = Instant::now();
        let mut f = Failures::new(5, Duration::from_secs(60));
        for i in 0..4 {
            assert!(!f.record(t0 + Duration::from_secs(i * 5)));
        }
        assert!(f.record(t0 + Duration::from_secs(20)), "5 within 60 s");
        let mut f = Failures::new(5, Duration::from_secs(60));
        for i in 0..20 {
            assert!(!f.record(t0 + Duration::from_secs(i * 20)), "one every 20 s never adds up to 5 in 60 s");
        }
    }
}
