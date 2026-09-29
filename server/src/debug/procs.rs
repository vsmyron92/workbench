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

fn language_of(name: &str, command: &str) -> &'static str {
    let first = command.split_whitespace().next().unwrap_or(name);
    let base = first.rsplit('/').next().unwrap_or(first);
    if base.starts_with("python") || name.starts_with("python") {
        "python"
    } else if base == "node" || base == "deno" || base == "bun" {
        "javascript"
    } else if base == "java" {
        "java"
    } else {
        "native"
    }
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
        assert_eq!(language_of("python3", "/usr/bin/python3 app.py"), "python");
        assert!(ptrace_hint(Some(1)).unwrap().contains("ptrace_scope = 1"));
        assert_eq!(ptrace_hint(Some(0)), None);
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
