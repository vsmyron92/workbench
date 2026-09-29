//! Shells: the user's interactive shell, the shell that runs command lines (runs,
//! pre-launch steps, service commands, the notify command), quoting for it, what it writes
//! to a piped stderr as text, and the command line other programs use to start Workbench
//! (the status line helper).
//!
//! Unix: `$SHELL -l`, `bash -lc`, POSIX quoting. Windows: PowerShell (`pwsh`, else the
//! Windows PowerShell every Windows 10 and 11 has); a command line travels UTF-16LE and
//! base64-encoded (`-EncodedCommand`), so no argv quoting on the way (portable-pty's,
//! cmd.exe's) can change it, `quote` follows PowerShell's rules, and the CLIXML PowerShell
//! writes its errors in on a pipe is read as text (docs/windows-port.md §2).

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

/// What a program started from `run_command` or `command` wrote to its piped stderr, as the
/// text to show (`util::proc::run_cmd`): UTF-8 (lossy) on Unix. On Windows also the run
/// shell's own records made readable (`powershell_stderr`).
pub fn readable_stderr(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    #[cfg(unix)]
    {
        s.into_owned()
    }
    #[cfg(windows)]
    {
        powershell_stderr(&s)
    }
}

/// The line PowerShell writes before the CLIXML it puts on a redirected stream.
#[cfg(any(windows, test))]
const CLIXML_HEADER: &str = "#< CLIXML";

/// PowerShell's stderr as the text its console would show. Started with `-EncodedCommand`,
/// not interactive and with stderr redirected, PowerShell serializes its own records there
/// as CLIXML (`#< CLIXML`, then `<Objs …><S S="Error">…_x000D__x000A_</S>…</Objs>`):
/// Windows PowerShell 5.1 always, pwsh without `-OutputFormat Text` (`win::argv_for` passes
/// it). Error strings stay as they are, warning, verbose and debug ones get the console's
/// `WARNING: ` prefix, objects (progress, information) are dropped, and any other text (a
/// native program's own stderr, which PowerShell leaves raw) is kept. Colour escapes (pwsh
/// 7's error view writes them in either format) and carriage returns are removed.
#[cfg(any(windows, test))]
fn powershell_stderr(s: &str) -> String {
    let text = if s.contains(CLIXML_HEADER) { from_clixml(s) } else { s.to_string() };
    crate::util::ansi::strip(&text)
}

/// `s` with its CLIXML (`powershell_stderr`) turned into text. Tolerant: text between or
/// around the elements stays, an element that does not parse stays as text, and an
/// unterminated `<Objs>` (a process ended mid-write) ends where the text does.
#[cfg(any(windows, test))]
fn from_clixml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut in_objs = false;
    while let Some(i) = rest.find(['#', '<']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match clixml_markup(rest, &mut in_objs, &mut out) {
            Some(len) => rest = &rest[len..],
            // Not markup: the character is text.
            None => {
                out.push_str(&rest[..1]);
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The CLIXML markup at the start of `s` (`from_clixml`), its text written to `out`: how
/// long it is. `None` when there is none.
#[cfg(any(windows, test))]
fn clixml_markup(s: &str, in_objs: &mut bool, out: &mut String) -> Option<usize> {
    if let Some(r) = s.strip_prefix(CLIXML_HEADER) {
        let eol = if r.starts_with("\r\n") { 2 } else { usize::from(r.starts_with('\n')) };
        return Some(CLIXML_HEADER.len() + eol);
    }
    if *in_objs && s.starts_with("</Objs>") {
        *in_objs = false;
        return Some("</Objs>".len());
    }
    let (name, tag, closed) = start_tag(s)?;
    if !*in_objs {
        // Outside the list, only its start is markup.
        if name != "Objs" {
            return None;
        }
        *in_objs = !closed;
        return Some(tag.len());
    }
    let body = &s[tag.len()..];
    match name {
        "S" => {
            let (text, len) = match body.find("</S>") {
                _ if closed => ("", 0),
                Some(end) => (&body[..end], end + "</S>".len()),
                // Cut off (the process ended mid-write): to the end.
                None => (body, body.len()),
            };
            clixml_line(out, &attr(tag, "S").unwrap_or_default(), &clixml_string(text));
            Some(tag.len() + len)
        }
        // An object (a progress or information record): dropped, to the end when cut off.
        "Obj" => Some(tag.len() + if closed { 0 } else { element_end(body, name).unwrap_or(body.len()) }),
        // Only strings and objects are records: anything else is text (a native program's).
        _ => None,
    }
}

/// The start tag at the beginning of `s` (`<Name attr="…">` or `<Name …/>`): its name, the
/// whole tag, and whether it closes itself.
#[cfg(any(windows, test))]
fn start_tag(s: &str) -> Option<(&str, &str, bool)> {
    /// CLIXML's start tags are short: a `<` in text that no `>` follows soon is text.
    const MAX_TAG: usize = 256;
    let b = s.as_bytes();
    if b.first() != Some(&b'<') {
        return None;
    }
    let name_len = b[1..].iter().position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))?;
    if name_len == 0 || !matches!(b[1 + name_len], b' ' | b'>' | b'/') {
        return None;
    }
    let end = b.iter().take(MAX_TAG).position(|&c| c == b'>')?;
    // ASCII at 1 + name_len and at end: character boundaries.
    let tag = &s[..end + 1];
    Some((&s[1..1 + name_len], tag, tag.ends_with("/>")))
}

/// The value of the attribute `name` in the start tag `tag`, unescaped.
#[cfg(any(windows, test))]
fn attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let len = tag[at..].find('"')?;
    Some(xml_unescape(&tag[at..at + len]))
}

