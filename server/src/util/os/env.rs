//! The environment a new sign-in of this user gets, where it differs from this process's.
//!
//! On Windows installers change the `Environment` registry keys (HKLM's and HKCU's) and
//! nothing else, so a running program keeps the variables it started with: Workbench would
//! not find a program installed since, and neither would what it starts. Windows builds the
//! sign-in environment with `CreateEnvironmentBlock`, which Workbench calls too: for the
//! `PATH` of its terminals, the folders its lookups try after a miss (`exe::which`) and the
//! `PATH` of the programs it starts by itself (`exe::program_env`), and for the environment
//! `workbench service` starts its supervisor with. Unix has no such
//! split (a login shell's profile is not something a running program can read back):
//! nothing here applies there, and terminals and services keep this process's environment.

use std::ffi::OsString;
#[cfg(windows)]
pub(super) use win::{block, path_added};

/// This user's environment as a new sign-in gets it, not this process's: Windows builds it
/// from the `Environment` keys of HKLM and HKCU (expanded; `Path` the system's then the
/// user's) and the variables it sets per user (`USERPROFILE`, `APPDATA`…). Read again once
/// either key changes. `None` on Unix, and when Windows cannot build it.
#[cfg_attr(unix, allow(dead_code))] // `workbench service` on Windows
pub fn user_default() -> Option<Vec<(OsString, OsString)>> {
    #[cfg(unix)]
    {
        None
    }
    #[cfg(windows)]
    {
        win::with_default(<[_]>::to_vec)
    }
}

/// The `PATH` of a terminal, when it is not this process's: on Windows the `Path` of
/// [`user_default`] (programs installed since Workbench started), then the absolute entries
/// of this process's `PATH` that it lacks (a virtual environment or a folder Workbench was
/// started with, after the sign-in's). `None` on Unix, where terminals keep this process's
/// `PATH`, and when Windows cannot build the sign-in environment.
pub fn fresh_path() -> Option<String> {
    #[cfg(unix)]
    {
        None
    }
    #[cfg(windows)]
    {
        let fresh = win::with_default(|vars| win::path_of(vars).map(|p| p.to_string_lossy().into_owned()))??;
        let own = std::env::var_os("PATH").unwrap_or_default();
        Some(merge_path(&fresh, &own.to_string_lossy()))
    }
}

/// The `PATH` of a program Workbench starts by itself outside a terminal (a language server,
/// a debug adapter: `exe::program_env`), when it is not this process's: on Windows this
/// process's `PATH`, then the folders of [`user_default`]'s `Path` it lacks, the ones
/// `exe::which` tries after a miss. A program found there (gopls, installed since Workbench
/// started) then finds what it runs in turn (`go`), and what the program found before still
/// comes first. `None` when a new sign-in adds no folder, on Unix, and when Windows cannot
/// build the sign-in environment: the program keeps this process's `PATH`.
pub fn child_path() -> Option<String> {
    #[cfg(unix)]
    {
        None
    }
    #[cfg(windows)]
    {
        let fresh = win::with_default(|vars| win::path_of(vars).map(|p| p.to_string_lossy().into_owned()))??;
        extend_path(&std::env::var_os("PATH").unwrap_or_default().to_string_lossy(), &fresh)
    }
}

/// The entries of a Windows `PATH` (`;`-separated; a `;` inside `"…"` is part of its entry).
#[cfg(any(windows, test))]
fn entries(path: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut start, mut quoted) = (0, false);
    for (i, c) in path.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&path[start..]);
    out
}

/// A `PATH` entry as Windows compares folders: without quotes, case or trailing separators,
/// `/` as `\`.
#[cfg(any(windows, test))]
fn path_key(entry: &str) -> String {
    let mut k = entry.trim().replace('"', "").replace('/', "\\").to_lowercase();
    // `c:\` keeps its separator: `c:` is the drive's current folder.
    while k.len() > 3 && k.ends_with('\\') {
        k.pop();
    }
    k
}

/// A Windows path with a drive and a root (`C:\…`) or a UNC one (`\\server\share`): what
/// `Path::is_absolute` accepts there. `\usr\bin` (on whatever drive the current folder is)
/// and `bin` (in the current folder) are not.
#[cfg(any(windows, test))]
fn windows_absolute(entry: &str) -> bool {
    let e = entry.trim().replace('"', "");
    let b = e.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')) || e.starts_with(r"\\")
}

