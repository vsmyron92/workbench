//! Finding programs (`PATH` lookup, the execute check) and how to start what was found.
//!
//! Unix keeps the rules Workbench always had: a command with a `/` is a path, anything
//! else is the first regular file of that name on `PATH`. On Windows a lookup goes over
//! `PATH` × `PATHEXT` (the extensions CreateProcess can start), then
//! `%USERPROFILE%\.local\bin` and `%APPDATA%\npm`, never the current directory, and yields
//! an absolute path. An npm `.cmd` shim is unwrapped to `node.exe` and the package script
//! named in the `.ps1` beside it, so cmd.exe neither parses the arguments nor looks for
//! `node` in the current directory (docs/windows-port.md §2).

use std::path::{Path, PathBuf};

/// What a lookup found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(unix, allow(dead_code))] // Unix finds only `Exe`
pub enum Kind {
    /// A program the OS starts itself (every program on Unix).
    Exe,
    /// An npm `.cmd` shim, unwrapped: `program` is node, `prefix_args` the package script.
    NpmShim,
    /// Another `.bat`/`.cmd` file: cmd.exe parses its arguments (`batch_args_safe`).
    Batch,
}

/// How to start a program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The file to execute (absolute on Windows).
    pub program: PathBuf,
    /// Arguments that go before the caller's own (an npm shim's package script).
    pub prefix_args: Vec<String>,
    pub kind: Kind,
}

impl Resolved {
    /// The command line: `program`, `prefix_args`, then `args`.
    pub fn argv(&self, args: &[&str]) -> Vec<String> {
        let mut v = vec![self.program.display().to_string()];
        v.extend(self.prefix_args.iter().cloned());
        v.extend(args.iter().map(|a| a.to_string()));
        v
    }
}

/// Find `cmd` (as `which` does) and say how to start it.
pub fn resolve(cmd: &str) -> Option<Resolved> {
    which(cmd).map(classify)
}

/// How to start the program file `path` (a lookup's result).
pub fn classify(path: PathBuf) -> Resolved {
    #[cfg(unix)]
    {
        Resolved { program: path, prefix_args: vec![], kind: Kind::Exe }
    }
    #[cfg(windows)]
    {
        win::classify(path)
    }
}

/// The program file `cmd` names: the path itself when `names_path(cmd)` (`~/` expanded),
/// else the first match on `PATH`.
pub fn which(cmd: &str) -> Option<PathBuf> {
    if names_path(cmd) {
        return program_file(crate::config::expand_tilde(cmd));
    }
    #[cfg(unix)]
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH")?).collect();
    #[cfg(windows)]
    let dirs = win::search_path();
    find_in(&dirs, cmd)
}

/// The first `dir/name` among `dirs` that is a program file (`program_file`). On Windows
/// only absolute directories count: `/usr/local/bin` would be `C:\usr\local\bin`, which
/// any local user can create.
pub fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        dirs.iter().find_map(|d| program_file(d.join(name)))
    }
    #[cfg(windows)]
    {
        dirs.iter().filter(|d| d.is_absolute()).find_map(|d| program_file(d.join(name)))
    }
}

/// `which(cmd)` for an agent CLI. On Windows a native `<cmd>.exe` in `~\.local\bin` (where
/// Claude Code's installer puts it) wins over a batch file found first (an npm shim in
/// `%APPDATA%\npm`, which npm puts on `PATH`).
pub fn which_preferring_native(cmd: &str) -> Option<PathBuf> {
    let found = which(cmd);
    #[cfg(windows)]
    if !names_path(cmd) && found.as_deref().is_some_and(win::is_batch) {
        let native = dirs::home_dir().map(|h| h.join(".local").join("bin").join(format!("{cmd}.exe")));
        if let Some(native) = native.filter(|p| win::is_file(p)) {
            return Some(native);
        }
    }
    found
}

