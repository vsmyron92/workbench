//! Debug servers: the programs between gdb and a microcontroller (OpenOCD, J-Link GDB
//! Server, pyOCD, ST-LINK's `st-util`) or standing in for one (QEMU). Each speaks gdb's
//! remote protocol on a TCP port; Workbench starts it for the length of a session, waits
//! until the port listens, points gdb at it and stops it at the end.
//!
//! Built-in presets merge with `[debug.servers.<id>]` from config.toml, field by field,
//! like adapters.
//!
//! **Trust.** A server is a command, so its command comes only from config.toml or a
//! preset. A repository's launch configuration names a server by id and may add
//! arguments (the board, the probe, the device): like `pre_launch`, they run only when
//! the user starts that configuration. Availability looks the command up on `PATH` and
//! never runs it (`JLinkGDBServer` would start serving).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::adapters::Availability;
use crate::app::AppState;

/// One `[debug.servers.<id>]` table. Every field is optional for a preset id.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Before the launch configuration's `server_args`. `{port}` is the gdb stub's port;
    /// `{port2}` … `{port9}` are free ports for the server's other listeners (telnet, SWO,
    /// RTT…), so two sessions never fight over 4444; `{program}` is the program's path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Plain environment for the server process.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Default `init`, `reset` and `download` of configurations that use this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<bool>,
    /// How long to wait for the stub's port (default 30).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_timeout_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
}

/// A resolved debug server (preset + overrides).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    /// Arguments that go after the configuration's own `server_args`, for what needs the
    /// target the user's files define (OpenOCD's `gdb-detach` handler). Not configurable.
    #[serde(skip)]
    pub post_args: Vec<String>,
    pub enabled: bool,
    pub builtin: bool,
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    pub init: Vec<String>,
    pub reset: Vec<String>,
    pub download: bool,
    #[serde(skip)]
    pub ready_timeout: Duration,
    pub install_hint: String,
}

pub const PRESETS: &[&str] = &["openocd", "jlink", "pyocd", "st-util", "qemu-arm"];

/// OpenOCD leaves the core halted when gdb detaches (Workbench's Stop), which freezes the
/// firmware: let it run instead. Goes after the user's `-f` files, whose targets it names;
/// `on_stop = "halt"` leaves it out. (st-util resumes by itself.)
const OPENOCD_RESUME_ON_DETACH: &str = "foreach t [target names] { $t configure -event gdb-detach { resume } }";

/// J-Link's command-line GDB server: `JLinkGDBServerCLExe` (`JLinkGDBServerCL` on
/// Windows); the bare `JLinkGDBServer` opens a window.
const JLINK: &str = if cfg!(windows) { "JLinkGDBServerCL" } else { "JLinkGDBServerCLExe" };

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|x| x.to_string()).collect()
}

