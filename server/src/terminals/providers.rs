//! Agent providers: Claude Code, OpenAI Codex CLI, Kimi Code CLI, Google Gemini CLI,
//! Aider and custom CLIs.
//!
//! A provider is a configured agent CLI (`[agents.providers.<name>]` in config.toml, plus
//! the built-in `claude`, `codex`, `kimi`, `gemini` and `aider` presets). Its *kind*
//! decides how Workbench drives it:
//!
//! | kind   | new / resume / fork                          | state from                         | MCP            |
//! |--------|----------------------------------------------|------------------------------------|----------------|
//! | claude | `--session-id` / `--resume` / `--fork-session` | HTTP hooks, status line, transcript | `--mcp-config` |
//! | codex  | `codex` / `codex resume <id>` / `codex fork <id>` | its rollout JSONL (`task_started`…) | `-c mcp_servers.workbench.*` |
//! | kimi   | `kimi` / `kimi --session <id>` / —            | output activity                    | —              |
//! | gemini | `--session-id <uuid>` / `--resume <uuid>` / — | output activity                    | —              |
//! | aider  | `aider` / `--restore-chat-history` / —        | output activity                    | —              |
//! | custom | the command / — / —                           | output activity                    | —              |
//!
//! Every flag used here was checked against `--help` of codex-cli 0.157.1, Kimi Code
//! 2.1.1 (Kimi's `@moonshot-ai/kimi-code`, which replaced the Python `kimi-cli`: kimi-cli
//! 1.52.0 only prints that it is no longer maintained), Gemini CLI 0.61.0
//! (`@google/gemini-cli`) and aider 0.86.2 (`aider-chat`), or against their published
//! source; anything else is marked UNVERIFIED. None of them was run with an account.
//!
//! No MCP for Kimi, Gemini and Aider: Kimi Code reads MCP servers only from its three
//! `mcp.json` files (its home, the git root's `.mcp.json`, `<cwd>/.kimi-code/`), Gemini
//! only from settings files (a system settings file must be owned by root) and
//! extensions, Aider has none. Writing Workbench into the user's or the repository's
//! files is not done.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::global::{AgentsConfig, ProviderConfig};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Claude,
    Codex,
    Kimi,
    Gemini,
    Aider,
    Custom,
}

/// The built-in presets, in the order the UI lists them.
pub const PRESETS: &[ProviderKind] = &[ProviderKind::Claude, ProviderKind::Codex, ProviderKind::Kimi, ProviderKind::Gemini, ProviderKind::Aider];

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "kimi" => Some(Self::Kimi),
            "gemini" => Some(Self::Gemini),
            "aider" => Some(Self::Aider),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Gemini => "gemini",
            Self::Aider => "aider",
            Self::Custom => "custom",
        }
    }

    fn default_label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Kimi => "Kimi Code",
            Self::Gemini => "Gemini CLI",
            Self::Aider => "Aider",
            Self::Custom => "Custom",
        }
    }

    fn default_command(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Gemini => "gemini",
            Self::Aider => "aider",
            Self::Custom => "",
        }
    }

    fn default_install_hint(self) -> &'static str {
        match self {
            Self::Claude => "npm install -g @anthropic-ai/claude-code",
            Self::Codex => "npm install -g @openai/codex",
            Self::Kimi => "npm install -g @moonshot-ai/kimi-code",
            Self::Gemini => "npm install -g @google/gemini-cli",
            // aider.chat's recommended installer.
            Self::Aider => "python -m pip install aider-install && aider-install",
            Self::Custom => "",
        }
    }

    /// Whether the kind resumes a conversation by id.
    pub fn resumes(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Kimi | Self::Gemini)
    }

    /// Whether the session id is chosen by Workbench at launch (`--session-id`), not
    /// discovered afterwards.
    pub fn id_at_launch(self) -> bool {
        matches!(self, Self::Claude | Self::Gemini)
    }

    /// Whether the kind takes extra workspace directories (`--add-dir`,
    /// `--include-directories`), and with them the Workspace deliverable folders.
    pub fn takes_add_dirs(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Kimi | Self::Gemini)
    }

    /// Whether the kind forks a conversation into a new one when launching.
    pub fn forks(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    /// Whether an initial prompt goes on the command line (otherwise it is pasted once
    /// the program is up).
    pub fn prompt_in_argv(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    /// Where the session state comes from.
    pub fn state_source(self) -> &'static str {
        match self {
            Self::Claude => "hooks",
            Self::Codex => "rollout",
            Self::Kimi | Self::Gemini | Self::Aider | Self::Custom => "activity",
        }
    }

    pub fn efforts(self) -> &'static [&'static str] {
        match self {
            Self::Claude => super::agent::EFFORTS,
            // codex-rs/protocol openai_models.rs `ReasoningEffort` (wire values); what a
            // model accepts varies, Codex reports unsupported values itself.
            Self::Codex => &["minimal", "low", "medium", "high", "xhigh", "max"],
            Self::Kimi | Self::Gemini | Self::Aider | Self::Custom => &[],
        }
    }

    pub fn permission_modes(self) -> &'static [PermissionPreset] {
        match self {
            Self::Claude => CLAUDE_MODES,
            Self::Codex => CODEX_MODES,
            Self::Kimi => KIMI_MODES,
            Self::Gemini => GEMINI_MODES,
            Self::Aider => AIDER_MODES,
            Self::Custom => &[],
        }
    }

    /// How long to wait after a paste before pressing Enter: TUIs (and Codex's paste
    /// burst guard) swallow an Enter that arrives with the paste.
    pub fn submit_delay_ms(self) -> u64 {
        match self {
            // Mr. Mak found 500 ms necessary for Codex after a resume.
            Self::Codex | Self::Kimi | Self::Gemini => 500,
            Self::Claude | Self::Aider | Self::Custom => 300,
        }
    }
}

/// One permission choice of a provider and what it means on the command line.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Skips approvals (and for Codex the sandbox): only on an explicit choice, never a default.
    pub dangerous: bool,
    #[serde(skip)]
    pub args: &'static [&'static str],
}