/// `path` when it is a program file: a regular file on Unix. On Windows the path itself
/// when its extension is one CreateProcess starts, else the first of `path` + a `PATHEXT`
/// extension that exists; always absolute.
pub fn program_file(path: PathBuf) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        path.is_file().then_some(path)
    }
    #[cfg(windows)]
    {
        win::program_file(path)
    }
}

/// Whether `cmd` names a file rather than a program to look up: it contains `/` (on
/// Windows also `\`, or starts with a drive such as `C:`).
pub fn names_path(cmd: &str) -> bool {
    #[cfg(unix)]
    {
        cmd.contains('/')
    }
    #[cfg(windows)]
    {
        cmd.contains(['/', '\\']) || cmd.as_bytes().get(1) == Some(&b':')
    }
}

/// Whether `path` is a file this user can execute: an execute bit on Unix; on Windows an
/// extension listed in `PATHEXT`.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        win::is_file(path) && win::has_pathext(path)
    }
}

/// `is_executable` for a regular file whose metadata is at hand (a directory walk).
pub fn is_executable_file(path: &Path, meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = path;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        let _ = meta;
        win::has_pathext(path)
    }
}

/// The `rustup` binary when `path` is one of its proxies (`~/.cargo/bin/rust-analyzer`),
/// which run a toolchain's component: a link to (or copy of) `rustup` under another name.
/// Unix: the link resolves to `rustup`. Windows: the proxies are hard links, the same
/// file as `rustup.exe` beside them or in `~/.cargo/bin`.
pub fn rustup_proxy(path: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name == "rustup" {
            return None;
        }
        let real = path.canonicalize().ok()?;
        real.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .is_some_and(|n| n == "rustup" || n == "rustup-init")
            .then_some(real)
    }
    #[cfg(windows)]
    {
        win::rustup_proxy(path)
    }
}

/// The command line that starts Python 3: `python3` on Unix. On Windows `python`, else the
/// `py -3` launcher; a `python` among the Microsoft Store's app aliases comes last, since
/// without the Store's Python it only opens the Store.
pub fn python() -> Vec<String> {
    #[cfg(unix)]
    {
        vec!["python3".into()]
    }
    #[cfg(windows)]
    {
        win::python()
    }
}

/// Environment for a program Workbench starts by itself (not a terminal's shell): on
/// Windows `NoDefaultCurrentDirectoryInExePath=1`, so a cmd.exe among its processes (a
/// batch file, a shim's bare `node`) never takes a program from its current directory, a
/// repository. Nothing on Unix.
pub fn child_env() -> &'static [(&'static str, &'static str)] {
    #[cfg(unix)]
    {
        &[]
    }
    #[cfg(windows)]
    {
        &[("NoDefaultCurrentDirectoryInExePath", "1")]
    }
}

/// A process starting `r`: its program and `prefix_args`, with `child_env`. The caller
/// adds its own arguments.
pub fn command(r: &Resolved) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&r.program);
    c.args(&r.prefix_args).envs(child_env().iter().copied());
    c
}

/// A process running `argv`, a command line from config.toml (a secret's `command`). Unix:
/// `argv[0]` as it is, looked up on `PATH` by the OS. Windows: resolved as `launch_argv`
/// does, so an npm shim or a batch file (`bw.cmd`) starts too, with `child_env`.
pub fn configured(argv: &[String]) -> std::io::Result<std::process::Command> {
    #[cfg(unix)]
    {
        let (first, rest) = argv.split_first().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;
        let mut c = std::process::Command::new(first);
        c.args(rest);
        Ok(c)
    }
    #[cfg(windows)]
    {
        let argv = launch_argv(argv.to_vec()).map_err(std::io::Error::other)?;
        let mut c = std::process::Command::new(&argv[0]);
        c.args(&argv[1..]).envs(child_env().iter().copied());
        Ok(c)
    }
}

/// Whether cmd.exe passes `args` to a `.bat`/`.cmd` file as they are: none contains
/// `% ! ^ & | < > "` or a line break. cmd.exe parses a batch file's command line again,
/// so such an argument could expand variables or start commands of its own (BatBadBut).
pub fn batch_args_safe<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter().all(|a| !a.as_ref().contains(['%', '!', '^', '&', '|', '<', '>', '"', '\n', '\r']))
}

