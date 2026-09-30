//! Starting Workbench with the user's session on Windows (`workbench service`,
//! docs/windows-port.md §2): values under HKEY_CURRENT_USER (the `Run` value, and the state
//! Task Manager keeps for it), the Start Menu folder and shortcuts written by the shell's own
//! ShellLink object, starting a program apart from the caller, and a message box for a
//! program without a console.

use std::ffi::{OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_BINARY, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW,
    RegSetValueExW,
};
use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
use windows_sys::Win32::UI::Shell::{FOLDERID_Programs, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};

use super::win32::{Handle, reg_string, reg_value, wide};

/// `HKCU\<RUN_KEY>`: what Windows starts when the user signs in.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager (Startup apps) and Settings › Apps › Startup record a `Run` entry the
/// user turned off.
pub const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

// ---------------------------------------------------------------- registry (HKCU)

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key this value opened, closed only here.
        unsafe { RegCloseKey(self.0) };
    }
}

fn check(rc: WIN32_ERROR) -> io::Result<()> {
    if rc == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(rc as i32)) }
}

/// A string value (`REG_SZ`, or `REG_EXPAND_SZ` expanded) of `HKCU\<key>`; `None` when the
/// key or the value does not exist.
pub fn get_string(key: &str, name: &str) -> io::Result<Option<String>> {
    reg_string(HKEY_CURRENT_USER, key, Some(name), 0)
}

/// Sets the `REG_SZ` value `name` of `HKCU\<key>`, creating the key when it is missing.
pub fn set_string(key: &str, name: &str, value: &str) -> io::Result<()> {
    let (wkey, wname, data) = (wide(key), wide(name), wide(value));
    let mut h: HKEY = ptr::null_mut();
    // SAFETY: `wkey` is NUL-terminated; no class, security or disposition; `h` receives a key
    // that `Key` closes.
    check(unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, wkey.as_ptr(), 0, ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, ptr::null(), &mut h, ptr::null_mut())
    })?;
    let h = Key(h);
    // REG_SZ data counts its terminating NUL.
    let bytes = u32::try_from(data.len() * 2).map_err(|_| io::Error::other("value too long"))?;
    // SAFETY: an open key with KEY_SET_VALUE; `wname` is NUL-terminated; `data` holds `bytes` bytes.
    check(unsafe { RegSetValueExW(h.0, wname.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), bytes) })
}

/// Deletes the value `name` of `HKCU\<key>`. `Ok(false)` when there was none.
pub fn delete_value(key: &str, name: &str) -> io::Result<bool> {
    let (wkey, wname) = (wide(key), wide(name));
    // SAFETY: both names are NUL-terminated and outlive the call.
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wkey.as_ptr(), wname.as_ptr()) } {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        rc => Err(io::Error::from_raw_os_error(rc as i32)),
    }
}

/// Tests: deletes `HKCU\<key>` with everything under it.
#[cfg(test)]
pub fn delete_tree(key: &str) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;
    let wkey = wide(key);
    // SAFETY: `wkey` is NUL-terminated and outlives the call.
    match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wkey.as_ptr()) } {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
        rc => Err(io::Error::from_raw_os_error(rc as i32)),
    }
}

/// Tests: sets a `REG_BINARY` value, as Task Manager does under `APPROVED_KEY`.
#[cfg(test)]
pub fn set_binary(key: &str, name: &str, data: &[u8]) -> io::Result<()> {
    use windows_sys::Win32::System::Registry::REG_BINARY;
    let (wkey, wname) = (wide(key), wide(name));
    let mut h: HKEY = ptr::null_mut();
    // SAFETY: as in `set_string`.
    check(unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, wkey.as_ptr(), 0, ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, ptr::null(), &mut h, ptr::null_mut())
    })?;
    let h = Key(h);
    // SAFETY: an open key with KEY_SET_VALUE; `data` holds `data.len()` bytes.
    check(unsafe { RegSetValueExW(h.0, wname.as_ptr(), 0, REG_BINARY, data.as_ptr(), data.len() as u32) })
}

/// Whether the user turned the `Run` entry `name` off in Task Manager's Startup apps (or in
/// Settings › Apps › Startup): its value under `approved_key` (`APPROVED_KEY`, or a test's)
/// starts with an odd byte (2 and 6 mean on, 3 and 7 off; a missing value means on).
pub fn startup_disabled(approved_key: &str, name: &str) -> bool {
    matches!(reg_value(HKEY_CURRENT_USER, approved_key, Some(name), RRF_RT_REG_BINARY), Ok(Some(b)) if b.first().is_some_and(|x| x & 1 == 1))
}

