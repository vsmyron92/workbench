//! Processes the user may attach to: this user's own processes from `/proc`, and
//! what Linux's Yama `ptrace_scope` allows.

use std::path::Path;

use serde::Serialize;

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

pub fn ptrace_scope() -> Option<u8> {
    std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok()?.trim().parse().ok()
}

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

/// `(ppid, starttime ticks)` from `/proc/<pid>/stat` (the name may contain spaces
/// and parentheses; fields after the last `)` are fixed).
fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] = state, f[1] = ppid, … f[19] = starttime.
    Some((f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?))
}

fn boot_time_ms() -> Option<i64> {
    let s = std::fs::read_to_string("/proc/stat").ok()?;
    let secs: i64 = s.lines().find_map(|l| l.strip_prefix("btime "))?.trim().parse().ok()?;
    Some(secs * 1000)
}

/// This user's processes, newest first (Workbench itself and kernel threads left out).
pub fn list(proc_root: &Path) -> ProcessList {
    use std::os::unix::fs::MetadataExt;
    let uid = nix::unistd::getuid().as_raw();
    let me = std::process::id();
    let boot = boot_time_ms();
    let ticks = 100i64; // USER_HZ on Linux
    let mut out = vec![];
    let mut truncated = false;
    if let Ok(rd) = std::fs::read_dir(proc_root) {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if pid == me {
                continue;
            }
            let dir = e.path();
            let Ok(meta) = std::fs::metadata(&dir) else { continue };
            if meta.uid() != uid {
                continue;
            }
            let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
            if cmdline.is_empty() {
                continue; // kernel thread or zombie
            }
            let mut command = cmdline.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join(" ");
            if command.len() > 400 {
                let mut cut = 400;
                while !command.is_char_boundary(cut) {
                    cut -= 1;
                }
                command.truncate(cut);
                command.push('…');
            }
            let name = std::fs::read_to_string(dir.join("comm")).map(|s| s.trim().to_string()).unwrap_or_default();
            let (ppid, start) = std::fs::read_to_string(dir.join("stat")).ok().and_then(|s| parse_stat(&s)).unwrap_or((0, 0));
            if out.len() >= MAX_PROCESSES {
                truncated = true;
                break;
            }
            let language = language_of(&name, &command);
            out.push(ProcessInfo { pid, ppid, name, command, started_at: boot.map(|b| b + start as i64 * 1000 / ticks), language });
        }
    }
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.pid.cmp(&a.pid)));
    let scope = ptrace_scope();
    ProcessList { processes: out, truncated, ptrace_scope: scope, ptrace_hint: ptrace_hint(scope) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_parsing_survives_odd_names() {
        let s = "4145729 (my (odd) prog) S 4145700 4145729 1 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 100";
        assert_eq!(parse_stat(s), Some((4145700, 987654)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn lists_own_processes_with_a_child() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        // `spawn` can return a moment before the kernel renames the child from our thread's
        // name to `sleep` (exec closes the status pipe first), so wait for the name.
        let mut l = list(Path::new("/proc"));
        for _ in 0..100 {
            if l.processes.iter().any(|p| p.pid == child.id() && p.name == "sleep") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            l = list(Path::new("/proc"));
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
}