fn preset(id: &str) -> Option<Server> {
    let make = |label: &str, command: &str, args: &[&str], init: &[&str], reset: &[&str], download: bool, hint: &str| Server {
        id: id.to_string(),
        label: label.into(),
        command: command.into(),
        args: s(args),
        post_args: vec![],
        enabled: true,
        builtin: true,
        env: vec![],
        init: s(init),
        reset: s(reset),
        download,
        ready_timeout: Duration::from_secs(30),
        install_hint: hint.into(),
    };
    Some(match id {
        "openocd" => {
            let mut o = make(
                "OpenOCD",
                "openocd",
                &["-c", "gdb_port {port}", "-c", "telnet_port {port2}", "-c", "tcl_port {port3}"],
                &[],
                &["monitor reset halt"],
                true,
                "Install OpenOCD (`sudo apt install openocd`, or your vendor's or xPack's build) and name the probe and the chip in `server_args`, e.g. [\"-f\", \"interface/stlink.cfg\", \"-f\", \"target/stm32f4x.cfg\"]. Set [debug.servers.openocd] command when it is not on PATH.",
            );
            o.post_args = s(&["-c", OPENOCD_RESUME_ON_DETACH]);
            o
        }
        "jlink" => make(
            "J-Link GDB Server",
            JLINK,
            &["-nogui", "-port", "{port}", "-swoport", "{port2}", "-telnetport", "{port3}"],
            &[],
            &["monitor reset", "monitor halt"],
            true,
            "Install SEGGER's J-Link software pack, put its folder (/opt/SEGGER/JLink) on PATH or set [debug.servers.jlink] command, and name the chip in `server_args`: [\"-device\", \"STM32F407VG\", \"-if\", \"SWD\", \"-speed\", \"4000\"].",
        ),
        "pyocd" => make(
            "pyOCD",
            "pyocd",
            &["gdbserver", "--port", "{port}", "--telnet-port", "{port2}"],
            &[],
            &["monitor reset halt"],
            true,
            "Install pyOCD (`pipx install pyocd`) and name the chip in `server_args`: [\"--target\", \"stm32f407vg\"].",
        ),
        "st-util" => make(
            "st-util (ST-LINK)",
            "st-util",
            &["-p", "{port}"],
            &[],
            &[],
            true,
            "Install stlink-tools (`sudo apt install stlink-tools`) or build github.com/stlink-org/stlink.",
        ),
        "qemu-arm" => make(
            "QEMU (Arm)",
            "qemu-system-arm",
            &["-S", "-gdb", "tcp:127.0.0.1:{port}", "-display", "none", "-monitor", "none", "-serial", "stdio"],
            &[],
            &[],
            false,
            "Install QEMU (`sudo apt install qemu-system-arm`) and name the machine and the image in `server_args`, e.g. [\"-M\", \"lm3s6965evb\", \"-kernel\", \"{program}\"]. The first serial port is shown in the debug console.",
        ),
        _ => return None,
    })
}

