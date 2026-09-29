//! `path`: paths as users, clients and other programs write them. Unix keeps the rules
//! Workbench always had: `/` separates, a leading `/` makes a path absolute, names compare
//! byte for byte. Windows adds drive letters, `\`, UNC roots, reserved device names, 8.3
//! short names and names that compare without regard to ASCII case (docs/windows-port.md,
//! "Paths"). The Windows rules are string logic (`win`), also compiled for tests on every
//! OS.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

/// Whether the file system compares names without regard to case (file-name globs).
pub const CASE_INSENSITIVE: bool = cfg!(windows);

/// Whether `s` is an absolute path as a user or a program wrote it (config values,
/// client input, hook payloads). Unix: it starts with `/`. Windows: `C:\` or `C:/`, a
/// UNC path, or a rooted one (`\x`, `/x`: on the current drive, never relative to a
/// project). `C:x` is neither: [`check_relative`] refuses it.
pub fn is_absolute_str(s: &str) -> bool {
    #[cfg(unix)]
    {
        s.starts_with('/')
    }
    #[cfg(windows)]
    {
        win::is_absolute(s)
    }
}

/// The rest of `s` after `~/` (also `~\` on Windows): a path in the home directory.
pub fn home_relative(s: &str) -> Option<&str> {
    #[cfg(unix)]
    {
        s.strip_prefix("~/")
    }
    #[cfg(windows)]
    {
        win::home_relative(s)
    }
}

/// Whether `s` holds a path separator (`/`, and `\` on Windows): a path, not a bare name.
pub fn has_separator(s: &str) -> bool {
    #[cfg(unix)]
    {
        s.contains('/')
    }
    #[cfg(windows)]
    {
        s.contains(['/', '\\'])
    }
}

/// The parts of a path string between separators (`/`, and `\` on Windows).
pub fn segments(s: &str) -> impl Iterator<Item = &str> {
    #[cfg(unix)]
    {
        s.split('/')
    }
    #[cfg(windows)]
    {
        s.split(['/', '\\'])
    }
}

/// `p` with `/` separators, as client paths are written. Unix: unchanged (a `\` there is
/// part of a name).
pub fn to_slash(p: &Path) -> String {
    #[cfg(unix)]
    {
        p.to_string_lossy().into_owned()
    }
    #[cfg(windows)]
    {
        p.to_string_lossy().replace('\\', "/")
    }
}

/// Why `name`, one component of a client's relative path, cannot name a file inside a
/// root. Unix: always fine (`/`, `.`, `..` and NUL are the callers' business). Windows:
/// a `\` (a second separator that checks splitting on `/` would not see), `:` (a drive,
/// an alternate data stream), characters Windows refuses, a trailing dot or space
/// (dropped by Windows: `.env.` opens `.env`), device names (`NUL`, `com1.txt`) and 8.3
/// short names (`GIT~1` opens `.git`).
pub fn check_component(name: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let _ = name;
        Ok(())
    }
    #[cfg(windows)]
    {
        win::check_component(name)
    }
}

/// [`check_component`] for each part of `rel`, a `/`-separated relative path (empty, `.`
/// and `..` parts are the caller's business). Windows also refuses absolute paths and
/// drives (`C:x`).
pub fn check_relative(rel: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let _ = rel;
        Ok(())
    }
    #[cfg(windows)]
    {
        win::check_relative(rel)
    }
}

/// Whether `rel`, a relative path from repository content (a workspace member, a
/// solution's project), stays below the folder it is joined to as far as its form goes
/// (`..` is the caller's business). Unix: always, as before. Windows: `\` counts as
/// `/` and [`check_relative`] must pass: `C:\x`, `C:x` and `\\server\share` replace
/// the folder in a join (and opening a UNC path connects to the server).
pub fn stays_inside(rel: &str) -> bool {
    #[cfg(unix)]
    {
        let _ = rel;
        true
    }
    #[cfg(windows)]
    {
        win::stays_inside(rel)
    }
}

/// Why files under `root` are not served on this OS: never on Unix; on Windows, UNC
/// paths (network shares, WSL's `\\wsl$`).
pub fn unsupported_root(root: &Path) -> Option<&'static str> {
    #[cfg(unix)]
    {
        let _ = root;
        None
    }
    #[cfg(windows)]
    {
        win::unsupported_root(&root.to_string_lossy())
    }
}

