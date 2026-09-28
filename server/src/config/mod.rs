//! Configuration: where Workbench keeps its files, the global `config.toml`,
//! and the per-project model (`project.rs`).

pub mod global;
pub mod project;

use std::path::{Path, PathBuf};

use anyhow::Context;

pub use global::GlobalConfig;
#[allow(unused_imports)]
pub use project::{ProjectFile, SecretRef};

/// Directories Workbench owns. Both can be overridden with environment
/// variables so several instances (tests, parallel development) never share state.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$WORKBENCH_CONFIG_DIR` or `~/.config/workbench`: `config.toml`, `projects/<id>.toml`.
    pub config_dir: PathBuf,
    /// `$WORKBENCH_DATA_DIR` or `~/.local/share/workbench`: token, sessions, terminal state.
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> anyhow::Result<Self> {
        let config_dir = match std::env::var_os("WORKBENCH_CONFIG_DIR") {
            Some(p) => PathBuf::from(p),
            None => dirs::config_dir().context("no config dir")?.join("workbench"),
        };
        let data_dir = match std::env::var_os("WORKBENCH_DATA_DIR") {
            Some(p) => PathBuf::from(p),
            None => dirs::data_dir().context("no data dir")?.join("workbench"),
        };
        for d in [&config_dir, &data_dir] {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
            crate::util::fs::set_mode(d, 0o700);
        }
        Ok(Self { config_dir, data_dir })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Machine-local overlay for one project (hosts, secret references).
    pub fn project_overlay(&self, id: &str) -> PathBuf {
        self.config_dir.join("projects").join(format!("{id}.toml"))
    }

    /// A subdirectory of the data dir, created on first use.
    pub fn data(&self, sub: &str) -> PathBuf {
        let p = self.data_dir.join(sub);
        let _ = std::fs::create_dir_all(&p);
        p
    }
}

/// `~/x` → `$HOME/x`. Other paths are returned unchanged.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

/// Inverse of `expand_tilde` for display.
pub fn contract_tilde(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}
