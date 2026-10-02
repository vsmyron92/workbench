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
        "openocd" => make(
            "OpenOCD",
            "openocd",
            &["-c", "gdb_port {port}", "-c", "telnet_port {port2}", "-c", "tcl_port {port3}"],
            &[],
            &["monitor reset halt"],
            true,
            "Install OpenOCD (`sudo apt install openocd`, or your vendor's or xPack's build) and name the probe and the chip in `server_args`, e.g. [\"-f\", \"interface/stlink.cfg\", \"-f\", \"target/stm32f4x.cfg\"]. Set [debug.servers.openocd] command when it is not on PATH.",
        ),
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

#[cfg(test)]
mod tests {
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
