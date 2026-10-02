//! Debug adapters: built-in presets merged with `[debug.adapters.<id>]` from
//! config.toml, and their availability on this computer.
//!
//! **Trust.** An adapter's command runs with the user's rights, so it comes only from
//! config.toml or a preset, never from repository config (`[[debug]]` entries name an
//! adapter by id). Availability probes run the adapter's own executable
//! (`gdb --version`, `python3 -c "import debugpy"`), never anything from a project.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::elf::Arch;
use crate::app::AppState;

/// `[debug]` in config.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DebugConfig {
    /// Overrides of the presets (`gdb`, `lldb-dap`, `codelldb`, `debugpy`, `delve`, the
    /// embedded GDBs) and custom adapters, by id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub adapters: BTreeMap<String, AdapterConfig>,
    /// The adapter to use per language when a launch configuration names none,
    /// e.g. `rust = "codelldb"`; `embedded` is the GDB for remote targets (otherwise
    /// the one that fits the program's architecture).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub default_adapter: BTreeMap<String, String>,
    /// Debug servers (OpenOCD, J-Link GDB Server…) that remote-target launch
    /// configurations start: overrides of the presets and custom ones, by id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub servers: BTreeMap<String, super::servers::ServerConfig>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AdapterKind {
    Gdb,
    Lldb,
    Codelldb,
    Debugpy,
    Delve,
    /// Any other DAP adapter: `program`, `args`, `cwd`, `env` (object), `stopOnEntry`.
    #[default]
    Generic,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// DAP on the adapter's stdin/stdout.
    #[default]
    Stdio,
    /// The adapter listens on a TCP port: `{port}` in `args` is replaced by a free
    /// loopback port Workbench then connects to.
    Tcp,
}

/// One `[debug.adapters.<id>]` table. Every field is optional for a preset id.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AdapterConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<AdapterKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub languages: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Plain environment for the adapter process (e.g. `PYTHONPATH`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// DAP `adapterID` sent in `initialize`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    /// Arguments merged into every launch/attach request of this adapter.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub launch_defaults: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
}

/// A resolved adapter (preset + overrides).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub id: String,
    pub kind: AdapterKind,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<String>,
    pub transport: Transport,
    pub enabled: bool,
    pub builtin: bool,
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    pub adapter_id: String,
    #[serde(skip)]
    pub launch_defaults: Map<String, Value>,
    #[serde(skip)]
    pub connect_timeout: Duration,
    pub install_hint: String,
}

impl Adapter {
    /// Whether this adapter can give the debuggee a Workbench terminal (DAP
    /// `runInTerminal`), and how to ask for it.
    pub fn supports_terminal(&self) -> bool {
        matches!(self.kind, AdapterKind::Lldb | AdapterKind::Codelldb | AdapterKind::Debugpy)
    }
}

const NATIVE: &[&str] = &["c", "cpp", "rust"];
const GDB_DAP: &[&str] = &["-q", "-i", "dap"];
/// Embedded GDBs list this language only: no ordinary C, C++ or Rust launch picks them
/// (`for_remote` does, by the program's architecture).
const EMBEDDED: &[&str] = &["embedded"];