// ---------------------------------------------------------------- Start Menu

/// The user's Start Menu programs folder (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`
/// unless redirected).
pub fn programs_dir() -> io::Result<PathBuf> {
    let mut p: PWSTR = ptr::null_mut();
    // SAFETY: a known folder id; no token (the current user); `p` receives a string that
    // CoTaskMemFree frees, which the documentation asks for whether the call succeeds or not.
    let rc = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT as u32, ptr::null_mut(), &mut p) };
    let out = if rc >= 0 && !p.is_null() {
        // SAFETY: a NUL-terminated string from the call.
        Ok(PathBuf::from(unsafe { os_from_wide(p) }))
    } else {
        Err(io::Error::from_raw_os_error(rc))
    };
    // SAFETY: null or the string the call allocated, freed once.
    unsafe { CoTaskMemFree(p.cast()) };
    out
}

/// # Safety
/// `p` points to a NUL-terminated string.
unsafe fn os_from_wide(p: *const u16) -> OsString {
    let mut len = 0;
    // SAFETY: the caller's promise: every unit up to the NUL is readable.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units were just read.
    OsString::from_wide(unsafe { std::slice::from_raw_parts(p, len) })
}

/// `p` NUL-terminated, as the shell link takes it: no `\\?\` prefix (links hold MAX_PATH
/// paths).
fn wide_os(p: &Path) -> io::Result<Vec<u16>> {
    let w: Vec<u16> = p.as_os_str().encode_wide().collect();
    if w.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL character"));
    }
    Ok(w.into_iter().chain([0]).collect())
}

/// What a shortcut runs, and its tooltip.
pub struct Shortcut<'a> {
    pub target: &'a Path,
    pub args: &'a str,
    pub workdir: &'a Path,
    pub description: &'a str,
}

/// IID_IShellLinkW, {000214F9-0000-0000-C000-000000000046}.
const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
/// IID_IPersistFile, {0000010B-0000-0000-C000-000000000046}.
const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

// The vtables mirror the C declarations whole; the methods not called are never read.

/// IUnknown's methods, which start every COM vtable.
#[repr(C)]
#[allow(dead_code)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// IShellLinkW's vtable (shobjidl_core.h), in declaration order; the methods not called
/// here are placeholders of pointer size.
#[repr(C)]
#[allow(dead_code)]
struct IShellLinkWVtbl {
    unknown: IUnknownVtbl,
    get_path: *const c_void,
    get_id_list: *const c_void,
    set_id_list: *const c_void,
    get_description: *const c_void,
    set_description: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
    get_working_directory: *const c_void,
    set_working_directory: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
    get_arguments: *const c_void,
    set_arguments: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
    get_hotkey: *const c_void,
    set_hotkey: *const c_void,
    get_show_cmd: *const c_void,
    set_show_cmd: *const c_void,
    get_icon_location: *const c_void,
    set_icon_location: *const c_void,
    set_relative_path: *const c_void,
    resolve: *const c_void,
    set_path: unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT,
}

/// IPersistFile's vtable (objidl.h): IUnknown, IPersist::GetClassID, then its own methods.
#[repr(C)]
#[allow(dead_code)]
struct IPersistFileVtbl {
    unknown: IUnknownVtbl,
    get_class_id: *const c_void,
    is_dirty: *const c_void,
    load: *const c_void,
    save: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
    save_completed: *const c_void,
    get_cur_file: *const c_void,
}

/// A COM interface pointer, released on drop.
struct Com(*mut c_void);

impl Com {
    /// The object's vtable, as `V`.
    ///
    /// # Safety
    /// The pointer is an interface whose vtable starts with `V`'s layout.
    unsafe fn vtbl<V>(&self) -> &V {
        // SAFETY: a COM object starts with its vtable pointer (the caller's promise for `V`).
        unsafe { &**(self.0 as *const *const V) }
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: a live interface pointer this value holds one reference to.
        unsafe { (self.vtbl::<IUnknownVtbl>().release)(self.0) };
    }
}

fn hr(hr: HRESULT) -> io::Result<()> {
    if hr >= 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(hr)) }
}