const fn preset(id: &'static str, label: &'static str, description: &'static str, dangerous: bool, args: &'static [&'static str]) -> PermissionPreset {
    PermissionPreset { id, label, description, dangerous, args }
}

/// Claude Code `--permission-mode` values (the argument is the id itself).
const CLAUDE_MODES: &[PermissionPreset] = &[
    preset("manual", "manual", "Ask before edits and commands", false, &[]),
    preset("acceptEdits", "acceptEdits", "Edit files without asking", false, &[]),
    preset("plan", "plan", "Read-only planning", false, &[]),
    preset("auto", "auto", "Claude decides what needs approval", false, &[]),
    preset("dontAsk", "dontAsk", "Deny whatever is not allowed already", false, &[]),
    preset("bypassPermissions", "bypassPermissions", "Never ask (dangerous)", true, &[]),
];

/// `codex --help` 0.157.1: `-s, --sandbox <read-only|workspace-write|danger-full-access>`,
/// `-a, --ask-for-approval <on-request|never>`, `--dangerously-bypass-approvals-and-sandbox`.
const CODEX_MODES: &[PermissionPreset] = &[
    preset("read-only", "Read only", "Sandbox: read-only; asks for approval", false, &["--sandbox", "read-only", "--ask-for-approval", "on-request"]),
    preset(
        "workspace-write",
        "Workspace write",
        "Sandbox: the workspace is writable; asks for approval",
        false,
        &["--sandbox", "workspace-write", "--ask-for-approval", "on-request"],
    ),
    preset(
        "never-ask",
        "Never ask (sandboxed)",
        "Sandbox: workspace-write; never asks, failures go back to the model",
        false,
        &["--sandbox", "workspace-write", "--ask-for-approval", "never"],
    ),
    preset(
        "bypass",
        "Bypass approvals and sandbox",
        "No sandbox, no approvals (dangerous)",
        true,
        &["--dangerously-bypass-approvals-and-sandbox"],
    ),
];

/// `kimi --help` (Kimi Code 2.1.1): `--plan`, `-y/--yolo` (Ask When Needed), `--auto`
/// (Never Ask). `--yolo` and `--auto` are mutually exclusive.
const KIMI_MODES: &[PermissionPreset] = &[
    preset("plan", "Plan mode", "Read-only exploration and a plan first", false, &["--plan"]),
    preset("yolo", "Ask when needed (yolo)", "Routine edits and commands run without asking (dangerous)", true, &["--yolo"]),
    preset("auto", "Never ask (auto)", "Everything runs and is decided automatically (dangerous)", true, &["--auto"]),
];

/// `gemini --help` (0.61.0): `--approval-mode default|auto_edit|yolo|plan`. Without a
/// preset Gemini's own default (it asks) applies.
const GEMINI_MODES: &[PermissionPreset] = &[
    preset("auto_edit", "Auto-approve edits", "File edits run without asking; commands still ask", false, &["--approval-mode", "auto_edit"]),
    preset("plan", "Plan mode", "Read-only", false, &["--approval-mode", "plan"]),
    preset("yolo", "YOLO", "Every tool runs without asking (dangerous)", true, &["--approval-mode", "yolo"]),
];

/// `aider --help` (0.86.2): `--yes-always` answers yes to every confirmation (shell
/// commands included).
const AIDER_MODES: &[PermissionPreset] =
    &[preset("yes-always", "Yes to everything", "Every confirmation is answered yes, shell commands too (dangerous)", true, &["--yes-always"])];

/// A provider as configured, with defaults applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Provider {
    /// The config name (`claude`, `codex`, `kimi`, `aider`…).
    pub id: String,
    pub kind: ProviderKind,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub env: BTreeMap<String, String>,
    pub install_hint: String,
}

impl Provider {
    pub fn permission_preset(&self, id: &str) -> Option<&'static PermissionPreset> {
        self.kind.permission_modes().iter().find(|p| p.id == id)
    }

    /// Whether a session started with `permission_mode` runs its commands inside the
    /// CLI's own sandbox: Codex, unless the `bypass` preset or its `args`
    /// (`--dangerously-bypass-approvals-and-sandbox`, its `--yolo` alias,
    /// `danger-full-access`) take it out. Without a preset Codex's own configuration
    /// decides, which is a sandbox unless the user changed it. Workbench must not run
    /// commands for such a session outside that sandbox (MCP `run_start`).
    pub fn sandboxed(&self, permission_mode: Option<&str>) -> bool {
        if self.kind != ProviderKind::Codex || permission_mode.and_then(|m| self.permission_preset(m)).is_some_and(|p| p.dangerous) {
            return false;
        }
        !self.args.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox" || a == "--yolo" || a.contains("danger-full-access"))
    }
}

/// Valid custom provider names: short, lowercase, usable in URLs and ids.
pub fn valid_provider_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn build(id: &str, kind: ProviderKind, c: Option<&ProviderConfig>, agents: &AgentsConfig) -> Provider {
    let d = ProviderConfig::default();
    let c = c.unwrap_or(&d);
    let (command, model, effort, permission_mode) = if kind == ProviderKind::Claude && id == "claude" {
        // `[agents]` keeps the Claude Code defaults.
        (
            non_empty(&c.command).unwrap_or_else(|| agents.command.clone()),
            non_empty(&c.model).or_else(|| non_empty(&agents.model)),
            non_empty(&c.effort).or_else(|| non_empty(&agents.effort)),
            non_empty(&c.permission_mode).or_else(|| non_empty(&agents.permission_mode)),
        )
    } else {
        (
            non_empty(&c.command).unwrap_or_else(|| kind.default_command().to_string()),
            non_empty(&c.model),
            non_empty(&c.effort),
            non_empty(&c.permission_mode),
        )
    };
    Provider {
        id: id.to_string(),
        kind,
        label: non_empty(&c.label).unwrap_or_else(|| {
            if kind == ProviderKind::Custom || id != kind.as_str() {
                id.to_string()
            } else {
                kind.default_label().to_string()
            }
        }),
        command,
        args: c.args.clone(),
        enabled: c.enabled.unwrap_or(true),
        model,
        effort,
        permission_mode,
        env: c.env.clone(),
        install_hint: non_empty(&c.install_hint).unwrap_or_else(|| kind.default_install_hint().to_string()),
    }
}

/// A model name that is safe on a command line.
pub fn valid_model(m: &str) -> bool {
    !m.is_empty() && m.len() <= 100 && m.chars().all(|c| c.is_ascii_alphanumeric() || "._:-[]/".contains(c))
}