/// How far into `body` (what follows a `<name …>` start tag) its element ends, after its
/// `</name>`, counting nested elements of the same name.
#[cfg(any(windows, test))]
fn element_end(body: &str, name: &str) -> Option<usize> {
    let close = format!("</{name}>");
    let mut depth = 1;
    let mut at = 0;
    while depth > 0 {
        let i = at + body[at..].find('<')?;
        let s = &body[i..];
        if s.starts_with(&close) {
            depth -= 1;
            at = i + close.len();
        } else {
            match start_tag(s) {
                Some((n, tag, closed)) => {
                    depth += usize::from(n == name && !closed);
                    at = i + tag.len();
                }
                None => at = i + 1,
            }
        }
    }
    Some(at)
}

/// One string of the CLIXML stream `stream` as a console line (`powershell_stderr`).
#[cfg(any(windows, test))]
fn clixml_line(out: &mut String, stream: &str, text: &str) {
    let prefix = match stream.to_ascii_lowercase().as_str() {
        "warning" => "WARNING: ",
        "verbose" => "VERBOSE: ",
        "debug" => "DEBUG: ",
        _ => "",
    };
    out.push_str(prefix);
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// A CLIXML string's text: XML's character references, then PowerShell's `_xHHHH_` escapes
/// (UTF-16 code units: control characters, each half of a surrogate pair, `_x005F_` for an
/// `_` that comes before an `x`).
#[cfg(any(windows, test))]
fn clixml_string(raw: &str) -> String {
    let s = xml_unescape(raw);
    let b = s.as_bytes();
    let mut units: Vec<u16> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if b[i] == b'_' && b.get(i + 1) == Some(&b'x') && b.get(i + 6) == Some(&b'_') && b[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit) {
            // ASCII at i + 2 and i + 6: both are character boundaries.
            if let Ok(u) = u16::from_str_radix(&s[i + 2..i + 6], 16) {
                units.push(u);
                i += 7;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap_or_default();
        units.extend_from_slice(c.encode_utf16(&mut [0; 2]));
        i += c.len_utf8();
    }
    String::from_utf16_lossy(&units)
}

/// `s` with XML's entities and character references replaced; anything else stays.
#[cfg(any(windows, test))]
fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let c = match &rest[1..e] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                r => match r.strip_prefix("#x").or_else(|| r.strip_prefix("#X")) {
                    Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?)?,
                    None => char::from_u32(r.strip_prefix('#')?.parse().ok()?)?,
                },
            };
            Some((c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
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
    use std::path::{Path, PathBuf};

    use super::super::exe;

    /// PowerShell 7 (`pwsh.exe`) when installed, else Windows PowerShell 5.1.
    pub(super) fn powershell() -> PathBuf {
        if let Some(p) = exe::which("pwsh") {
            return p;
        }
        windows_powershell().or_else(|| exe::which("powershell")).unwrap_or_else(|| "powershell.exe".into())
    }

    /// Windows PowerShell 5.1, which every Windows 10 and 11 has in System32.
    pub(super) fn windows_powershell() -> Option<PathBuf> {
        let inbox = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
        inbox.filter(|p| p.is_file())
    }

    /// PowerShell running `command` (`ps_script`).
    pub(super) fn encoded_argv(command: &str) -> Vec<String> {
        argv_for(&powershell(), command)
    }

    /// The PowerShell `ps` running `command` (`ps_script`).
    /// - `-OutputFormat Text`: pwsh (6.2 and later) then writes its own errors to a
    ///   redirected stderr as text. Without it, and always in Windows PowerShell 5.1 (which
    ///   has no such exception), they are CLIXML there when the command came as
    ///   `-EncodedCommand`; `readable_stderr` reads that. It is the default output format:
    ///   nothing else changes.
    /// - Windows PowerShell's default execution policy (Restricted) blocks every script,
    ///   `npm.ps1` included, which PowerShell picks over `npm.cmd`: it gets pwsh's default,
    ///   RemoteSigned, for this process only (Group Policy still wins; pwsh keeps what the
    ///   user set).
    pub(super) fn argv_for(ps: &Path, command: &str) -> Vec<String> {
        let mut argv = vec![ps.display().to_string(), "-NoLogo".into(), "-NoProfile".into(), "-OutputFormat".into(), "Text".into()];
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

    /// What PowerShell 7.4.6 wrote to a piped stderr for `no-such-cmd-xyz` run as the run shell
    /// runs it (`-NoLogo -NoProfile -EncodedCommand`, stdin null), byte for byte.
    const PWSH_NOT_FOUND_CLIXML: &str = concat!(
        "#< CLIXML\n",
        r#"<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04">"#,
        r#"<S S="Error">_x001B_[31;1mno-such-cmd-xyz: _x001B_[31;1mThe term 'no-such-cmd-xyz' is not recognized as a name of a cmdlet, function, script file, or executable program._x001B_[0m_x000A_</S>"#,
        r#"<S S="Error">_x001B_[31;1m_x001B_[31;1mCheck the spelling of the name, or if a path was included, verify that the path is correct and try again._x001B_[0m_x000A_</S>"#,
        "</Objs>"
    );
    /// The same with `-OutputFormat Text`: plain text, colour escapes included.
    const PWSH_NOT_FOUND_TEXT: &str = "\x1b[31;1mno-such-cmd-xyz: \x1b[31;1mThe term 'no-such-cmd-xyz' is not recognized as a name of a cmdlet, function, script file, or executable program.\x1b[0m\n\x1b[31;1m\x1b[31;1mCheck the spelling of the name, or if a path was included, verify that the path is correct and try again.\x1b[0m\n";
    const PWSH_NOT_FOUND: &str = "no-such-cmd-xyz: The term 'no-such-cmd-xyz' is not recognized as a name of a cmdlet, function, script file, or executable program.\nCheck the spelling of the name, or if a path was included, verify that the path is correct and try again.\n";

    #[test]
    fn powershell_clixml_errors_read_as_the_console_shows_them() {
        assert_eq!(powershell_stderr(PWSH_NOT_FOUND_CLIXML), PWSH_NOT_FOUND);
        assert_eq!(powershell_stderr(PWSH_NOT_FOUND_TEXT), PWSH_NOT_FOUND);

        // PowerShell 7.4.6 for `Write-Warning 'careful: a_x & <b>'; Write-Progress …;
        // Write-Information 'info'; Write-Verbose 'loud' -Verbose; sh -c 'echo native-err >&2';
        // Write-Error "bad: é ✓ 😀 _x000A_ tab`tend"`. Its information record's user,
        // computer, ids and time are replaced. The native program's stderr stays raw, before
        // the list PowerShell's XML writer held back; XML and `_xHHHH_` escapes (a surrogate
        // pair, `_x005F_` for an `_` before an `x`) are decoded; objects are dropped.
        let mixed = concat!(
            "#< CLIXML\nnative-err\n",
            r#"<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04">"#,
            r#"<S S="warning">careful: a_x005F_x &amp; &lt;b&gt;</S>"#,
            r#"<Obj S="information" RefId="0"><TN RefId="0"><T>System.Management.Automation.InformationRecord</T><T>System.Object</T></TN><ToString>info</ToString>"#,
            r#"<Props><S N="MessageData">info</S><S N="Source">Write-Information</S><DT N="TimeGenerated">2026-01-01T12:00:00.0000000+00:00</DT>"#,
            r#"<Obj N="Tags" RefId="1"><TN RefId="1"><T>System.Collections.Generic.List`1[[System.String, System.Private.CoreLib, Version=8.0.0.0, Culture=neutral, PublicKeyToken=7cec85d7bea7798e]]</T><T>System.Object</T></TN><LST /></Obj>"#,
            r#"<S N="User">user</S><S N="Computer">host</S><U32 N="ProcessId">4242</U32><U32 N="NativeThreadId">4243</U32><U32 N="ManagedThreadId">15</U32></Props></Obj>"#,
            r#"<S S="verbose">loud</S>"#,
            "<S S=\"Error\">_x001B_[31;1mWrite-Error: _x001B_[31;1mbad: é ✓ _xD83D__xDE00_ _x005F_x000A_ tab_x0009_end_x001B_[0m_x000A_</S>",
            "</Objs>"
        );
        assert_eq!(powershell_stderr(mixed), "native-err\nWARNING: careful: a_x & <b>\nVERBOSE: loud\nWrite-Error: bad: é ✓ 😀 _x000A_ tab\tend\n");

        // PowerShell 7.4.6 for `Write-Warning 'first'; <a native program printing "usage:
        // tool <file> & more" to stderr>; throw 'stopped: 100% <done>'`: raw text is not XML.
        let thrown = concat!(
            "#< CLIXML\nusage: tool <file> & more\n",
            r#"<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04">"#,
            r#"<S S="warning">first</S><S S="Error">_x001B_[31;1mException: _x001B_[31;1mstopped: 100% &lt;done&gt;_x001B_[0m_x000A_</S></Objs>"#
        );
        assert_eq!(powershell_stderr(thrown), "usage: tool <file> & more\nWARNING: first\nException: stopped: 100% <done>\n");

        // Windows PowerShell 5.1's form: CRLF, the error view's lines one string each (wrapped
        // at the console's width), after the progress record of its first module load.
        let windows_powershell = concat!(
            "#< CLIXML\r\n",
            r#"<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04">"#,
            r#"<Obj S="progress" RefId="0"><TN RefId="0"><T>System.Management.Automation.PSCustomObject</T><T>System.Object</T></TN><MS><I64 N="SourceId">1</I64>"#,
            r#"<PR N="Record"><AV>Preparing modules for first use.</AV><AI>0</AI><Nil /><PI>-1</PI><PC>-1</PC><T>Completed</T><SR>-1</SR><SD> </SD></PR></MS></Obj>"#,
            r#"<S S="Error">no-such-cmd-xyz : The term 'no-such-cmd-xyz' is not recognized as the name of a cmdlet, function, script file, or operable _x000D__x000A_</S>"#,
            r#"<S S="Error">program. Check the spelling of the name, or if a path was included, verify that the path is correct and try again._x000D__x000A_</S>"#,
            r#"<S S="Error">At line:1 char:1_x000D__x000A_</S><S S="Error">+ no-such-cmd-xyz_x000D__x000A_</S><S S="Error">+ ~~~~~~~~~~~~~~~_x000D__x000A_</S>"#,
            r#"<S S="Error">    + CategoryInfo          : ObjectNotFound: (no-such-cmd-xyz:String) [], CommandNotFoundException_x000D__x000A_</S>"#,
            r#"<S S="Error">    + FullyQualifiedErrorId : CommandNotFoundException_x000D__x000A_</S><S S="Error"> _x000D__x000A_</S></Objs>"#
        );
        assert_eq!(
            powershell_stderr(windows_powershell),
            "no-such-cmd-xyz : The term 'no-such-cmd-xyz' is not recognized as the name of a cmdlet, function, script file, or operable \n\
             program. Check the spelling of the name, or if a path was included, verify that the path is correct and try again.\n\
             At line:1 char:1\n+ no-such-cmd-xyz\n+ ~~~~~~~~~~~~~~~\n\
             \x20   + CategoryInfo          : ObjectNotFound: (no-such-cmd-xyz:String) [], CommandNotFoundException\n\
             \x20   + FullyQualifiedErrorId : CommandNotFoundException\n \n"
        );
    }

    #[test]
    fn powershell_stderr_is_read_tolerantly() {
        // Not CLIXML: as it is, without colour escapes and carriage returns.
        assert_eq!(powershell_stderr("fatal: not a git repository\r\n"), "fatal: not a git repository\n");
        assert_eq!(powershell_stderr("a <b> & c"), "a <b> & c");
        let objs = r#"<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04">"#;
        // pwsh run from pwsh: two headers, two lists.
        assert_eq!(powershell_stderr(&format!("#< CLIXML\r\n#< CLIXML\r\n{objs}<S S=\"Error\">one</S></Objs>{objs}<S S=\"Error\">two</S></Objs>")), "one\ntwo\n");
        // Cut off mid-write: what there is.
        assert_eq!(powershell_stderr(&format!("#< CLIXML\r\n{objs}<S S=\"Error\">one_x000D__x000A_</S><S S=\"Error\">tw")), "one\ntw\n");
        assert_eq!(powershell_stderr(&format!("#< CLIXML\r\n{objs}<S S=\"Error\">one</S><Obj S=\"progress\" RefId=\"0\"><TN>")), "one\n");
        // Native text among the records, markup-like or not; an empty string; the debug stream.
        assert_eq!(
            powershell_stderr(&format!("#< CLIXML\n{objs}<S S=\"debug\">d</S>usage: x <file>\n<S S=\"Error\" />&amp;</Objs>")),
            "DEBUG: d\nusage: x <file>\n\n&amp;"
        );
        let long = format!("<Obj {}>", "x".repeat(300));
        assert_eq!(powershell_stderr(&format!("#< CLIXML\n{objs}{long}</Obj></Objs>")), format!("{long}</Obj>"));
        // Escapes that are not ones stay.
        assert_eq!(clixml_string("_x00G1_ _x41 &unknown; &#xZZ; _x0041_&#65;&#x42;"), "_x00G1_ _x41 &unknown; &#xZZ; AAB");
        // A lone surrogate becomes U+FFFD, not an error.
        assert_eq!(clixml_string("_xD83D_!"), "\u{FFFD}!");
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
        assert_eq!(argv[1..5], ["-NoLogo", "-NoProfile", "-OutputFormat", "Text"]);
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

    /// A failing command's message, as a stop command's or a version probe's error shows it,
    /// is text in both PowerShells: Windows PowerShell 5.1 writes CLIXML to the pipe, pwsh
    /// plain text (`-OutputFormat Text`) with colour escapes.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_powershell_errors_read_as_text() {
        let mut shells = vec![win::windows_powershell().expect("System32 powershell.exe")];
        match crate::util::os::exe::which("pwsh") {
            Some(pwsh) => shells.push(pwsh),
            None => eprintln!("pwsh is not installed: only Windows PowerShell is checked"),
        }
        // Windows PowerShell wraps its error view at the console's width.
        let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        for ps in shells {
            let run = |line: &str| crate::util::proc::run_cmd(super::command(&win::argv_for(&ps, line)), std::time::Duration::from_secs(90));
            let out = run("no-such-cmd-xyz").await.unwrap();
            let msg = out.message();
            assert_eq!(out.code, Some(127), "{}: {msg}", ps.display());
            assert!(words(&msg).contains("'no-such-cmd-xyz' is not recognized"), "{}: {msg:?}", ps.display());
            assert!(!msg.contains("CLIXML") && !msg.contains("_x00") && !msg.contains(['<', '\u{1b}', '\r']), "{}: {msg:?}", ps.display());
            let out = run("Write-Warning 'w & <x>'; Write-Error 'e & <y>'").await.unwrap();
            let msg = out.message();
            assert_eq!(out.code, Some(1), "{}: {msg}", ps.display());
            assert!(words(&msg).contains("e & <y>") && !msg.contains("CLIXML") && !msg.contains("&lt;") && !msg.contains('\u{1b}'), "{}: {msg:?}", ps.display());
        }
    }
}