/// Writes the shortcut `lnk` (replacing one there) through the shell's ShellLink object, so
/// it is exactly what Explorer writes.
pub fn create_shortcut(lnk: &Path, s: &Shortcut) -> io::Result<()> {
    let (lnk, target, workdir) = (wide_os(lnk)?, wide_os(s.target)?, wide_os(s.workdir)?);
    let (args, description) = (wide(s.args), wide(s.description));
    // COM on a thread of its own, so whatever apartment the caller's thread has stays as is.
    let written = std::thread::spawn(move || {
        // SAFETY: COM is initialised on this new thread and uninitialised after every
        // interface pointer (the `Com` guards, dropped inside `write`) is released.
        unsafe {
            hr(CoInitializeEx(ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32))?;
            let result = write_link(&lnk, &target, &args, &workdir, &description);
            CoUninitialize();
            result
        }
    });
    written.join().map_err(|_| io::Error::other("the shortcut writer panicked"))?
}

/// # Safety
/// COM is initialised on this thread; the strings are NUL-terminated.
unsafe fn write_link(lnk: &[u16], target: &[u16], args: &[u16], workdir: &[u16], description: &[u16]) -> io::Result<()> {
    let mut p: *mut c_void = ptr::null_mut();
    // SAFETY: the ShellLink class and IShellLinkW's id; `p` receives the interface on success.
    hr(unsafe { CoCreateInstance(&ShellLink, ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELLLINKW, &mut p) })?;
    let link = Com(p);
    // SAFETY: `link` is an IShellLinkW; the strings outlive the calls, which copy them.
    unsafe {
        let v = link.vtbl::<IShellLinkWVtbl>();
        hr((v.set_path)(link.0, target.as_ptr()))?;
        hr((v.set_arguments)(link.0, args.as_ptr()))?;
        hr((v.set_working_directory)(link.0, workdir.as_ptr()))?;
        hr((v.set_description)(link.0, description.as_ptr()))?;
    }
    let mut p: *mut c_void = ptr::null_mut();
    // SAFETY: `link` is live; `p` receives an IPersistFile reference on success.
    hr(unsafe { (link.vtbl::<IUnknownVtbl>().query_interface)(link.0, &IID_IPERSISTFILE, &mut p) })?;
    let file = Com(p);
    // SAFETY: `file` is an IPersistFile; `lnk` is an absolute, NUL-terminated path; TRUE
    // makes it the object's file, as a save by the shell does.
    hr(unsafe { (file.vtbl::<IPersistFileVtbl>().save)(file.0, lnk.as_ptr(), 1) })
}

/// Whether the shortcut file `lnk` holds `text` among its strings: the shell writes a
/// link's description, arguments and working directory as UTF-16.
pub fn shortcut_mentions(lnk: &Path, text: &str) -> bool {
    let Ok(bytes) = std::fs::read(lnk) else { return false };
    let needle: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !needle.is_empty() && bytes.windows(needle.len()).any(|w| w == needle.as_slice())
}

// ---------------------------------------------------------------- processes

/// `program` and `args` as a command line that CommandLineToArgvW and the C runtime split
/// back into these words: each argument double-quoted when it is empty or has a space or a
/// tab, with the backslashes that end a quoted one doubled (one before the closing quote
/// would escape it). The program is always quoted (its name is read up to the closing quote,
/// backslashes and all). A word with a double quote or a NUL is refused.
pub fn command_line(program: &Path, args: &[&str]) -> io::Result<String> {
    let program = program.to_string_lossy();
    if std::iter::once(program.as_ref()).chain(args.iter().copied()).any(|w| w.contains(['"', '\0'])) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a double quote in a command-line word"));
    }
    let mut words = vec![format!("\"{program}\"")];
    for a in args {
        words.push(if a.is_empty() || a.contains([' ', '\t']) {
            let trailing = a.len() - a.trim_end_matches('\\').len();
            format!("\"{a}{}\"", "\\".repeat(trailing))
        } else {
            // Backslashes not followed by a quote are taken as they are.
            a.to_string()
        });
    }
    Ok(words.join(" "))
}

/// Starts `program args…` on its own and returns: it inherits no handles (a terminal's or
/// a pipe's would stay open while it runs), gets no console window (a console program gets
/// a hidden console of its own, so the caller's Ctrl-C never reaches it), and starts in
/// `cwd` (not the caller's folder, which it would keep from being deleted). Not in a new
/// process group: that would make it, and everything it starts, ignore Ctrl-C (terminals
/// included). It also leaves the caller's job when the job allows that: a terminal that ends
/// its job when it closes would end it too. `Ok(false)`: it had to stay in the caller's job.
pub fn start_detached(program: &Path, args: &[&str], cwd: Option<&Path>) -> io::Result<bool> {
    match create_detached(program, args, cwd, true) {
        Ok(()) => Ok(true),
        // A job without JOB_OBJECT_LIMIT_BREAKAWAY_OK refuses the breakaway (access denied):
        // start inside it instead.
        Err(_) => create_detached(program, args, cwd, false).map(|()| false),
    }
}

