//! Per-project dev container state kept on this machine: `data_dir/devcontainer/<id>.json`
//! (0600). Approvals are sha256 hashes of an approved plan, never the plan itself.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    /// Config path → approved plan hash.
    pub approvals: BTreeMap<String, String>,
    /// Run terminals and runs in the container (`None`: yes once Workbench started it).
    pub use_container: Option<bool>,
    /// The config the user picked (several configs).
    pub config: Option<String>,
    /// Run configurations that always run on the host.
    pub host_runs: BTreeSet<String>,
    /// Workbench started or attached to the container (the bridge listener runs for it).
    pub attached: bool,
    pub last: Option<LastUp>,
}

/// What the last successful start reported.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LastUp {
    pub config: String,
    pub container_id: String,
    pub engine: String,
    pub remote_user: Option<String>,
    pub workspace_folder: Option<String>,
    pub compose_project: Option<String>,
    pub at: i64,
}

pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("devcontainer")
}

fn file(data_dir: &Path, pid: &str) -> PathBuf {
    dir(data_dir).join(format!("{pid}.json"))
}

/// Scratch files of one project's operations (scripts, env files), 0700.
pub fn work_dir(data_dir: &Path, pid: &str) -> std::io::Result<PathBuf> {
    let d = dir(data_dir).join(pid);
    std::fs::create_dir_all(&d)?;
    crate::util::fs::set_mode(&dir(data_dir), 0o700);
    crate::util::fs::set_mode(&d, 0o700);
    Ok(d)
}

pub fn load(data_dir: &Path, pid: &str) -> Saved {
    crate::util::fs::read_json::<Saved>(&file(data_dir, pid)).ok().flatten().unwrap_or_default()
}

pub fn save(data_dir: &Path, pid: &str, s: &Saved) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir(data_dir))?;
    crate::util::fs::set_mode(&dir(data_dir), 0o700);
    crate::util::fs::write_json(&file(data_dir, pid), s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path(), "app"), Saved::default());
        let mut s = Saved::default();
        s.approvals.insert(".devcontainer/devcontainer.json".into(), "ab".repeat(32));
        s.use_container = Some(false);
        save(d.path(), "app", &s).unwrap();
        assert_eq!(load(d.path(), "app"), s);
        let mode = std::fs::metadata(file(d.path(), "app")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
