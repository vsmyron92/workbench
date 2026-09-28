//! Atomic file writes and small JSON persistence helpers.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::Context;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Write via a temp file in the same directory, fsync, then rename, so readers
/// (and concurrent agents) never see a half-written file. `mode` applies to new files;
/// an existing file keeps its permissions.
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> anyhow::Result<()> {
    let dir = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let keep_mode = std::fs::metadata(path).ok().map(|m| m.permissions().mode());
    let tmp = dir.join(format!(
        ".{}.wb-tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        crate::util::random_token(6)
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.set_permissions(std::fs::Permissions::from_mode(keep_mode.unwrap_or(mode) & 0o7777))?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("write {}", path.display()))
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    write_atomic(path, &data, 0o600)
}

/// `Ok(None)` when the file does not exist.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn set_mode(path: &Path, mode: u32) {
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
