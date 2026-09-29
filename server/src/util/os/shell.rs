//! Shells: the user's interactive shell, the shell that runs command lines (runs,
//! pre-launch steps, service commands, the notify command), quoting for it, and the
//! command line other programs use to start Workbench (the status line helper).
//!
//! Unix: `$SHELL -l`, `bash -lc`, POSIX quoting. Windows: PowerShell (`pwsh`, else the
//! Windows PowerShell every Windows 10 and 11 has); a command line travels UTF-16LE and
//! base64-encoded (`-EncodedCommand`), so no argv quoting on the way (portable-pty's,
//! cmd.exe's) can change it, and `quote` follows PowerShell's rules
//! (docs/windows-port.md §2).

use std::path::Path;

/// argv of the user's interactive shell for a terminal: `$SHELL -l` when it names a file,
/// else `/bin/bash -l` (Unix); `pwsh.exe -NoLogo`, else `powershell.exe -NoLogo` (Windows).
#[allow(dead_code)] // the terminals slice starts shells with it (docs/windows-port.md, step 7)
pub fn interactive() -> Vec<String> {
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty() && Path::new(s).is_file())
            .unwrap_or_else(|| "/bin/bash".into());
        vec![shell, "-l".into()]
    }
    #[cfg(windows)]
    {
        vec![win::powershell().display().to_string(), "-NoLogo".into()]
    }
}

/// argv running the command line `command` in the run shell: `bash -lc` on Unix (a login
/// shell, so the user's `PATH` and toolchains are set up); PowerShell without a profile
/// and with the command encoded on Windows.
pub fn run_argv(command: &str) -> Vec<String> {
    #[cfg(unix)]
    {
        vec!["bash".into(), "-lc".into(), command.to_string()]
    }
    #[cfg(windows)]
    {
        win::encoded_argv(command)
    }
}

/// `run_argv` as a process to spawn.
pub fn run_command(command: &str) -> tokio::process::Command {
    command_of(run_argv(command))
}

/// A short command from config.toml (the notify command) as a process to spawn: `sh -c`
/// (no login shell) on Unix; the run shell on Windows.
pub fn plain_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        command_of(vec!["sh".into(), "-c".into(), command.to_string()])
    }
    #[cfg(windows)]
    {
        command_of(win::encoded_argv(command))
    }
}

/// On Windows with `NoDefaultCurrentDirectoryInExePath`, so a cmd.exe started from the
/// command never runs a program from the current directory (a repository).
fn command_of(argv: Vec<String>) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&argv[0]);
    c.args(&argv[1..]);
    #[cfg(windows)]
    c.env("NoDefaultCurrentDirectoryInExePath", "1");
    c
}

/// argv running the bash script `script` on this computer (the dev container setup):
/// `bash <script>`; on Windows Git for Windows' bash, which takes `C:/…` paths.
pub fn script_argv(script: &Path) -> Vec<String> {
    #[cfg(unix)]
    {
        vec!["bash".into(), script.display().to_string()]
    }
    #[cfg(windows)]
    {
        let bash = win::git_bash().map_or_else(|| "bash.exe".into(), |p| p.display().to_string());
        vec![bash, script.display().to_string().replace('\\', "/")]
    }
}

/// `s` as one word of the run shell (`run_argv`): unchanged when it is plain, else
/// single-quoted (POSIX on Unix; PowerShell on Windows, where a quote is doubled).
pub fn quote(s: &str) -> String {
    #[cfg(unix)]
    {
        if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
            return s.to_string();
        }
        format!("'{}'", s.replace('\'', r"'\''"))
    }
    #[cfg(windows)]
    {
        ps_quote(s)
    }
}

/// A path for insertion at a shell or agent prompt (a pasted image), quoted like `quote`.
pub fn quote_path(p: &str) -> String {
    #[cfg(unix)]
    {
        if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c)) {
            p.to_string()
        } else {
            format!("'{}'", p.replace('\'', r"'\''"))
        }
    }
    #[cfg(windows)]
    {
        ps_quote(p)
    }
}

/// The command line another program runs to start Workbench's executable `exe` with
/// `args` (plain words): Claude Code's status line and `SessionStart` hooks. POSIX-quoted
/// on Unix; on Windows the path is double-quoted with forward slashes, which both cmd.exe
/// and Git Bash read.
pub fn helper_command(exe: &Path, args: &[&str]) -> String {
    #[cfg(unix)]
    {
        let q = |s: &str| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
                s.to_string()
            } else {
                format!("'{}'", s.replace('\'', r"'\''"))
            }
        };
        std::iter::once(q(&exe.to_string_lossy())).chain(args.iter().map(|a| q(a))).collect::<Vec<_>>().join(" ")
    }
    #[cfg(windows)]
    {
        let exe = dunce::simplified(exe).display().to_string().replace('\\', "/");
        std::iter::once(format!("\"{exe}\"")).chain(args.iter().map(|a| a.to_string())).collect::<Vec<_>>().join(" ")
    }
}