/// Drop configured defaults the provider's kind cannot take, each with a warning. A
/// default that stayed would fail every start (`resolve_launch` refuses it) while the
/// provider looked available.
fn check_defaults(p: &mut Provider, c: Option<&ProviderConfig>, warnings: &mut Vec<String>) {
    let k = p.kind;
    let id = p.id.clone();
    // Claude's defaults may come from `[agents]` instead of its provider section.
    let section = |set: bool| if k == ProviderKind::Claude && id == "claude" && !set { "agents".to_string() } else { format!("agents.providers.{id}") };
    let own = |f: fn(&ProviderConfig) -> &Option<String>| c.is_some_and(|c| non_empty(f(c)).is_some());
    if let Some(m) = p.model.clone() {
        let why = if k == ProviderKind::Custom {
            Some(format!("{} takes no model option; put it in `args`", p.label))
        } else if !valid_model(&m) {
            Some("not a valid model name".to_string())
        } else {
            None
        };
        if let Some(why) = why {
            warnings.push(format!("{}: model {m:?} ignored ({why})", section(own(|c| &c.model))));
            p.model = None;
        }
    }
    if let Some(e) = p.effort.clone() {
        if !k.efforts().contains(&e.as_str()) {
            let why = if k.efforts().is_empty() { format!("{} has no effort setting", p.label) } else { format!("one of {}", k.efforts().join(", ")) };
            warnings.push(format!("{}: effort {e:?} ignored ({why})", section(own(|c| &c.effort))));
            p.effort = None;
        }
    }
    if let Some(m) = p.permission_mode.clone() {
        let why = match p.permission_preset(&m) {
            None if k.permission_modes().is_empty() => Some(format!("{} has no permission modes", p.label)),
            None => Some(format!("one of {}", k.permission_modes().iter().map(|x| x.id).collect::<Vec<_>>().join(", "))),
            // Other kinds never start in a dangerous mode by default: only a request selects it.
            Some(x) if x.dangerous && k != ProviderKind::Claude => Some("a dangerous mode is never a default; choose it when starting a session".to_string()),
            Some(_) => None,
        };
        if let Some(why) = why {
            warnings.push(format!("{}: permission_mode {m:?} ignored ({why})", section(own(|c| &c.permission_mode))));
            p.permission_mode = None;
        }
    }
}

/// Every provider (enabled or not): the three presets first, then custom ones by name.
/// Entries that cannot work are left out, and defaults their kind cannot take are
/// dropped, each with a warning; so is a `default_provider` that names no enabled one.
pub fn list(agents: &AgentsConfig) -> (Vec<Provider>, Vec<String>) {
    let (out, mut warnings) = list_unchecked_default(agents);
    if let Some(d) = agents.default_provider.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match out.iter().find(|p| p.id == d) {
            None => warnings.push(format!("agents.default_provider: {d:?} is not a configured provider; asking an agent fails until it is")),
            Some(p) if !p.enabled => warnings.push(format!("agents.default_provider: {d:?} is disabled; asking an agent fails until it is enabled")),
            Some(_) => {}
        }
    }
    (out, warnings)
}

fn list_unchecked_default(agents: &AgentsConfig) -> (Vec<Provider>, Vec<String>) {
    let mut out = vec![];
    let mut warnings = vec![];
    for &kind in PRESETS {
        let id = kind.as_str();
        let c = agents.providers.get(id);
        if let Some(k) = c.and_then(|c| c.kind.as_deref()).filter(|k| *k != id) {
            warnings.push(format!("agents.providers.{id}: kind {k:?} ignored; {id} is always the {id} preset"));
        }
        let mut p = build(id, kind, c, agents);
        check_defaults(&mut p, c, &mut warnings);
        out.push(p);
    }
    for (id, c) in &agents.providers {
        if PRESETS.iter().any(|k| k.as_str() == id) {
            continue;
        }
        if !valid_provider_id(id) {
            warnings.push(format!("agents.providers.{id}: names are lowercase letters, digits, '-' and '_' (at most 32)"));
            continue;
        }
        let kind = match c.kind.as_deref() {
            None => ProviderKind::Custom,
            Some(k) => match ProviderKind::parse(k) {
                Some(k) => k,
                None => {
                    warnings.push(format!("agents.providers.{id}: unknown kind {k:?} (claude, codex, kimi, gemini, aider or custom)"));
                    continue;
                }
            },
        };
        if kind == ProviderKind::Custom && non_empty(&c.command).is_none() {
            warnings.push(format!("agents.providers.{id}: a custom provider needs `command`"));
            continue;
        }
        let mut p = build(id, kind, Some(c), agents);
        check_defaults(&mut p, Some(c), &mut warnings);
        out.push(p);
    }
    (out, warnings)
}

/// The provider named `id` (`None`: the default provider).
pub fn find(agents: &AgentsConfig, id: Option<&str>) -> Option<Provider> {
    let id = id.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| default_id(agents));
    list_unchecked_default(agents).0.into_iter().find(|p| p.id == id)
}

pub fn default_id(agents: &AgentsConfig) -> String {
    agents.default_provider.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("claude").to_string()
}

// ---------------------------------------------------------------- command lines

/// What a launch continues.
#[derive(Debug, Clone, PartialEq)]
pub enum Continue {
    New,
    Resume(String),
    Fork(String),
}

/// Optional Codex flags, found in the installed version's `--help` (`codex_features`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CodexFeatures {
    /// `--no-daemon`: run in this process instead of a shared background app-server,
    /// which would not see our environment (its README: "Shared clients use the
    /// environment inherited when the daemon started").
    pub no_daemon: bool,
    /// `--no-alt-screen`: inline mode, so scrollback holds the conversation.
    pub no_alt_screen: bool,
}

impl CodexFeatures {
    pub fn from_help(help: &str) -> Self {
        Self { no_daemon: help.contains("--no-daemon"), no_alt_screen: help.contains("--no-alt-screen") }
    }
}

/// A TOML basic string for `-c key=value` (Codex parses the value as TOML).
fn toml_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Everything a non-Claude command line needs.
#[derive(Debug, Clone, Default)]
pub struct LaunchArgs<'a> {
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub permission: Option<&'a PermissionPreset>,
    pub add_dirs: &'a [String],
    pub extra_args: &'a [String],
    /// Workbench's MCP endpoint and this terminal's id (the token travels in
    /// `WORKBENCH_AGENT_TOKEN`, never in argv).
    pub mcp: Option<(&'a str, &'a str)>,
    pub prompt: Option<&'a str>,
}