/// `std::fs::canonicalize`. On Windows without the `\\?\` prefix wherever a plain path
/// names the same file (dunce), and with an uppercase drive letter, so canonical paths
/// compare and print like the ones users type.
pub fn canonicalize(p: impl AsRef<Path>) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        std::fs::canonicalize(p)
    }
    #[cfg(windows)]
    {
        let p = dunce::canonicalize(p)?;
        Ok(match p.to_str().and_then(win::upper_drive) {
            Some(s) => PathBuf::from(s),
            None => p,
        })
    }
}

/// `p` below `base`, like `Path::strip_prefix`. Windows compares names without regard
/// to ASCII case and takes `\\?\C:\` for `C:\` (both name the same files).
pub fn strip_prefix<'a>(p: &'a Path, base: &Path) -> Option<&'a Path> {
    #[cfg(unix)]
    {
        p.strip_prefix(base).ok()
    }
    #[cfg(windows)]
    {
        match (p.to_str(), base.to_str()) {
            (Some(a), Some(b)) => win::strip_prefix(a, b).map(Path::new),
            _ => p.strip_prefix(base).ok(),
        }
    }
}

/// Whether `p` is `base` or below it ([`strip_prefix`]'s comparison).
pub fn starts_with(p: &Path, base: &Path) -> bool {
    strip_prefix(p, base).is_some()
}

/// Whether the file name `a` is `b` (Windows: without regard to ASCII case).
pub fn same_name(a: impl AsRef<OsStr>, b: &str) -> bool {
    #[cfg(unix)]
    {
        a.as_ref() == b
    }
    #[cfg(windows)]
    {
        a.as_ref().eq_ignore_ascii_case(b)
    }
}

/// A host path as the path of a `file://` URI: a part that goes into the URI as it is
/// (`/C:` on Windows, empty elsewhere) and the rest with `/` separators, still to be
/// percent-encoded.
pub fn uri_path(path: &str) -> (String, String) {
    #[cfg(unix)]
    {
        (String::new(), path.to_string())
    }
    #[cfg(windows)]
    {
        win::uri_path(path)
    }
}

/// The host path of a decoded `file://` URI path. Unix: the path itself. Windows:
/// `/C:/x`, `/c:/x` → `C:\x`; a path without a drive names nothing there (`None`).
pub fn from_uri_path(p: String) -> Option<String> {
    #[cfg(unix)]
    {
        Some(p)
    }
    #[cfg(windows)]
    {
        win::from_uri_path(&p)
    }
}

/// Where Workbench keeps its data unless `WORKBENCH_DATA_DIR` says otherwise:
/// `~/.local/share`. On Windows `%LOCALAPPDATA%`, not the roaming `%APPDATA%` that holds
/// the config: tokens do not roam, and the config watcher does not see data writes.
pub fn data_home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        dirs::data_dir()
    }
    #[cfg(windows)]
    {
        dirs::data_local_dir()
    }
}

/// Claude Code's managed settings folder (`managed-mcp.json`), as Claude Code names it.
pub fn claude_managed_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/etc/claude-code")
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    }
}

/// Claude Code's per-user temp folder (task output, agents' scratch files):
/// `/tmp/claude-<uid>`. On Windows `%TEMP%\claude-0`: Claude Code joins its temp dir
/// and `claude-${process.getuid?.() ?? 0}`, and Windows has no uid.
pub fn claude_temp_dir() -> String {
    #[cfg(unix)]
    {
        format!("/tmp/claude-{}", nix::unistd::getuid())
    }
    #[cfg(windows)]
    {
        std::env::temp_dir().join("claude-0").display().to_string()
    }
}

/// Other programs' credential folders and Workbench's default config and data folders
/// (whatever `WORKBENCH_*_DIR` says for this instance): the debugger's source view never
/// shows files from them. Unix: `~/.config/gh`, `~/.config/gcloud`, `~/.config/workbench`,
/// `~/.local/share/workbench`. Windows: `GitHub CLI`, `gcloud`, `postgresql`
/// (`pgpass.conf`) and `workbench` in `%APPDATA%`, and `%LOCALAPPDATA%\workbench`.
pub fn private_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let home = dirs::home_dir().unwrap_or_default();
        [".config/gh", ".config/gcloud", ".config/workbench", ".local/share/workbench"].iter().map(|d| home.join(d)).collect()
    }
    #[cfg(windows)]
    {
        let mut out: Vec<PathBuf> = vec![];
        if let Some(c) = dirs::config_dir() {
            out.extend(["GitHub CLI", "gcloud", "postgresql", "workbench"].iter().map(|d| c.join(d)));
        }
        out.extend(data_home().map(|d| d.join("workbench")));
        out
    }
}