fn apply(a: &mut Server, c: &ServerConfig) {
    if let Some(l) = &c.label {
        a.label = l.clone();
    }
    if let Some(cmd) = &c.command {
        a.command = cmd.clone();
    }
    if let Some(args) = &c.args {
        a.args = args.clone();
    }
    if let Some(e) = c.enabled {
        a.enabled = e;
    }
    a.env.extend(c.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    if let Some(i) = &c.init {
        a.init = i.clone();
    }
    if let Some(r) = &c.reset {
        a.reset = r.clone();
    }
    if let Some(d) = c.download {
        a.download = d;
    }
    if let Some(t) = c.ready_timeout_s {
        a.ready_timeout = Duration::from_secs(t.clamp(1, 300));
    }
    if let Some(h) = &c.install_hint {
        a.install_hint = h.clone();
    }
}

/// Every debug server: presets (with their overrides), then custom ones from config.toml.
/// The second value lists config problems (a custom server without a command).
pub fn all(configured: &BTreeMap<String, ServerConfig>) -> (Vec<Server>, Vec<String>) {
    let mut out = vec![];
    let mut warnings = vec![];
    for id in PRESETS {
        let mut a = preset(id).expect("preset");
        if let Some(c) = configured.get(*id) {
            apply(&mut a, c);
        }
        out.push(a);
    }
    for (id, c) in configured {
        if PRESETS.contains(&id.as_str()) {
            continue;
        }
        if !super::adapters::valid_id(id) {
            warnings.push(format!("[debug.servers.{id}]: invalid id (letters, digits, - _ . only)"));
            continue;
        }
        let Some(command) = c.command.clone().filter(|c| !c.trim().is_empty()) else {
            warnings.push(format!("[debug.servers.{id}]: `command` is required for a custom server"));
            continue;
        };
        let mut a = Server {
            id: id.clone(),
            label: id.clone(),
            command,
            args: vec![],
            post_args: vec![],
            enabled: true,
            builtin: false,
            env: vec![],
            init: vec![],
            reset: vec![],
            download: true,
            ready_timeout: Duration::from_secs(30),
            install_hint: format!("Check `command` in [debug.servers.{id}] of config.toml."),
        };
        apply(&mut a, c);
        out.push(a);
    }
    (out, warnings)
}

pub fn find(configured: &BTreeMap<String, ServerConfig>, id: &str) -> Option<Server> {
    all(configured).0.into_iter().find(|s| s.id == id)
}

// ---------------------------------------------------------------- placeholders

/// The most ports a configuration can ask for: `{port}` and `{port2}` … `{port9}`.
pub const MAX_PORTS: usize = 9;

fn port_name(i: usize) -> String {
    if i == 0 { "{port}".to_string() } else { format!("{{port{}}}", i + 1) }
}

/// Free ports a server's arguments ask for: the highest of `{port}`, `{port2}`… `{port9}`.
pub fn ports_needed(args: &[String]) -> usize {
    (0..MAX_PORTS).rev().find(|i| args.iter().any(|a| a.contains(&port_name(*i)))).map(|i| i + 1).unwrap_or(0)
}

/// The 0-based index a `{port}`, `{port2}`… reference names (`port` is `0`, `{port4}` is 3).
pub fn port_index(reference: &str) -> Option<usize> {
    (0..MAX_PORTS).find(|i| port_name(*i) == reference.trim())
}

/// `args` with `{port}`, `{port2}`… replaced by `ports`.
pub fn substitute_ports(args: &[String], ports: &[u16]) -> Vec<String> {
    args.iter()
        .map(|a| {
            let mut a = a.clone();
            for (i, p) in ports.iter().enumerate() {
                a = a.replace(&port_name(i), &p.to_string());
            }
            a
        })
        .collect()
}

// ---------------------------------------------------------------- availability

/// Where the vendor's installer puts a server that is not on `PATH`.
fn well_known(command: &str) -> Vec<std::path::PathBuf> {
    if command.starts_with(JLINK) {
        let dirs: &[&str] = if cfg!(windows) { &[r"C:\Program Files\SEGGER\JLink", r"C:\Program Files (x86)\SEGGER\JLink"] } else { &["/opt/SEGGER/JLink"] };
        return dirs.iter().map(std::path::PathBuf::from).collect();
    }
    vec![]
}

/// The server's executable: `command` on `PATH`, else in the folder its vendor's
/// installer uses.
pub fn locate(command: &str) -> Option<std::path::PathBuf> {
    if let Some(p) = crate::util::os::exe::which(command) {
        return Some(p);
    }
    let dirs = well_known(command);
    crate::util::os::exe::find_in(&dirs, command)
}

fn probe_uncached(a: &Server) -> Availability {
    if !a.enabled {
        return Availability::missing(format!("disabled in config.toml ([debug.servers.{}] enabled = false)", a.id));
    }
    match locate(&a.command) {
        Some(p) => Availability { available: true, path: Some(p.display().to_string()), version: None, problem: None },
        None => Availability::missing(format!("`{}` was not found on PATH", a.command)),
    }
}

const PROBE_TTL: Duration = Duration::from_secs(30);

/// Whether `a` can start here (cached for half a minute, with the adapters' probes).
pub fn probe(state: &AppState, a: &Server) -> Availability {
    let key = format!("server\u{0}{}\u{0}{}\u{0}{}", a.id, a.command, a.enabled);
    if let Some((at, v)) = state.debug.probes.lock().get(&key) {
        if at.elapsed() < PROBE_TTL {
            return v.clone();
        }
    }
    let v = probe_uncached(a);
    state.debug.probes.lock().insert(key, (Instant::now(), v.clone()));
    v
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    #[serde(flatten)]
    pub server: Server,
    pub availability: Availability,
}

pub fn views(state: &AppState) -> (Vec<ServerView>, Vec<String>) {
    let cfg = state.config.read().debug.servers.clone();
    let (list, warnings) = all(&cfg);
    (list.into_iter().map(|server| ServerView { availability: probe(state, &server), server }).collect(), warnings)
}

/// What a debug server's own output says about a problem that will not fix itself.
#[derive(Debug, PartialEq)]
pub enum Finding {
    /// Worth telling the user; the session goes on.
    Hint(String),
    /// The session cannot go on.
    Fatal(String),
}

/// Reads a debug server's output lines for what its authors print but a person misses among
/// dozens of `Info:` lines: a chip it has no description of (st-util 1.8.0 on a newer STM32
/// says `unknown chip id! 0x44d`, connects anyway, and then hangs on the first memory read),
/// and a probe that stopped answering (`LIBUSB_ERROR_TIMEOUT` over and over). Without this
/// the session waits out the adapter's own timeout with the reason hidden in the console.
#[derive(Debug, Default)]
pub struct ServerWatch {
    said_unknown_chip: bool,
    timeouts: std::collections::VecDeque<std::time::Instant>,
}

/// USB timeouts within [`TIMEOUT_WINDOW`] that mean the probe is stuck, not slow.
const TIMEOUTS_FATAL: usize = 3;
const TIMEOUT_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

impl ServerWatch {
    pub fn line(&mut self, line: &str, now: std::time::Instant) -> Option<Finding> {
        let lower = line.to_ascii_lowercase();
        if lower.contains("unknown chip id") {
            if std::mem::replace(&mut self.said_unknown_chip, true) {
                return None;
            }
            let id = line.split_whitespace().last().filter(|w| w.starts_with("0x")).map(|w| format!(" {w}")).unwrap_or_default();
            return Some(Finding::Hint(format!(
                "The debug server does not know this chip (id{id}): its chip table has no entry for it. It may connect but then fail to read or flash memory. Use a newer version of the tool, or add a description of the chip to it."
            )));
        }
        if lower.contains("libusb_error_timeout") {
            self.timeouts.push_back(now);
            while self.timeouts.front().is_some_and(|t| now.duration_since(*t) > TIMEOUT_WINDOW) {
                self.timeouts.pop_front();
            }
            if self.timeouts.len() == TIMEOUTS_FATAL {
                return Some(Finding::Fatal(
                    "The debug server has stopped getting answers from the debug probe over USB (repeated LIBUSB_ERROR_TIMEOUT): the probe is stuck. End this session, unplug the board's USB cable and plug it back in, then try again.".into(),
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_server_that_cannot_talk_to_its_probe_or_knows_no_chip_is_noticed() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut w = ServerWatch::default();
        // The chip is named once, with its id; ordinary lines say nothing.
        assert_eq!(w.line("Info : STLINK V2J46M31 (API v2) VID:PID 0483:3752", t0), None);
        let hint = w.line("2026-10-04T12:14:37 WARN common.c: unknown chip id! 0x44d", t0);
        assert!(matches!(&hint, Some(Finding::Hint(m)) if m.contains("0x44d") && m.contains("does not know this chip")), "{hint:?}");
        assert_eq!(w.line("2026-10-04T12:14:38 WARN common.c: unknown chip id! 0x44d", t0), None, "said once");
        // Two timeouts are slow; the third within half a minute is a stuck probe, said once.
        let timeout = "2026-10-04T12:14:41 ERROR usb.c: READMEM_32BIT send request failed: LIBUSB_ERROR_TIMEOUT";
        assert_eq!(w.line(timeout, t0), None);
        assert_eq!(w.line(timeout, t0 + Duration::from_secs(3)), None);
        let fatal = w.line(timeout, t0 + Duration::from_secs(6));
        assert!(matches!(&fatal, Some(Finding::Fatal(m)) if m.contains("unplug")), "{fatal:?}");
        assert_eq!(w.line(timeout, t0 + Duration::from_secs(9)), None, "not again");
        // Timeouts spread over minutes are not a stuck probe.
        let mut w = ServerWatch::default();
        for i in 0..6 {
            assert_eq!(w.line(timeout, t0 + Duration::from_secs(40 * i)), None, "{i}");
        }
        // The text is matched whatever its case.
        let mut w = ServerWatch::default();
        assert!(matches!(w.line("Error: Unknown Chip ID", t0), Some(Finding::Hint(_))));
    }

    use super::*;

    fn cfg(text: &str) -> BTreeMap<String, ServerConfig> {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn presets_merge_with_config_and_custom_servers_need_a_command() {
        let c = cfg(r#"
            [openocd]
            command = "/opt/openocd/bin/openocd"
            reset = ["monitor reset init"]
            [renode]
            command = "renode"
            args = ["--port", "{port}"]
            download = false
            [broken]
            args = ["x"]
        "#);
        let (list, warnings) = all(&c);
        let ocd = list.iter().find(|s| s.id == "openocd").unwrap();
        assert_eq!(ocd.command, "/opt/openocd/bin/openocd");
        assert_eq!(ocd.reset, vec!["monitor reset init"]);
        assert!(ocd.args.contains(&"gdb_port {port}".to_string()), "preset arguments stay: {:?}", ocd.args);
        assert!(ocd.builtin && ocd.download);
        let renode = list.iter().find(|s| s.id == "renode").unwrap();
        assert!(!renode.builtin && !renode.download);
        assert!(!list.iter().any(|s| s.id == "broken"));
        assert!(warnings.iter().any(|w| w.contains("broken") && w.contains("command")), "{warnings:?}");
        assert_eq!(list.iter().map(|s| s.id.as_str()).take(PRESETS.len()).collect::<Vec<_>>(), PRESETS.to_vec(), "presets first, in order");
        // config.toml is rewritten by Settings: it must survive a round trip.
        let again = toml::to_string(&c).unwrap();
        assert_eq!(toml::from_str::<BTreeMap<String, ServerConfig>>(&again).unwrap(), c);
    }

    #[test]
    fn every_preset_gives_its_stub_a_port_of_its_own() {
        for id in PRESETS {
            let p = preset(id).unwrap();
            assert!(p.args.iter().any(|a| a.contains("{port}")), "{id} must listen on {{port}}: {:?}", p.args);
            // Auxiliary listeners (telnet, SWO) never use their fixed defaults, so two
            // sessions can run side by side.
            for a in &p.args {
                assert!(!a.contains("4444") && !a.contains("6666") && !a.contains("2331"), "{id}: {a}");
            }
        }
    }

    #[test]
    fn ports_are_counted_and_substituted() {
        let args = s(&["-c", "gdb_port {port}", "-c", "telnet_port {port2}", "--x={port3}", "{port}"]);
        assert_eq!(ports_needed(&args), 3);
        assert_eq!(ports_needed(&s(&["--rtt", "{port9}"])), 9);
        assert_eq!(port_index("{port}"), Some(0));
        assert_eq!(port_index(" {port4} "), Some(3));
        assert_eq!(port_index("{port10}"), None);
        assert_eq!(port_index("4444"), None);
        assert_eq!(ports_needed(&s(&["-p", "{port}"])), 1);
        assert_eq!(ports_needed(&s(&["-p", "1234"])), 0);
        assert_eq!(substitute_ports(&args, &[10, 20, 30]), s(&["-c", "gdb_port 10", "-c", "telnet_port 20", "--x=30", "10"]));
        // A server that needs fewer ports than were given leaves nothing behind.
        assert_eq!(substitute_ports(&s(&["-p", "{port}"]), &[7, 8, 9]), s(&["-p", "7"]));
    }

    #[test]
    fn availability_never_runs_the_server() {
        let mut a = preset("openocd").unwrap();
        a.command = "wb-no-such-debug-server".into();
        let p = probe_uncached(&a);
        assert!(!p.available && p.problem.unwrap().contains("not found"));
        a.command = "sh".into();
        a.enabled = false;
        assert!(probe_uncached(&a).problem.unwrap().contains("disabled"));
    }
}
