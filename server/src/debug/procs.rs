//! Processes the user may attach to: this user's own processes
//! (`os::proc::user_processes`), and what Linux's Yama `ptrace_scope` allows.

use serde::Serialize;

pub use crate::util::os::proc::ptrace_scope;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub command: String,
    /// Start time, milliseconds since the epoch (approximate).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// A guess of the language, for picking an adapter (`python`, `go`, `native`).
    pub language: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessList {
    pub processes: Vec<ProcessInfo>,
    pub truncated: bool,
    /// `/proc/sys/kernel/yama/ptrace_scope` (0 classic, 1 children only, 2 admin, 3 none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ptrace_scope: Option<u8>,
    /// What the scope means for attaching, when it restricts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ptrace_hint: Option<String>,
}

pub const MAX_PROCESSES: usize = 2000;

/// Why attaching may fail under this scope, and what to do.
pub fn ptrace_hint(scope: Option<u8>) -> Option<String> {
    match scope? {
        0 => None,
        1 => Some(
            "Linux only lets a debugger attach to its own child processes here (kernel.yama.ptrace_scope = 1). \
             Start the program from Workbench's debugger instead, allow attaching until the next reboot with \
             `sudo sysctl kernel.yama.ptrace_scope=0`, or let the program allow it (prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY))."
                .into(),
        ),
        2 => Some("Only root may attach a debugger here (kernel.yama.ptrace_scope = 2): run `sudo sysctl kernel.yama.ptrace_scope=0` to allow it until the next reboot.".into()),
        _ => Some("Attaching debuggers is disabled on this system (kernel.yama.ptrace_scope = 3) until the next reboot.".into()),
    }
}

/// A guess of a process's language: by its image name (`node`, `node.exe`), else by the
/// program its command line starts with (a process may name itself anything).
fn language_of(name: &str, command: &str) -> &'static str {
    match language_of_program(name) {
        "native" => language_of_program(program_name(command)),
        known => known,
    }
}

/// The language of the program file `file` (`python3`, `node.exe`, `javaw.exe`).
fn language_of_program(file: &str) -> &'static str {
    let base = without_exe(file).unwrap_or(file);
    if base.starts_with("python") {
        "python"
    } else if matches!(base, "node" | "deno" | "bun") {
        "javascript"
    } else if matches!(base, "java" | "javaw") {
        "java"
    } else {
        "native"
    }
}