/// `fresh`'s entries, then the absolute ones of `own` that it lacks, joined by `;` (Windows
/// `PATH`s). A relative entry of `own` is left out, as Workbench's own lookups skip it (it
/// would find programs in the terminal's folder, a repository). Entries compare as
/// [`path_key`]s; empty ones and repeats go, the first of each kept.
#[cfg(any(windows, test))]
fn merge_path(fresh: &str, own: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let fresh = entries(fresh).into_iter().map(|e| (e, true));
    let own = entries(own).into_iter().map(|e| (e, windows_absolute(e)));
    fresh.chain(own).filter(|&(e, keep)| keep && !e.trim().is_empty() && seen.insert(path_key(e))).map(|(e, _)| e).collect::<Vec<_>>().join(";")
}

/// `own` as it is, then the absolute entries of `fresh` whose folders it lacks, each once
/// (Windows `PATH`s; entries compare as [`path_key`]s). `None` when `fresh` adds none. A
/// relative entry of `fresh` is left out, as by [`merge_path`]; `own`'s stay, since a
/// program started with `own` has them today.
#[cfg(any(windows, test))]
fn extend_path(own: &str, fresh: &str) -> Option<String> {
    let mut seen: std::collections::HashSet<String> = entries(own).into_iter().map(path_key).collect();
    let added: Vec<&str> = entries(fresh).into_iter().filter(|e| windows_absolute(e) && seen.insert(path_key(e))).collect();
    if added.is_empty() {
        return None;
    }
    let sep = if own.is_empty() || own.ends_with(';') { "" } else { ";" };
    Some(format!("{own}{sep}{}", added.join(";")))
}

