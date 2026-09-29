//! `fs`: renames that never replace, symlinks, and the trash (docs/windows-port.md §1.C,
//! §1.L). Creating a file without following a symlink is `perm::open_new`.
//!
//! Unix: `renameat2` (`RENAME_NOREPLACE`, `RENAME_EXCHANGE`) and the desktop trash:
//! `gio trash` when available, otherwise the freedesktop.org Trash specification.
//! Windows: `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` (it never replaces; there is
//! no atomic exchange), file or directory symlinks, and the Recycle Bin
//! (`SHFileOperationW`).

use std::io;
use std::path::Path;

use crate::error::ApiResult;

#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
use win as sys;

/// `rename(2)` that fails with `AlreadyExists` instead of replacing the destination.
/// A filesystem without an atomic way gets a check, then a rename (a small race, still
/// never silently); across filesystems it fails. Windows: a case-only rename of the same
/// file (`a.txt` → `A.txt`) is not refused.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    sys::rename_noreplace(from, to)
}

/// Only the atomic no-replace rename: `AlreadyExists` when `to` exists, and an error for
/// which [`rename_unsupported`] holds where the filesystem cannot do it, so the caller
/// picks its own fallback.
pub fn rename_noreplace_atomic(from: &Path, to: &Path) -> io::Result<()> {
    sys::rename_noreplace_atomic(from, to)
}

/// Swap `a` and `b` atomically (`RENAME_EXCHANGE`). Windows has no such rename: the
/// error satisfies [`rename_unsupported`], so the caller's fallback runs.
pub fn rename_exchange(a: &Path, b: &Path) -> io::Result<()> {
    sys::rename_exchange(a, b)
}

/// `e`, from one of the renames above, says the filesystem or the OS lacks that rename.
pub fn rename_unsupported(e: &io::Error) -> bool {
    sys::rename_unsupported(e)
}

/// Create `link` pointing at `target`. Windows: a directory or a file link by what
/// `target` is, seen from `link`'s folder (a missing target makes a file link); without
/// Developer Mode or an administrator it fails with a clear `PermissionDenied`.
#[cfg_attr(not(test), allow(dead_code))] // the tests' symlink; copies use `copy_symlink`
pub fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    sys::symlink(target.as_ref(), link.as_ref())
}

/// Recreate the symlink `from` at `to`: the same target (Windows: the same kind, file or
/// directory, even where the target does not resolve from `to`).
pub fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    sys::copy_symlink(from, to)
}

/// Move `path` to the trash, never unlinking it (a final symlink goes, not its target).
/// Returns what took it: `gio` or `trash-spec` (Unix: `gio trash` when available, it knows
/// every desktop's quirks, otherwise the freedesktop.org Trash specification: the home
/// trash on the same filesystem, else `$topdir/.Trash/$uid` if the admin created a sticky
/// `.Trash`, or `$topdir/.Trash-$uid`), or `recycle-bin` (Windows: refused where the bin
/// would delete for good: no bin on that drive, the bin turned off, a file larger than
/// it; should Windows still find it cannot recycle the item, it asks on the host's
/// desktop, and after a minute the request stops waiting with an error).
pub async fn trash(path: &Path) -> ApiResult<&'static str> {
    sys::trash(path).await
}

/// Only the desktop's own trash: `gio trash` on Unix (`Ok(false)` without gio or when it
/// exits non-zero, an error when it cannot run), the Recycle Bin on Windows (`Ok(false)`
/// when it refuses, an error while it is asking on the desktop, as in [`trash`]).
pub async fn desktop_trash(path: &Path) -> ApiResult<bool> {
    sys::desktop_trash(path).await
}