/// The Codex command line. Verified against `codex --help`, `codex resume --help` and
/// `codex fork --help` (0.157.1); the `-c mcp_servers.workbench.*` overrides were checked
/// with `codex mcp list --json`. UNVERIFIED (not run with an account): that `--` before
/// the prompt is accepted (clap's standard end-of-options marker).
pub fn codex_argv(command: &str, cont: &Continue, f: CodexFeatures, a: &LaunchArgs) -> Vec<String> {
    let mut v: Vec<String> = vec![command.to_string()];
    match cont {
        Continue::New => {}
        Continue::Resume(_) => v.push("resume".into()),
        Continue::Fork(_) => v.push("fork".into()),
    }
    if f.no_daemon {
        v.push("--no-daemon".into());
    }
    if f.no_alt_screen {
        v.push("--no-alt-screen".into());
    }
    if let Some((url, terminal)) = a.mcp {
        v.push("-c".into());
        v.push(format!("mcp_servers.workbench.url={}", toml_str(url)));
        v.push("-c".into());
        v.push("mcp_servers.workbench.bearer_token_env_var=\"WORKBENCH_AGENT_TOKEN\"".into());
        v.push("-c".into());
        v.push(format!("mcp_servers.workbench.http_headers={{\"X-Workbench-Terminal\"={}}}", toml_str(terminal)));
    }
    if let Some(m) = a.model {
        v.push("--model".into());
        v.push(m.to_string());
    }
    if let Some(e) = a.effort {
        v.push("-c".into());
        v.push(format!("model_reasoning_effort={}", toml_str(e)));
    }
    if let Some(p) = a.permission {
        v.extend(p.args.iter().map(|s| s.to_string()));
    }
    for d in a.add_dirs {
        v.push("--add-dir".into());
        v.push(d.clone());
    }
    v.extend(a.extra_args.iter().cloned());
    match cont {
        Continue::Resume(id) | Continue::Fork(id) => v.push(id.clone()),
        Continue::New => {}
    }
    if let Some(p) = a.prompt.filter(|p| !p.trim().is_empty()) {
        v.push("--".into());
        v.push(p.to_string());
    }
    v
}

/// The Kimi Code command line (`kimi --help`, 2.1.1): `-S/--session <id>`, `-m/--model`,
/// `--plan`/`--yolo`/`--auto`, repeatable `--add-dir`. The initial prompt is pasted
/// (Kimi's `-p` is non-interactive). Kimi has no effort flag and no MCP flag (its MCP
/// servers come from `$KIMI_CODE_HOME/mcp.json` and the project's `.kimi-code/mcp.json`).
pub fn kimi_argv(command: &str, cont: &Continue, a: &LaunchArgs) -> Vec<String> {
    let mut v: Vec<String> = vec![command.to_string()];
    if let Continue::Resume(id) = cont {
        v.push("--session".into());
        v.push(id.clone());
    }
    if let Some(m) = a.model {
        v.push("--model".into());
        v.push(m.to_string());
    }
    if let Some(p) = a.permission {
        v.extend(p.args.iter().map(|s| s.to_string()));
    }
    for d in a.add_dirs {
        v.push("--add-dir".into());
        v.push(d.clone());
    }
    v.extend(a.extra_args.iter().cloned());
    v
}

/// The Gemini CLI command line (`gemini --help`, 0.61.0): `--session-id <uuid>` starts a
/// session under the id Workbench chose, `-r/--resume <uuid>` resumes one ("--resume
/// {number}, --resume {uuid}, or --resume latest"), `-m/--model`, `--approval-mode`,
/// repeatable `--include-directories`. The initial prompt is pasted once it is up.
/// UNVERIFIED (no account): the interactive behaviour after these flags.
pub fn gemini_argv(command: &str, cont: &Continue, session_id: &str, a: &LaunchArgs) -> Vec<String> {
    let mut v: Vec<String> = vec![command.to_string()];
    match cont {
        Continue::Resume(id) => v.extend(["--resume".to_string(), id.clone()]),
        _ => v.extend(["--session-id".to_string(), session_id.to_string()]),
    }
    if let Some(m) = a.model {
        v.push("--model".into());
        v.push(m.to_string());
    }
    if let Some(p) = a.permission {
        v.extend(p.args.iter().map(|s| s.to_string()));
    }
    for d in a.add_dirs {
        v.push("--include-directories".into());
        v.push(d.clone());
    }
    v.extend(a.extra_args.iter().cloned());
    v
}

/// Aider's chat history (the default of its `--chat-history-file`): at the git root of
/// the directory it starts in, else in that directory.
pub const AIDER_HISTORY: &str = ".aider.chat.history.md";

/// A file as it was at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileMark {
    pub exists: bool,
    pub len: u64,
    /// Modification time (ms since the epoch).
    pub mtime_ms: i64,
}

impl FileMark {
    pub fn of(path: &std::path::Path) -> Self {
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => FileMark {
                exists: true,
                len: m.len(),
                mtime_ms: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64),
            },
            _ => FileMark::default(),
        }
    }
}

/// Whether a restarted Aider session restores its chat history (`--restore-chat-history`
/// loads the history file into the model's context as earlier turns). Only a history
/// Aider wrote for this session: changed since the session first started (`first`), and
/// not tracked by git — committed repository content is untrusted (a hostile clone could
/// ship a fabricated conversation). Aider's own default is not to restore.
pub fn aider_restores(first: Option<&FileMark>, now: &FileMark, tracked: bool) -> bool {
    first.is_some_and(|f| now.exists && now != f && !tracked)
}

/// Where Aider keeps the chat history of a session started in `cwd`, and the git root
/// it is in (`None`: not in a repository).
pub fn aider_history_path(cwd: &std::path::Path) -> (PathBuf, Option<PathBuf>) {
    match cwd.ancestors().find(|d| d.join(".git").exists()) {
        Some(root) => (root.join(AIDER_HISTORY), Some(root.to_path_buf())),
        None => (cwd.join(AIDER_HISTORY), None),
    }
}

/// The Aider command line (`aider --help`, 0.86.2): `--model`, `--yes-always`, and
/// `--restore-chat-history` when a restarted session restores what Aider wrote for it
/// (`aider_restores`; Aider has no session ids). The initial prompt is pasted
/// (`--message` would exit after it).
pub fn aider_argv(command: &str, restore: bool, a: &LaunchArgs) -> Vec<String> {
    let mut v: Vec<String> = vec![command.to_string()];
    if restore {
        v.push("--restore-chat-history".into());
    }
    if let Some(m) = a.model {
        v.push("--model".into());
        v.push(m.to_string());
    }
    if let Some(p) = a.permission {
        v.extend(p.args.iter().map(|s| s.to_string()));
    }
    v.extend(a.extra_args.iter().cloned());
    v
}

/// A custom CLI: its command and configured arguments, nothing else.
pub fn custom_argv(command: &str, a: &LaunchArgs) -> Vec<String> {
    let mut v = vec![command.to_string()];
    v.extend(a.extra_args.iter().cloned());
    v
}

