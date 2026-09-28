//! Persistence under `data_dir/terminals/<id>/`, all files 0600:
//! * `meta.json` — the `Record` (what the UI shows plus how to relaunch);
//! * `screen.bin` — the last screen snapshot (replayed into a fresh mirror on start);
//! * `claude-settings.json`, `mcp.json` — per-session Claude config (agents only).
//!
//! Writes are atomic (`util::fs`). Screens are saved per terminal when dirty, debounced,
//! never all at once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::TerminalInfo;

/// How an agent session is (re)launched. Never holds secrets.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentLaunch {
    /// `[agents.providers.<id>]` (`None`: `claude`, as before providers existed). The
    /// command, extra arguments and environment are read from config.toml at each launch.
    pub provider: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub remote_control: bool,
    pub add_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub info: TerminalInfo,
    #[serde(default)]
    pub launch: Option<AgentLaunch>,
    /// The user named the terminal: automatic titles no longer replace it.
    #[serde(default)]
    pub title_locked: bool,
    /// Transcript the session writes (Claude: from hooks; Codex: its rollout), for the
    /// tailer and resume.
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// The process was running when Workbench stopped (restore candidates).
    #[serde(default)]
    pub was_running: bool,
    /// Aider: its chat history file as it was when the session first started
    /// (`providers::aider_restores`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aider_history: Option<super::providers::FileMark>,
}

pub fn term_dir(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

/// The terminal's directory, created 0700 (it holds per-session tokens for agents).
pub fn ensure_dir(root: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let dir = term_dir(root, id);
    if !dir.is_dir() {
        std::fs::create_dir_all(&dir)?;
        crate::util::fs::set_mode(&dir, 0o700);
    }
    Ok(dir)
}

pub fn save_meta(root: &Path, rec: &Record) -> anyhow::Result<()> {
    crate::util::fs::write_json(&ensure_dir(root, &rec.info.id)?.join("meta.json"), rec)
}

pub fn save_screen(root: &Path, id: &str, data: &[u8]) -> anyhow::Result<()> {
    crate::util::fs::write_atomic(&ensure_dir(root, id)?.join("screen.bin"), data, 0o600)
}

/// Screens larger than this are not restored (a corrupt or hostile file).
const MAX_SCREEN: u64 = 32 * 1024 * 1024;

/// Every saved terminal with its last screen. Unreadable entries are skipped with a warning.
pub fn load_all(root: &Path) -> Vec<(Record, Option<Vec<u8>>)> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !super::valid_id(&name) {
            continue;
        }
        let rec: Record = match crate::util::fs::read_json(&dir.join("meta.json")) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!("skipping terminal {name}: {err:#}");
                continue;
            }
        };
        if rec.info.id != name {
            continue;
        }
        let screen_file = dir.join("screen.bin");
        let screen = match std::fs::metadata(&screen_file) {
            Ok(m) if m.len() <= MAX_SCREEN => std::fs::read(&screen_file).ok(),
            _ => None,
        };
        out.push((rec, screen));
    }
    out
}

pub fn remove(root: &Path, id: &str) {
    if super::valid_id(id) {
        let _ = std::fs::remove_dir_all(term_dir(root, id));
    }
}