#[cfg(unix)]
mod unix {
    use std::io::{self, ErrorKind, Write};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use crate::error::{ApiError, ApiResult};

    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        let Err(err) = renameat2(from, to, libc::RENAME_NOREPLACE) else {
            return Ok(());
        };
        match err.raw_os_error() {
            // Filesystems without RENAME_NOREPLACE: check, then rename (small race, still never silently).
            Some(libc::EINVAL) | Some(libc::ENOSYS) => {
                if std::fs::symlink_metadata(to).is_ok() {
                    return Err(ErrorKind::AlreadyExists.into());
                }
                std::fs::rename(from, to)
            }
            Some(libc::EXDEV) => Err(io::Error::other("cannot move across filesystems")),
            _ => Err(err),
        }
    }

    pub fn rename_noreplace_atomic(from: &Path, to: &Path) -> io::Result<()> {
        renameat2(from, to, libc::RENAME_NOREPLACE)
    }

    pub fn rename_exchange(a: &Path, b: &Path) -> io::Result<()> {
        renameat2(a, b, libc::RENAME_EXCHANGE)
    }

    pub fn rename_unsupported(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP))
    }

    fn renameat2(from: &Path, to: &Path, flags: libc::c_uint) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let f = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let t = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: both pointers are valid NUL-terminated strings for the call's duration.
        let rc = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), flags) };
        if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    pub fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
    }

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
        tokio::task::spawn_blocking(move || {
            let home_trash = dirs::data_dir()
                .ok_or_else(|| ApiError::internal("no XDG data dir"))?
                .join("Trash");
            trash_spec(&path, &home_trash, nix::unistd::getuid().as_raw())
                .map(|_| "trash-spec")
                .map_err(|e| ApiError::internal(format!("cannot move {} to the trash: {e}", path.display())))
        })
        .await
        .map_err(|e| ApiError::internal(format!("background task failed: {e}")))?
    }

    pub async fn desktop_trash(path: &Path) -> ApiResult<bool> {
        if crate::util::which("gio") {
            let out = crate::util::proc::run("gio", &["trash", "--", &path.to_string_lossy()], Path::new("/"), Duration::from_secs(30)).await?;
            if out.ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Freedesktop trash for `path` (not following a final symlink). Returns the path
    /// of the trashed item inside `…/files/`.
    fn trash_spec(path: &Path, home_trash: &Path, uid: u32) -> io::Result<PathBuf> {
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
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(trash_dir.join(sub))?;
        }
        let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        let encoded = percent_encoding::utf8_percent_encode(&info_path, PATH_SET).to_string();
        for n in 0..10_000u32 {
            let candidate = if n == 0 { name.clone() } else { numbered(&name, n + 1) };
            let info = trash_dir.join("info").join(format!("{candidate}.trashinfo"));
            let dest = trash_dir.join("files").join(&candidate);
            // The .trashinfo file is the lock on the name (spec: create it with O_EXCL first).
            let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&info) {
                Ok(f) => f,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
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
        Err(io::Error::other("no free name in the trash"))
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
    fn device_of_nearest(p: &Path) -> io::Result<u64> {
        let mut cur = Some(p);
        while let Some(c) = cur {
            if let Ok(m) = std::fs::metadata(c) {
                return Ok(m.dev());
            }
            cur = c.parent();
        }
        Err(io::Error::other("no existing ancestor"))
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
        fn unsupported_renames_are_the_errnos_with_a_fallback() {
            for errno in [libc::EINVAL, libc::ENOSYS, libc::EOPNOTSUPP] {
                assert!(rename_unsupported(&io::Error::from_raw_os_error(errno)));
            }
            assert!(!rename_unsupported(&io::Error::from_raw_os_error(libc::EEXIST)));
            assert!(!rename_unsupported(&io::Error::from_raw_os_error(libc::EXDEV)));
        }

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
}

#[cfg(windows)]
mod win {
    use std::ffi::OsString;
    use std::fs::OpenOptions;
    use std::io::{self, ErrorKind};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{FileTypeExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_NOT_SAME_DEVICE, ERROR_PRIVILEGE_NOT_HELD, ERROR_SUCCESS,
        MAX_PATH,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetDriveTypeW, GetFileInformationByHandle,
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, MoveFileExW,
    };
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW,
    };

    use crate::error::{ApiError, ApiResult};

    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        match move_file(from, to) {
            Ok(()) => Ok(()),
            // Only the case changes and `to` is this very file: NTFS renames it in place,
            // a filesystem that refuses gets it in two steps.
            Err(e) if e.kind() == ErrorKind::AlreadyExists && case_only(from, to) => rename_via_temp(from, to),
            Err(e) if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE as i32) => Err(io::Error::other("cannot move across filesystems")),
            Err(e) => Err(e),
        }
    }

    pub fn rename_noreplace_atomic(from: &Path, to: &Path) -> io::Result<()> {
        move_file(from, to)
    }

    pub fn rename_exchange(_a: &Path, _b: &Path) -> io::Result<()> {
        Err(io::Error::new(ErrorKind::Unsupported, "Windows has no atomic rename exchange"))
    }

    pub fn rename_unsupported(e: &io::Error) -> bool {
        e.kind() == ErrorKind::Unsupported
    }

    /// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` (so `AlreadyExists` when `to`
    /// exists) and without `MOVEFILE_COPY_ALLOWED` (so never a copy across volumes).
    fn move_file(from: &Path, to: &Path) -> io::Result<()> {
        let (f, t) = (wide_path(from)?, wide_path(to)?);
        // SAFETY: both are NUL-terminated UTF-16 strings that live across the call.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error().map(|c| c as u32) {
            Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => Err(io::Error::new(ErrorKind::AlreadyExists, e)),
            _ => Err(e),
        }
    }

    /// `from` and `to` differ only in the case of the final name, and are one file.
    fn case_only(from: &Path, to: &Path) -> bool {
        let (Some(a), Some(b)) = (from.file_name(), to.file_name()) else { return false };
        a != b && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() && same_file(from, to).unwrap_or(false)
    }

    /// Volume serial number and file index: equal for two names of one file.
    fn file_id(p: &Path) -> io::Result<(u32, u32, u32)> {
        // No access rights are needed to read the id; the entry itself, a directory too.
        let f = OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(p)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `f` keeps the handle open for the call, and `info` is a valid out-pointer.
        if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    }

    fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
        Ok(file_id(a)? == file_id(b)?)
    }

    /// Rename through a free temporary name beside `from` (a case-only rename on a
    /// filesystem that refuses it); `from` is put back if the second step fails.
    fn rename_via_temp(from: &Path, to: &Path) -> io::Result<()> {
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = from.with_file_name(format!(".{name}.wb-case-{}", crate::util::random_token(6)));
        move_file(from, &tmp)?;
        move_file(&tmp, to).inspect_err(|_| {
            let _ = move_file(&tmp, from);
        })
    }

    /// `p` for the `…W` functions: absolute (which also turns `/` into `\`),
    /// NUL-terminated, and with the `\\?\` prefix when it is too long for MAX_PATH.
    fn wide_path(p: &Path) -> io::Result<Vec<u16>> {
        let abs = std::path::absolute(p)?;
        let mut w: Vec<u16> = abs.as_os_str().encode_wide().collect();
        if w.contains(&0) {
            return Err(io::Error::new(ErrorKind::InvalidInput, "path contains a NUL character"));
        }
        if w.len() >= 248 && !starts_with(&w, r"\\?\") && !starts_with(&w, r"\\.\") {
            if starts_with(&w, r"\\") {
                // \\server\share\… → \\?\UNC\server\share\…
                w.splice(..2, r"\\?\UNC\".encode_utf16());
            } else {
                w.splice(..0, r"\\?\".encode_utf16());
            }
        }
        w.push(0);
        Ok(w)
    }

    fn starts_with(w: &[u16], s: &str) -> bool {
        s.encode_utf16().enumerate().all(|(i, c)| w.get(i) == Some(&c))
    }

    /// `s` NUL-terminated, for the `…W` functions.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A UTF-16 buffer up to its first NUL.
    fn wide_str(w: &[u16]) -> String {
        String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
    }

    pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
        // A relative target resolves only with backslashes.
        let w: Vec<u16> = target.as_os_str().encode_wide().map(|c| if c == u16::from(b'/') { u16::from(b'\\') } else { c }).collect();
        let target = PathBuf::from(OsString::from_wide(&w));
        let seen = link.parent().unwrap_or(Path::new("")).join(&target);
        symlink_as(&target, link, std::fs::metadata(seen).is_ok_and(|m| m.is_dir()))
    }

    pub fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
        let dir = std::fs::symlink_metadata(from)?.file_type().is_symlink_dir();
        symlink_as(&std::fs::read_link(from)?, to, dir)
    }

    fn symlink_as(target: &Path, link: &Path, dir: bool) -> io::Result<()> {
        let made = if dir { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) };
        made.map_err(|e| {
            if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD as i32) {
                io::Error::new(
                    ErrorKind::PermissionDenied,
                    format!("cannot create the symbolic link {}: Windows allows it only with Developer Mode on or as an administrator", link.display()),
                )
            } else {
                e
            }
        })
    }

    /// How long a request waits for the Recycle Bin. Only an item Windows finds it cannot
    /// recycle takes longer: Windows is then asking on the desktop.
    const RECYCLE_WAIT: Duration = Duration::from_secs(60);

    pub async fn trash(path: &Path) -> ApiResult<&'static str> {
        recycle_waiting(path)
            .await?
            .map(|()| "recycle-bin")
            .map_err(|e| ApiError::internal(format!("cannot move {} to the Recycle Bin: {e}", path.display())))
    }

    pub async fn desktop_trash(path: &Path) -> ApiResult<bool> {
        match recycle_waiting(path).await? {
            Ok(()) => Ok(true),
            Err(e) => {
                tracing::info!("cannot move {} to the Recycle Bin: {e}", path.display());
                Ok(false)
            }
        }
    }

    /// [`recycle`] on a blocking thread, waited on for at most [`RECYCLE_WAIT`]. Past that,
    /// Windows is asking on the desktop whether to delete the item for good, and may still
    /// do so once answered: that is an error, not a refusal, so no fallback moves the item
    /// meanwhile. The thread stays with the question until it is answered.
    async fn recycle_waiting(path: &Path) -> ApiResult<io::Result<()>> {
        let p = path.to_path_buf();
        match tokio::time::timeout(RECYCLE_WAIT, tokio::task::spawn_blocking(move || recycle(&p))).await {
            Ok(done) => done.map_err(|e| ApiError::internal(format!("background task failed: {e}"))),
            Err(_) => Err(ApiError::conflict(format!(
                "Windows is asking on this computer whether to delete {} for good (the Recycle Bin cannot take it); answer there",
                path.display()
            ))),
        }
    }

    /// Move `path` to the Recycle Bin. Only local fixed drives have one; elsewhere
    /// FO_DELETE would delete for good, so it is refused, as is what the bin is known not
    /// to take ([`bin_takes`]). Should Windows still find it cannot recycle the item (a
    /// folder larger than the bin), it asks on the desktop (FOF_WANTNUKEWARNING) instead
    /// of deleting silently.
    fn recycle(path: &Path) -> io::Result<()> {
        let mut from = shell_path(path)?;
        let mut root = vec![0u16; from.len() + 1];
        // SAFETY: `from` is NUL-terminated and `root` has room for `root.len()` characters.
        if unsafe { GetVolumePathNameW(from.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetVolumePathNameW left a NUL-terminated root in `root`.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
            return Err(io::Error::other(format!("{} has no Recycle Bin", wide_str(&root))));
        }
        bin_takes(&root, &std::fs::symlink_metadata(path)?)?;
        // `pFrom` is a list of NUL-terminated names that ends with an empty one.
        from.push(0);
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING) as u16,
            ..Default::default()
        };
        // SAFETY: `op` is initialised, `pFrom` is double-NUL-terminated and outlives the
        // call, and FO_DELETE reads no `pTo`.
        let rc = unsafe { SHFileOperationW(&mut op) };
        if rc != 0 {
            // Below 0x71 the result is a Win32 error; from there on, the shell's DE_* codes.
            return Err(if (1..0x71).contains(&rc) { io::Error::from_raw_os_error(rc) } else { io::Error::other(format!("SHFileOperation error {rc:#x}")) });
        }
        if op.fAnyOperationsAborted != 0 {
            return Err(io::Error::other("cancelled"));
        }
        Ok(())
    }

    /// `path` as `SHFileOperationW` takes it: full, without the `\\?\` prefix, shorter
    /// than MAX_PATH, NUL-terminated. A name with a character Windows never allows in one
    /// is refused: the shell reads `*` and `?` (and `<`, `>`, `"`) as wildcards, so such a
    /// name could delete other files.
    fn shell_path(path: &Path) -> io::Result<Vec<u16>> {
        let abs = std::path::absolute(path)?;
        let plain = dunce::simplified(&abs);
        for c in plain.components() {
            if let Component::Normal(name) = c {
                if name.encode_wide().any(|c| c < 32 || br#"<>:"|?*"#.iter().any(|&b| c == u16::from(b))) {
                    return Err(io::Error::new(ErrorKind::InvalidInput, format!("{name:?} is not a name Windows allows")));
                }
            }
        }
        let mut w: Vec<u16> = plain.as_os_str().encode_wide().collect();
        if w.contains(&0) {
            return Err(io::Error::new(ErrorKind::InvalidInput, "path contains a NUL character"));
        }
        if w.len() >= MAX_PATH as usize {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                format!("the path is too long for the Recycle Bin ({} characters, at most {})", w.len(), MAX_PATH - 1),
            ));
        }
        // dunce keeps the prefix only where the path cannot do without it.
        if starts_with(&w, r"\\?\") || starts_with(&w, r"\\.\") {
            return Err(io::Error::new(ErrorKind::InvalidInput, format!("the Recycle Bin cannot take {}", plain.display())));
        }
        w.push(0);
        Ok(w)
    }

    /// Refuse, before the shell would ask on the desktop, what the Recycle Bin of the
    /// volume at `root` does not take: anything when the bin is turned off (the volume's
    /// "Don't move files to the Recycle Bin", or the NoRecycleFiles policy), or a file
    /// larger than the bin. A folder larger than the bin is not seen here.
    fn bin_takes(root: &[u16], md: &std::fs::Metadata) -> io::Result<()> {
        const POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
        if [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER].into_iter().any(|k| reg_dword(k, POLICY, "NoRecycleFiles") == Some(1)) {
            return Err(io::Error::other("a policy turns the Recycle Bin off"));
        }
        let Some(guid) = volume_guid(root) else { return Ok(()) };
        let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
        if reg_dword(HKEY_CURRENT_USER, &key, "NukeOnDelete") == Some(1) {
            return Err(io::Error::other(format!("the Recycle Bin is turned off on {}", wide_str(root))));
        }
        match reg_dword(HKEY_CURRENT_USER, &key, "MaxCapacity") {
            Some(mb) if md.is_file() && md.len() > u64::from(mb) << 20 => {
                Err(io::Error::other(format!("the file is larger than the Recycle Bin on {} ({mb} MB)", wide_str(root))))
            }
            _ => Ok(()),
        }
    }

    /// The `{…}` of the volume mounted at `root` (its name is `\\?\Volume{…}\`).
    fn volume_guid(root: &[u16]) -> Option<String> {
        let mut name = [0u16; 64];
        // SAFETY: `root` is NUL-terminated and `name` has room for `name.len()` characters.
        if unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32) } == 0 {
            return None;
        }
        let name = wide_str(&name);
        let (start, end) = (name.find('{')?, name.find('}')?);
        (start < end).then(|| name[start..=end].to_owned())
    }

    /// A REG_DWORD value; `None` when it is missing or of another type.
    fn reg_dword(key: HKEY, sub: &str, value: &str) -> Option<u32> {
        let (sub, value) = (wide(sub), wide(value));
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: both names are NUL-terminated, `data` is a valid out-buffer of `size` bytes,
        // and no type is asked for (null).
        let rc = unsafe {
            RegGetValueW(key, sub.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut data).cast(), &mut size)
        };
        (rc == ERROR_SUCCESS).then_some(data)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_recycle_bin_gets_no_wildcards_or_long_paths() {
            for name in ["*", "a?.txt", "a<b", "x>", "\"q\"", "a|b", "file.txt:stream", "tab\there"] {
                let e = shell_path(&Path::new(r"C:\proj").join(name)).unwrap_err();
                assert_eq!(e.kind(), ErrorKind::InvalidInput, "{name}");
            }
            assert!(shell_path(&Path::new(r"C:\").join("x".repeat(300))).is_err());
            assert_eq!(shell_path(Path::new(r"C:/proj/sub/../a b.txt")).unwrap(), wide(r"C:\proj\a b.txt"));
            assert_eq!(shell_path(Path::new(r"\\?\C:\proj\a.txt")).unwrap(), wide(r"C:\proj\a.txt"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// Windows without Developer Mode or an administrator cannot create symlinks: the
    /// test that needs one skips.
    fn link_or_skip(target: &str, link: &Path) -> bool {
        match symlink(target, link) {
            Ok(()) => true,
            Err(e) if cfg!(windows) && e.kind() == ErrorKind::PermissionDenied => {
                eprintln!("skipped: {e}");
                false
            }
            Err(e) => panic!("symlink {}: {e}", link.display()),
        }
    }

    #[test]
    fn rename_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert_eq!(rename_noreplace(&a, &b).unwrap_err().kind(), ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        rename_noreplace(&a, &dir.path().join("c")).unwrap();
        assert!(!a.exists());
    }

    #[test]
    fn rename_may_change_only_the_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        rename_noreplace(&dir.path().join("a.txt"), &dir.path().join("A.txt")).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["A.txt"]);
        assert_eq!(std::fs::read_to_string(dir.path().join("A.txt")).unwrap(), "a");
    }

    #[test]
    fn atomic_renames_never_replace_and_exchange_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        match rename_noreplace_atomic(&a, &b) {
            Err(e) if rename_unsupported(&e) => {}
            other => assert_eq!(other.unwrap_err().kind(), ErrorKind::AlreadyExists),
        }
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        match rename_exchange(&a, &b) {
            Ok(()) => {
                assert_eq!(std::fs::read_to_string(&a).unwrap(), "b");
                assert_eq!(std::fs::read_to_string(&b).unwrap(), "a");
            }
            Err(e) => assert!(rename_unsupported(&e), "{e}"),
        }
        #[cfg(windows)]
        assert!(rename_exchange(&a, &b).is_err_and(|e| rename_unsupported(&e)));
        rename_noreplace_atomic(&a, &dir.path().join("c")).unwrap();
        assert!(!a.exists());
    }

    #[test]
    fn symlinks_and_their_copies_keep_target_and_kind() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("inner/sub")).unwrap();
        std::fs::write(dir.path().join("inner/x.sh"), "echo").unwrap();
        let (file_link, dir_link) = (dir.path().join("file-link"), dir.path().join("dir-link"));
        if !link_or_skip("inner/x.sh", &file_link) || !link_or_skip("inner/sub", &dir_link) {
            return;
        }
        assert_eq!(std::fs::read_to_string(&file_link).unwrap(), "echo");
        assert!(std::fs::metadata(&dir_link).unwrap().is_dir());
        // Copies elsewhere keep the target as it was, even where it no longer resolves.
        std::fs::create_dir(dir.path().join("copy")).unwrap();
        for link in [&file_link, &dir_link] {
            let to = dir.path().join("copy").join(link.file_name().unwrap());
            copy_symlink(link, &to).unwrap();
            assert!(std::fs::symlink_metadata(&to).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read_link(&to).unwrap(), std::fs::read_link(link).unwrap());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            let kind = |p: &Path| std::fs::symlink_metadata(p).unwrap().file_type();
            assert!(kind(&dir.path().join("copy/dir-link")).is_symlink_dir());
            assert!(kind(&dir.path().join("copy/file-link")).is_symlink_file());
        }
    }

    /// The Recycle Bin tests use the real bin of whoever runs them: only on CI, or when
    /// asked for with WORKBENCH_TEST_RECYCLE_BIN.
    #[cfg(windows)]
    fn recycle_bin_or_skip() -> bool {
        let wanted = std::env::var_os("CI").is_some() || std::env::var_os("WORKBENCH_TEST_RECYCLE_BIN").is_some();
        if !wanted {
            eprintln!("skipped: uses the real Recycle Bin (set WORKBENCH_TEST_RECYCLE_BIN to run it)");
        }
        wanted
    }

    /// `trash(path)`, `false` where the drive has no Recycle Bin.
    #[cfg(windows)]
    async fn recycled(path: &Path) -> bool {
        match trash(path).await {
            Ok(with) => {
                assert_eq!(with, "recycle-bin");
                true
            }
            Err(e) if e.message.contains("has no Recycle Bin") => {
                eprintln!("skipped: {}", e.message);
                false
            }
            Err(e) => panic!("{}", e.message),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn trash_moves_to_the_recycle_bin() {
        if !recycle_bin_or_skip() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("workbench-recycle-test.txt");
        std::fs::write(&f, "x").unwrap();
        if recycled(&f).await {
            assert!(!f.exists());
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn trash_recycles_links_not_their_targets() {
        if !recycle_bin_or_skip() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "keep").unwrap();
        // A junction needs no privilege; a directory symlink may.
        let junction = dir.path().join("junction");
        let made = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(&junction).arg(&target).output().unwrap();
        assert!(made.status.success(), "mklink /J: {}{}", String::from_utf8_lossy(&made.stdout), String::from_utf8_lossy(&made.stderr));
        let mut links = vec![junction];
        let dir_link = dir.path().join("dir-link");
        if link_or_skip("target", &dir_link) {
            links.push(dir_link);
        }
        for link in links {
            if !recycled(&link).await {
                return;
            }
            assert!(std::fs::symlink_metadata(&link).is_err(), "{} is still there", link.display());
            assert_eq!(std::fs::read_to_string(target.join("keep.txt")).unwrap(), "keep");
        }
    }
}
