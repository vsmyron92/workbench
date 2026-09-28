//! Per-project code intelligence settings, `data_dir/lsp/<projectId>.json` (0600):
//! whether the user enabled it (language servers run project code, so nothing starts
//! before that), where servers run, and servers turned off for this project.
//!
//! Only the user writes this file (REST routes refuse in-process callers); repository
//! content never does.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::projects::Project;

/// Where a project's language servers run.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// In the dev container when it runs, the project uses it, and the server exists
    /// there; on the host otherwise.
    #[default]
    Auto,
    Host,
    Container,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Record {
    pub enabled: bool,
    /// The directory it was enabled for: an id that ever named another directory does
    /// not carry the approval over.
    pub root: String,
    pub enabled_at: i64,
    pub mode: Mode,
    /// Server ids turned off for this project.
    pub disabled_servers: Vec<String>,
}

#[derive(Default)]
pub struct TrustStore {
    cache: Mutex<HashMap<String, Record>>,
}

fn file(state: &AppState, pid: &str) -> PathBuf {
    state.paths.data_dir.join("lsp").join(format!("{pid}.json"))
}

impl TrustStore {
    pub fn get(&self, state: &AppState, pid: &str) -> Record {
        if let Some(r) = self.cache.lock().get(pid) {
            return r.clone();
        }
        let r: Record = crate::util::fs::read_json(&file(state, pid)).ok().flatten().unwrap_or_default();
        self.cache.lock().insert(pid.to_string(), r.clone());
        r
    }

    pub fn update(&self, state: &AppState, pid: &str, f: impl FnOnce(&mut Record)) -> anyhow::Result<Record> {
        let mut r = self.get(state, pid);
        f(&mut r);
        let path = file(state, pid);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            crate::util::fs::set_mode(dir, 0o700);
        }
        crate::util::fs::write_json(&path, &r)?;
        self.cache.lock().insert(pid.to_string(), r.clone());
        Ok(r)
    }

    /// Whether the user enabled code intelligence for this project (and this directory).
    pub fn enabled(&self, state: &AppState, project: &Project) -> bool {
        let r = self.get(state, &project.id);
        r.enabled && r.root == project.root.display().to_string()
    }
}