/// As `start_detached`, but only outside the caller's job: `Ok(false)`, with nothing
/// started, when the job does not let it leave (no `JOB_OBJECT_LIMIT_BREAKAWAY_OK`; a
/// Workbench terminal's job allows it), so it would end with the job.
pub fn start_apart(program: &Path, args: &[&str], cwd: Option<&Path>) -> io::Result<bool> {
    match create_detached(program, args, cwd, true) {
        Ok(()) => Ok(true),
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `start_detached`'s CreateProcessW; `breakaway` leaves the caller's job, and then fails
/// when the job does not allow it (outside a job the flag does nothing).
fn create_detached(program: &Path, args: &[&str], cwd: Option<&Path>, breakaway: bool) -> io::Result<()> {
    // CreateProcessW may write to the command line: a buffer of its own.
    let mut line = wide(&command_line(program, args)?);
    let app = wide_os(program)?;
    let cwd = cwd.map(wide_os).transpose()?;
    let flags = CREATE_NO_WINDOW | if breakaway { CREATE_BREAKAWAY_FROM_JOB } else { 0 };
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: NUL-terminated program, command line and folder that outlive the call;
    // default attributes; no inherited handles; the parent's environment; `si` and `pi`
    // are valid for the call, and the handles it returns are closed by `Handle`.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            flags,
            ptr::null(),
            cwd.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
            &si,
            &mut pi,
        )
    } != 0;
    if !ok {
        return Err(io::Error::last_os_error());
    }
    let _process = Handle::new(pi.hProcess);
    let _thread = Handle::new(pi.hThread);
    Ok(())
}

/// Whether this process runs elevated through UAC while the user's session does not: its
/// token is the elevated half of a split token, so what it starts runs as administrator,
/// unlike what Explorer, the Start Menu and the sign-in entry start. The built-in
/// Administrator, and administrators where UAC is off, have no split token: everything runs
/// with the full token there, and this is false.
pub fn elevated() -> bool {
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION_TYPE, TOKEN_QUERY, TokenElevationType};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = ptr::null_mut();
    // SAFETY: the current process's pseudo handle; `token` receives a handle `Handle` closes.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let Some(token) = Handle::new(token) else { return false };
    let mut t: TOKEN_ELEVATION_TYPE = 0;
    let mut len = 0u32;
    // SAFETY: a token with TOKEN_QUERY; `t` is this class's value (a TOKEN_ELEVATION_TYPE),
    // with its size.
    let ok = unsafe {
        GetTokenInformation(token.0, TokenElevationType, ptr::from_mut(&mut t).cast(), size_of::<TOKEN_ELEVATION_TYPE>() as u32, &mut len)
    };
    ok != 0 && split_elevated(t)
}

/// `TokenElevationTypeFull`: the elevated half of a split token. `Default` is a token without
/// a split (a standard user, the built-in Administrator, UAC off); `Limited` the other half.
fn split_elevated(t: windows_sys::Win32::Security::TOKEN_ELEVATION_TYPE) -> bool {
    t == windows_sys::Win32::Security::TokenElevationTypeFull
}

/// Starts `cmd`'s program without a console window: a console program gets a hidden
/// console of its own (`CREATE_NO_WINDOW`).
pub fn no_console_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Whether this process's windows reach a user: its window station is the interactive one,
/// and not in session 0, where services and what an SSH sign-in starts run and which no user
/// sees since Windows Vista.
pub fn interactive() -> bool {
    use windows_sys::Win32::System::StationsAndDesktops::{GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS};
    use windows_sys::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;
    if super::proc::session_of(std::process::id()).is_none_or(|s| s == 0) {
        return false;
    }
    let mut flags = USEROBJECTFLAGS::default();
    let mut needed = 0u32;
    // SAFETY: the process's own window station, a handle not to be closed (null on failure,
    // which fails the call); `flags` is writable for its size.
    let ok = unsafe {
        GetUserObjectInformationW(
            GetProcessWindowStation(),
            UOI_FLAGS,
            (&mut flags as *mut USEROBJECTFLAGS).cast(),
            size_of::<USEROBJECTFLAGS>() as u32,
            &mut needed,
        )
    } != 0;
    ok && flags.dwFlags & WSF_VISIBLE as u32 != 0
}