/// Whether `id` can be a session id of this kind (resume targets come from clients).
pub fn valid_session_id(kind: ProviderKind, id: &str) -> bool {
    match kind {
        ProviderKind::Claude | ProviderKind::Codex | ProviderKind::Gemini => super::transcript::is_uuid(id),
        ProviderKind::Aider => false,
        // Kimi Code: `session_<uuid>` (`createSessionId`); older kimi-cli used bare UUIDs.
        ProviderKind::Kimi => {
            !id.is_empty() && id.len() <= 100 && !id.starts_with('-') && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }
        ProviderKind::Custom => false,
    }
}

// ---------------------------------------------------------------- dialogs on screen

/// Texts of the dialogs Codex shows while it waits for an answer (codex-cli 0.157.1,
/// from its binary; `Yes, proceed` is what earlier versions offered). `true`: it asks
/// for permission; `false`: a question or the folder trust prompt. None of them reach
/// the rollout, and an Enter typed into one picks the highlighted choice ("Yes").
const CODEX_DIALOGS: &[(&str, bool)] = &[
    ("Would you like to run the following command?", true),
    ("Would you like to make the following edits?", true),
    ("Would you like to grant these permissions?", true),
    ("Would you like to send input to", true),
    ("Do you want to approve network access to", true),
    ("Yes, proceed", true),
    ("Yes, just this once", true),
    ("No, and tell Codex what to do differently", true),
    ("No, continue without running it", true),
    ("Apply full access for this session", true),
    ("Yes, provide the requested info", false),
    ("Respond to the MCP server request to continue", false),
    ("Respond to the tool suggestion to continue", false),
    ("Other (write an answer)", false),
    ("Trust this folder?", false),
    ("Trust and continue", false),
];

/// Kimi Code 2.1.1 (its bundled source): the approval panel's choices and footer, the
/// plan approval, the question panel's and other dialogs' footers, the trust prompt.
const KIMI_DIALOGS: &[(&str, bool)] = &[
    ("Approve once", true),
    ("Approve for this session", true),
    ("Reject with feedback", true),
    ("choose · ↵ confirm", true),
    ("Ready to build with this plan?", true),
    ("Trust this folder?", false),
    ("↑↓ select", false),
    ("↑↓ navigate", false),
    ("esc cancel", false),
    ("Esc to cancel", false),
];

/// Gemini CLI 0.61.0 (its bundled source): the tool confirmation's choices, its footer,
/// and the folder trust prompt.
const GEMINI_DIALOGS: &[(&str, bool)] = &[
    ("Allow once", true),
    ("Allow for this session", true),
    ("Allow for all future sessions", true),
    ("No, suggest changes (esc)", true),
    ("Modify with external editor", true),
    ("Waiting for user confirmation", true),
    ("Do you trust the files in this folder?", false),
    ("Do you trust the following folders", false),
];

/// A yes/no prompt on a custom CLI's last line (`[y/N]`, `(y/n)`, aider's `(Y)es/(N)o`).
static YES_NO: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\[y(es)?/n(o)?\]|\(y(es)?/n(o)?\)|\(y\)es/\(n\)o").unwrap());

/// A dialog of the kind's CLI on `screen` (the bottom of the visible screen): the state it
/// puts the session in (`NeedsPermission` for approvals, `NeedsInput` otherwise) and what
/// to tell the user. Claude Code reports its dialogs through hooks: always `None`.
pub fn dialog_on_screen(kind: ProviderKind, screen: &str) -> Option<(super::AgentState, &'static str)> {
    use super::AgentState::{NeedsInput, NeedsPermission};
    let known = |list: &[(&str, bool)]| list.iter().filter(|(t, _)| screen.contains(t)).map(|(_, p)| *p).reduce(|a, b| a || b);
    match kind {
        ProviderKind::Claude => None,
        ProviderKind::Codex => known(CODEX_DIALOGS).map(|p| {
            if p { (NeedsPermission, "Codex asks for approval — answer in the terminal") } else { (NeedsInput, "Codex asks a question — answer in the terminal") }
        }),
        ProviderKind::Kimi => known(KIMI_DIALOGS).map(|p| {
            if p { (NeedsPermission, "Kimi Code asks for approval — answer in the terminal") } else { (NeedsInput, "Kimi Code asks a question — answer in the terminal") }
        }),
        ProviderKind::Gemini => known(GEMINI_DIALOGS).map(|p| {
            if p { (NeedsPermission, "Gemini asks for approval — answer in the terminal") } else { (NeedsInput, "Gemini asks a question — answer in the terminal") }
        }),
        // Aider's confirmations (`Run shell command? (Y)es/(N)o [Yes]:`) end its last line.
        ProviderKind::Aider => {
            let last = screen.lines().rev().find(|l| !l.trim().is_empty())?;
            YES_NO.is_match(last).then_some((NeedsInput, "Aider asks for confirmation — answer in the terminal"))
        }
        ProviderKind::Custom => {
            let last = screen.lines().rev().find(|l| !l.trim().is_empty())?;
            YES_NO.is_match(last).then_some((NeedsInput, "The agent asks a yes/no question — answer in the terminal"))
        }
    }
}

// ---------------------------------------------------------------- what the UI needs