/// A terminal's argv, ready for the PTY. Unix: unchanged. Windows: the program becomes an
/// absolute path, an npm shim is unwrapped, and a batch file is refused when cmd.exe would
/// misread an argument (the caller can paste the prompt instead).
pub fn launch_argv(argv: Vec<String>) -> Result<Vec<String>, String> {
    #[cfg(unix)]
    {
        Ok(argv)
    }
    #[cfg(windows)]
    {
        let (first, rest) = argv.split_first().ok_or("empty command")?;
        let r = resolve(first).ok_or_else(|| format!("{first} was not found"))?;
        if r.kind == Kind::Batch && !batch_args_safe(rest) {
            return Err(format!(
                "{} is a batch file, and cmd.exe would misread an argument with % ! ^ & | < > \" or a line break",
                r.program.display()
            ));
        }
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        Ok(r.argv(&rest))
    }
}

/// The calls `(program, args)` an npm PowerShell shim (cmd-shim's `<name>.ps1`) makes, in
/// order, still with its `$basedir` and `$exe`:
/// `& "$basedir/node$exe"  "$basedir/node_modules/<pkg>/cli.js" $args`, then the same
/// with `node$exe` from `PATH`. `None` for anything else, and for a shim that sets
/// variables the program needs (pnpm's `$env:NODE_PATH`).
#[cfg(any(windows, test))]
fn shim_calls(text: &str) -> Option<Vec<(String, Vec<String>)>> {
    if text.contains("$env:") {
        return None;
    }
    let mut out: Vec<(String, Vec<String>)> = vec![];
    for line in text.lines() {
        let l = line.trim();
        let l = l.strip_prefix("$input |").map(str::trim_start).unwrap_or(l);
        let Some(call) = l.strip_prefix("& ") else { continue };
        let mut words = ps_words(call)?;
        if words.pop()? != "$args" || words.is_empty() {
            return None;
        }
        // Nothing but the two known variables: anything else is not a shim we can read.
        if words.iter().any(|w| w.replace("$basedir", "").replace("$exe", "").contains(['$', '`'])) {
            return None;
        }
        let program = words.remove(0);
        if !out.iter().any(|(p, a)| *p == program && *a == words) {
            out.push((program, words));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The words of a PowerShell command line made of bare words and `"…"` strings.
#[cfg(any(windows, test))]
fn ps_words(s: &str) -> Option<Vec<String>> {
    let mut out = vec![];
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut w = String::new();
        if c == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '"' => break,
                    c => w.push(c),
                }
            }
            if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                if matches!(c, '"' | '\'') {
                    return None;
                }
                w.push(c);
                chars.next();
            }
        }
        out.push(w);
    }
    Some(out)
}

#[cfg(windows)]
mod win {
    use std::path::{Path, PathBuf};

    use super::{Kind, Resolved};

    /// The extensions CreateProcess starts (a `.bat`/`.cmd` through cmd.exe).
    const LAUNCHABLE: &[&str] = &[".exe", ".com", ".bat", ".cmd"];

    /// `PATHEXT`, lowercase (Windows' default when unset).
    fn pathext() -> Vec<String> {
        let v = std::env::var("PATHEXT")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC".into());
        v.split(';').map(|e| e.trim().to_ascii_lowercase()).filter(|e| e.len() > 1 && e.starts_with('.')).collect()
    }

    /// `.ext` of `p`, lowercase.
    fn ext_of(p: &Path) -> Option<String> {
        p.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
    }

    /// A `.bat` or `.cmd` file (cmd.exe runs it).
    pub(super) fn is_batch(p: &Path) -> bool {
        matches!(ext_of(p).as_deref(), Some(".bat" | ".cmd"))
    }

    pub(super) fn has_pathext(p: &Path) -> bool {
        ext_of(p).is_some_and(|e| pathext().contains(&e))
    }

