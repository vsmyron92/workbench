//! `workbench service install [--enable] | uninstall | status`: run Workbench as a
//! systemd user service and add a desktop launcher.
//!
//! * `$XDG_CONFIG_HOME/systemd/user/<name>.service` runs this binary's absolute path
//!   with `serve`, `Restart=on-failure`, the current `PATH` (agents, git and language
//!   servers need the user's tools) and the current `WORKBENCH_CONFIG_DIR` /
//!   `WORKBENCH_DATA_DIR` / `WORKBENCH_LOG` when set.
//! * `$XDG_DATA_HOME/applications/<name>.desktop` runs `workbench open`, with the
//!   icon in `$XDG_DATA_HOME/icons/hicolor/scalable/apps/<name>.svg`.
//! * `--enable` runs `systemctl --user daemon-reload` and `enable --now`; without
//!   it nothing is started. `--dry-run` prints everything and changes nothing.
//!
//! Files carry a marker line; `uninstall` removes only files that have it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use clap::{Args, Subcommand};

use crate::util;

const MARKER: &str = "Written by `workbench service install`";
const ICON_SVG: &str = include_str!("../../../web/public/icons/workbench.svg");
/// Environment carried into the unit and the launcher when set.
const CARRIED_VARS: &[&str] = &["WORKBENCH_CONFIG_DIR", "WORKBENCH_DATA_DIR", "WORKBENCH_LOG"];

#[derive(Args, Debug)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Write a systemd user unit (`workbench serve`) and a desktop launcher (`workbench open`).
    Install {
        /// Also run `systemctl --user daemon-reload` and `enable --now`: Workbench starts now and at every login.
        #[arg(long)]
        enable: bool,
        /// Print what would be written and run, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Unit and launcher name (another instance with its own WORKBENCH_CONFIG_DIR/DATA_DIR needs its own).
        #[arg(long, default_value = "workbench")]
        name: String,
    },
    /// Stop and disable the unit, then remove the unit, the launcher and the icon.
    Uninstall {
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "workbench")]
        name: String,
    },
    /// Show the installed files and the unit's state.
    Status {
        #[arg(long, default_value = "workbench")]
        name: String,
    },
}

/// Where things go and what they run: the process environment in `cli`, scratch
/// directories and a fake `systemctl` in tests.
#[derive(Debug, Clone)]
pub struct Env {
    pub exe: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
    pub systemctl: Option<PathBuf>,
    /// `PATH` for the unit.
    pub path: Option<String>,
    /// `CARRIED_VARS` that are set.
    pub vars: Vec<(String, String)>,
    /// The data dir of this instance (runtime.json), to see whether it already runs.
    pub data_dir: Option<PathBuf>,
}

/// An absolute `$XDG_*_HOME`, else the default under the home directory (the
/// spec says relative values are to be ignored).
fn xdg_dir(var: &str, default: &str) -> anyhow::Result<PathBuf> {
    if let Some(v) = std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Ok(v);
    }
    Ok(dirs::home_dir().context("no home directory")?.join(default))
}

impl Env {
    pub fn from_process() -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("cannot find this executable")?;
        let exe = exe.canonicalize().unwrap_or(exe);
        if exe.to_string_lossy().ends_with(" (deleted)") {
            bail!("this workbench binary was replaced while running; run the new one");
        }
        let vars = CARRIED_VARS
            .iter()
            .filter_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()).map(|v| (k.to_string(), v)))
            .map(|(k, v)| {
                // Directories go in absolute, whatever the cwd of this shell was.
                if k != "WORKBENCH_LOG" {
                    let p = crate::config::expand_tilde(&v);
                    let p = if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
                    (k, p.to_string_lossy().into_owned())
                } else {
                    (k, v)
                }
            })
            .collect();
        Ok(Self {
            exe,
            config_home: xdg_dir("XDG_CONFIG_HOME", ".config")?,
            data_home: xdg_dir("XDG_DATA_HOME", ".local/share")?,
            systemctl: util::which_path("systemctl"),
            path: std::env::var("PATH").ok().map(|p| clean_path(&p)).filter(|p| !p.is_empty()),
            vars,
            data_dir: crate::config::Paths::from_env().ok().map(|p| p.data_dir),
        })
    }

    fn unit_file(&self, name: &str) -> PathBuf {
        self.config_home.join("systemd/user").join(format!("{name}.service"))
    }
    fn desktop_file(&self, name: &str) -> PathBuf {
        self.data_home.join("applications").join(format!("{name}.desktop"))
    }
    fn icon_file(&self, name: &str) -> PathBuf {
        self.data_home.join("icons/hicolor/scalable/apps").join(format!("{name}.svg"))
    }
}