/// Shows `text` in a message box and waits for OK: for a program without a console
/// (`workbenchw`'s service) that has to tell the user something. Nothing where no one could
/// answer it (not [`interactive`]): the box would wait for good.
pub fn message_box(title: &str, text: &str, error: bool) {
    if !interactive() {
        return;
    }
    let (title, text) = (wide(title), wide(text));
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: no owner window; both strings are NUL-terminated and outlive the call.
    unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | icon | MB_SETFOREGROUND) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch key under HKCU\Software, deleted on drop.
    struct Scratch(String);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(&self.0);
        }
    }

    fn scratch() -> Scratch {
        Scratch(format!(r"Software\Workbench-test-{}", crate::util::random_token(8).replace(['-', '_'], "x")))
    }

    #[test]
    fn registry_values_round_trip() {
        let key = scratch();
        let run = format!(r"{}\Run", key.0);
        assert_eq!(get_string(&run, "Workbench").unwrap(), None, "no key yet");
        set_string(&run, "Workbench", r#""C:\Program Files\Workbench\workbenchw.exe""#).unwrap();
        assert_eq!(get_string(&run, "Workbench").unwrap().as_deref(), Some(r#""C:\Program Files\Workbench\workbenchw.exe""#));
        // Long values take the resize path.
        let long = "x".repeat(2000);
        set_string(&run, "Long", &long).unwrap();
        assert_eq!(get_string(&run, "Long").unwrap(), Some(long));
        assert!(delete_value(&run, "Workbench").unwrap());
        assert!(!delete_value(&run, "Workbench").unwrap());
        assert_eq!(get_string(&run, "Workbench").unwrap(), None);
    }

    /// Session 0 (services, SSH) has no desktop anyone sees, so no message box waits there.
    #[test]
    fn session_zero_is_not_interactive() {
        let session = crate::util::os::proc::session_of(std::process::id()).expect("this process's session");
        if session == 0 {
            assert!(!interactive());
        }
    }

    /// The reader desktop's App Paths lookup shares: a key's default value, in a registry view.
    #[test]
    fn default_values_are_read_in_a_view() {
        use windows_sys::Win32::System::Registry::RRF_SUBKEY_WOW6464KEY;
        let key = scratch();
        let app = format!(r"{}\App Paths\tool.exe", key.0);
        assert_eq!(reg_string(HKEY_CURRENT_USER, &app, None, RRF_SUBKEY_WOW6464KEY).unwrap(), None, "no key yet");
        set_string(&app, "Path", r"C:\Tools").unwrap();
        assert_eq!(reg_string(HKEY_CURRENT_USER, &app, None, RRF_SUBKEY_WOW6464KEY).unwrap(), None, "no default value yet");
        let long = format!(r"C:\Tools\{}\tool.exe", "d".repeat(600));
        set_string(&app, "", &long).unwrap();
        assert_eq!(reg_string(HKEY_CURRENT_USER, &app, None, RRF_SUBKEY_WOW6464KEY).unwrap(), Some(long));
        // A value of another type is an error, not a string.
        set_binary(&app, "Bin", &[1, 2, 3]).unwrap();
        assert!(reg_string(HKEY_CURRENT_USER, &app, Some("Bin"), 0).is_err());
    }

    #[test]
    fn task_manager_state_is_read() {
        let key = scratch();
        let approved = format!(r"{}\Approved", key.0);
        assert!(!startup_disabled(&approved, "Workbench"), "no value: on");
        set_binary(&approved, "Workbench", &[3, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert!(startup_disabled(&approved, "Workbench"));
        set_binary(&approved, "Workbench", &[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!(!startup_disabled(&approved, "Workbench"));
    }

    #[test]
    fn shortcuts_are_written_by_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let lnk = dir.path().join("Workbench.lnk");
        let target = std::env::current_exe().unwrap();
        let s = Shortcut { target: &target, args: "open", workdir: dir.path(), description: "A test link. Marker-7f3a" };
        create_shortcut(&lnk, &s).unwrap();
        assert!(lnk.is_file());
        assert!(shortcut_mentions(&lnk, "Marker-7f3a") && shortcut_mentions(&lnk, "open"));
        assert!(!shortcut_mentions(&lnk, "Not-there") && !shortcut_mentions(&dir.path().join("none.lnk"), "open"));
        // Replacing works.
        create_shortcut(&lnk, &Shortcut { description: "Another", ..s }).unwrap();
        assert!(shortcut_mentions(&lnk, "Another") && !shortcut_mentions(&lnk, "Marker-7f3a"));
        assert!(programs_dir().unwrap().is_absolute());
    }

    #[test]
    fn command_lines_quote_words() {
        let exe = Path::new(r"C:\Program Files\Workbench\workbench.exe");
        assert_eq!(command_line(exe, &[]).unwrap(), r#""C:\Program Files\Workbench\workbench.exe""#);
        assert_eq!(command_line(exe, &["service", "run", "a b"]).unwrap(), r#""C:\Program Files\Workbench\workbench.exe" service run "a b""#);
        // A quoted word's final backslashes are doubled; elsewhere they stay single.
        assert_eq!(
            command_line(exe, &[r"C:\My Data\", r"C:\data\", ""]).unwrap(),
            r#""C:\Program Files\Workbench\workbench.exe" "C:\My Data\\" C:\data\ """#
        );
        assert!(command_line(exe, &["x\"y"]).is_err());
    }

    #[test]
    fn only_the_elevated_half_of_a_split_token_counts() {
        use windows_sys::Win32::Security::{TokenElevationTypeDefault, TokenElevationTypeFull, TokenElevationTypeLimited};
        assert!(split_elevated(TokenElevationTypeFull));
        assert!(!split_elevated(TokenElevationTypeDefault), "no split: the built-in Administrator, UAC off, a standard user");
        assert!(!split_elevated(TokenElevationTypeLimited));
        // Whatever this runner's token is, it is read.
        let _ = elevated();
    }

    #[test]
    fn a_detached_program_starts() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\cmd.exe");
        let flag = dir.path().join("started");
        start_detached(&cmd, &["/c", "echo", "x>started"], Some(dir.path())).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !flag.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(flag.exists(), "it ran in `cwd`");
    }

    /// build.rs embeds `packaging/windows/workbench.manifest` in every executable, this test's
    /// included, and Windows applies it: with its `supportedOS`, GetVersionExW reports the
    /// real version (6.2, Windows 8, without it), and with its Common Controls dependency
    /// `comctl32.dll` loads in version 6 (5.82 without it), which [`message_box`] draws with.
    #[test]
    fn the_application_manifest_is_in_effect() {
        use windows_sys::Win32::Foundation::{FARPROC, FreeLibrary};
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
        use windows_sys::Win32::System::SystemInformation::{GetVersionExW, OSVERSIONINFOW};
        use windows_sys::Win32::UI::Shell::{DLLGETVERSIONPROC, DLLVERSIONINFO};

        let mut os = OSVERSIONINFOW { dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
        // SAFETY: `os` is writable and gives its size.
        assert_ne!(unsafe { GetVersionExW(&mut os) }, 0, "{}", io::Error::last_os_error());
        assert!(os.dwMajorVersion >= 10, "Windows reports {}.{}: no supportedOS in effect", os.dwMajorVersion, os.dwMinorVersion);

        let name = wide("comctl32.dll");
        // SAFETY: a NUL-terminated name; the activation context redirects it before System32
        // is searched.
        let lib = unsafe { LoadLibraryExW(name.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        assert!(!lib.is_null(), "{}", io::Error::last_os_error());
        // SAFETY: a loaded module and a NUL-terminated name.
        let export = unsafe { GetProcAddress(lib, c"DllGetVersion".as_ptr().cast()) };
        // SAFETY: comctl32's DllGetVersion has DLLGETVERSIONPROC's signature; both types are
        // optional function pointers.
        let get_version = unsafe { std::mem::transmute::<FARPROC, DLLGETVERSIONPROC>(export) }.expect("comctl32.dll exports DllGetVersion");
        let mut v = DLLVERSIONINFO { cbSize: size_of::<DLLVERSIONINFO>() as u32, ..Default::default() };
        // SAFETY: `v` is writable and gives its size; the module stays loaded until FreeLibrary.
        let hr = unsafe { get_version(&mut v) };
        // SAFETY: the module loaded above, released once.
        unsafe { FreeLibrary(lib) };
        assert_eq!(hr, 0);
        assert_eq!(v.dwMajorVersion, 6, "comctl32.dll {}.{}: no Common Controls 6 in effect", v.dwMajorVersion, v.dwMinorVersion);
    }
}
