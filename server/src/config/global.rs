//! `~/.config/workbench/config.toml` — machine-wide settings.
//!
//! Secrets are never stored here: fields name an entry of `[secrets]`, and each
//! entry says where the value lives (`{ file = "~/.gitlab_token" }`, …).

use std::collections::BTreeMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::{Paths, SecretRef, expand_tilde};

fn is_default<T: Default + PartialEq>(t: &T) -> bool {
    *t == T::default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GlobalConfig {
    pub server: ServerConfig,
    pub projects: ProjectsConfig,
    pub agents: AgentsConfig,
    /// Terminals: the shell new ones run.
    #[serde(skip_serializing_if = "is_default")]
    pub terminals: TerminalsConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gitlab: Option<GitlabConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<GithubConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atlassian: Option<AtlassianConfig>,
    #[serde(skip_serializing_if = "is_default")]
    pub notify: NotifyConfig,
    /// Dev containers: the docker and devcontainer CLI executables, and the engine.
    #[serde(skip_serializing_if = "is_default")]
    pub devcontainer: DevcontainerConfig,
    /// Code intelligence: language server presets and overrides.
    #[serde(skip_serializing_if = "is_default")]
    pub lsp: crate::lsp::LspConfig,
    /// Debugger: debug adapter presets and overrides.
    #[serde(skip_serializing_if = "is_default")]
    pub debug: crate::debug::DebugConfig,
    /// Web Push to phones and other devices.
    #[serde(skip_serializing_if = "is_default")]
    pub push: crate::platform::push::PushConfig,
    /// Updates: the daily look for a newer release, and where releases come from.
    #[serde(skip_serializing_if = "is_default")]
    pub update: crate::platform::update::UpdateConfig,
    /// Directories outside any project that the editor may open read-only
    /// (Claude scratchpads, `~/.claude`). Project roots are always allowed.
    pub extra_roots: Vec<String>,
    pub secrets: BTreeMap<String, SecretRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    /// `127.0.0.1:7777` keeps Workbench local. A LAN/Tailscale address enables remote access
    /// (every client still needs the token or a paired device cookie).
    pub bind: String,
    /// Extra `Host` header values accepted besides loopback names and the bind address,
    /// e.g. `"box.tailnet.ts.net"` when behind `tailscale serve`.
    pub allowed_hosts: Vec<String>,
    /// URL other devices use to reach this server (pairing links / QR codes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Serve HTTPS directly (paths to PEM files). Usually a proxy terminates TLS instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:7777".into(),
            allowed_hosts: vec![],
            public_url: None,
            tls: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectsConfig {
    /// Every directory directly under a root that is a git repository becomes a project.
    pub roots: Vec<String>,
    /// Explicit project directories (anywhere).
    pub include: Vec<String>,
    /// Directories (absolute or `~/`) never treated as projects.
    pub exclude: Vec<String>,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self { roots: vec!["~/workspace".into()], include: vec![], exclude: vec![] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentsConfig {
    /// The Claude Code executable.
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// low | medium | high | xhigh | max
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// acceptEdits | auto | bypassPermissions | manual | dontAsk | plan
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Start new sessions with `--remote-control`.
    pub remote_control: bool,
    /// Resume sessions that were open when Workbench stopped.
    pub restore_on_start: bool,
    /// Install Workbench's status line into hosted sessions (skipped when the
    /// user's own settings define one).
    pub statusline: bool,
    /// Answer Claude Code permission requests from Workbench (the session's card and
    /// tab, the attention toast, the phone, push notifications). Claude's own prompt in
    /// the terminal stays usable meanwhile, and the first answer wins.
    pub answer_permissions: bool,
    /// How long (seconds, 30–3600) a permission request stays answerable from
    /// Workbench; afterwards it is answered in the terminal only.
    pub permission_wait: u64,
    /// Provider for sessions started without one (`/api/agents/ask`, MCP): `claude`
    /// unless set to another `[agents.providers.<name>]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,
    /// What happens when an account is at its usage limit and has `fallback` accounts:
    /// `off` (only show the usage), `new` (a new session starts on the first account
    /// that is not at its limit; the default) or `session` (a running session that hits
    /// its limit also continues on the next account, as a new session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<String>,
    /// What a session moved to another account takes along: `conversation` (the default: the
    /// conversation itself when the CLI is the same, else its text as a Markdown file) or
    /// `notes` (a short note on where it stopped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer: Option<String>,
    /// Agent CLIs besides Claude Code: the built-in `codex` and `kimi` presets, or any
    /// command (`[agents.providers.aider] command = "aider"`). The fields above stay the
    /// Claude Code defaults. Must stay the last field: TOML tables follow plain values.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            command: "claude".into(),
            model: None,
            effort: Some("xhigh".into()),
            permission_mode: None,
            remote_control: false,
            restore_on_start: true,
            statusline: true,
            answer_permissions: true,
            permission_wait: 600,
            default_provider: None,
            failover: None,
            transfer: None,
            providers: BTreeMap::new(),
        }
    }
}

/// `[agents.providers.<name>]`. Every field is optional: `[agents.providers.codex]` with
/// nothing in it is the Codex preset. A name other than `claude`, `codex`, `kimi`,
/// `gemini` or `aider` is a custom CLI and needs `command`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    /// `claude`, `codex`, `kimi`, `gemini`, `aider` or `custom`. Defaults to the name for
    /// the built-in presets and to `custom` otherwise (e.g. a second Codex with its own
    /// `CODEX_HOME`: `[agents.providers.codex-work] kind = "codex"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The executable (on PATH, absolute or `~/…`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Extra arguments for every launch (a custom CLI's whole argument list).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// `false` hides the provider. Default `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Name shown in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Accounts (other provider names) to use, in this order, when this one is at its
    /// usage limit: a second subscription, then perhaps a local model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
    /// A default permission preset of the provider. Dangerous presets (Codex `bypass`,
    /// Kimi `yolo`/`auto`, Gemini `yolo`, Aider `yes-always`) are never defaults: the user
    /// picks them per session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Environment for the provider's sessions (e.g. `CODEX_HOME`, `KIMI_CODE_HOME`).
    /// Values are plain text (`~/` is expanded); secrets do not go here.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// How to install the command, shown while it is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_hint: Option<String>,
    /// Run the CLI against a model server of your own (Ollama, LM Studio, any
    /// OpenAI-compatible server) instead of the vendor's. `model` names the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalModelConfig>,
    /// Use a hosted model API (DeepSeek, OpenRouter, an Anthropic or OpenAI API key…) with an
    /// API key instead of a subscription login. `model` names the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiConfig>,
}

/// `[agents.providers.<name>.api]`: a hosted model API reached with an API key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    /// `deepseek`, `openrouter`, `zai`, `moonshot`, `fireworks`, `anthropic`, `openai` or
    /// `custom` (which needs `url`). Which of them a CLI can use depends on the API it speaks.
    pub service: String,
    /// The API's address. Empty: the service's usual one. Always `https://` (plain `http://` only
    /// for this computer).
    pub url: String,
    /// Name of a `[secrets]` entry that holds the API key: the key itself is never in this file.
    pub key: String,
    /// The model's context window in tokens (see `local.context`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

/// `[agents.providers.<name>.local]`: a model server on this machine or your network.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalModelConfig {
    /// `ollama`, `lmstudio` or `openai` (any server with an OpenAI-compatible API, such as
    /// llama.cpp's `llama-server` or vLLM).
    pub server: String,
    /// The server's address, such as `http://localhost:11434`. Empty: the server's usual one.
    pub url: String,
    /// The model's context window in tokens, when it is not the 200 000 Claude Code assumes
    /// for a model it does not know (a local model often has 32 000 to 128 000).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitlabConfig {
    #[serde(default = "gitlab_com")]
    pub host: String,
    /// Name of a `[secrets]` entry holding a personal access token.
    pub token: String,
}