/// `PATH` for the unit: absolute entries only (a relative one would resolve against
/// the service's working directory), each once, in order.
fn clean_path(path: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    path.split(':').filter(|p| p.starts_with('/') && seen.insert(*p)).collect::<Vec<_>>().join(":")
}

fn check_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("--name must be letters, digits, - or _ (got {name:?})");
    }
    Ok(())
}

fn no_control(what: &str, s: &str) -> anyhow::Result<()> {
    if s.chars().any(char::is_control) {
        bail!("{what} contains a control character; it cannot go into a unit file");
    }
    Ok(())
}

/// A double-quoted systemd word: `\` and `"` escaped, `%` doubled (specifiers), and
/// in command lines `$` doubled (variable expansion).
fn systemd_quote(s: &str, exec: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if exec => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One argument of a desktop entry `Exec=` (Desktop Entry spec): quoted when it has
/// reserved characters, `%` doubled; then the value's own escaping of `\`.
fn desktop_arg(s: &str) -> String {
    const RESERVED: &[char] = &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];
    let s = s.replace('%', "%%");
    let arg = if s.contains(RESERVED) {
        let mut q = String::from("\"");
        for c in s.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        s
    };
    arg.replace('\\', "\\\\")
}

pub fn unit_text(env: &Env) -> anyhow::Result<String> {
    let exe = env.exe.to_string_lossy();
    no_control("the executable path", &exe)?;
    let mut s = format!(
        "# {MARKER}; `workbench service uninstall` removes it.\n\
         [Unit]\n\
         Description=Workbench, an AI-centric developer workspace\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} serve\n\
         WorkingDirectory=%h\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         # Workbench stops its terminals and agents itself (process groups) on SIGTERM.\n\
         KillMode=mixed\n\
         TimeoutStopSec=30\n",
        systemd_quote(&exe, true)
    );
    if let Some(path) = &env.path {
        no_control("PATH", path)?;
        s.push_str(&format!("Environment={}\n", systemd_quote(&format!("PATH={path}"), false)));
    }
    for (k, v) in &env.vars {
        no_control(k, v)?;
        s.push_str(&format!("Environment={}\n", systemd_quote(&format!("{k}={v}"), false)));
    }
    s.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(s)
}

pub fn desktop_text(env: &Env, name: &str) -> anyhow::Result<String> {
    let exe = env.exe.to_string_lossy();
    no_control("the executable path", &exe)?;
    let mut exec = vec![];
    let dirs: Vec<&(String, String)> = env.vars.iter().filter(|(k, _)| k != "WORKBENCH_LOG").collect();
    if !dirs.is_empty() {
        exec.push("env".to_string());
        for (k, v) in dirs {
            no_control(k, v)?;
            exec.push(desktop_arg(&format!("{k}={v}")));
        }
    }
    exec.push(desktop_arg(&exe));
    exec.push("open".into());
    let icon = env.icon_file(name).to_string_lossy().into_owned();
    no_control("the icon path", &icon)?;
    Ok(format!(
        "# {MARKER}\n\
         [Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Workbench\n\
         GenericName=Developer workspace\n\
         Comment=Agents, editor, git, CI and docs in one window\n\
         Exec={}\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=Development;IDE;\n\
         Keywords=agent;claude;git;ide;\n\
         StartupNotify=false\n",
        exec.join(" "),
        icon.replace('\\', "\\\\")
    ))
}

fn icon_text() -> String {
    format!("<!-- {MARKER} -->\n{ICON_SVG}")
}

fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.lines().take(3).any(|l| l.contains(MARKER)))
}

/// Runs `systemctl --user …`; returns (ok, output).
fn systemctl(env: &Env, args: &[&str]) -> anyhow::Result<(bool, String)> {
    let bin = env.systemctl.as_ref().context("systemctl was not found on PATH (is this a systemd system?)")?;
    let out = Command::new(bin)
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("cannot run {}", bin.display()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text.trim().to_string()))
}