fn preset(id: &str) -> Option<Adapter> {
    let base = |kind: AdapterKind, label: &str, command: &str, args: &[&str], langs: &[&str], transport: Transport, adapter_id: &str, hint: &str| Adapter {
        id: id.to_string(),
        kind,
        label: label.into(),
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        languages: langs.iter().map(|s| s.to_string()).collect(),
        transport,
        enabled: true,
        builtin: true,
        env: vec![],
        adapter_id: adapter_id.into(),
        launch_defaults: Map::new(),
        connect_timeout: Duration::from_secs(10),
        install_hint: hint.into(),
    };
    Some(match id {
        "gdb" => base(
            AdapterKind::Gdb,
            "GDB",
            "gdb",
            GDB_DAP,
            &["c", "cpp", "rust", "fortran", "ada", "d", "objc"],
            Transport::Stdio,
            "gdb",
            "GDB 14 or newer speaks DAP (gdb -i dap). Install it with your package manager, e.g. `sudo apt install gdb`.",
        ),
        "lldb-dap" => base(
            AdapterKind::Lldb,
            "LLDB (lldb-dap)",
            &lldb_dap_command(),
            &[],
            &["c", "cpp", "rust", "objc", "swift"],
            Transport::Stdio,
            "lldb-dap",
            "Install LLVM's lldb-dap (`sudo apt install lldb`; releases before LLVM 18 call it lldb-vscode), or set [debug.adapters.lldb-dap] command.",
        ),
        "codelldb" => base(
            AdapterKind::Codelldb,
            "CodeLLDB",
            "codelldb",
            &["--port", "{port}"],
            NATIVE,
            Transport::Tcp,
            "lldb",
            "Download CodeLLDB from github.com/vadimcn/codelldb/releases, unpack the .vsix (a zip) and set [debug.adapters.codelldb] command to its extension/adapter/codelldb.",
        ),
        // `python3`; on Windows `python`, or `py` (whose `-3` is `launcher_args`).
        "debugpy" => base(
            AdapterKind::Debugpy,
            "debugpy",
            &crate::util::os::exe::python()[0],
            &["-m", "debugpy.adapter"],
            &["python"],
            Transport::Stdio,
            "debugpy",
            "Install debugpy for the interpreter in `command` (`python3 -m pip install debugpy`), or point [debug.adapters.debugpy] env.PYTHONPATH at a folder where it is installed.",
        ),
        "delve" => base(
            AdapterKind::Delve,
            "Delve",
            "dlv",
            &["dap", "--listen", "127.0.0.1:{port}"],
            &["go"],
            Transport::Tcp,
            "go",
            "Install Delve: `go install github.com/go-delve/delve/cmd/dlv@latest` (it lands in ~/go/bin; put that on PATH or set [debug.adapters.delve] command).",
        ),
        "gdb-multiarch" => base(
            AdapterKind::Gdb,
            "GDB (multi-architecture)",
            "gdb-multiarch",
            GDB_DAP,
            EMBEDDED,
            Transport::Stdio,
            "gdb",
            "Install gdb-multiarch (`sudo apt install gdb-multiarch`): one GDB for Arm, RISC-V, Xtensa and more. GDB 14 or newer, built with Python, speaks DAP.",
        ),
        "arm-none-eabi-gdb" => base(
            AdapterKind::Gdb,
            "GDB for Arm (arm-none-eabi)",
            &first_on_path(&["arm-none-eabi-gdb-py", "arm-none-eabi-gdb"]),
            GDB_DAP,
            EMBEDDED,
            Transport::Stdio,
            "gdb",
            "Install the Arm GNU Toolchain (developer.arm.com, release 13.3 or newer has GDB 14) and put its bin folder on PATH, or set [debug.adapters.arm-none-eabi-gdb] command. gdb-multiarch works too.",
        ),
        "riscv-gdb" => base(
            AdapterKind::Gdb,
            "GDB for RISC-V",
            &first_on_path(&["riscv32-unknown-elf-gdb", "riscv64-unknown-elf-gdb", "riscv-none-elf-gdb", "riscv32-esp-elf-gdb", "riscv-none-embed-gdb"]),
            GDB_DAP,
            EMBEDDED,
            Transport::Stdio,
            "gdb",
            "Install a RISC-V GDB 14 or newer (xPack riscv-none-elf-gdb, Espressif's riscv32-esp-elf-gdb, your SDK's) and put it on PATH, or set [debug.adapters.riscv-gdb] command. gdb-multiarch works too.",
        ),
        "xtensa-gdb" => base(
            AdapterKind::Gdb,
            "GDB for Xtensa (ESP32)",
            &first_on_path(&["xtensa-esp32-elf-gdb", "xtensa-esp32s3-elf-gdb", "xtensa-esp-elf-gdb", "xtensa-esp32s2-elf-gdb", "xtensa-lx106-elf-gdb"]),
            GDB_DAP,
            EMBEDDED,
            Transport::Stdio,
            "gdb",
            "Install Espressif's GDB (ESP-IDF's install script puts xtensa-esp-elf-gdb under ~/.espressif/tools) and put it on PATH, or set [debug.adapters.xtensa-gdb] command.",
        ),
        _ => return None,
    })
}

pub const PRESETS: &[&str] = &["gdb", "lldb-dap", "codelldb", "debugpy", "delve", "gdb-multiarch", "arm-none-eabi-gdb", "riscv-gdb", "xtensa-gdb"];

/// The first of `names` that is on PATH, else the first name (so the error names it).
fn first_on_path(names: &[&str]) -> String {
    names.iter().find(|n| crate::util::which(n)).unwrap_or(&names[0]).to_string()
}