/// `GET /api/agents/defaults` → `providers[]`.
pub fn describe(p: &Provider, available: Option<&PathBuf>) -> Value {
    let k = p.kind;
    let reason = if !p.enabled {
        Some("disabled in config.toml".to_string())
    } else if available.is_none() {
        Some(format!("{} ({:?}) was not found on this machine", p.label, p.command))
    } else {
        None
    };
    json!({
        "id": p.id,
        "kind": k,
        "label": p.label,
        "command": p.command,
        "enabled": p.enabled,
        "available": p.enabled && available.is_some(),
        "reason": reason,
        "installHint": (!p.install_hint.is_empty()).then(|| p.install_hint.clone()),
        "stateSource": k.state_source(),
        "initialPrompt": if k.prompt_in_argv() { "argv" } else { "paste" },
        "supports": {
            "resume": k.resumes(),
            "fork": k.forks(),
            "model": k != ProviderKind::Custom,
            "mcp": matches!(k, ProviderKind::Claude | ProviderKind::Codex),
            "remoteControl": k == ProviderKind::Claude,
            "addDirs": k.takes_add_dirs(),
            "history": k.resumes(),
            // Permission requests answered from Workbench (Claude Code's hooks).
            "answerPermissions": k == ProviderKind::Claude,
        },
        "efforts": k.efforts(),
        "permissionModes": k.permission_modes(),
        "defaults": {
            "model": p.model,
            "effort": p.effort,
            // Other kinds never start in a dangerous mode by default (Claude keeps its
            // `[agents].permission_mode` as before).
            "permissionMode": p.permission_mode.as_deref().filter(|m| p.permission_preset(m).is_some_and(|x| k == ProviderKind::Claude || !x.dangerous)),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml_text: &str) -> AgentsConfig {
        let g: crate::config::GlobalConfig = toml::from_str(toml_text).unwrap();
        g.agents
    }

    #[test]
    fn presets_and_custom_providers_from_config() {
        let a = cfg(
            r#"
            [agents]
            command = "/opt/claude"
            effort = "high"
            default_provider = "codex"
            [agents.providers.codex]
            model = "gpt-5.5"
            effort = "xhigh"
            args = ["--search"]
            [agents.providers.kimi]
            enabled = false
            [agents.providers.aider]
            args = ["--no-auto-commits"]
            [agents.providers.opencode]
            command = "opencode"
            args = ["--print-logs"]
            label = "OpenCode"
            [agents.providers.codex-work]
            kind = "codex"
            command = "~/bin/codex"
            env = { CODEX_HOME = "~/.codex-work" }
            [agents.providers.nocmd]
            label = "broken"
            [agents.providers."Bad Name"]
            command = "x"
            [agents.providers.weird]
            kind = "goose"
            command = "goose"
            "#,
        );
        let (list, warnings) = list(&a);
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["claude", "codex", "kimi", "gemini", "aider", "codex-work", "opencode"]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        let claude = &list[0];
        assert_eq!((claude.command.as_str(), claude.effort.as_deref(), claude.label.as_str()), ("/opt/claude", Some("high"), "Claude Code"));
        let codex = &list[1];
        assert_eq!((codex.command.as_str(), codex.model.as_deref(), codex.effort.as_deref()), ("codex", Some("gpt-5.5"), Some("xhigh")));
        assert_eq!(codex.args, ["--search"]);
        assert!(!list[2].enabled);
        assert_eq!((list[3].kind, list[3].label.as_str(), list[3].command.as_str()), (ProviderKind::Gemini, "Gemini CLI", "gemini"));
        // `[agents.providers.aider]` configures the Aider preset.
        assert_eq!((list[4].kind, list[4].label.as_str(), list[4].args.as_slice()), (ProviderKind::Aider, "Aider", &["--no-auto-commits".to_string()][..]));
        assert_eq!((list[5].kind, list[5].label.as_str()), (ProviderKind::Codex, "codex-work"));
        assert_eq!(list[5].env.get("CODEX_HOME").map(String::as_str), Some("~/.codex-work"));
        assert_eq!((list[6].kind, list[6].label.as_str()), (ProviderKind::Custom, "OpenCode"));
        assert_eq!(find(&a, None).unwrap().id, "codex");
        assert_eq!(find(&a, Some("aider")).unwrap().command, "aider");
        assert!(find(&a, Some("nocmd")).is_none());
        assert!(warnings.iter().any(|w| w.contains("unknown kind \"goose\"")), "{warnings:?}");
        // Defaults without any [agents.providers]: the presets.
        let (plain, w) = super::list(&AgentsConfig::default());
        assert_eq!(plain.iter().map(|p| p.kind).collect::<Vec<_>>(), PRESETS);
        assert!(w.is_empty());
        assert_eq!(default_id(&AgentsConfig::default()), "claude");
    }

    fn preset_of(kind: ProviderKind, id: &str) -> &'static PermissionPreset {
        kind.permission_modes().iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn codex_command_lines() {
        let f = CodexFeatures { no_daemon: true, no_alt_screen: true };
        let dirs = vec!["/data/workspace/p".to_string()];
        let a = LaunchArgs {
            model: Some("gpt-5.5"),
            effort: Some("high"),
            permission: Some(preset_of(ProviderKind::Codex, "workspace-write")),
            add_dirs: &dirs,
            extra_args: &[],
            mcp: Some(("http://127.0.0.1:7822/mcp", "abc123")),
            prompt: Some("-fix the build"),
        };
        assert_eq!(
            codex_argv("codex", &Continue::New, f, &a),
            [
                "codex",
                "--no-daemon",
                "--no-alt-screen",
                "-c",
                "mcp_servers.workbench.url=\"http://127.0.0.1:7822/mcp\"",
                "-c",
                "mcp_servers.workbench.bearer_token_env_var=\"WORKBENCH_AGENT_TOKEN\"",
                "-c",
                "mcp_servers.workbench.http_headers={\"X-Workbench-Terminal\"=\"abc123\"}",
                "--model",
                "gpt-5.5",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "--add-dir",
                "/data/workspace/p",
                "--",
                "-fix the build",
            ]
        );
        let id = "019a0000-0000-7000-8000-000000000001".to_string();
        let plain = LaunchArgs::default();
        assert_eq!(codex_argv("codex", &Continue::Resume(id.clone()), CodexFeatures::default(), &plain), ["codex", "resume", id.as_str()]);
        let bypass = LaunchArgs { permission: Some(preset_of(ProviderKind::Codex, "bypass")), ..Default::default() };
        assert_eq!(
            codex_argv("/x/codex", &Continue::Fork(id.clone()), f, &bypass),
            ["/x/codex", "fork", "--no-daemon", "--no-alt-screen", "--dangerously-bypass-approvals-and-sandbox", id.as_str()]
        );
        // The session id comes after the options, the prompt after `--`.
        let resume_prompt = LaunchArgs { prompt: Some("continue"), ..Default::default() };
        let v = codex_argv("codex", &Continue::Resume(id.clone()), CodexFeatures::default(), &resume_prompt);
        assert_eq!(&v[v.len() - 3..], [id.as_str(), "--", "continue"]);
        // Never ask stays inside the sandbox.
        assert_eq!(preset_of(ProviderKind::Codex, "never-ask").args, ["--sandbox", "workspace-write", "--ask-for-approval", "never"]);
        assert!(preset_of(ProviderKind::Codex, "bypass").dangerous);
        assert!(CODEX_MODES.iter().filter(|p| p.dangerous).count() == 1);
    }

    #[test]
    fn kimi_and_custom_command_lines() {
        let dirs = vec!["/w/home".to_string()];
        let extra = vec!["--thinking".to_string()];
        let a = LaunchArgs {
            model: Some("kimi-k2"),
            permission: Some(preset_of(ProviderKind::Kimi, "plan")),
            add_dirs: &dirs,
            extra_args: &extra,
            prompt: Some("ignored: pasted later"),
            ..Default::default()
        };
        assert_eq!(kimi_argv("kimi", &Continue::New, &a), ["kimi", "--model", "kimi-k2", "--plan", "--add-dir", "/w/home", "--thinking"]);
        let r = kimi_argv("kimi", &Continue::Resume("session_abc".into()), &LaunchArgs { permission: Some(preset_of(ProviderKind::Kimi, "yolo")), ..Default::default() });
        assert_eq!(r, ["kimi", "--session", "session_abc", "--yolo"]);
        assert!(preset_of(ProviderKind::Kimi, "yolo").dangerous && preset_of(ProviderKind::Kimi, "auto").dangerous);
        let args = vec!["--model".to_string(), "x".to_string()];
        assert_eq!(custom_argv("aider", &LaunchArgs { extra_args: &args, model: Some("ignored"), ..Default::default() }), ["aider", "--model", "x"]);
    }

    #[test]
    fn gemini_and_aider_command_lines() {
        let dirs = vec!["/w/data".to_string()];
        let id = "0f0e0d0c-0000-4000-8000-000000000000";
        let a = LaunchArgs {
            model: Some("gemini-2.5-pro"),
            permission: Some(preset_of(ProviderKind::Gemini, "auto_edit")),
            add_dirs: &dirs,
            prompt: Some("pasted, never argv"),
            ..Default::default()
        };
        assert_eq!(
            gemini_argv("gemini", &Continue::New, id, &a),
            ["gemini", "--session-id", id, "--model", "gemini-2.5-pro", "--approval-mode", "auto_edit", "--include-directories", "/w/data"]
        );
        assert_eq!(gemini_argv("gemini", &Continue::Resume(id.into()), id, &LaunchArgs::default()), ["gemini", "--resume", id]);
        assert!(preset_of(ProviderKind::Gemini, "yolo").dangerous && !preset_of(ProviderKind::Gemini, "plan").dangerous);
        let extra = vec!["--no-auto-commits".to_string()];
        let a = LaunchArgs { model: Some("sonnet"), permission: Some(preset_of(ProviderKind::Aider, "yes-always")), extra_args: &extra, add_dirs: &dirs, ..Default::default() };
        assert_eq!(aider_argv("aider", false, &a), ["aider", "--model", "sonnet", "--yes-always", "--no-auto-commits"]);
        assert_eq!(aider_argv("aider", true, &LaunchArgs::default()), ["aider", "--restore-chat-history"]);
        assert!(AIDER_MODES.iter().all(|p| p.dangerous));
        // A restart restores only a history Aider wrote for the session, never a
        // committed (repository) one.
        let none = FileMark::default();
        let before = FileMark { exists: true, len: 10, mtime_ms: 1 };
        let after = FileMark { exists: true, len: 90, mtime_ms: 2 };
        assert!(aider_restores(Some(&none), &after, false), "created by this session");
        assert!(aider_restores(Some(&before), &after, false), "appended by this session");
        assert!(!aider_restores(Some(&before), &before, false), "untouched since the first start");
        assert!(!aider_restores(Some(&none), &after, true), "tracked by git: repository content");
        assert!(!aider_restores(Some(&before), &after, true));
        assert!(!aider_restores(Some(&before), &none, false), "gone");
        assert!(!aider_restores(None, &after, false), "nothing noted at the first start");
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("repo/sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(aider_history_path(&sub), (sub.join(AIDER_HISTORY), None));
        std::fs::create_dir_all(d.path().join("repo/.git")).unwrap();
        assert_eq!(aider_history_path(&sub), (d.path().join("repo").join(AIDER_HISTORY), Some(d.path().join("repo"))));
        assert!(!FileMark::of(&d.path().join("repo").join(AIDER_HISTORY)).exists);
        // Ids: Gemini's are chosen at launch; Aider has none.
        assert!(ProviderKind::Gemini.id_at_launch() && ProviderKind::Claude.id_at_launch() && !ProviderKind::Codex.id_at_launch());
        assert!(valid_session_id(ProviderKind::Gemini, id) && !valid_session_id(ProviderKind::Gemini, "latest"));
        assert!(!valid_session_id(ProviderKind::Aider, id));
        assert!(!ProviderKind::Aider.resumes() && !ProviderKind::Aider.takes_add_dirs() && ProviderKind::Gemini.takes_add_dirs());
        assert_eq!(ProviderKind::parse("gemini"), Some(ProviderKind::Gemini));
    }

    #[test]
    fn gemini_and_aider_dialogs_are_recognized_on_screen() {
        use crate::terminals::AgentState::{NeedsInput, NeedsPermission};
        let gemini = " ╭──────────────────────────╮\n │ ? Shell  rm -rf build     │\n │ Allow execution of: 'rm'? │\n │ ● 1. Allow once           │\n │   2. Allow for this session │\n │   3. No, suggest changes (esc) │\n ╰──────────────────────────╯";
        assert_eq!(dialog_on_screen(ProviderKind::Gemini, gemini).map(|d| d.0), Some(NeedsPermission));
        assert_eq!(dialog_on_screen(ProviderKind::Gemini, "Do you trust the files in this folder?\n● 1. Trust folder").map(|d| d.0), Some(NeedsInput));
        assert!(dialog_on_screen(ProviderKind::Gemini, "✦ Done.\n>   Type your message").is_none());
        assert_eq!(dialog_on_screen(ProviderKind::Aider, "ls -la\nRun shell command? (Y)es/(N)o/(D)on't ask again [Yes]:").map(|d| d.0), Some(NeedsInput));
        assert!(dialog_on_screen(ProviderKind::Aider, "Run shell command? (Y)es/(N)o [Yes]: y\n> ").is_none());
    }

    #[test]
    fn defaults_a_kind_cannot_take_are_dropped_with_a_warning() {
        let a = cfg(
            r#"
            [agents]
            effort = "extreme"
            default_provider = "codx"
            [agents.providers.kimi]
            effort = "high"
            model = "kimi-k2"
            [agents.providers.codex]
            effort = "ultra"
            permission_mode = "bypass"
            model = "gpt 5; rm"
            [agents.providers.mycli2]
            command = "mycli"
            model = "gpt-x"
            permission_mode = "plan"
            "#,
        );
        let (list, warnings) = list(&a);
        let get = |id: &str| list.iter().find(|p| p.id == id).unwrap();
        // Dropped, so starting a session no longer fails on them…
        assert_eq!((get("kimi").effort.as_deref(), get("kimi").model.as_deref()), (None, Some("kimi-k2")));
        assert_eq!((get("codex").effort.as_deref(), get("codex").permission_mode.as_deref(), get("codex").model.as_deref()), (None, None, None));
        assert_eq!((get("mycli2").model.as_deref(), get("mycli2").permission_mode.as_deref()), (None, None));
        assert_eq!(get("claude").effort.as_deref(), None);
        // …and each one is reported where it was set.
        let has = |s: &str| warnings.iter().any(|w| w.contains(s));
        assert!(has("agents.providers.kimi: effort \"high\" ignored (Kimi Code has no effort setting)"), "{warnings:?}");
        assert!(has("agents.providers.codex: effort \"ultra\" ignored (one of minimal"), "{warnings:?}");
        assert!(has("agents.providers.codex: permission_mode \"bypass\" ignored"), "{warnings:?}");
        assert!(has("agents.providers.codex: model \"gpt 5; rm\" ignored"), "{warnings:?}");
        assert!(has("agents.providers.mycli2: model \"gpt-x\" ignored"), "{warnings:?}");
        assert!(has("agents.providers.mycli2: permission_mode \"plan\" ignored (mycli2 has no permission modes)"), "{warnings:?}");
        assert!(has("agents: effort \"extreme\" ignored"), "{warnings:?}");
        assert!(has("agents.default_provider: \"codx\" is not a configured provider"), "{warnings:?}");
        assert_eq!(warnings.len(), 8, "{warnings:?}");
        assert_eq!(find(&a, Some("kimi")).unwrap().effort, None);
        // A disabled default is reported too; a valid setup has no warnings.
        let off = cfg("[agents]\ndefault_provider = \"kimi\"\n[agents.providers.kimi]\nenabled = false\n");
        assert_eq!(super::list(&off).1, ["agents.default_provider: \"kimi\" is disabled; asking an agent fails until it is enabled"]);
        let ok = cfg("[agents]\neffort = \"high\"\npermission_mode = \"bypassPermissions\"\n[agents.providers.codex]\neffort = \"xhigh\"\npermission_mode = \"never-ask\"\n");
        assert!(super::list(&ok).1.is_empty(), "{:?}", super::list(&ok).1);
    }

    #[test]
    fn codex_dialogs_are_recognized_on_screen() {
        use crate::terminals::AgentState::{NeedsInput, NeedsPermission};
        let approval = "• Working on: fix the build\n\nWould you like to run the following command?\n\n  $ rm -rf ./build && git push --force\n\n› 1. Yes, just this once (y)\n  2. No, and tell Codex what to do differently (esc)\n";
        assert_eq!(dialog_on_screen(ProviderKind::Codex, approval).map(|d| d.0), Some(NeedsPermission));
        assert_eq!(dialog_on_screen(ProviderKind::Codex, "› 1. Yes, proceed").map(|d| d.0), Some(NeedsPermission));
        assert_eq!(dialog_on_screen(ProviderKind::Codex, "Trust this folder? Codex can read, edit, and run files here").map(|d| d.0), Some(NeedsInput));
        assert_eq!(dialog_on_screen(ProviderKind::Codex, "Would you like to make the following edits?").map(|d| d.0), Some(NeedsPermission));
        assert!(dialog_on_screen(ProviderKind::Codex, "• Done: fix the build\n› ").is_none());
        // Kimi Code's approval panel, question panel and trust prompt.
        let kimi = "  ▶ Run this command?\n\n  $ make deploy\n\n  ▶ 1. Approve once\n    2. Approve for this session\n    3. Reject\n\n  ↑/↓ select · 1/2/3/4 choose · ↵ confirm";
        assert_eq!(dialog_on_screen(ProviderKind::Kimi, kimi).map(|d| d.0), Some(NeedsPermission));
        assert_eq!(dialog_on_screen(ProviderKind::Kimi, "  ↑↓ select  1-3 / ↵ choose  esc cancel").map(|d| d.0), Some(NeedsInput));
        assert_eq!(dialog_on_screen(ProviderKind::Kimi, " Trust this folder?\n ↑↓ navigate · Enter select · Esc exit").map(|d| d.0), Some(NeedsInput));
        assert!(dialog_on_screen(ProviderKind::Kimi, "Done with the question.\n> ").is_none());
        // Custom CLIs: a yes/no prompt on the last line only.
        assert!(dialog_on_screen(ProviderKind::Custom, "Run shell command? (Y)es/(N)o [Yes]:").is_some());
        assert!(dialog_on_screen(ProviderKind::Custom, "Overwrite? [y/N]\n").is_some());
        assert!(dialog_on_screen(ProviderKind::Custom, "Overwrite? [y/N] y\ndone\n> ").is_none());
        // Claude Code's dialogs come through hooks.
        assert!(dialog_on_screen(ProviderKind::Claude, approval).is_none());
    }

    #[test]
    fn only_codex_sessions_inside_their_sandbox_count_as_sandboxed() {
        let a = cfg("[agents.providers.codex-open]\nkind = \"codex\"\nargs = [\"--sandbox\", \"danger-full-access\"]\n[agents.providers.aider]\ncommand = \"aider\"\n");
        let p = |id: &str| find(&a, Some(id)).unwrap();
        for mode in [None, Some("read-only"), Some("workspace-write"), Some("never-ask")] {
            assert!(p("codex").sandboxed(mode), "{mode:?}");
        }
        assert!(!p("codex").sandboxed(Some("bypass")));
        assert!(!p("codex-open").sandboxed(None));
        assert!(!p("claude").sandboxed(Some("plan")) && !p("kimi").sandboxed(Some("plan")) && !p("aider").sandboxed(None));
    }

    #[test]
    fn session_ids_and_feature_probe() {
        assert!(valid_session_id(ProviderKind::Codex, "019a0000-0000-7000-8000-000000000001"));
        assert!(!valid_session_id(ProviderKind::Codex, "--last"));
        assert!(valid_session_id(ProviderKind::Kimi, "session_0f0e0d0c-0000-4000-8000-000000000000"));
        assert!(!valid_session_id(ProviderKind::Kimi, "-x"));
        assert!(!valid_session_id(ProviderKind::Kimi, "a/../b"));
        assert!(!valid_session_id(ProviderKind::Custom, "anything"));
        let f = CodexFeatures::from_help("      --no-alt-screen\n          Disable alternate screen mode\n");
        assert_eq!(f, CodexFeatures { no_daemon: false, no_alt_screen: true });
        assert_eq!(toml_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert!(valid_provider_id("codex-work") && valid_provider_id("aider") && !valid_provider_id("-x") && !valid_provider_id("A"));
    }
}