/// The pid of a Workbench already serving this data dir, from its runtime.json.
fn running_pid(env: &Env) -> Option<u32> {
    let rt: serde_json::Value = util::fs::read_json(&env.data_dir.as_ref()?.join("runtime.json")).ok()??;
    let pid = rt["pid"].as_u64()? as u32;
    util::os::proc::pid_alive(pid as i32).then_some(pid)
}

fn write_file(path: &Path, text: &str, mode: u32) -> anyhow::Result<()> {
    util::fs::write_atomic(path, text.as_bytes(), mode)?;
    util::fs::set_mode(path, mode);
    Ok(())
}

pub fn install(env: &Env, name: &str, enable: bool, dry_run: bool, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    check_name(name)?;
    let unit = format!("{name}.service");
    let files = [
        (env.unit_file(name), unit_text(env)?),
        (env.desktop_file(name), desktop_text(env, name)?),
        (env.icon_file(name), icon_text()),
    ];
    for (path, _) in &files {
        if path.exists() && !is_ours(path) {
            bail!("{} exists and was not written by Workbench; remove or rename it first", path.display());
        }
    }
    if dry_run {
        for (path, text) in &files {
            if path.extension().is_some_and(|e| e == "svg") {
                writeln!(out, "would write {} (icon, {} bytes)\n", path.display(), text.len())?;
            } else {
                writeln!(out, "would write {}:\n{text}", path.display())?;
            }
        }
        if enable {
            writeln!(out, "would run: systemctl --user daemon-reload")?;
            writeln!(out, "would run: systemctl --user enable --now {unit}")?;
        }
        return Ok(());
    }
    // Whether the unit runs already (asked when it matters: enabling, a re-install, a
    // server running), and a server serving this data dir outside it (started by
    // hand: it holds the port, so the unit would fail and restart forever).
    let pid = running_pid(env);
    let reinstall = env.unit_file(name).exists();
    let active =
        (enable || reinstall || pid.is_some()) && env.systemctl.is_some() && systemctl(env, &["is-active", &unit]).is_ok_and(|(ok, _)| ok);
    let outside = pid.filter(|_| !active);
    if enable {
        if let Some(pid) = outside {
            bail!("Workbench is already running (pid {pid}) outside the service; stop it first, then run this again");
        }
        if env.systemctl.is_none() {
            bail!("systemctl was not found on PATH; install without --enable and start `workbench serve` another way");
        }
    }
    for (path, text) in &files {
        write_file(path, text, 0o644)?;
        writeln!(out, "wrote {}", path.display())?;
    }
    if enable {
        let (ok, text) = systemctl(env, &["daemon-reload"])?;
        if !ok {
            bail!("systemctl --user daemon-reload failed: {text}");
        }
        if active {
            // `enable --now` leaves a running unit alone: it would keep the old binary
            // and environment. Restart it to load what was just written.
            let (ok, text) = systemctl(env, &["enable", &unit])?;
            if !ok {
                bail!("systemctl --user enable {unit} failed: {text}");
            }
            writeln!(out, "restarting {unit} to load the new unit (open Workbench tabs reconnect)")?;
            out.flush()?;
            let (ok, text) = systemctl(env, &["restart", &unit])?;
            if !ok {
                bail!("systemctl --user restart {unit} failed: {text}");
            }
            writeln!(out, "restarted {unit}; it starts at login")?;
        } else {
            let (ok, text) = systemctl(env, &["enable", "--now", &unit])?;
            if !ok {
                bail!("systemctl --user enable --now {unit} failed: {text}");
            }
            writeln!(out, "enabled and started {unit}: Workbench now starts at login")?;
        }
        writeln!(out, "open it with the Workbench launcher or `workbench open`; logs: journalctl --user -u {unit}")?;
    } else if active {
        writeln!(out, "\n{unit} is running the previous unit. To load this one:")?;
        writeln!(out, "  systemctl --user daemon-reload && systemctl --user restart {unit}")?;
        writeln!(out, "(or run `workbench service install --enable`)")?;
    } else {
        writeln!(out, "\nnothing was started.")?;
        if let Some(pid) = outside {
            // Starting the unit now would fail to bind and restart forever.
            writeln!(out, "Workbench is running outside the service (pid {pid}): stop it first. Then, to run Workbench now and at every login:")?;
        } else {
            writeln!(out, "To run Workbench now and at every login:")?;
        }
        writeln!(out, "  systemctl --user daemon-reload && systemctl --user enable --now {unit}")?;
        writeln!(out, "(or run `workbench service install --enable`)")?;
    }
    Ok(())
}

