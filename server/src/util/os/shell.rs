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
    self::command(&run_argv(command))
}

/// A short command from config.toml (the notify command) as a process to spawn: `sh -c`
/// (no login shell) on Unix; the run shell on Windows.
pub fn plain_command(command: &str) -> tokio::process::Command {
    #[cfg(unix)]
    {
        self::command(&["sh".into(), "-c".into(), command.to_string()])
    }
    #[cfg(windows)]
    {
        self::command(&win::encoded_argv(command))
    }
}

/// A shell's argv (`run_argv`, or `apps::remote::argv`'s ssh) as a process to spawn, with
/// `exe::child_env`: on Windows a cmd.exe started from the command never runs a program
/// from the current directory (a repository).
pub fn command(argv: &[String]) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(&argv[0]);
    c.args(&argv[1..]).envs(super::exe::child_env().iter().copied());
    c
}

/// argv running the bash script `script` on this computer (the dev container setup):
/// `bash <script>`; on Windows Git for Windows' bash, which takes `C:/…` paths (an error
/// without it: a bare `bash.exe` would be WSL's).
pub fn script_argv(script: &Path) -> std::io::Result<Vec<String>> {
    #[cfg(unix)]
    {
        Ok(vec!["bash".into(), script.display().to_string()])
    }
    #[cfg(windows)]
    {
        let bash = win::git_bash().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Git for Windows' bash.exe was not found: install Git for Windows"))?;
        Ok(vec![bash.display().to_string(), script.display().to_string().replace('\\', "/")])
    }
}

/// `command` in the run shell with its standard output written to `file` (a `quote`d
/// path): `command > file` on Unix. On Windows the console code page becomes UTF-8 first,
/// since PowerShell before 7.4 decodes a program's output with it; Windows PowerShell 5.1
/// writes the file as UTF-16LE, so read it with `read_output`.
pub fn redirect_stdout(command: &str, file: &str) -> String {
    #[cfg(unix)]
    {
        format!("{command} > {file}")
    }
    #[cfg(windows)]
    {
        format!("try {{ [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false) }} catch {{ }}\n{command} > {file}")
    }
}

/// The text a `redirect_stdout` command wrote to `path`: UTF-8 on Unix; on Windows also
/// UTF-16LE with its byte order mark (Windows PowerShell 5.1), a UTF-8 one dropped.
pub fn read_output(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        std::fs::read_to_string(path).ok()
    }
    #[cfg(windows)]
    {
        decode_output(std::fs::read(path).ok()?)
    }
}

/// `bytes` as text by their byte order mark: UTF-16LE (`FF FE`), else UTF-8 (a BOM dropped).
#[cfg(any(windows, test))]
pub(super) fn decode_output(bytes: Vec<u8>) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if rest.len() % 2 != 0 {
            return None;
        }
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16(&units).ok();
    }
    let bytes = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => rest.to_vec(),
        None => bytes,
    };
    String::from_utf8(bytes).ok()
}

/// `s` as one word of the local run shell (`run_argv`): unchanged when it is plain, else
/// single-quoted (`posix_quote` on Unix; PowerShell on Windows, where a quote is doubled).
/// A command for an ssh host or a container takes `posix_quote` on every OS.
pub fn quote(s: &str) -> String {
    #[cfg(unix)]
    {
        posix_quote(s)
    }
    #[cfg(windows)]
    {
        ps_quote(s)
    }
}

/// `s` as one word of a POSIX shell, on every OS: unchanged when it is plain, else
/// single-quoted. For commands that run in `sh` or `bash` wherever Workbench runs: over ssh
/// (`apps::remote`), in a container, in a generated bash script.
pub fn posix_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The language of command lines for the local run shell (`run_argv`), which detected
/// run commands are written in (docs/windows-port.md §2, "Detected commands"). A
/// command for an ssh host or a container is POSIX on every OS (`posix_quote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// `bash -lc` (Unix).
    Posix,
    /// PowerShell (Windows): `pwsh`, or Windows PowerShell 5.1, which has no `&&`.
    PowerShell,
}

impl Dialect {
    /// The run shell's language on this OS.
    pub const HOST: Dialect = if cfg!(windows) { Dialect::PowerShell } else { Dialect::Posix };

    /// `s` as one word of this language (`quote` on its OS).
    pub fn quote(self, s: &str) -> String {
        match self {
            Dialect::Posix => posix_quote(s),
            Dialect::PowerShell => ps_quote(s),
        }
    }

    /// The program at `path` as the first word of a command: quoted like `quote`; a
    /// quoted one needs PowerShell's call operator (`& 'C:\my tools\x.exe'`), without
    /// which it is a string, not a command.
    pub fn program(self, path: &str) -> String {
        let q = self.quote(path);
        match self {
            Dialect::PowerShell if q.starts_with('\'') => format!("& {q}"),
            _ => q,
        }
    }

    /// `first`, then `then` when `first` succeeded: `first && then`. Windows PowerShell
    /// 5.1 has no `&&`, and `first; if ($?) { then }` would leave the exit status to what
    /// `$?` is after an `if`: a failure exits at once with `first`'s status, as `&&` does
    /// (`first; PS_EXIT_ON_FAILURE; then`).
    pub fn and_then(self, first: &str, then: &str) -> String {
        match self {
            Dialect::Posix => format!("{first} && {then}"),
            Dialect::PowerShell => format!("{first}; {PS_EXIT_ON_FAILURE}; {then}"),
        }
    }
}

