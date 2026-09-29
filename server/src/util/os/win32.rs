//! Small Win32 helpers the areas' Windows code shares: owned handles and blocks, UTF-16
//! strings and paths for the `…W` functions, file identity, the user a process runs as, and
//! registry values.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Registry::{HKEY, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// An owned kernel handle, closed on drop.
pub struct Handle(pub HANDLE);

// SAFETY: a kernel handle is valid in every thread of the process, and the calls made
// on these (wait, terminate, query, set) are thread-safe.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

impl Handle {
    /// `None` for the failure results: null and `INVALID_HANDLE_VALUE`.
    pub fn new(h: HANDLE) -> Option<Handle> {
        (!h.is_null() && h != INVALID_HANDLE_VALUE).then_some(Handle(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned, valid and closed only here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Frees a block the system allocated with `LocalAlloc` (descriptors, strings) when dropped.
pub struct Local(pub *mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a LocalAlloc'ed block we own, freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// `s` NUL-terminated, for the `…W` functions.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A NUL-terminated UTF-16 string from a Win32 call.
///
/// # Safety
/// `p` points to a NUL-terminated string.
pub unsafe fn from_wide(p: *const u16) -> String {
    let mut len = 0;
    // SAFETY: the caller's promise: every unit up to the NUL is readable.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// Whether the UTF-16 string `w` starts with `s`.
pub fn starts_with(w: &[u16], s: &str) -> bool {
    s.encode_utf16().enumerate().all(|(i, c)| w.get(i) == Some(&c))
}

/// `p` for the `…W` functions: absolute (which also turns `/` into `\`),
/// NUL-terminated, and with the `\\?\` prefix when it is too long for MAX_PATH.
pub fn wide_path(p: &Path) -> io::Result<Vec<u16>> {
    let abs = std::path::absolute(p)?;
    let mut w: Vec<u16> = abs.as_os_str().encode_wide().collect();
    if w.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL character"));
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

/// Volume serial number and file index: equal for two names of one file (hard links
/// included). `follow`: of what a symlink points to, else of the entry itself.
pub fn file_id(p: &Path, follow: bool) -> io::Result<(u32, u32, u32)> {
    // No access rights are needed to read the id, as for `std::fs::metadata`; a directory too.
    let flags = FILE_FLAG_BACKUP_SEMANTICS | if follow { 0 } else { FILE_FLAG_OPEN_REPARSE_POINT };
    // Dropping `f` closes the handle.
    let f = OpenOptions::new().access_mode(0).custom_flags(flags).open(p)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `f` keeps the handle open for the call, and `info` is a valid out-pointer.
    if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Whether `a` and `b` are one file (`file_id`).
pub fn same_file(a: &Path, b: &Path, follow: bool) -> io::Result<bool> {
    Ok(file_id(a, follow)? == file_id(b, follow)?)
}

/// The user a process runs as: its `TOKEN_USER`, in a buffer of its own.
pub struct User(Vec<u64>);

impl User {
    /// The user of `process`, a handle with PROCESS_QUERY_LIMITED_INFORMATION (or the
    /// current process's pseudo handle).
    pub fn of(process: HANDLE) -> io::Result<User> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is a valid process handle; `token` receives a handle the guard closes.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut len: u32 = 0;
        // SAFETY: a size query: no buffer; it fails with ERROR_INSUFFICIENT_BUFFER and sets `len`.
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        // u64s: TOKEN_USER starts with a pointer.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        let size = (buf.len() * 8) as u32;
        // SAFETY: `buf` holds `size` bytes, aligned for TOKEN_USER.
        if unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), size, &mut len) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(User(buf))
    }

    /// The user of this process.
    pub fn current() -> io::Result<User> {
        // SAFETY: no preconditions; the pseudo handle needs no closing.
        User::of(unsafe { GetCurrentProcess() })
    }

    /// The user's SID, valid while `self` lives.
    pub fn sid(&self) -> PSID {
        // SAFETY: GetTokenInformation filled the buffer with a TOKEN_USER whose SID lies in it.
        unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
    }
}

/// A SID's string form (`S-1-5-21-…`).
///
/// # Safety
/// `sid` points to a valid SID.
pub unsafe fn sid_string(sid: PSID) -> Option<String> {
    let mut s: *mut u16 = std::ptr::null_mut();
    // SAFETY: a valid SID (the caller's promise); `s` receives a LocalAlloc'ed string that
    // `Local` frees.
    if unsafe { ConvertSidToStringSidW(sid, &mut s) } == 0 {
        return None;
    }
    let _free = Local(s.cast());
    // SAFETY: a NUL-terminated string from the call.
    Some(unsafe { from_wide(s) })
}

/// The value `name` of the registry key `root\key` (`None`: the key's default value),
/// restricted to the types in `flags` (`RRF_RT_…`, with an `RRF_SUBKEY_WOW64…` view where
/// it matters). `Ok(None)` when the key or the value does not exist.
pub fn reg_value(root: HKEY, key: &str, name: Option<&str>, flags: u32) -> io::Result<Option<Vec<u8>>> {
    let wkey = wide(key);
    let wname = name.map(wide);
    let name_ptr = wname.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut buf = vec![0u8; 512];
    for _ in 0..4 {
        let mut len = buf.len() as u32;
        // SAFETY: the key and the value name (or null, for the default value) are
        // NUL-terminated and outlive the call; `buf` is writable for `len` bytes.
        let rc = unsafe { RegGetValueW(root, wkey.as_ptr(), name_ptr, flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        match rc {
            ERROR_SUCCESS => {
                buf.truncate(len as usize);
                return Ok(Some(buf));
            }
            // `len` is the size needed now; the value may still grow before the next try.
            ERROR_MORE_DATA => buf = vec![0u8; len as usize + 64],
            ERROR_FILE_NOT_FOUND => return Ok(None),
            rc => return Err(io::Error::from_raw_os_error(rc as i32)),
        }
    }
    Err(io::Error::other("the registry value kept changing size"))
}

/// A string value (`REG_SZ`, or `REG_EXPAND_SZ` expanded) of `root\key` ([`reg_value`]), read
/// in the registry `view` (`RRF_SUBKEY_WOW64…`, or 0 for this process's own).
pub fn reg_string(root: HKEY, key: &str, name: Option<&str>, view: u32) -> io::Result<Option<String>> {
    let Some(bytes) = reg_value(root, key, name, RRF_RT_REG_SZ | view)? else { return Ok(None) };
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
    Ok(Some(String::from_utf16_lossy(&units[..end])))
}