pub fn uninstall(env: &Env, name: &str, dry_run: bool, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    check_name(name)?;
    let unit = format!("{name}.service");
    let unit_file = env.unit_file(name);
    let files = [unit_file.clone(), env.desktop_file(name), env.icon_file(name)];
    let unit_ours = unit_file.exists() && is_ours(&unit_file);
    if dry_run {
        if unit_ours {
            writeln!(out, "would run: systemctl --user disable --now {unit}")?;
        }
        for f in files.iter().filter(|f| f.exists()) {
            if is_ours(f) {
                writeln!(out, "would remove {}", f.display())?;
            } else {
                writeln!(out, "would keep {} (not written by Workbench)", f.display())?;
            }
        }
        if unit_ours {
            writeln!(out, "would run: systemctl --user daemon-reload")?;
        }
        return Ok(());
    }
    if unit_ours && env.systemctl.is_some() {
        let (ok, text) = systemctl(env, &["disable", "--now", &unit])?;
        if ok {
            writeln!(out, "stopped and disabled {unit}")?;
        } else {
            writeln!(out, "systemctl --user disable --now {unit}: {text}")?;
        }
    }
    let mut removed = 0;
    for f in &files {
        if !f.exists() {
            continue;
        }
        if !is_ours(f) {
            writeln!(out, "kept {} (not written by Workbench)", f.display())?;
            continue;
        }
        std::fs::remove_file(f).with_context(|| format!("remove {}", f.display()))?;
        writeln!(out, "removed {}", f.display())?;
        removed += 1;
    }
    if unit_ours && env.systemctl.is_some() {
        let _ = systemctl(env, &["daemon-reload"]);
    }
    if removed == 0 {
        writeln!(out, "nothing to remove")?;
    }
    Ok(())
}