/// The PowerShell statement that ends a script when the command before it failed, with the
/// status bash would give: the failed program's exit code, 127 when the command was not
/// found, else 1. `-Command` alone ends with 1 for any failure.
const PS_EXIT_ON_FAILURE: &str = "if (-not $?) { if ($LASTEXITCODE) { exit $LASTEXITCODE }; if ($Error[0].Exception -is [System.Management.Automation.CommandNotFoundException]) { exit 127 }; exit 1 }";

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

/// The script PowerShell runs for `command`: the command, then on its own line
/// `PS_EXIT_ON_FAILURE`, which keeps its exit status as bash does. A command that ends
/// with `exit N` keeps N.
#[cfg(any(windows, test))]
fn ps_script(command: &str) -> String {
    format!("{command}\n{PS_EXIT_ON_FAILURE}")
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

    /// PowerShell running `command` (`ps_script`). Windows PowerShell's default execution
    /// policy (Restricted) blocks every script, `npm.ps1` included, which PowerShell picks
    /// over `npm.cmd`: it gets pwsh's default, RemoteSigned, for this process only (Group
    /// Policy still wins; pwsh keeps what the user set).
    pub(super) fn encoded_argv(command: &str) -> Vec<String> {
        let ps = powershell();
        let mut argv = vec![ps.display().to_string(), "-NoLogo".into(), "-NoProfile".into()];
        if !ps.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("pwsh")) {
            argv.extend(["-ExecutionPolicy".into(), "RemoteSigned".into()]);
        }
        argv.extend(["-EncodedCommand".into(), super::encode_command(&super::ps_script(command))]);
        argv
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
    fn posix_quoting_on_every_os() {
        assert_eq!(posix_quote("http://127.0.0.1:8081/api/health"), "http://127.0.0.1:8081/api/health");
        assert_eq!(posix_quote(""), "''");
        assert_eq!(posix_quote("a b"), "'a b'");
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
        assert_eq!(posix_quote(r"C:\x"), r"'C:\x'");
    }

    #[test]
    fn dialects_quote_programs_and_chain_commands() {
        let (sh, ps) = (Dialect::Posix, Dialect::PowerShell);
        assert_eq!(sh.quote("a b"), "'a b'");
        assert_eq!(ps.quote("it's"), "'it''s'");
        assert_eq!(sh.program("./build/app"), "./build/app");
        assert_eq!(sh.program("./my app"), "'./my app'");
        assert_eq!(ps.program(r".\build\Debug\app.exe"), r".\build\Debug\app.exe");
        assert_eq!(ps.program(r".\my app.exe"), r"& '.\my app.exe'");
        assert_eq!(sh.and_then("make", "./app"), "make && ./app");
        // A failure keeps its status: the program's exit code, 127 for a command not found.
        assert_eq!(ps.and_then("cmake --build build", r".\app.exe"), format!(r"cmake --build build; {PS_EXIT_ON_FAILURE}; .\app.exe"));
        assert!(PS_EXIT_ON_FAILURE.contains("exit $LASTEXITCODE") && PS_EXIT_ON_FAILURE.contains("exit 127"));
        assert_eq!(Dialect::HOST, if cfg!(windows) { ps } else { sh });
    }

    #[test]
    fn encoded_commands_are_utf16le_base64() {
        assert_eq!(encode_command("dir"), "ZABpAHIA");
        assert_eq!(encode_command(""), "");
    }

    #[test]
    fn powershell_scripts_keep_the_exit_status() {
        let s = ps_script("npm run dev # watch");
        let (first, rest) = s.split_once('\n').unwrap();
        assert_eq!(first, "npm run dev # watch");
        assert!(rest.starts_with("if (-not $?) {") && rest.contains("exit $LASTEXITCODE") && rest.contains("exit 127"), "{rest}");
    }

    #[test]
    fn redirected_output_is_read_by_its_byte_order_mark() {
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("{\"é\":1}\r\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(decode_output(utf16).as_deref(), Some("{\"é\":1}\r\n"));
        assert_eq!(decode_output(b"\xEF\xBB\xBF{}".to_vec()).as_deref(), Some("{}"));
        assert_eq!(decode_output(b"{}\n".to_vec()).as_deref(), Some("{}\n"));
        assert_eq!(decode_output(vec![0xFF, 0xFE, 0x41]), None);
        assert_eq!(decode_output(vec![0xC3]), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_shells_and_quoting() {
        assert_eq!(run_argv("cargo run"), vec!["bash", "-lc", "cargo run"]);
        assert_eq!(script_argv(Path::new("/d/up.sh")).unwrap(), vec!["bash", "/d/up.sh"]);
        assert_eq!(redirect_stdout("cargo build", "/tmp/x.json"), "cargo build > /tmp/x.json");
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
        assert_eq!(argv[1..3], ["-NoLogo", "-NoProfile"]);
        assert_eq!(argv[argv.len() - 2], "-EncodedCommand");
        assert_eq!(argv[argv.len() - 1], encode_command(&ps_script("cargo run")));
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