/// Arguments before the adapter's `args`: the Python launcher's `-3` while debugpy runs
/// the preset's `py` (Windows without `python`). Kept out of `args`, so a `command` of the
/// user's own (a venv's python.exe) never gets it. None on Unix.
pub fn launcher_args(a: &Adapter) -> Vec<String> {
    let python = crate::util::os::exe::python();
    if a.kind == AdapterKind::Debugpy && a.command == python[0] { python[1..].to_vec() } else { vec![] }
}

/// lldb-dap's executable: `lldb-dap`, the older `lldb-vscode`, or a versioned one
/// (`lldb-dap-19`), whichever is on PATH.
fn lldb_dap_command() -> String {
    for c in ["lldb-dap", "lldb-vscode"] {
        if crate::util::which(c) {
            return c.into();
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        let mut best: Option<(u32, String)> = None;
        for dir in std::env::split_paths(&path) {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten().take(5000) {
                let n = e.file_name().to_string_lossy().into_owned();
                // `lldb-dap-19.exe` on Windows is looked up as `lldb-dap-19`.
                let n = n.strip_suffix(std::env::consts::EXE_SUFFIX).unwrap_or(&n).to_string();
                for prefix in ["lldb-dap-", "lldb-vscode-"] {
                    if let Some(v) = n.strip_prefix(prefix).and_then(|v| v.parse::<u32>().ok()) {
                        if best.as_ref().is_none_or(|(b, _)| v > *b) {
                            best = Some((v, n.clone()));
                        }
                    }
                }
            }
        }
        if let Some((_, n)) = best {
            return n;
        }
    }
    "lldb-dap".into()
}

fn apply(a: &mut Adapter, c: &AdapterConfig) {
    if let Some(k) = c.kind {
        a.kind = k;
    }
    if let Some(l) = &c.label {
        a.label = l.clone();
    }
    if let Some(cmd) = &c.command {
        a.command = cmd.clone();
    }
    if let Some(args) = &c.args {
        a.args = args.clone();
    }
    if let Some(l) = &c.languages {
        a.languages = l.iter().map(|s| s.to_ascii_lowercase()).collect();
    }
    if let Some(t) = c.transport {
        a.transport = t;
    }
    if let Some(e) = c.enabled {
        a.enabled = e;
    }
    a.env.extend(c.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    if let Some(i) = &c.adapter_id {
        a.adapter_id = i.clone();
    }
    for (k, v) in &c.launch_defaults {
        a.launch_defaults.insert(k.clone(), v.clone());
    }
    if let Some(s) = c.connect_timeout_s {
        a.connect_timeout = Duration::from_secs(s.clamp(1, 120));
    }
    if let Some(h) = &c.install_hint {
        a.install_hint = h.clone();
    }
}

/// Valid adapter ids: what `[debug.adapters.<id>]` and `[[debug]] adapter` may use.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Every adapter: presets (with their overrides) in preference order, then custom
/// adapters. The second value lists config problems (a custom adapter without a
/// command).
pub fn all(cfg: &DebugConfig) -> (Vec<Adapter>, Vec<String>) {
    let mut out = vec![];
    let mut warnings = vec![];
    for id in PRESETS {
        let mut a = preset(id).expect("preset");
        if let Some(c) = cfg.adapters.get(*id) {
            apply(&mut a, c);
        }
        out.push(a);
    }
    for (id, c) in &cfg.adapters {
        if PRESETS.contains(&id.as_str()) {
            continue;
        }
        if !valid_id(id) {
            warnings.push(format!("[debug.adapters.{id}]: invalid id (letters, digits, - _ . only)"));
            continue;
        }
        let Some(command) = c.command.clone().filter(|c| !c.trim().is_empty()) else {
            warnings.push(format!("[debug.adapters.{id}]: `command` is required for a custom adapter"));
            continue;
        };
        let mut a = Adapter {
            id: id.clone(),
            kind: AdapterKind::Generic,
            label: id.clone(),
            command,
            args: vec![],
            languages: vec![],
            transport: Transport::Stdio,
            enabled: true,
            builtin: false,
            env: vec![],
            adapter_id: id.clone(),
            launch_defaults: Map::new(),
            connect_timeout: Duration::from_secs(10),
            install_hint: format!("Check `command` in [debug.adapters.{id}] of config.toml."),
        };
        apply(&mut a, c);
        out.push(a);
    }
    (out, warnings)
}

pub fn find(cfg: &DebugConfig, id: &str) -> Option<Adapter> {
    all(cfg).0.into_iter().find(|a| a.id == id)
}

// ---------------------------------------------------------------- availability

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl Availability {
    pub(crate) fn missing(problem: String) -> Self {
        Self { available: false, path: None, version: None, problem: Some(problem) }
    }
}

/// `(major, minor)` of `GNU gdb (Ubuntu 17.1-2ubuntu1) 17.1`.
pub fn gdb_version(first_line: &str) -> Option<(u32, u32)> {
    let last = first_line.split_whitespace().last()?;
    let mut it = last.split(['.', '-']);
    let major = it.next()?.parse().ok()?;
    let minor = it.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

const PROBE_TTL: Duration = Duration::from_secs(30);

async fn probe_uncached(a: &Adapter) -> Availability {
    if !a.enabled {
        return Availability::missing(format!("disabled in config.toml ([debug.adapters.{}] enabled = false)", a.id));
    }
    // An npm shim (a Node.js adapter on Windows) is probed as node and its script.
    let Some(resolved) = crate::util::os::exe::resolve(&a.command) else {
        return Availability::missing(format!("`{}` was not found on PATH", a.command));
    };
    let path_s = resolved.program.display().to_string();
    let lead = launcher_args(a);
    let run = |args: Vec<&'static str>| {
        let mut cmd = crate::util::os::exe::command(&resolved);
        cmd.args(&lead).args(args).current_dir("/");
        for (k, v) in &a.env {
            cmd.env(k, v);
        }
        crate::util::proc::run_cmd(cmd, Duration::from_secs(8))
    };
    match a.kind {
        AdapterKind::Gdb => match run(vec!["--version"]).await {
            Ok(out) => {
                let line = out.stdout.lines().next().unwrap_or("").trim().to_string();
                match gdb_version(&line) {
                    Some((major, _)) if major < 14 => Availability {
                        available: false,
                        path: Some(path_s),
                        version: Some(line),
                        problem: Some("this GDB has no DAP support: GDB 14 or newer is needed".into()),
                    },
                    // DAP is a Python module of gdb: one built without Python (some vendor
                    // toolchains, `gdb-minimal`) fails `-i dap` with an error nobody reads.
                    _ => match run(vec!["-nx", "-batch", "-ex", "python print(6 * 7)"]).await {
                        Ok(o) if o.stdout.trim() == "42" => Availability { available: true, path: Some(path_s), version: Some(line), problem: None },
                        Ok(_) => Availability {
                            available: false,
                            path: Some(path_s),
                            version: Some(line),
                            problem: Some("this GDB was built without Python, which its DAP server needs".into()),
                        },
                        Err(e) => Availability::missing(format!("`{} -batch` failed: {}", a.command, e.message)),
                    },
                }
            }
            Err(e) => Availability::missing(format!("`{} --version` failed: {}", a.command, e.message)),
        },
        AdapterKind::Debugpy if a.args.first().map(String::as_str) == Some("-m") => {
            match run(vec!["-c", "import debugpy; print(debugpy.__version__)"]).await {
                Ok(out) if out.ok() => Availability {
                    available: true,
                    path: Some(path_s),
                    version: out.stdout.lines().next().map(|v| format!("debugpy {}", v.trim())),
                    problem: None,
                },
                Ok(_) => Availability {
                    available: false,
                    path: Some(path_s),
                    version: None,
                    problem: Some(format!("debugpy is not importable by {}", a.command)),
                },
                Err(e) => Availability::missing(format!("`{}` failed: {}", a.command, e.message)),
            }
        }
        _ => Availability { available: true, path: Some(path_s), version: None, problem: None },
    }
}

fn probe_key(a: &Adapter) -> String {
    format!("{}\u{0}{}\u{0}{:?}\u{0}{}", a.id, a.command, a.kind, a.enabled)
}

/// Tests: say what probing `a` found without running its executable (a fake adapter
/// that must be `kind = "gdb"` is not a GDB `--version` can talk to).
#[cfg(test)]
pub fn seed_probe(state: &AppState, a: &Adapter, found: Availability) {
    state.debug.probes.lock().insert(probe_key(a), (Instant::now(), found));
}

/// Whether `a` can run here (cached for half a minute: config edits apply soon).
pub async fn probe(state: &AppState, a: &Adapter) -> Availability {
    let key = probe_key(a);
    if let Some((at, v)) = state.debug.probes.lock().get(&key) {
        if at.elapsed() < PROBE_TTL {
            return v.clone();
        }
    }
    let v = probe_uncached(a).await;
    state.debug.probes.lock().insert(key, (Instant::now(), v.clone()));
    v
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterView {
    #[serde(flatten)]
    pub adapter: Adapter,
    pub availability: Availability,
}

pub async fn views(state: &AppState) -> (Vec<AdapterView>, Vec<String>) {
    let cfg = state.config.read().debug.clone();
    let (list, warnings) = all(&cfg);
    let avail = futures::future::join_all(list.iter().map(|a| probe(state, a))).await;
    (list.into_iter().zip(avail).map(|(adapter, availability)| AdapterView { adapter, availability }).collect(), warnings)
}

/// The adapter for `language`: `[debug] default_adapter.<language>`, else the first
/// available adapter that lists the language, else the first that lists it (so the
/// error names what to install). `None` when no adapter knows the language.
/// `gdb: false` passes over gdb unless it is the default or the only one that lists
/// the language (an attach where gdb cannot attach: the caller refuses it).
pub async fn for_language(state: &AppState, language: &str, gdb: bool) -> Option<Adapter> {
    let cfg = state.config.read().debug.clone();
    let language = language.to_ascii_lowercase();
    if let Some(id) = cfg.default_adapter.get(&language) {
        if let Some(a) = find(&cfg, id) {
            return Some(a);
        }
    }
    let (list, _) = all(&cfg);
    let candidates: Vec<Adapter> = list.into_iter().filter(|a| a.enabled && a.languages.iter().any(|l| *l == language)).collect();
    let usable = |a: &&Adapter| gdb || a.kind != AdapterKind::Gdb;
    for a in candidates.iter().filter(usable) {
        if probe(state, a).await.available {
            return Some(a.clone());
        }
    }
    candidates.iter().find(usable).or(candidates.first()).cloned()
}

/// The adapter ids that can debug a program built for `arch`, best first. The GDB of
/// the program's own toolchain, then the multi-architecture one; the native `gdb` only
/// where it is the same architecture (or nothing is known).
pub fn gdb_candidates(arch: Option<Arch>) -> Vec<&'static str> {
    let host = Arch::host();
    match arch {
        Some(a) if a == host => vec!["gdb", "gdb-multiarch"],
        Some(Arch::Arm) => vec!["arm-none-eabi-gdb", "gdb-multiarch"],
        Some(Arch::Riscv) => vec!["riscv-gdb", "gdb-multiarch"],
        Some(Arch::Xtensa) => vec!["xtensa-gdb", "gdb-multiarch"],
        Some(Arch::Aarch64 | Arch::X86 | Arch::X86_64 | Arch::Avr | Arch::Msp430) => vec!["gdb-multiarch", "gdb"],
        // Not built yet (a pre-launch step makes it) or not an ELF file: any GDB that works.
        Some(Arch::Other) | None => vec!["gdb-multiarch", "arm-none-eabi-gdb", "riscv-gdb", "xtensa-gdb", "gdb"],
    }
}

/// The GDB for a remote target built for `arch`: `[debug] default_adapter.embedded`, else
/// the first of `gdb_candidates` that is available, else the first (so the error names
/// what to install).
pub async fn for_remote(state: &AppState, arch: Option<Arch>) -> Adapter {
    let cfg = state.config.read().debug.clone();
    if let Some(a) = cfg.default_adapter.get("embedded").and_then(|id| find(&cfg, id)) {
        return a;
    }
    let mut first = None;
    for id in gdb_candidates(arch) {
        let Some(a) = find(&cfg, id).filter(|a| a.enabled) else { continue };
        if probe(state, &a).await.available {
            return a;
        }
        first.get_or_insert(a);
    }
    first.unwrap_or_else(|| find(&cfg, "gdb-multiarch").expect("preset"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_targets_get_the_gdb_that_fits_their_architecture() {
        let first = |a: Option<Arch>| gdb_candidates(a)[0];
        assert_eq!(first(Some(Arch::Arm)), if Arch::host() == Arch::Arm { "gdb" } else { "arm-none-eabi-gdb" });
        assert_eq!(first(Some(Arch::Riscv)), if Arch::host() == Arch::Riscv { "gdb" } else { "riscv-gdb" });
        assert_eq!(first(Some(Arch::Xtensa)), "xtensa-gdb");
        assert_eq!(first(Some(Arch::host())), "gdb", "the native GDB for the computer's own architecture");
        assert_eq!(first(None), "gdb-multiarch", "an unbuilt program: the one that debugs everything");
        // The multi-architecture GDB is always the fallback; a vendor GDB never debugs another's chip.
        for a in [Arch::Arm, Arch::Riscv, Arch::Xtensa] {
            assert!(gdb_candidates(Some(a)).contains(&"gdb-multiarch"));
        }
        assert!(!gdb_candidates(Some(Arch::Riscv)).contains(&"arm-none-eabi-gdb"));
        // Every candidate is a preset, and none of them is picked for ordinary languages.
        for id in gdb_candidates(None) {
            let a = find(&DebugConfig::default(), id).unwrap();
            assert_eq!(a.kind, AdapterKind::Gdb, "{id}");
            if id != "gdb" {
                assert_eq!(a.languages, vec!["embedded"], "{id} is for remote targets only");
            }
        }
    }

    #[test]
    fn presets_merge_with_config_and_custom_adapters_need_a_command() {
        let cfg: DebugConfig = toml::from_str(
            r#"
            [adapters.gdb]
            command = "/opt/gdb/bin/gdb"
            launch_defaults = { stopAtBeginningOfMainSubprogram = false }
            [adapters.codelldb]
            command = "~/tools/codelldb/adapter/codelldb"
            [adapters.jsdebug]
            command = "node"
            args = ["/opt/js-debug/src/dapDebugServer.js", "{port}"]
            transport = "tcp"
            languages = ["JavaScript", "typescript"]
            [adapters.broken]
            args = ["x"]
            [default_adapter]
            rust = "codelldb"
            "#,
        )
        .unwrap();
        let (list, warnings) = all(&cfg);
        let gdb = list.iter().find(|a| a.id == "gdb").unwrap();
        assert_eq!(gdb.command, "/opt/gdb/bin/gdb");
        assert_eq!(gdb.args, vec!["-q", "-i", "dap"], "preset args stay");
        assert_eq!(gdb.launch_defaults["stopAtBeginningOfMainSubprogram"], false);
        let js = list.iter().find(|a| a.id == "jsdebug").unwrap();
        assert_eq!((js.kind, js.transport, js.builtin), (AdapterKind::Generic, Transport::Tcp, false));
        assert_eq!(js.languages, vec!["javascript", "typescript"]);
        assert!(!list.iter().any(|a| a.id == "broken"));
        assert!(warnings.iter().any(|w| w.contains("broken") && w.contains("command")), "{warnings:?}");
        // Order: presets first (preference order), custom after.
        assert_eq!(list.iter().map(|a| a.id.as_str()).take(PRESETS.len()).collect::<Vec<_>>(), PRESETS.to_vec());
        // Round-trips through TOML (config.toml is rewritten by Settings).
        let text = toml::to_string(&cfg).unwrap();
        assert_eq!(toml::from_str::<DebugConfig>(&text).unwrap(), cfg);
    }

    #[test]
    fn launcher_arguments_only_for_the_preset_python() {
        // `py -3` on Windows without `python`: the `-3` belongs to the launcher, not to a
        // venv interpreter set as `command`.
        let preset = find(&DebugConfig::default(), "debugpy").unwrap();
        assert_eq!(preset.args, vec!["-m", "debugpy.adapter"]);
        assert_eq!(launcher_args(&preset), crate::util::os::exe::python()[1..]);
        let cfg: DebugConfig = toml::from_str("[adapters.debugpy]\ncommand = \"/work/.venv/bin/python\"\n").unwrap();
        let own = find(&cfg, "debugpy").unwrap();
        assert_eq!(own.args, vec!["-m", "debugpy.adapter"]);
        assert!(launcher_args(&own).is_empty());
        assert!(launcher_args(&find(&cfg, "gdb").unwrap()).is_empty());
    }

    #[test]
    fn gdb_versions() {
        assert_eq!(gdb_version("GNU gdb (Ubuntu 17.1-2ubuntu1) 17.1"), Some((17, 1)));
        assert_eq!(gdb_version("GNU gdb (GDB) 13.2"), Some((13, 2)));
        assert_eq!(gdb_version("GNU gdb (GDB) Fedora Linux 14.2-1.fc40"), Some((14, 2)));
        assert_eq!(gdb_version("nonsense"), None);
    }
}
