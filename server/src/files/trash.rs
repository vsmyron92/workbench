//! Delete = move to the trash, never unlink. `gio trash` when available (it knows
//! every desktop's quirks), otherwise the freedesktop.org Trash specification:
//! the home trash when the file is on the same filesystem, else `$topdir/.Trash/$uid`
//! (if the admin created a sticky `.Trash`) or `$topdir/.Trash-$uid`.

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{ApiError, ApiResult};
use crate::util::os::perm;

/// Trash `path`. Returns which mechanism was used (`gio` or `trash-spec`).
pub async fn trash(path: &Path) -> ApiResult<&'static str> {
    if crate::util::which("gio") {
        let p = path.to_string_lossy().into_owned();
        let parent = path.parent().unwrap_or(Path::new("/")).to_path_buf();
        match crate::util::proc::run("gio", &["trash", "--", &p], &parent, Duration::from_secs(30)).await {
            Ok(out) if out.ok() => return Ok("gio"),
            Ok(out) => tracing::info!("gio trash failed ({}); using the trash spec directly", out.message()),
            Err(e) => tracing::info!("gio trash failed ({e}); using the trash spec directly"),
        }
    }
    let path = path.to_path_buf();
    super::blocking(move || {
        let home_trash = dirs::data_dir()
            .ok_or_else(|| ApiError::internal("no XDG data dir"))?
            .join("Trash");
        trash_spec(&path, &home_trash, nix::unistd::getuid().as_raw())
            .map(|_| "trash-spec")
            .map_err(|e| ApiError::internal(format!("cannot move {} to the trash: {e}", path.display())))
    })
    .await
}

/// Freedesktop trash for `path` (not following a final symlink). Returns the path
/// of the trashed item inside `…/files/`.
pub fn trash_spec(path: &Path, home_trash: &Path, uid: u32) -> std::io::Result<PathBuf> {
    let md = std::fs::symlink_metadata(path)?;
    let abs = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    let dev = md.dev();
    let (trash_dir, info_path) = if device_of_nearest(home_trash)? == dev {
        (home_trash.to_path_buf(), abs.to_string_lossy().into_owned())
    } else {
        let top = mount_top(&abs, dev);
        let admin = top.join(".Trash");
        let dir = match std::fs::symlink_metadata(&admin) {
            Ok(m) if m.is_dir() && m.mode() & 0o1000 != 0 => admin.join(uid.to_string()),
            _ => top.join(format!(".Trash-{uid}")),
        };
        let rel = abs.strip_prefix(&top).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| abs.to_string_lossy().into_owned());
        (dir, rel)
    };
    for sub in ["files", "info"] {
        perm::create_dir_private(&trash_dir.join(sub))?;
    }
    let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let encoded = percent_encoding::utf8_percent_encode(&info_path, PATH_SET).to_string();
    for n in 0..10_000u32 {
        let candidate = if n == 0 { name.clone() } else { numbered(&name, n + 1) };
        let info = trash_dir.join("info").join(format!("{candidate}.trashinfo"));
        let dest = trash_dir.join("files").join(&candidate);
        // The .trashinfo file is the lock on the name (spec: create it with O_EXCL first).
        let mut f = match perm::open_new(&info, 0o600, false) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        if std::fs::symlink_metadata(&dest).is_ok() {
            // A stray file without info: keep looking, leave the orphan alone.
            drop(f);
            let _ = std::fs::remove_file(&info);
            continue;
        }
        f.write_all(format!("[Trash Info]\nPath={encoded}\nDeletionDate={date}\n").as_bytes())?;
        f.sync_all()?;
        drop(f);
        if let Err(e) = std::fs::rename(&abs, &dest) {
            let _ = std::fs::remove_file(&info);
            return Err(e);
        }
        return Ok(dest);
    }
    Err(std::io::Error::other("no free name in the trash"))
}

/// Characters escaped in the `Path=` key (RFC 2396 path, `/` kept).
const PATH_SET: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// `report.pdf` → `report.2.pdf` (a unique name inside the trash).
fn numbered(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}.{n}{}", &name[..i], &name[i..]),
        _ => format!("{name}.{n}"),
    }
}

/// Device of `p` or of its nearest existing ancestor.
fn device_of_nearest(p: &Path) -> std::io::Result<u64> {
    let mut cur = Some(p);
    while let Some(c) = cur {
        if let Ok(m) = std::fs::metadata(c) {
            return Ok(m.dev());
        }
        cur = c.parent();
    }
    Err(std::io::Error::other("no existing ancestor"))
}

/// The top directory of the mount holding `abs` (highest ancestor on device `dev`).
fn mount_top(abs: &Path, dev: u64) -> PathBuf {
    let mut top = abs.parent().unwrap_or(abs).to_path_buf();
    while let Some(parent) = top.parent() {
        match std::fs::metadata(parent) {
            Ok(m) if m.dev() == dev => top = parent.to_path_buf(),
            _ => break,
        }
    }
    top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trashes_into_home_trash_with_info_and_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join("share/Trash");
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(proj.join("sub dir")).unwrap();
        std::fs::write(proj.join("a.txt"), "1").unwrap();
        std::fs::write(proj.join("sub dir/a.txt"), "2").unwrap();

        let d1 = trash_spec(&proj.join("a.txt"), &trash, 1000).unwrap();
        let d2 = trash_spec(&proj.join("sub dir/a.txt"), &trash, 1000).unwrap();
        assert_eq!(d1, trash.join("files/a.txt"));
        assert_eq!(d2, trash.join("files/a.2.txt"));
        assert!(!proj.join("a.txt").exists());
        assert_eq!(std::fs::read_to_string(&d2).unwrap(), "2");
        let info = std::fs::read_to_string(trash.join("info/a.2.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"), "{info}");
        assert!(info.contains("sub%20dir/a.txt"), "{info}");
        assert!(info.contains("DeletionDate="));

        // Directories move as a whole.
        let d3 = trash_spec(&proj.join("sub dir"), &trash, 1000).unwrap();
        assert!(d3.is_dir());
        assert!(!proj.join("sub dir").exists());
    }

    #[test]
    fn trashes_symlinks_not_their_targets() {
        let dir = tempfile::tempdir().unwrap();
        let trash = dir.path().join("Trash");
        std::fs::write(dir.path().join("target.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(dir.path().join("target.txt"), dir.path().join("link")).unwrap();
        trash_spec(&dir.path().join("link"), &trash, 1000).unwrap();
        assert!(dir.path().join("target.txt").exists());
        assert!(std::fs::symlink_metadata(trash.join("files/link")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("a.txt", 2), "a.2.txt");
        assert_eq!(numbered(".env", 3), ".env.3");
        assert_eq!(numbered("Makefile", 2), "Makefile.2");
    }
}