/// libpq's password file: `~/.pgpass`; on Windows `%APPDATA%\postgresql\pgpass.conf`.
pub fn pgpass_file() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        dirs::home_dir().map(|home| home.join(".pgpass"))
    }
    #[cfg(windows)]
    {
        dirs::config_dir().map(|d| d.join("postgresql").join("pgpass.conf"))
    }
}

/// Windows path rules as string logic.
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod win {
    fn is_sep(c: char) -> bool {
        c == '/' || c == '\\'
    }

    /// The drive letter of `C:…`.
    fn drive(s: &str) -> Option<char> {
        let mut c = s.chars();
        match (c.next(), c.next()) {
            (Some(d), Some(':')) if d.is_ascii_alphabetic() => Some(d),
            _ => None,
        }
    }

    pub fn is_absolute(s: &str) -> bool {
        s.starts_with(is_sep) || (drive(s).is_some() && s[2..].starts_with(is_sep))
    }

    pub fn home_relative(s: &str) -> Option<&str> {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    }

    /// `NUL`, `com1.txt`, `Lpt¹`, `con .log`: what precedes the first dot, trailing
    /// spaces dropped, names a device in every folder.
    pub fn is_device_name(name: &str) -> bool {
        let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()) {
            return true;
        }
        match stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT")) {
            Some(n) => (n.len() == 1 && n.as_bytes()[0].is_ascii_digit()) || matches!(n, "¹" | "²" | "³"),
            None => false,
        }
    }

    /// `GIT~1`, `ENV~12.TXT`: the form of an 8.3 short name, another spelling of a long
    /// name that checks by name would not recognize.
    pub fn is_short_name(name: &str) -> bool {
        let (base, ext) = match name.rsplit_once('.') {
            Some((b, e)) => (b, Some(e)),
            None => (name, None),
        };
        let Some((head, num)) = base.rsplit_once('~') else { return false };
        base.len() <= 8
            && !head.is_empty()
            && !base.contains('.')
            && !num.is_empty()
            && num.bytes().all(|b| b.is_ascii_digit())
            && ext.is_none_or(|e| (1..=3).contains(&e.len()))
    }

    pub fn check_component(name: &str) -> Result<(), String> {
        if name.contains('\\') {
            return Err(format!("{name:?}: use / to separate folders"));
        }
        if name.contains(':') {
            return Err(format!("{name:?}: a name cannot hold ':' on Windows (a drive or an alternate data stream)"));
        }
        if name.chars().any(|c| c < ' ' || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
            return Err(format!("{name:?}: a name cannot hold control characters or any of <>\"|?* on Windows"));
        }
        if name.ends_with(['.', ' ']) {
            return Err(format!("{name:?}: Windows drops a dot or a space at the end of a name"));
        }
        if is_device_name(name) {
            return Err(format!("{name:?} is a device name on Windows"));
        }
        if is_short_name(name) {
            return Err(format!("{name:?} looks like an 8.3 short name: use the full name"));
        }
        Ok(())
    }

    pub fn check_relative(rel: &str) -> Result<(), String> {
        if is_absolute(rel) || drive(rel).is_some() {
            return Err(format!("{rel:?} is not a relative path"));
        }
        rel.split('/').filter(|c| !matches!(*c, "" | "." | "..")).try_for_each(check_component)
    }

    pub fn stays_inside(rel: &str) -> bool {
        check_relative(&rel.replace('\\', "/")).is_ok()
    }

    const WSL: &str = r"projects inside WSL (\\wsl$, \\wsl.localhost) are not supported on Windows: run Workbench inside WSL for them";
    const UNC: &str = r"projects on network paths (\\server\share) are not supported on Windows: clone the repository to a local drive";

    pub fn unsupported_root(root: &str) -> Option<&'static str> {
        let r = root.replace('/', "\\");
        // `\??\` is the NT namespace `\\?\` stands for: Win32 passes it through, so
        // `\??\UNC\server\share` reaches the network redirector too.
        let host = match r.strip_prefix(r"\\?\").or_else(|| r.strip_prefix(r"\??\")) {
            // `\\?\C:\x` is a local drive.
            Some(rest) if drive(rest).is_some() => return None,
            Some(rest) => match rest.get(..4) {
                Some(p) if p.eq_ignore_ascii_case(r"UNC\") => &rest[4..],
                _ => rest,
            },
            None => r.strip_prefix(r"\\")?,
        };
        let host = host.split('\\').next().unwrap_or("").to_ascii_lowercase();
        Some(if host == "wsl$" || host == "wsl.localhost" { WSL } else { UNC })
    }

    /// A path's root as a key equal for every spelling of it (`C:\`, `c:/`, `\\?\C:\` and
    /// `\??\C:\`; `\\Server\Share` and `\\?\UNC\server\share`), and the rest of the path.
    fn split_root(s: &str) -> (String, &str) {
        let (unc, rest) = match s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix("//?/")).or_else(|| s.strip_prefix(r"\??\")) {
            Some(v) => match v.get(..4) {
                Some(p) if p.eq_ignore_ascii_case(r"UNC\") || p.eq_ignore_ascii_case("UNC/") => (true, &v[4..]),
                _ if drive(v).is_some() => (false, v),
                // `\\?\Volume{…}\`: the volume is the root.
                _ => {
                    let end = v.find(is_sep).unwrap_or(v.len());
                    return (format!(r"\\?\{}", v[..end].to_ascii_lowercase()), &v[end..]);
                }
            },
            None if s.starts_with(is_sep) && s[1..].starts_with(is_sep) => (true, &s[2..]),
            None => (false, s),
        };
        if unc {
            let server_end = rest.find(is_sep).unwrap_or(rest.len());
            let share = rest[server_end..].trim_start_matches(is_sep);
            let share_end = share.find(is_sep).unwrap_or(share.len());
            let key = format!(r"\\{}\{}", rest[..server_end].to_ascii_lowercase(), share[..share_end].to_ascii_lowercase());
            return (key, &share[share_end..]);
        }
        match drive(rest) {
            Some(d) if rest[2..].starts_with(is_sep) => (format!("{}:\\", d.to_ascii_uppercase()), &rest[3..]),
            Some(d) => (format!("{}:", d.to_ascii_uppercase()), &rest[2..]),
            None if rest.starts_with(is_sep) => ("\\".into(), &rest[1..]),
            None => (String::new(), rest),
        }
    }

    /// `s` without leading separators and `.` components.
    fn skip_empty(mut s: &str) -> &str {
        loop {
            s = s.trim_start_matches(is_sep);
            match s.strip_prefix('.') {
                Some(r) if r.is_empty() || r.starts_with(is_sep) => s = r,
                _ => return s,
            }
        }
    }

    /// The first component of `s` and what follows it.
    fn next_component(s: &str) -> Option<(&str, &str)> {
        let s = skip_empty(s);
        if s.is_empty() {
            return None;
        }
        Some(s.split_at(s.find(is_sep).unwrap_or(s.len())))
    }

    /// ASCII letters without regard to case, everything else exactly. Beyond ASCII each
    /// volume's own upcase table (written when it was formatted) decides, and no fixed
    /// table matches every volume: KELVIN SIGN (U+212A) lowercases to `k`, yet NTFS keeps
    /// `wor\u{212A}` and `work` apart. Calling two names different refuses a path;
    /// calling them the same could let one out of a root.
    fn same_name(a: &str, b: &str) -> bool {
        a.eq_ignore_ascii_case(b)
    }

    pub fn strip_prefix<'a>(p: &'a str, base: &str) -> Option<&'a str> {
        if base.is_empty() {
            return Some(p);
        }
        let (pk, mut rest) = split_root(p);
        let (bk, mut brest) = split_root(base);
        if pk != bk {
            return None;
        }
        while let Some((b, bafter)) = next_component(brest) {
            let (c, after) = next_component(rest)?;
            if !same_name(c, b) {
                return None;
            }
            (rest, brest) = (after, bafter);
        }
        Some(skip_empty(rest))
    }

    /// `C:` for `c:` (also after `\\?\`); `None` when nothing changes.
    pub fn upper_drive(s: &str) -> Option<String> {
        let (pre, rest) = match s.strip_prefix(r"\\?\") {
            Some(r) => (r"\\?\", r),
            None => ("", s),
        };
        let d = drive(rest).filter(char::is_ascii_lowercase)?;
        Some(format!("{pre}{}{}", d.to_ascii_uppercase(), &rest[1..]))
    }

    pub fn uri_path(path: &str) -> (String, String) {
        let p = match path.strip_prefix(r"\\?\") {
            Some(r) if r.get(..4).is_some_and(|u| u.eq_ignore_ascii_case(r"UNC\")) => format!(r"\\{}", &r[4..]),
            Some(r) => r.to_string(),
            None => path.to_string(),
        };
        match drive(&p) {
            Some(d) => {
                let rest = p[2..].replace('\\', "/");
                let rest = if rest.starts_with('/') { rest } else { format!("/{rest}") };
                (format!("/{}:", d.to_ascii_uppercase()), rest)
            }
            None => (String::new(), p.replace('\\', "/")),
        }
    }

    pub fn from_uri_path(p: &str) -> Option<String> {
        if let Some(unc) = p.strip_prefix("//") {
            return (!unc.is_empty()).then(|| format!(r"\\{}", unc.replace('/', "\\")));
        }
        let rest = p.strip_prefix('/')?;
        let d = drive(rest)?;
        let tail = &rest[2..];
        if !(tail.is_empty() || tail.starts_with('/')) {
            return None;
        }
        let tail = if tail.is_empty() { "/" } else { tail };
        Some(format!("{}:{}", d.to_ascii_uppercase(), tail.replace('/', "\\")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_rules_are_unchanged() {
        assert!(is_absolute_str("/x") && !is_absolute_str("x") && !is_absolute_str(r"C:\x") && !is_absolute_str(r"\x"));
        assert_eq!(home_relative("~/x/y"), Some("x/y"));
        assert_eq!(home_relative(r"~\x"), None);
        assert!(has_separator("a/b") && !has_separator(r"a\b"));
        assert_eq!(segments(r"a\b/c").collect::<Vec<_>>(), [r"a\b", "c"]);
        assert_eq!(to_slash(Path::new(r"a\b/c")), r"a\b/c");
        for name in [r"a\b", "a:b", "NUL", "com1.txt", "x.", "x ", "GIT~1", "a*b"] {
            assert!(check_component(name).is_ok(), "{name}");
        }
        assert!(check_relative(r"..\x").is_ok() && check_relative("C:x").is_ok());
        assert!(stays_inside(r"\\server\share") && stays_inside("C:x"));
        assert!(private_dirs().iter().any(|d| d.ends_with(".config/gh")) && private_dirs().iter().any(|d| d.ends_with(".local/share/workbench")));
        assert_eq!(unsupported_root(Path::new(r"\\wsl$\Ubuntu")), None);
        assert_eq!(strip_prefix(Path::new("/a/B/c"), Path::new("/a/b")), None);
        assert_eq!(strip_prefix(Path::new("/a/b/c"), Path::new("/a/b/")), Some(Path::new("c")));
        assert!(same_name(".git", ".git") && !same_name(".GIT", ".git"));
        assert_eq!(uri_path("/x/y"), (String::new(), "/x/y".to_string()));
        assert_eq!(from_uri_path("/c:/x".into()).as_deref(), Some("/c:/x"));
        assert!(!CASE_INSENSITIVE);
    }

    #[test]
    fn windows_absolute_and_home_paths() {
        for abs in [r"C:\x", "c:/x", r"\\server\share", "//server/share", r"\x", "/x", r"\\?\C:\x"] {
            assert!(win::is_absolute(abs), "{abs}");
        }
        for rel in ["C:x", "C:", "x", r"a\b", r"~\x", ""] {
            assert!(!win::is_absolute(rel), "{rel}");
        }
        assert_eq!(win::home_relative(r"~\x\y"), Some(r"x\y"));
        assert_eq!(win::home_relative("~/x"), Some("x"));
        assert_eq!(win::home_relative("~x"), None);
    }

    #[test]
    fn windows_names_refuse_separators_streams_devices_and_aliases() {
        for bad in [
            r"a\b", "..\\x", "C:", "a.txt:secret", "a.txt::$DATA", "x.", "x ", "...", ".env.", "a<b", "a|b", "a?", "a*", "a\"b", "a\u{1}",
            "CON", "con", "nul.txt", "NUL.tar.gz", "Aux", "prn.", "com1", "COM9.log", "lpt3.x", "LPT0", "COM¹", "lpt³.txt", "con .txt", "CONIN$",
            "GIT~1", "git~1", "ENV~12.TXT", "PROGRA~1", "A~1.B",
        ] {
            assert!(win::check_component(bad).is_err(), "{bad:?}");
        }
        for ok in [
            "a", "a.txt", ".env", ".git", "console.txt", "nul-device", "COM10", "LPT", "COMX", "auxiliary.rs", "con_log", "file~name.txt", "a~1b",
            "backup.txt~", "~1", "TOOLONGNAME~1", "x~1.long", "é.rs", "a b",
        ] {
            assert!(win::check_component(ok).is_ok(), "{ok:?}");
        }
        assert!(win::check_component(r"a\b").unwrap_err().contains("use /"));
    }

    #[test]
    fn windows_relative_paths_cannot_leave_through_backslashes_or_drives() {
        for bad in [
            r"..\x", r"a\..\..\x", r"C:\x", "C:x", "C:/x", "c:", "/x", r"\x", r"\\server\share", "a/b:stream", "a/NUL", "a/com1.txt", "a/x.", "a/x ",
            "GIT~1/config", "src/../GIT~1",
        ] {
            assert!(win::check_relative(bad).is_err(), "{bad:?}");
        }
        for ok in ["", ".", "a/b.txt", "src/main.rs", "a/../b", "./a", "../x", "a//b", "con_log/x", ".git/config"] {
            assert!(win::check_relative(ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn windows_repository_paths_stay_inside() {
        for bad in [r"C:\x", "C:x", "c:/x", r"\\server\share\x.csproj", "//server/share", r"\x", "/x", "a/b:stream", r"a\NUL", "x/com1.txt"] {
            assert!(!win::stays_inside(bad), "{bad:?}");
        }
        for ok in ["crates/", "crates/api", r"src\App\App.csproj", "./server.js", "", "a/../b"] {
            assert!(win::stays_inside(ok), "{ok:?}");
        }
    }

    #[test]
    fn windows_unc_roots_are_refused() {
        for wsl in [r"\\wsl$\Ubuntu\home\u\proj", r"\\WSL.localhost\Ubuntu", "//wsl$/Debian", r"\\?\UNC\wsl$\Ubuntu", r"\??\UNC\wsl$\Ubuntu"] {
            assert!(win::unsupported_root(wsl).is_some_and(|m| m.contains("WSL")), "{wsl}");
        }
        for unc in [r"\\server\share\proj", "//server/share", r"\\?\UNC\server\share\x", r"\\.\pipe\x", r"\??\UNC\server\share\proj", r"\??\unc\server\share"] {
            assert!(win::unsupported_root(unc).is_some_and(|m| m.contains("network")), "{unc}");
        }
        for local in [r"C:\Users\me\proj", "c:/x", r"\\?\C:\x", r"\??\C:\x", r"\x", "rel"] {
            assert_eq!(win::unsupported_root(local), None, "{local}");
        }
    }

    #[test]
    fn windows_prefixes_compare_without_case_or_verbatim_prefixes() {
        let s = win::strip_prefix;
        assert_eq!(s(r"c:\Users\Me\proj\src\main.rs", r"C:\users\me\proj"), Some(r"src\main.rs"));
        assert_eq!(s(r"\\?\C:\p\very\x", r"C:\p"), Some(r"very\x"));
        assert_eq!(s(r"C:\p\x", r"\\?\c:\P"), Some("x"));
        assert_eq!(s("C:/p/x/", r"C:\p\"), Some("x/"));
        assert_eq!(s(r"C:\p\.\x", r"C:\p"), Some("x"));
        assert_eq!(s(r"C:\p", r"C:\P"), Some(""));
        assert_eq!(s(r"C:\Ärger\x", r"c:\Ärger"), Some("x"));
        assert_eq!(s(r"C:\Ärger\x", r"C:\ärger"), None);
        for sign in ["\u{212A}", "\u{212B}", "\u{2126}"] {
            assert_eq!(s(&format!(r"C:\src\wor{sign}\x"), r"C:\src\work"), None, "{sign}");
        }
        assert_eq!(s("C:\\src\\wor\u{212A}\\x", "C:\\src\\wor\u{212A}"), Some("x"));
        assert_eq!(s(r"\\Server\Share\x", r"\\?\UNC\server\share"), Some("x"));
        assert_eq!(s(r"\??\C:\p\x", r"C:\p"), Some("x"));
        assert_eq!(s(r"\??\UNC\server\share\x", r"\\server\share"), Some("x"));
        assert_eq!(s(r"C:\p2\x", r"C:\p"), None);
        assert_eq!(s(r"D:\p\x", r"C:\p"), None);
        assert_eq!(s(r"C:p\x", r"C:\p"), None);
        assert_eq!(s(r"\\server\share2\x", r"\\server\share"), None);
        assert_eq!(s(r"C:\p", r"C:\p\x"), None);
        assert_eq!(s(r"C:\p\x", ""), Some(r"C:\p\x"));
    }

    #[test]
    fn windows_drive_letters_are_uppercased() {
        assert_eq!(win::upper_drive(r"c:\x").as_deref(), Some(r"C:\x"));
        assert_eq!(win::upper_drive(r"\\?\d:\x").as_deref(), Some(r"\\?\D:\x"));
        assert_eq!(win::upper_drive(r"C:\x"), None);
        assert_eq!(win::upper_drive(r"\\server\share"), None);
    }

    #[test]
    fn windows_file_uri_paths() {
        let parts = |h: &str, r: &str| (h.to_string(), r.to_string());
        assert_eq!(win::uri_path(r"C:\Users\me\a b.rs"), parts("/C:", "/Users/me/a b.rs"));
        assert_eq!(win::uri_path(r"c:\x"), parts("/C:", "/x"));
        assert_eq!(win::uri_path(r"C:\"), parts("/C:", "/"));
        assert_eq!(win::uri_path(r"\\?\C:\x"), parts("/C:", "/x"));
        assert_eq!(win::uri_path(r"\\server\share\x"), parts("", "//server/share/x"));
        assert_eq!(win::uri_path(r"\\?\UNC\server\share\x"), parts("", "//server/share/x"));
        assert_eq!(win::from_uri_path("/c:/Users/me/x.rs").as_deref(), Some(r"C:\Users\me\x.rs"));
        assert_eq!(win::from_uri_path("/C:/").as_deref(), Some(r"C:\"));
        assert_eq!(win::from_uri_path("/C:").as_deref(), Some(r"C:\"));
        assert_eq!(win::from_uri_path("//server/share/x").as_deref(), Some(r"\\server\share\x"));
        assert_eq!(win::from_uri_path("/usr/lib/x"), None);
        assert_eq!(win::from_uri_path("/c:x"), None);
        assert_eq!(win::from_uri_path("c:/x"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_canonical_paths_have_no_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let c = canonicalize(dir.path().join("sub")).unwrap();
        let s = c.to_str().unwrap();
        assert!(!s.starts_with(r"\\?\") && s.as_bytes()[0].is_ascii_uppercase() && s.as_bytes()[1] == b':', "{s}");
        let lower = format!("{}{}", s[..1].to_ascii_lowercase(), &s[1..]);
        assert_eq!(canonicalize(&lower).unwrap(), c);
        // The temp dir itself may be spelled with 8.3 names (`RUNNER~1`): compare canonical paths.
        assert!(starts_with(Path::new(&lower.to_ascii_uppercase()), c.parent().unwrap()));
        assert!(is_absolute_str(s) && is_absolute_str(r"\x") && !is_absolute_str("C:x"));
        assert_eq!(home_relative(r"~\x"), Some("x"));
        assert_eq!(to_slash(Path::new(r"a\b")), "a/b");
        assert!(same_name(".GIT", ".git") && CASE_INSENSITIVE);
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_dirs_are_where_windows_programs_keep_them() {
        let dirs = private_dirs();
        let appdata = dirs::config_dir().unwrap();
        for d in ["GitHub CLI", "gcloud", "workbench"] {
            assert!(dirs.contains(&appdata.join(d)), "{d}: {dirs:?}");
        }
        assert!(dirs.contains(&data_home().unwrap().join("workbench")), "{dirs:?}");
        assert!(dirs.iter().any(|d| pgpass_file().unwrap().starts_with(d)), "{dirs:?}");
    }
}
