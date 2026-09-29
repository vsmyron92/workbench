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