#[cfg(windows)]
mod win {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString, c_void};
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use std::ptr;
    use std::sync::{Mutex, PoisonError};

    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::{TOKEN_DUPLICATE, TOKEN_QUERY};
    use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_THREAD_AGNOSTIC, RegNotifyChangeKeyValue,
        RegOpenKeyExW,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject};

    use super::super::win32::{Handle, Key, wide};

    /// Every user's variables (HKLM), then this user's (HKCU).
    const KEYS: [(HKEY, &str); 2] =
        [(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"), (HKEY_CURRENT_USER, "Environment")];

    /// A registry key whose next change of values signals `event` (auto-reset).
    struct Watched {
        key: Key,
        event: Handle,
    }

    impl Watched {
        fn open(root: HKEY, path: &str) -> Option<Watched> {
            let (wpath, mut h) = (wide(path), ptr::null_mut());
            // SAFETY: the path is NUL-terminated and outlives the call; `h` receives a key
            // that `Key` closes.
            if unsafe { RegOpenKeyExW(root, wpath.as_ptr(), 0, KEY_NOTIFY, &mut h) } != ERROR_SUCCESS {
                return None;
            }
            let key = Key(h);
            // SAFETY: an unnamed auto-reset event, not signalled, default security.
            let event = Handle::new(unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) })?;
            let w = Watched { key, event };
            w.arm().then_some(w)
        }

        /// Asks for the next change of the key's values (one signal per call). Not tied to
        /// this thread, which may end first (`REG_NOTIFY_THREAD_AGNOSTIC`, Windows 8 on).
        fn arm(&self) -> bool {
            // SAFETY: a key opened with KEY_NOTIFY and an event, both live while `self` is.
            let rc = unsafe {
                RegNotifyChangeKeyValue(self.key.0, 0, REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC, self.event.0, 1)
            };
            rc == ERROR_SUCCESS
        }

        /// Whether the key changed since it was last armed (the wait resets the event), armed
        /// again when it did. `None` when it could not be armed again.
        fn changed(&self) -> Option<bool> {
            // SAFETY: an event handle this value owns; a zero timeout only tests it.
            if unsafe { WaitForSingleObject(self.event.0, 0) } != WAIT_OBJECT_0 {
                return Some(false);
            }
            self.arm().then_some(true)
        }
    }

    struct Cache {
        /// Both `KEYS`, armed before each read. `None`: they cannot be watched, so every call
        /// reads.
        watch: Option<Vec<Watched>>,
        /// The last read, `None` before one succeeds.
        vars: Option<Vec<(OsString, OsString)>>,
    }

    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

    /// `f` of the sign-in environment (`super::user_default`), read again only after one of
    /// `KEYS` changed.
    pub(super) fn with_default<R>(f: impl FnOnce(&[(OsString, OsString)]) -> R) -> Option<R> {
        let mut guard = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        let c = guard.get_or_insert_with(|| Cache { watch: KEYS.iter().map(|&(root, path)| Watched::open(root, path)).collect(), vars: None });
        // Every key is tested, so each one that changed is armed again.
        let changed = c.watch.as_ref().map(|w| w.iter().map(Watched::changed).collect::<Option<Vec<bool>>>());
        let unchanged = match changed {
            Some(Some(changed)) => !changed.contains(&true),
            Some(None) => {
                c.watch = None;
                false
            }
            None => false,
        };
        if !unchanged || c.vars.is_none() {
            c.vars = read();
        }
        c.vars.as_deref().map(f)
    }

    /// `CreateEnvironmentBlock` for this process's user, inheriting nothing of this process's
    /// environment.
    fn read() -> Option<Vec<(OsString, OsString)>> {
        let mut token = ptr::null_mut();
        // SAFETY: the current process's pseudo handle; `token` receives a handle `Handle` closes.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut token) } == 0 {
            return None;
        }
        let token = Handle::new(token)?;
        let mut block: *mut c_void = ptr::null_mut();
        // SAFETY: a primary token with the access the call asks for (query, duplicate);
        // FALSE: nothing from this process's environment; `block` receives a block that
        // DestroyEnvironmentBlock frees.
        if unsafe { CreateEnvironmentBlock(&mut block, token.0, 0) } == 0 || block.is_null() {
            return None;
        }
        // SAFETY: a Unicode environment block from the call.
        let vars = unsafe { block_vars(block.cast()) };
        // SAFETY: the block the call allocated, freed once.
        unsafe { DestroyEnvironmentBlock(block) };
        Some(vars)
    }

    /// # Safety
    /// `p` points to an environment block: NUL-terminated strings, then an empty one.
    pub(super) unsafe fn block_vars(p: *const u16) -> Vec<(OsString, OsString)> {
        // SAFETY: the caller's promise: the block's first unit is readable.
        if unsafe { *p } == 0 {
            return vec![];
        }
        let mut len = 1;
        // SAFETY: the caller's promise: every unit up to the empty string's NUL is readable,
        // and a string's NUL is followed by the next string or that NUL.
        while unsafe { *p.add(len - 1) != 0 || *p.add(len) != 0 } {
            len += 1;
        }
        // SAFETY: `len + 1` units were just read.
        vars_of(unsafe { std::slice::from_raw_parts(p, len + 1) })
    }

    /// The variables of an environment block's `NAME=value` strings; a name may start with
    /// `=` (`=C:`, a drive's current folder).
    pub(super) fn vars_of(block: &[u16]) -> Vec<(OsString, OsString)> {
        block
            .split(|&c| c == 0)
            .take_while(|s| !s.is_empty())
            .filter_map(|s| {
                let eq = 1 + s.iter().skip(1).position(|&c| c == u16::from(b'='))?;
                Some((OsString::from_wide(&s[..eq]), OsString::from_wide(&s[eq + 1..])))
            })
            .collect()
    }

    /// `vars` as CreateProcessW's Unicode environment block (`CREATE_UNICODE_ENVIRONMENT`):
    /// `NAME=value` strings sorted by name without regard to case, as Windows asks, each
    /// ended by a NUL, then an empty one. Of names that differ in case only, the last one's
    /// value is kept. A name that is empty or holds `=` past its first character, and a NUL
    /// in a name or value, are refused.
    pub fn block(vars: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
        let upper = |k: &[u16]| k.iter().map(|&c| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }).collect::<Vec<u16>>();
        let mut sorted: BTreeMap<Vec<u16>, (Vec<u16>, Vec<u16>)> = BTreeMap::new();
        for (k, v) in vars {
            let (k, v): (Vec<u16>, Vec<u16>) = (k.encode_wide().collect(), v.encode_wide().collect());
            if k.is_empty() || k[1..].contains(&u16::from(b'=')) || k.contains(&0) || v.contains(&0) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{:?} cannot be an environment variable", String::from_utf16_lossy(&k))));
            }
            sorted.insert(upper(&k), (k, v));
        }
        let mut out = vec![];
        for (k, v) in sorted.into_values() {
            out.extend(k);
            out.push(u16::from(b'='));
            out.extend(v);
            out.push(0);
        }
        // An empty block is two NULs.
        if out.is_empty() {
            out.push(0);
        }
        out.push(0);
        Ok(out)
    }

    /// The value of `Path` among `vars` (names compare without case).
    pub(super) fn path_of(vars: &[(OsString, OsString)]) -> Option<&OsStr> {
        vars.iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.as_os_str())
    }

    /// The folders of the sign-in environment's `Path` (`super::user_default`) that
    /// `searched` lacks: where a lookup that missed looks next, for a program installed since
    /// Workbench started (`exe::which`).
    pub fn path_added(searched: &[PathBuf]) -> Vec<PathBuf> {
        let Some(Some(fresh)) = with_default(|vars| path_of(vars).map(OsStr::to_os_string)) else { return vec![] };
        let known: std::collections::HashSet<String> = searched.iter().map(|d| super::path_key(&d.to_string_lossy())).collect();
        std::env::split_paths(&fresh).filter(|d| !d.as_os_str().is_empty() && !known.contains(&super::path_key(&d.to_string_lossy()))).collect()
    }
}