fn gitlab_com() -> String {
    "gitlab.com".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GithubConfig {
    /// `github.com`, or a GitHub Enterprise host (its API is `https://<host>/api/v3`).
    #[serde(default = "github_com")]
    pub host: String,
    /// Name of a `[secrets]` entry holding a token. Empty: public repositories only,
    /// unauthenticated (60 requests/hour).
    #[serde(default)]
    pub token: String,
}

fn github_com() -> String {
    "github.com".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AtlassianConfig {
    /// `https://<site>.atlassian.net`
    pub site: String,
    pub email: String,
    /// Name of a `[secrets]` entry holding an API token (or `email:token`).
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotifyConfig {
    /// Desktop notifications via `notify-send` when an agent needs attention.
    pub desktop: bool,
    /// Extra command run with the message as `$WORKBENCH_MESSAGE` (e.g. an ntfy curl).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// `[terminals]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalsConfig {
    /// The shell of new terminals, program and arguments (`["pwsh.exe", "-NoLogo"]`,
    /// `["/bin/zsh", "-l"]`). Empty: `$SHELL -l` (Unix), PowerShell (Windows).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
}

/// `[devcontainer]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DevcontainerConfig {
    /// The docker executable (default: `docker` on PATH).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub docker: String,
    /// The devcontainer CLI: empty = `devcontainer` on PATH, `npx` = `npx -y
    /// @devcontainers/cli` (downloads it), or a path. Needed for features.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cli: String,
    /// `auto` (built-in docker engine, the CLI for features), `docker` or `cli`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub engine: String,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self { desktop: true, command: None }
    }
}

impl GlobalConfig {
    /// Load `config.toml`, writing a detected default on first run.
    pub fn load_or_init(paths: &Paths) -> anyhow::Result<Self> {
        let file = paths.config_file();
        if file.exists() {
            let text = std::fs::read_to_string(&file)?;
            return toml::from_str(&text).with_context(|| format!("parse {}", file.display()));
        }
        let cfg = Self::detect_default();
        cfg.save(paths)?;
        tracing::info!("wrote default config to {}", file.display());
        Ok(cfg)
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        let text = format!(
            "# Workbench configuration. See docs/ARCHITECTURE.md#configuration.\n# Secret values never go here; [secrets] says where each one lives.\n\n{}",
            toml::to_string_pretty(self)?
        );
        crate::util::fs::write_atomic(&paths.config_file(), text.as_bytes(), 0o600)
    }

    /// A first-run config built from what exists on this machine.
    fn detect_default() -> Self {
        let mut cfg = Self::default();
        cfg.extra_roots = vec![crate::util::os::path::claude_temp_dir(), "~/.claude".into()];
        if expand_tilde("~/.gitlab_token").exists() {
            cfg.secrets.insert("gitlab".into(), SecretRef::File("~/.gitlab_token".into()));
            cfg.gitlab = Some(GitlabConfig { host: gitlab_com(), token: "gitlab".into() });
        }
        if expand_tilde("~/.github_token").exists() {
            cfg.secrets.insert("github".into(), SecretRef::File("~/.github_token".into()));
            cfg.github = Some(GithubConfig { host: github_com(), token: "github".into() });
        }
        if expand_tilde("~/.atlassian_token").exists() {
            cfg.secrets.insert("atlassian".into(), SecretRef::File("~/.atlassian_token".into()));
            let email = std::process::Command::new("git")
                .args(["config", "--global", "user.email"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            cfg.atlassian = Some(AtlassianConfig { site: String::new(), email, token: "atlassian".into() });
        }
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_shell_is_written_only_when_set() {
        let cfg: GlobalConfig = toml::from_str("[terminals]\nshell = [\"pwsh.exe\", \"-NoLogo\"]\n").unwrap();
        assert_eq!(cfg.terminals.shell, ["pwsh.exe", "-NoLogo"]);
        assert!(toml::to_string_pretty(&cfg).unwrap().contains("[terminals]"));
        assert!(!toml::to_string_pretty(&GlobalConfig::default()).unwrap().contains("[terminals]"));
    }

    #[test]
    fn agent_providers_parse_and_round_trip() {
        let text = r#"
            [agents]
            command = "claude"
            default_provider = "codex"

            [agents.providers.codex]
            model = "gpt-5.5"
            args = ["--search"]
            env = { CODEX_HOME = "~/.codex" }

            [agents.providers.aider]
            command = "aider"
            label = "Aider"
            enabled = false
        "#;
        let cfg: GlobalConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.agents.default_provider.as_deref(), Some("codex"));
        assert_eq!(cfg.agents.providers.len(), 2);
        let codex = &cfg.agents.providers["codex"];
        assert_eq!((codex.model.as_deref(), codex.args.as_slice()), (Some("gpt-5.5"), &["--search".to_string()][..]));
        assert_eq!(codex.env.get("CODEX_HOME").map(String::as_str), Some("~/.codex"));
        assert_eq!(cfg.agents.providers["aider"].enabled, Some(false));
        // Written back (Settings saves the whole config), it means the same.
        let written = toml::to_string_pretty(&cfg).unwrap();
        assert!(written.contains("[agents.providers.codex]"), "{written}");
        assert_eq!(toml::from_str::<GlobalConfig>(&written).unwrap(), cfg);
        // Configs without providers are unchanged by them.
        let plain = toml::to_string_pretty(&GlobalConfig::default()).unwrap();
        assert!(!plain.contains("providers") && !plain.contains("default_provider"), "{plain}");
    }
}