    /// A file, App Execution Aliases (`WindowsApps\pwsh.exe`) included: `metadata` cannot
    /// open those, their own metadata says file.
    pub(super) fn is_file(p: &Path) -> bool {
        p.is_file() || std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file())
    }

    /// Absolute `PATH` entries (a relative one would search the current directory), then
    /// where the Claude Code installer and `npm install -g` put programs.
    pub(super) fn search_path() -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).filter(|d| d.is_absolute()).collect())
            .unwrap_or_default();
        if let Some(home) = dirs::home_dir() {
            out.push(home.join(".local").join("bin"));
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            out.push(PathBuf::from(appdata).join("npm"));
        }
        out
    }

    pub(super) fn program_file(path: PathBuf) -> Option<PathBuf> {
        let path = std::path::absolute(&path).ok()?;
        let mut exts: Vec<String> = pathext().into_iter().filter(|e| LAUNCHABLE.contains(&e.as_str())).collect();
        // CreateProcess appends `.exe` whatever PATHEXT says.
        if !exts.iter().any(|e| e == ".exe") {
            exts.push(".exe".into());
        }
        if ext_of(&path).is_some_and(|e| exts.contains(&e)) && is_file(&path) {
            return Some(path);
        }
        exts.iter()
            .map(|e| {
                let mut s = path.clone().into_os_string();
                s.push(e);
                PathBuf::from(s)
            })
            .find(|p| is_file(p))
    }

    pub(super) fn classify(path: PathBuf) -> Resolved {
        let path = std::path::absolute(&path).unwrap_or(path);
        if !is_batch(&path) {
            return Resolved { program: path, prefix_args: vec![], kind: Kind::Exe };
        }
        match npm_shim(&path) {
            Some((program, prefix_args)) => Resolved { program, prefix_args, kind: Kind::NpmShim },
            None => Resolved { program: path, prefix_args: vec![], kind: Kind::Batch },
        }
    }

    /// What the npm shim `cmd_file` runs, from the `.ps1` beside it: a native program
    /// (node) and its arguments, each package file present. `None`: run the batch file.
    fn npm_shim(cmd_file: &Path) -> Option<(PathBuf, Vec<String>)> {
        let dir = cmd_file.parent()?;
        let ps1 = cmd_file.with_extension("ps1");
        if std::fs::metadata(&ps1).ok()?.len() > 64 * 1024 {
            return None;
        }
        let text = std::fs::read_to_string(&ps1).ok()?;
        let base = dir.to_string_lossy();
        let expand = |w: &str| {
            let from_base = w.contains("$basedir");
            let s = w.replace("$basedir", &base).replace("$exe", ".exe");
            (if from_base { s.replace('/', "\\") } else { s }, from_base)
        };
        for (program, args) in super::shim_calls(&text)? {
            let (program, _) = expand(&program);
            let found = if super::names_path(&program) { program_file(PathBuf::from(&program)) } else { super::which(&program) };
            // Never another batch file: that is what the unwrapping avoids.
            let Some(program) = found.filter(|p| matches!(ext_of(p).as_deref(), Some(".exe" | ".com"))) else { continue };
            let mut out = vec![];
            for a in &args {
                let (a, from_base) = expand(a);
                // A stale shim (package removed): let cmd.exe report it.
                if from_base && !Path::new(&a).is_file() {
                    return None;
                }
                out.push(a);
            }
            return Some((program, out));
        }
        None
    }

    pub(super) fn rustup_proxy(path: &Path) -> Option<PathBuf> {
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if stem == "rustup" || stem == "rustup-init" {
            return None;
        }
        let beside = path.parent().map(|d| d.join("rustup.exe"));
        let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".cargo")));
        let installed = cargo_home.map(|c| c.join("bin").join("rustup.exe"));
        [beside, installed].into_iter().flatten().find(|r| r.is_file() && crate::util::os::win32::same_file(path, r, true).unwrap_or(false))
    }

    pub(super) fn python() -> Vec<String> {
        let store = |p: &Path| p.components().any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"));
        let python = super::which("python");
        if let Some(p) = python.as_ref().filter(|p| !store(p)) {
            return vec![p.display().to_string()];
        }
        if let Some(py) = super::which("py") {
            return vec![py.display().to_string(), "-3".into()];
        }
        vec![python.map_or_else(|| "python".into(), |p| p.display().to_string())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cmd-shim's PowerShell shim for a global `npm install -g @anthropic-ai/claude-code`.
    const NPM_PS1: &str = r#"#!/usr/bin/env pwsh
$basedir=Split-Path $MyInvocation.MyCommand.Definition -Parent

$exe=""
if ($PSVersionTable.PSVersion -lt "6.0" -or $IsWindows) {
  # Fix case when both the Windows and Linux builds of Node
  # are installed in the same directory
  $exe=".exe"
}
$ret=0
if (Test-Path "$basedir/node$exe") {
  # Support pipeline input
  if ($MyInvocation.ExpectingInput) {
    $input | & "$basedir/node$exe"  "$basedir/node_modules/@anthropic-ai/claude-code/cli.js" $args
  } else {
    & "$basedir/node$exe"  "$basedir/node_modules/@anthropic-ai/claude-code/cli.js" $args
  }
  $ret=$LASTEXITCODE
} else {
  # Support pipeline input
  if ($MyInvocation.ExpectingInput) {
    $input | & "node$exe"  "$basedir/node_modules/@anthropic-ai/claude-code/cli.js" $args
  } else {
    & "node$exe"  "$basedir/node_modules/@anthropic-ai/claude-code/cli.js" $args
  }
  $ret=$LASTEXITCODE
}
exit $ret
"#;

    #[test]
    fn reads_npm_powershell_shims() {
        let script = "$basedir/node_modules/@anthropic-ai/claude-code/cli.js".to_string();
        assert_eq!(
            shim_calls(NPM_PS1).unwrap(),
            vec![("$basedir/node$exe".to_string(), vec![script.clone()]), ("node$exe".to_string(), vec![script])]
        );
        // Flags from the script's shebang come before it.
        let flags = "& \"$basedir/node$exe\"  --no-warnings \"$basedir/node_modules/x/cli.js\" $args\n";
        assert_eq!(shim_calls(flags).unwrap()[0].1, vec!["--no-warnings", "$basedir/node_modules/x/cli.js"]);
        // pnpm's shims set NODE_PATH; npm's own npm.ps1 runs variables: neither is unwrapped.
        assert!(shim_calls(&format!("$env:NODE_PATH=\"$new_node_path\"\n{NPM_PS1}")).is_none());
        assert!(shim_calls("$NODE_EXE=\"$PSScriptRoot/node.exe\"\n  & $NODE_EXE $NPM_CLI_JS $args\n").is_none());
        assert!(shim_calls("& \"$basedir/node$exe\" \"a\"b $args").is_none());
        assert!(shim_calls("& \"$basedir/node$exe\" `\"x $args").is_none());
        assert!(shim_calls("Write-Output hi").is_none());
    }

    #[test]
    fn batch_arguments_without_cmd_metacharacters() {
        assert!(batch_args_safe(&["--print", "fix the build", "C:\\work\\a b.txt", ""]));
        for bad in ["50%", "hi!", "a^b", "a & b", "a|b", "<x", "x>", "say \"hi\"", "two\nlines", "cr\r"] {
            assert!(!batch_args_safe(&["ok", bad]), "{bad:?}");
        }
    }

    #[test]
    fn resolved_argv_puts_the_prefix_first() {
        let r = Resolved { program: PathBuf::from("node"), prefix_args: vec!["cli.js".into()], kind: Kind::NpmShim };
        assert_eq!(r.argv(&["--version"]), vec!["node", "cli.js", "--version"]);
    }

    #[cfg(unix)]
    #[test]
    fn unix_lookup_rules() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        // A path is the file itself; the execute bit is not required to be found.
        assert_eq!(which(&tool.display().to_string()), Some(tool.clone()));
        assert_eq!(which(&dir.path().join("nope").display().to_string()), None);
        assert!(!is_executable(&tool));
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&tool));
        assert!(!is_executable(dir.path()));
        assert_eq!(find_in(&[dir.path().join("missing"), dir.path().to_path_buf()], "tool"), Some(tool.clone()));
        assert_eq!(resolve(&tool.display().to_string()), Some(Resolved { program: tool.clone(), prefix_args: vec![], kind: Kind::Exe }));
        assert!(names_path("./x") && names_path("~/bin/x") && !names_path("x") && !names_path("C:x"));
        assert_eq!(python(), vec!["python3"]);
        // A rustup proxy is a link to `rustup`; `rustup` itself is not one.
        std::fs::write(dir.path().join("rustup"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("rustup"), dir.path().join("rust-analyzer")).unwrap();
        let real = dir.path().join("rustup").canonicalize().unwrap();
        assert_eq!(rustup_proxy(&dir.path().join("rust-analyzer")), Some(real));
        assert_eq!(rustup_proxy(&dir.path().join("rustup")), None);
        assert_eq!(rustup_proxy(&tool), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_lookup_rules() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("tool.exe"), "").unwrap();
        std::fs::write(d.join("tool"), "#!/bin/sh\n").unwrap();
        // PATHEXT is appended; an extensionless file (npm's sh shim) is never a program.
        let found = which(&d.join("tool").display().to_string()).unwrap();
        assert!(found.is_absolute() && found.ends_with("tool.exe"), "{}", found.display());
        assert_eq!(find_in(&[d.to_path_buf()], "tool"), Some(found.clone()));
        assert!(is_executable(&found) && !is_executable(&d.join("tool")));
        assert!(names_path(r"C:\x") && names_path("C:x") && names_path(r"bin\x") && !names_path("x"));
        // A directory without a drive (`/usr/local/bin` is `C:\usr\local\bin`) is never searched.
        let rootless: PathBuf = d.components().skip(1).collect();
        assert!(!rootless.is_absolute());
        assert_eq!(find_in(&[rootless], "tool"), None);
        // An npm shim runs node with the package script; without the script it stays a batch file.
        std::fs::create_dir_all(d.join("node_modules/@anthropic-ai/claude-code")).unwrap();
        std::fs::write(d.join("node_modules/@anthropic-ai/claude-code/cli.js"), "").unwrap();
        std::fs::write(d.join("node.exe"), "").unwrap();
        std::fs::write(d.join("claude.cmd"), "@ECHO off\r\n").unwrap();
        std::fs::write(d.join("claude.ps1"), NPM_PS1).unwrap();
        let r = resolve(&d.join("claude.cmd").display().to_string()).unwrap();
        assert_eq!(r.kind, Kind::NpmShim);
        assert_eq!(r.program, std::path::absolute(d.join("node.exe")).unwrap());
        assert_eq!(r.prefix_args, vec![format!("{}\\node_modules\\@anthropic-ai\\claude-code\\cli.js", std::path::absolute(d).unwrap().display())]);
        std::fs::remove_file(d.join("node_modules/@anthropic-ai/claude-code/cli.js")).unwrap();
        assert_eq!(resolve(&d.join("claude.cmd").display().to_string()).unwrap().kind, Kind::Batch);
        // A rustup proxy is a hard link to rustup.exe.
        std::fs::write(d.join("rustup.exe"), "rustup").unwrap();
        std::fs::hard_link(d.join("rustup.exe"), d.join("rust-analyzer.exe")).unwrap();
        assert_eq!(rustup_proxy(&d.join("rust-analyzer.exe")), Some(d.join("rustup.exe")));
        assert_eq!(rustup_proxy(&d.join("tool.exe")), None);
        assert_eq!(rustup_proxy(&d.join("rustup.exe")), None);
    }
}