/// The file name of the program a command line starts with (the process's arguments
/// joined with spaces, as listed): `/usr/bin/python3 app.py`, `"C:\Program Files\x\y.exe" a`,
/// or a Windows path with spaces unquoted, which ends at the first word ending in `.exe`
/// before anything that starts another argument (`C:\Program Files\nodejs\node.exe
/// app.js`). Both `/` and `\` separate: a guess from text, the same on every OS.
fn program_name(command: &str) -> &str {
    let command = command.trim_start();
    let path = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => {
            let first = command.split_whitespace().next().unwrap_or("");
            let b = first.as_bytes();
            let drive_path = (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')) || first.starts_with(r"\\");
            if drive_path && without_exe(first).is_none() { unquoted_exe(command).unwrap_or(first) } else { first }
        }
    };
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `s` without its final `.exe` (in any case), when it has one.
fn without_exe(s: &str) -> Option<&str> {
    let i = s.len().checked_sub(4)?;
    s.get(i..).filter(|ext| ext.eq_ignore_ascii_case(".exe")).map(|_| &s[..i])
}

/// The start of `command` up to the first word ending in `.exe`, taken as one path with
/// spaces; `None` when a word that starts another argument (`-x`, `/x`, a drive, a quote)
/// comes first.
fn unquoted_exe(command: &str) -> Option<&str> {
    let mut end = 0;
    for (i, word) in command.split_whitespace().enumerate() {
        let b = word.as_bytes();
        if i > 0 && (matches!(b[0], b'-' | b'/' | b'"') || (b.len() > 1 && b[0].is_ascii_alphabetic() && b[1] == b':')) {
            return None;
        }
        // The word's end: past the whitespace before it and the word itself.
        end += command[end..].find(word)? + word.len();
        if without_exe(word).is_some() {
            return Some(&command[..end]);
        }
    }
    None
}

/// This user's processes, newest first (Workbench itself and kernel threads left out).
pub fn list() -> ProcessList {
    let mut out = vec![];
    let mut truncated = false;
    for p in crate::util::os::proc::user_processes() {
        let mut command = p.command;
        if command.len() > 400 {
            let mut cut = 400;
            while !command.is_char_boundary(cut) {
                cut -= 1;
            }
            command.truncate(cut);
            command.push('…');
        }
        if out.len() >= MAX_PROCESSES {
            truncated = true;
            break;
        }
        let language = language_of(&p.name, &command);
        out.push(ProcessInfo { pid: p.pid, ppid: p.ppid, name: p.name, command, started_at: p.started_at, language });
    }
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.pid.cmp(&a.pid)));
    let scope = ptrace_scope();
    ProcessList { processes: out, truncated, ptrace_scope: scope, ptrace_hint: ptrace_hint(scope) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn lists_own_processes_with_a_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        // `spawn` can return a moment before the kernel renames the child from our thread's
        // name to `sleep` (exec closes the status pipe first), so wait for the name.
        let mut l = list();
        for _ in 0..100 {
            if l.processes.iter().any(|p| p.pid == child.id() && p.name == "sleep") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            l = list();
        }
        let found = l.processes.iter().find(|p| p.pid == child.id());
        let _ = child.kill();
        let _ = child.wait();
        let p = found.expect("our child is listed");
        assert_eq!(p.name, "sleep");
        assert_eq!(p.command, "sleep 30");
        assert_eq!(p.ppid, std::process::id());
        assert!(!l.processes.iter().any(|p| p.pid == std::process::id()), "Workbench itself is not offered");
    }

    #[test]
    fn languages_and_hints() {
        assert_eq!(language_of("python3", "/usr/bin/python3 app.py"), "python");
        assert_eq!(language_of("node", "node server.js"), "javascript");
        assert_eq!(language_of("java", "/usr/lib/jvm/bin/java -jar app.jar"), "java");
        assert_eq!(language_of("sleep", "sleep 30"), "native");
        // A process that names its main thread: the command line decides.
        assert_eq!(language_of("MainThread", "/usr/bin/node server.js"), "javascript");
        assert_eq!(language_of("MainThread", "/usr/bin/bun x"), "javascript");
        // Windows image names, whatever the command line holds.
        assert_eq!(language_of("node.exe", r"C:\Program Files\nodejs\node.exe app.js"), "javascript");
        assert_eq!(language_of("deno.EXE", ""), "javascript");
        assert_eq!(language_of("java.exe", "java -jar app.jar"), "java");
        assert_eq!(language_of("javaw.exe", r"C:\jdk\bin\javaw.exe -jar ide.jar"), "java");
        assert_eq!(language_of("python.exe", r"C:\Python312\python.exe -m http.server"), "python");
        // Unknown image names: the program the command line starts with, `\` paths and spaces included.
        assert_eq!(language_of("", r"C:\Program Files\nodejs\node.exe app.js"), "javascript");
        assert_eq!(language_of("", r#""C:\Program Files\Java\bin\java.exe" -jar app.jar"#), "java");
        assert_eq!(language_of("", r"C:\Python312\python.exe"), "python");
        assert_eq!(language_of("", r"\\server\tools\deno.exe run x"), "javascript");
        assert_eq!(language_of("", r"C:\tools\run --out C:\x\node.exe"), "native");
        assert_eq!(language_of("", r"C:\tools\run C:\x\node.exe"), "native");
        assert_eq!(language_of("", r"C:\tools\run\node"), "javascript");
        assert_eq!(language_of("", ""), "native");
        assert!(ptrace_hint(Some(1)).unwrap().contains("ptrace_scope = 1"));
        assert_eq!(ptrace_hint(Some(0)), None);
        assert_eq!(ptrace_hint(None), None);
    }

    #[cfg(windows)]
    #[test]
    fn lists_own_processes_with_a_child() {
        let mut child = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let mut l = list();
        for _ in 0..100 {
            if l.processes.iter().any(|p| p.pid == child.id() && p.command.contains("127.0.0.1")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            l = list();
        }
        let found = l.processes.iter().find(|p| p.pid == child.id()).cloned();
        let _ = child.kill();
        let _ = child.wait();
        let p = found.expect("our child is listed");
        assert!(p.name.eq_ignore_ascii_case("ping.exe"), "{}", p.name);
        assert!(p.command.ends_with("-n 30 127.0.0.1"), "{}", p.command);
        assert_eq!(p.ppid, std::process::id());
        assert!(p.started_at.is_some());
        assert!(!l.processes.iter().any(|p| p.pid == std::process::id()), "Workbench itself is not offered");
        assert_eq!(l.ptrace_scope, None);
        assert_eq!(l.ptrace_hint, None);
    }
}