/// Tests: a variable with a name of its own in this user's `Environment` registry key (never
/// `Path`), removed on drop. `new` returns once the sign-in environment has it.
#[cfg(all(test, windows))]
pub(crate) struct UserVar {
    pub name: String,
    pub value: String,
}

#[cfg(all(test, windows))]
impl UserVar {
    pub(crate) fn new(value: &str) -> UserVar {
        let name = format!("WORKBENCH_TEST_{}", crate::util::random_token(6).replace(['-', '_'], "x").to_uppercase());
        super::autostart::set_string("Environment", &name, value).unwrap();
        let var = UserVar { name, value: value.to_string() };
        // The change notification may come a moment after the write.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !var.in_default() {
            assert!(std::time::Instant::now() < deadline, "{} never reached the sign-in environment", var.name);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        var
    }

    /// Whether [`user_default`] has this variable with its value.
    pub(crate) fn in_default(&self) -> bool {
        user_default().is_some_and(|vars| vars.iter().any(|(k, v)| k.eq_ignore_ascii_case(&self.name) && *v == *self.value))
    }
}

#[cfg(all(test, windows))]
impl Drop for UserVar {
    fn drop(&mut self) {
        let _ = super::autostart::delete_value("Environment", &self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_path_comes_first_and_keeps_the_own_entries_it_lacks() {
        let fresh = r"C:\Windows\system32;C:\Windows;;C:\Program Files\Git\cmd;C:\Users\me\AppData\Local\Microsoft\WindowsApps;C:\Windows\";
        let own = concat!(
            r"C:\work\.venv\Scripts;c:\windows\System32\;C:/Windows;",
            r#""C:\Program Files\Git\cmd";bin;\usr\local\bin;C:relative;C:\Tools\;\\server\share\bin;C:\work\.venv\Scripts;"#
        );
        assert_eq!(
            merge_path(fresh, own),
            concat!(
                r"C:\Windows\system32;C:\Windows;C:\Program Files\Git\cmd;C:\Users\me\AppData\Local\Microsoft\WindowsApps;",
                r"C:\work\.venv\Scripts;C:\Tools\;\\server\share\bin"
            ),
            "the sign-in's entries first; then this process's absolute ones, each once"
        );
        // A `;` inside quotes belongs to its entry.
        assert_eq!(entries(r#"C:\a;"C:\b;c";C:\d"#), vec![r"C:\a", r#""C:\b;c""#, r"C:\d"]);
        assert_eq!(merge_path(r#""C:\b;c""#, r#"C:\b;c;C:\B;"C:\B;C""#), r#""C:\b;c";C:\b"#);
        // Nothing fresh: this process's absolute entries alone.
        assert_eq!(merge_path("", r"C:\x;.;C:\x\"), r"C:\x");
        assert_eq!(path_key(r"C:\"), r"c:\");
        assert_eq!(path_key(r#" "D:/Tools//" "#), r"d:\tools");
        assert!(windows_absolute(r"C:\x") && windows_absolute("c:/x") && windows_absolute(r"\\host\share") && windows_absolute(r#""C:\x y""#));
        assert!(!windows_absolute(r"\x") && !windows_absolute("C:x") && !windows_absolute("x") && !windows_absolute(""));
    }

    #[test]
    fn a_child_path_is_this_process_then_the_folders_it_lacks() {
        let own = r"C:\work\.venv\Scripts;C:\Windows\system32;bin;C:\Windows\system32";
        let fresh = concat!(
            r"c:\windows\System32\;C:\Program Files\Go\bin;%GOROOT%\bin;;C:\Users\me\go\bin;",
            r"C:/Program Files/Go/bin;\usr\bin;\\server\share\bin;C:\Users\me\go\bin\"
        );
        assert_eq!(
            extend_path(own, fresh).as_deref(),
            Some(concat!(r"C:\work\.venv\Scripts;C:\Windows\system32;bin;C:\Windows\system32;", r"C:\Program Files\Go\bin;C:\Users\me\go\bin;\\server\share\bin")),
            "this process's PATH as it is, then the sign-in's absolute folders it lacks, each once"
        );
        // Nothing new: the program keeps this process's PATH.
        assert_eq!(extend_path(own, r"C:\WINDOWS\System32;C:\work\.venv\Scripts\;bin;\x"), None);
        assert_eq!(extend_path(r"C:\a;", r"C:\b").as_deref(), Some(r"C:\a;C:\b"));
        assert_eq!(extend_path("", r"C:\b;C:\B\").as_deref(), Some(r"C:\b"));
        // A `;` inside quotes belongs to its entry.
        assert_eq!(extend_path(r"C:\x", r#""C:\b;c";C:\b"#).as_deref(), Some(r#"C:\x;"C:\b;c";C:\b"#));
        assert_eq!(extend_path(r#""C:\b;c""#, r#""c:\B;C\""#), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_keeps_this_process_environment() {
        assert_eq!(user_default(), None);
        assert_eq!(fresh_path(), None);
        assert_eq!(child_path(), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_sign_in_environment_comes_from_the_registry() {
        use std::path::PathBuf;
        let mut var = UserVar::new(r"C:\wb-test\one");
        // A change is seen by the next call once its notification comes (the keys are watched).
        var.value = r"C:\wb-test\two".into();
        super::super::autostart::set_string("Environment", &var.name, &var.value).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !var.in_default() {
            assert!(std::time::Instant::now() < deadline, "the change was not seen");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Only the sign-in's variables: not one this process alone has.
        let only_here = format!("{}_HERE", var.name);
        // SAFETY: always safe on Windows (std's documentation); the name is this test's own.
        unsafe { std::env::set_var(&only_here, "shell") };
        let vars = user_default().unwrap();
        assert!(!vars.iter().any(|(k, _)| k.eq_ignore_ascii_case(&only_here)));
        let get = |k: &str| vars.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        assert!(get("SystemRoot").is_some() && get("USERPROFILE").is_some(), "{vars:?}");
        // A terminal's PATH: the sign-in's `Path`, then this process's absolute entries.
        let fresh = get("Path").unwrap().to_string_lossy().into_owned();
        let path = fresh_path().unwrap();
        let head = merge_path(&fresh, "");
        assert!(path == head || path.starts_with(&format!("{head};")), "{path}\n{fresh}");
        let keys: Vec<String> = entries(&path).into_iter().map(path_key).collect();
        for own in entries(&std::env::var("PATH").unwrap()).into_iter().filter(|e| windows_absolute(e)) {
            assert!(keys.contains(&path_key(own)), "{own} is missing from {path}");
        }
        // What Workbench starts itself: this process's PATH, then the sign-in's folders it lacks.
        assert_eq!(child_path(), extend_path(&std::env::var("PATH").unwrap(), &fresh));
        // A lookup that missed tries the sign-in's folders it has not searched.
        let fresh_dirs: Vec<PathBuf> = std::env::split_paths(&fresh).filter(|d| !d.as_os_str().is_empty()).collect();
        assert!(path_added(&fresh_dirs).is_empty());
        assert_eq!(path_added(&[]).len(), fresh_dirs.len());
        drop(var);
        // SAFETY: as above.
        unsafe { std::env::remove_var(&only_here) };
    }

    #[cfg(windows)]
    #[test]
    fn environment_blocks_are_sorted_and_read_back() {
        let v = |k: &str, val: &str| (OsString::from(k), OsString::from(val));
        let b = block(&[v("b", "2"), v("Path", r"C:\x;C:\y"), v("A", "1=one"), v("=C:", r"C:\work"), v("B", "3")]).unwrap();
        assert_eq!(win::vars_of(&b), vec![v("=C:", r"C:\work"), v("A", "1=one"), v("B", "3"), v("Path", r"C:\x;C:\y")]);
        assert!(b.ends_with(&[0, 0]));
        assert_eq!(block(&[]).unwrap(), vec![0, 0]);
        for bad in [v("", "x"), v("A=B", "x"), v("A\0", "x"), v("A", "x\0y")] {
            assert!(block(&[bad]).is_err());
        }
        // SAFETY: a block built above, ended by an empty string.
        assert_eq!(unsafe { win::block_vars(b.as_ptr()) }, win::vars_of(&b));
        let empty = [0u16, 0];
        // SAFETY: as above.
        assert_eq!(unsafe { win::block_vars(empty.as_ptr()) }, vec![]);
    }
}