pub fn status(env: &Env, name: &str, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    check_name(name)?;
    let unit = format!("{name}.service");
    let expected = [
        ("unit", env.unit_file(name), unit_text(env)?),
        ("launcher", env.desktop_file(name), desktop_text(env, name)?),
        ("icon", env.icon_file(name), icon_text()),
    ];
    for (what, path, text) in &expected {
        let state = match std::fs::read_to_string(path) {
            Err(_) => "not installed",
            Ok(t) if t == *text => "installed",
            Ok(_) if is_ours(path) => "installed (differs from this binary's; run install again)",
            Ok(_) => "present, not written by Workbench",
        };
        writeln!(out, "{what:9} {}: {state}", path.display())?;
    }
    if env.systemctl.is_some() {
        let enabled = systemctl(env, &["is-enabled", &unit]).map(|(_, t)| t).unwrap_or_else(|e| e.to_string());
        let active = systemctl(env, &["is-active", &unit]).map(|(_, t)| t).unwrap_or_else(|e| e.to_string());
        writeln!(out, "systemd   {unit}: {} / {}", first_line(&enabled), first_line(&active))?;
    } else {
        writeln!(out, "systemd   systemctl not found")?;
    }
    match running_pid(env) {
        Some(pid) => writeln!(out, "server    running (pid {pid})")?,
        None => writeln!(out, "server    not running")?,
    }
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// `workbench service …`
pub fn cli(args: ServiceArgs) -> anyhow::Result<()> {
    let env = Env::from_process()?;
    let mut out = std::io::stdout();
    match args.action {
        ServiceAction::Install { enable, dry_run, name } => install(&env, &name, enable, dry_run, &mut out),
        ServiceAction::Uninstall { dry_run, name } => uninstall(&env, &name, dry_run, &mut out),
        ServiceAction::Status { name } => status(&env, &name, &mut out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        log: PathBuf,
    }

    /// Scratch XDG dirs and a fake `systemctl` that logs its arguments.
    fn fixture(fail: bool) -> Fixture {
        fixture_with(fail, false)
    }

    /// `active`: the unit runs already (`is-active` says so).
    fn fixture_with(fail: bool, active: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("systemctl.log");
        let fake = dir.path().join("bin/systemctl");
        std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
        let is_active = if active { "echo active; exit 0" } else { "echo inactive; exit 3" };
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\ncase \"$2\" in is-active) {is_active};; is-enabled) echo disabled; exit 1;; esac\n{}",
            log.display(),
            if fail { "echo 'Failed to connect to bus' >&2; exit 1\n" } else { "exit 0\n" }
        );
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test's child forked while the script was open for writing can hold it
        // until that child execs: running it then fails with ETXTBSY. Wait that out, then
        // start the log clean.
        for _ in 0..200 {
            match std::process::Command::new(&fake).arg("--warm-up").output() {
                Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(std::time::Duration::from_millis(10)),
                _ => break,
            }
        }
        let _ = std::fs::remove_file(&log);
        let env = Env {
            exe: PathBuf::from("/opt/work bench/bin/workbench"),
            config_home: dir.path().join("config"),
            data_home: dir.path().join("data"),
            systemctl: Some(fake),
            path: Some("/usr/bin:/home/me/.cargo/bin:/home/me/100%/bin".into()),
            vars: vec![
                ("WORKBENCH_CONFIG_DIR".into(), "/scratch/cfg".into()),
                ("WORKBENCH_DATA_DIR".into(), "/scratch/data $x".into()),
            ],
            data_dir: Some(dir.path().join("wbdata")),
        };
        Fixture { _dir: dir, env, log }
    }

    fn calls(f: &Fixture) -> Vec<String> {
        std::fs::read_to_string(&f.log).unwrap_or_default().lines().map(str::to_string).collect()
    }

    #[test]
    fn install_writes_unit_launcher_and_icon_without_starting() {
        let f = fixture(false);
        let mut out = vec![];
        install(&f.env, "workbench", false, false, &mut out).unwrap();
        let unit = std::fs::read_to_string(f.env.config_home.join("systemd/user/workbench.service")).unwrap();
        assert!(unit.contains("ExecStart=\"/opt/work bench/bin/workbench\" serve\n"), "{unit}");
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/home/me/.cargo/bin:/home/me/100%%/bin\"\n"), "{unit}");
        assert!(unit.contains("Environment=\"WORKBENCH_CONFIG_DIR=/scratch/cfg\"\n"));
        assert!(unit.contains("Environment=\"WORKBENCH_DATA_DIR=/scratch/data $x\"\n"), "Environment= does not expand $");
        let desktop = std::fs::read_to_string(f.env.data_home.join("applications/workbench.desktop")).unwrap();
        assert!(
            desktop.contains("Exec=env WORKBENCH_CONFIG_DIR=/scratch/cfg \"WORKBENCH_DATA_DIR=/scratch/data \\\\$x\" \"/opt/work bench/bin/workbench\" open\n"),
            "{desktop}"
        );
        assert!(desktop.contains(&format!("Icon={}\n", f.env.data_home.join("icons/hicolor/scalable/apps/workbench.svg").display())));
        let icon = std::fs::read_to_string(f.env.data_home.join("icons/hicolor/scalable/apps/workbench.svg")).unwrap();
        assert!(icon.contains("<svg"));
        let mode = std::fs::metadata(f.env.config_home.join("systemd/user/workbench.service")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert!(calls(&f).is_empty(), "nothing is started without --enable");
        assert!(String::from_utf8(out).unwrap().contains("nothing was started"));
    }

    #[test]
    fn dry_run_changes_nothing() {
        let f = fixture(false);
        let mut out = vec![];
        install(&f.env, "workbench", true, true, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("would write") && text.contains("would run: systemctl --user enable --now workbench.service"), "{text}");
        assert!(!f.env.config_home.exists() && !f.env.data_home.exists());
        assert!(calls(&f).is_empty());
    }

    #[test]
    fn enable_reloads_and_enables_through_systemctl() {
        let f = fixture(false);
        install(&f.env, "workbench", true, false, &mut vec![]).unwrap();
        assert_eq!(calls(&f), vec!["--user is-active workbench.service", "--user daemon-reload", "--user enable --now workbench.service"]);
        // A failing systemctl is reported.
        let g = fixture(true);
        let err = install(&g.env, "workbench", true, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("daemon-reload failed") && err.contains("Failed to connect to bus"), "{err}");
    }

    #[test]
    fn enable_refuses_while_a_server_runs_outside_the_unit() {
        let f = fixture(false);
        let data = f.env.data_dir.clone().unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("runtime.json"), format!("{{\"pid\": {}}}", std::process::id())).unwrap();
        let err = install(&f.env, "workbench", true, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("already running"), "{err}");
        assert!(!f.env.unit_file("workbench").exists());
    }

    fn fake_running_server(f: &Fixture) {
        let data = f.env.data_dir.clone().unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("runtime.json"), format!("{{\"pid\": {}}}", std::process::id())).unwrap();
    }

    /// Without --enable, the printed next step must not be one that crash-loops.
    #[test]
    fn install_without_enable_warns_about_a_server_started_by_hand() {
        let f = fixture(false);
        fake_running_server(&f);
        let mut out = vec![];
        install(&f.env, "workbench", false, false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let stop = text.find(&format!("running outside the service (pid {})", std::process::id())).unwrap_or_else(|| panic!("{text}"));
        assert!(stop < text.find("enable --now").unwrap(), "stop it first, then enable: {text}");
        assert_eq!(calls(&f), vec!["--user is-active workbench.service"], "only asked, nothing started");
    }

    /// Re-installing over a running unit restarts it, so it runs the new binary and environment.
    #[test]
    fn reinstall_restarts_a_running_unit() {
        let f = fixture_with(false, true);
        fake_running_server(&f);
        let mut out = vec![];
        install(&f.env, "workbench", true, false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            calls(&f),
            vec!["--user is-active workbench.service", "--user daemon-reload", "--user enable workbench.service", "--user restart workbench.service"]
        );
        assert!(text.contains("restarted workbench.service") && !text.contains("enabled and started"), "{text}");
        // Without --enable: the hint restarts it instead of `enable --now` (a no-op on a running unit).
        let g = fixture_with(false, true);
        fake_running_server(&g);
        let mut out = vec![];
        install(&g.env, "workbench", false, false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("systemctl --user restart workbench.service") && !text.contains("outside the service"), "{text}");
    }

    #[test]
    fn uninstall_disables_and_removes_only_our_files() {
        let f = fixture(false);
        install(&f.env, "workbench", false, false, &mut vec![]).unwrap();
        // Someone's own launcher with our name is left alone.
        std::fs::write(f.env.desktop_file("workbench"), "[Desktop Entry]\nName=Mine\n").unwrap();
        let mut out = vec![];
        uninstall(&f.env, "workbench", false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!f.env.unit_file("workbench").exists());
        assert!(!f.env.icon_file("workbench").exists());
        assert!(f.env.desktop_file("workbench").exists(), "{text}");
        assert!(text.contains("kept"), "{text}");
        assert_eq!(calls(&f), vec!["--user disable --now workbench.service", "--user daemon-reload"]);
        // Install refuses to overwrite a file that is not ours.
        let err = install(&f.env, "workbench", false, false, &mut vec![]).unwrap_err().to_string();
        assert!(err.contains("not written by Workbench"), "{err}");
    }

    #[test]
    fn status_reports_files_and_unit_state() {
        let f = fixture(false);
        let mut out = vec![];
        status(&f.env, "workbench", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("not installed") && text.contains("disabled / inactive") && text.contains("not running"), "{text}");
        install(&f.env, "workbench", false, false, &mut vec![]).unwrap();
        let mut out = vec![];
        let mut other = f.env.clone();
        other.exe = PathBuf::from("/usr/local/bin/workbench");
        status(&other, "workbench", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("differs from this binary's"), "{text}");
        assert!(text.contains("icon") && text.lines().filter(|l| l.ends_with(": installed")).count() == 1, "{text}");
    }

    #[test]
    fn names_and_quoting() {
        assert!(check_name("workbench-dev_2").is_ok());
        for bad in ["", "../x", "a b", "x.service", "ü"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert_eq!(systemd_quote("/a b/c\"d\\e%f$g", true), "\"/a b/c\\\"d\\\\e%%f$$g\"");
        assert_eq!(systemd_quote("X=$HOME", false), "\"X=$HOME\"");
        assert_eq!(desktop_arg("/usr/bin/workbench"), "/usr/bin/workbench");
        assert_eq!(desktop_arg("/opt/a b/wb"), "\"/opt/a b/wb\"");
        assert_eq!(desktop_arg("100%"), "100%%");
        assert_eq!(desktop_arg("K=/plain/dir"), "K=/plain/dir");
        assert_eq!(clean_path("/usr/bin:.:bin:/usr/bin:/home/me/.cargo/bin::/usr/local/bin"), "/usr/bin:/home/me/.cargo/bin:/usr/local/bin");
        let mut env = fixture(false).env;
        env.exe = PathBuf::from("/bin/work\nbench");
        assert!(unit_text(&env).is_err());
    }
}