/// `s` as one PowerShell word: unchanged when plain, else a single-quoted string, in which
/// nothing is special but the quote itself, doubled (PowerShell also ends such a string
/// at a typographic quote, `‘ ’ ‚ ‛`).
#[cfg(any(windows, test))]
fn ps_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:\\=+".contains(c)) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// `-EncodedCommand`'s value: the command as UTF-16LE, base64.
#[cfg(any(windows, test))]
fn encode_command(command: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    use super::super::exe;

    /// PowerShell 7 (`pwsh.exe`) when installed, else Windows PowerShell 5.1.
    pub(super) fn powershell() -> PathBuf {
        if let Some(p) = exe::which("pwsh") {
            return p;
        }
        let inbox = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
        inbox.filter(|p| p.is_file()).or_else(|| exe::which("powershell")).unwrap_or_else(|| "powershell.exe".into())
    }

    pub(super) fn encoded_argv(command: &str) -> Vec<String> {
        vec![
            powershell().display().to_string(),
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-EncodedCommand".into(),
            super::encode_command(command),
        ]
    }

    /// Git for Windows' bash (`<Git>\bin\bash.exe`, found from `git.exe`), never WSL's
    /// `System32\bash.exe`, which cannot read Windows paths.
    pub(super) fn git_bash() -> Option<PathBuf> {
        let git = exe::which("git")?;
        git.ancestors().skip(1).take(3).map(|d| d.join("bin").join("bash.exe")).find(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_quoting() {
        assert_eq!(ps_quote(r"C:\Users\me\a.png"), r"C:\Users\me\a.png");
        assert_eq!(ps_quote("--release"), "--release");
        assert_eq!(ps_quote(""), "''");
        assert_eq!(ps_quote("a b"), "'a b'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote("it\u{2019}s"), "'it\u{2019}\u{2019}s'");
        assert_eq!(ps_quote("$(rm x); `n"), "'$(rm x); `n'");
        assert_eq!(ps_quote("--%"), "'--%'");
        assert_eq!(ps_quote("@args"), "'@args'");
    }

    #[test]
    fn encoded_commands_are_utf16le_base64() {
        assert_eq!(encode_command("dir"), "ZABpAHIA");
        assert_eq!(encode_command(""), "");
    }

    #[cfg(unix)]
    #[test]
    fn unix_shells_and_quoting() {
        assert_eq!(run_argv("cargo run"), vec!["bash", "-lc", "cargo run"]);
        assert_eq!(script_argv(Path::new("/d/up.sh")), vec!["bash", "/d/up.sh"]);
        assert_eq!(interactive()[1], "-l");
        assert_eq!(quote("http://127.0.0.1:8081/api/health"), "http://127.0.0.1:8081/api/health");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote_path("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(quote_path("/tmp/a=b.png"), "'/tmp/a=b.png'");
        assert_eq!(quote_path("/tmp/it's"), r"'/tmp/it'\''s'");
        assert_eq!(helper_command(Path::new("/home/u/.cache/wb/debug/workbench"), &["statusline"]), "/home/u/.cache/wb/debug/workbench statusline");
        assert_eq!(helper_command(Path::new("/opt/my apps/wb"), &["statusline"]), "'/opt/my apps/wb' statusline");
    }

    #[cfg(windows)]
    #[test]
    fn windows_shells_and_quoting() {
        let argv = run_argv("cargo run");
        assert_eq!(argv[1..4], ["-NoLogo", "-NoProfile", "-EncodedCommand"]);
        assert_eq!(argv[4], encode_command("cargo run"));
        assert!(Path::new(&argv[0]).is_absolute() || argv[0] == "powershell.exe", "{}", argv[0]);
        assert_eq!(interactive()[1], "-NoLogo");
        assert_eq!(quote("it's"), "'it''s'");
        assert_eq!(quote_path(r"C:\Users\me\my file.png"), r"'C:\Users\me\my file.png'");
        assert_eq!(
            helper_command(Path::new(r"C:\Program Files\Workbench\workbench.exe"), &["statusline"]),
            r#""C:/Program Files/Workbench/workbench.exe" statusline"#
        );
    }
}
