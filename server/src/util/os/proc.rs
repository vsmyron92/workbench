//! Processes: children that end together with what they start (`ProcGroup`), single pids,
//! exit statuses, this executable, the debugger's process list, and the server's shutdown
//! signal.
//!
//! Unix: a child leads its own process group (or session) and is signalled through it.
//! Windows: a child starts in a new process group with a hidden console of its own, so the
//! server's Ctrl-C never reaches it, and joins a Job Object right after spawn; closing the
//! last handle to the job kills what is left in it (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`).
//! There, `terminate` and `kill` both end the job: the graceful step is the protocol one
//! that callers run first (LSP shutdown/exit, DAP disconnect).

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use tokio::process::{Child, Command};

// ---------------------------------------------------------------- process groups

/// A child process and the processes it starts, ended together. Empty (every call does
/// nothing) when the child was gone before `attach`. Windows: dropping the last clone
/// kills what is left in the job.
#[derive(Clone, Default)]
pub struct ProcGroup(imp::Group);

impl ProcGroup {
    /// Before spawning: the child leads a new process group, so it can be ended with what
    /// it starts and signals meant for Workbench's own group do not reach it.
    pub fn prepare(cmd: &mut Command) {
        imp::prepare(cmd);
    }

    /// Before spawning: the child leads a new session (Unix), so it has no controlling
    /// terminal to prompt on. Windows: as `prepare`, but with no console at all
    /// (`DETACHED_PROCESS`): Git for Windows then starts ssh without one too, and ssh fails
    /// instead of prompting on a hidden console nobody sees.
    pub fn prepare_session(cmd: &mut Command) {
        imp::prepare_session(cmd);
    }

    /// Right after spawning a child prepared with `prepare` or `prepare_session`.
    pub fn attach(child: &Child) -> ProcGroup {
        child.id().map(Self::attach_pid).unwrap_or_default()
    }

    /// `attach` for a child known by its pid, the leader of its own group (a PTY's
    /// process). Windows: the process joins a new job; what it started before stays out.
    pub fn attach_pid(pid: u32) -> ProcGroup {
        ProcGroup(imp::Group::attach(pid))
    }

    /// `attach_pid` for a terminal's process (`os::session`): what it starts may leave the
    /// job when it asks to (`CREATE_BREAKAWAY_FROM_JOB`, allowed by
    /// `JOB_OBJECT_LIMIT_BREAKAWAY_OK`), as a Unix daemon leaves its terminal's session. The
    /// service `workbench service install --enable` starts from a Workbench terminal then
    /// outlives that terminal. With `reports` (a completion port and a key), the job reports
    /// its events there, `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO` once no process of it runs;
    /// the port is associated before the process joins, so no report is missed.
    #[cfg(windows)]
    pub fn attach_terminal(pid: u32, reports: Option<(windows_sys::Win32::Foundation::HANDLE, usize)>) -> ProcGroup {
        ProcGroup(imp::Group::attach_job(pid, true, reports))
    }

    /// `attach_pid` for a child spawned without `prepare`, which stays in Workbench's group:
    /// on Unix the group is that process alone (`terminate` and `kill` signal its pid).
    /// Windows: as `attach_pid`, so what it starts from then on ends with it.
    pub fn attach_single(pid: u32) -> ProcGroup {
        ProcGroup(imp::Group::attach_single(pid))
    }

    /// Ask the group to end: SIGTERM (Unix). Windows: ends the job.
    pub fn terminate(&self) {
        self.0.terminate();
    }

    /// End the group now: SIGKILL (Unix). Windows: ends the job.
    pub fn kill(&self) {
        self.0.kill();
    }

    /// SIGTERM to the leader alone, which may have left the group (Unix). Only while it has
    /// not been reaped: its pid could have been reused then. Windows: ends the leader.
    pub fn terminate_leader(&self) {
        self.0.terminate_leader();
    }

    /// SIGKILL to the leader alone (see `terminate_leader`). Windows: ends the leader.
    pub fn kill_leader(&self) {
        self.0.kill_leader();
    }

    /// The pids in the group, the leader included while it runs (Unix: scans `/proc`).
    #[cfg_attr(unix, allow(dead_code))] // Windows: a terminal's session (`os::session`)
    pub fn members(&self) -> Vec<u32> {
        self.0.members()
    }
}

// ---------------------------------------------------------------- single processes

/// Whether a pid is alive, including processes of other users. Pid 0 is none.
pub fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid)
}

/// Whether a pid of this user's own (Workbench's, from its runtime.json) is alive: a
/// process that is not ours to see counts as gone. Unix: `/proc/<pid>` exists, which it
/// does not for other users' processes under `hidepid`. Windows: it opens and runs.
pub fn own_pid_alive(pid: u32) -> bool {
    imp::own_pid_alive(pid)
}

/// Whether process `pid` still runs: alive and, on Unix, not a zombie its parent has yet
/// to reap (Windows has none: an ended process is gone for all but its handles' holders).
#[cfg(test)]
pub fn pid_running(pid: u32) -> bool {
    imp::pid_running(pid)
}

/// End process `pid` at once (SIGKILL). The caller makes sure the pid is still the one
/// it means (a child not yet reaped). Pid 0 is ignored, and on Unix so is a pid above
/// `i32::MAX`, which `kill` would read as a process group or as every process (-1).
pub fn kill_pid(pid: u32) {
    imp::kill_pid(pid);
}

/// The parent of process `pid`, if it runs.
pub fn parent_of(pid: u32) -> Option<u32> {
    imp::parent_of(pid)
}

/// The signal that ended a process (Unix); always `None` on Windows.
pub fn exit_signal(st: &ExitStatus) -> Option<i32> {
    imp::exit_signal(st)
}

/// How a process ended, for messages: `exit code 2`, `killed by SIGKILL`.
pub fn exit_text(st: &ExitStatus) -> String {
    imp::exit_text(st)
}

// ---------------------------------------------------------------- this executable

/// This executable (`std::env::current_exe`). Linux reports a binary replaced on disk
/// while it runs (an upgrade, a rebuild) as `… (deleted)`: the suffix is dropped, so the
/// path names the new file.
pub fn current_exe() -> std::io::Result<PathBuf> {
    imp::current_exe()
}

/// A file that runs this program, for helpers other processes start: `current_exe` while
/// it is a file; after the binary was replaced on disk, the new file at the same path, or
/// else the running image (Linux: `/proc/<pid>/exe`).
pub fn runnable_exe() -> Option<PathBuf> {
    imp::runnable_exe()
}

/// Whether another file took this executable's place on disk since the process started
/// (an update, an installer run by hand): a restart then runs the new one. Linux only;
/// Windows cannot tell (a running image is renamed aside, and keeps reporting the path it
/// was loaded from), so `false` there.
pub fn exe_replaced() -> bool {
    imp::exe_replaced()
}

/// Replaces this process with `exe` run with `args`, keeping the pid, the environment and
/// the working directory, so whoever supervises it (systemd, a terminal) sees one process
/// that never stopped. Returns only when that failed. Unix: `exec`; file descriptors are
/// close-on-exec, so the caller's listening sockets are free for the new image. Windows
/// has no such call: always an error there (`support::Feature::SelfUpdate`).
pub fn reexec(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Error {
    imp::reexec(exe, args)
}

// ---------------------------------------------------------------- debugger support

/// A process of this user, for the debugger's attach list.
pub struct ProcEntry {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// The command line, arguments joined by spaces.
    pub command: String,
    /// Start time, milliseconds since the epoch (approximate).
    pub started_at: Option<i64>,
}

/// This user's processes, read as the iterator advances (Workbench itself and kernel
/// threads left out).
pub fn user_processes() -> impl Iterator<Item = ProcEntry> {
    imp::user_processes()
}

/// Linux's `/proc/sys/kernel/yama/ptrace_scope`; `None` where there is none (Windows).
pub fn ptrace_scope() -> Option<u8> {
    imp::ptrace_scope()
}

/// Whether a debugger is attached to `pid` now (Linux: `TracerPid` in
/// `/proc/<pid>/status`; Windows: `CheckRemoteDebuggerPresent`).
pub async fn debugger_attached(pid: u32) -> bool {
    imp::debugger_attached(pid).await
}

// ---------------------------------------------------------------- shutdown

/// Resolves when the server is asked to stop. Unix: SIGINT or SIGTERM. Windows: Ctrl-C,
/// Ctrl-Break, the console closing, or the stop event of `data_dir` (`request_stop`), which
/// only this user and SYSTEM may set; when another process already holds that event, the
/// server does not listen to it (and says so in the log).
pub async fn shutdown_signal(data_dir: &Path) {
    imp::shutdown_signal(data_dir).await;
}

/// Lets Ctrl-C through to this process and to what it starts from now on. Windows keeps
/// "ignore Ctrl-C" per process and hands it down to the processes it starts, and a process
/// started in a new process group gets it: a server started that way, or below such a
/// process, would open every terminal with Ctrl-C doing nothing. `serve` calls it before it
/// starts anything. Nothing to do on Unix.
pub fn enable_ctrl_c() {
    imp::enable_ctrl_c();
}

#[cfg(windows)]
pub use imp::{Event, request_stop, server_running, session_of, stop_event_name};

// ---------------------------------------------------------------- Unix

#[cfg(unix)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::ExitStatus;

    use tokio::process::Command;

    use super::ProcEntry;

    pub fn prepare(cmd: &mut Command) {
        cmd.process_group(0);
    }

    pub fn prepare_session(cmd: &mut Command) {
        // SAFETY: setsid is async-signal-safe and runs in the child before exec.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    /// The process-group id, which is the leader's pid; with `single`, the pid of a process
    /// left in Workbench's group, signalled alone.
    #[derive(Clone, Copy, Default)]
    pub struct Group {
        id: Option<i32>,
        single: bool,
    }

    impl Group {
        pub fn attach(pid: u32) -> Group {
            // 0 and 1 would name Workbench's own group and init (`kill(-1)`: everything).
            Group { id: i32::try_from(pid).ok().filter(|p| *p > 1), single: false }
        }

        pub fn attach_single(pid: u32) -> Group {
            Group { single: true, ..Group::attach(pid) }
        }

        pub fn terminate(&self) {
            self.signal(libc::SIGTERM);
        }

        pub fn kill(&self) {
            self.signal(libc::SIGKILL);
        }

        pub fn terminate_leader(&self) {
            self.signal_leader(libc::SIGTERM);
        }

        pub fn kill_leader(&self) {
            self.signal_leader(libc::SIGKILL);
        }

        fn signal(&self, sig: i32) {
            if self.single {
                return self.signal_leader(sig);
            }
            if let Some(pg) = self.id {
                // SAFETY: plain syscall; the group is the one the process was started in.
                unsafe { libc::killpg(pg, sig) };
            }
        }

        fn signal_leader(&self, sig: i32) {
            if let Some(pg) = self.id {
                // SAFETY: plain syscall; the leader's pid is the group id.
                unsafe { libc::kill(pg, sig) };
            }
        }

        pub fn members(&self) -> Vec<u32> {
            let Some(pg) = self.id else { return vec![] };
            let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
            rd.flatten()
                .filter_map(|e| {
                    let pid = e.file_name().to_str()?.parse::<u32>().ok()?;
                    if self.single && pid as i32 != pg {
                        return None;
                    }
                    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                    // Fields after the command name's last `)`: state, ppid, pgrp.
                    let f: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
                    (*f.first()? != "Z" && (self.single || f.get(2)?.parse::<i32>().ok()? == pg)).then_some(pid)
                })
                .collect()
        }
    }

    /// `pid` as libc's `pid_t` for a single process: `None` for 0 (the caller's own
    /// group) and above `i32::MAX` (negative: a group, or with -1 every process).
    pub(super) fn pid_t(pid: u32) -> Option<i32> {
        i32::try_from(pid).ok().filter(|p| *p > 0)
    }

    /// Whether a pid is alive (signal 0 probe).
    pub fn pid_alive(pid: u32) -> bool {
        let Some(pid) = pid_t(pid) else { return false };
        // SAFETY: signal 0 only checks existence and permission.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    pub fn own_pid_alive(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    #[cfg(test)]
    pub fn pid_running(pid: u32) -> bool {
        // The state follows the command name's last `)`.
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.rsplit_once(')').is_some_and(|(_, rest)| rest.split_whitespace().next() != Some("Z")))
    }

    pub fn kill_pid(pid: u32) {
        let Some(pid) = pid_t(pid) else { return };
        // SAFETY: plain syscall; the caller vouches for the pid.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }

    pub fn parent_of(pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = &stat[stat.rfind(')')? + 1..];
        rest.split_whitespace().nth(1)?.parse().ok()
    }

    pub fn exit_signal(st: &ExitStatus) -> Option<i32> {
        std::os::unix::process::ExitStatusExt::signal(st)
    }

    pub fn exit_text(st: &ExitStatus) -> String {
        use std::os::unix::process::ExitStatusExt;
        match (st.code(), st.signal()) {
            (Some(c), _) => format!("exit code {c}"),
            (None, Some(sig)) => match nix::sys::signal::Signal::try_from(sig) {
                Ok(n) => format!("killed by {}", n.as_str()),
                Err(_) => format!("killed by signal {sig}"),
            },
            _ => "no exit status".into(),
        }
    }

    pub fn current_exe() -> std::io::Result<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        let exe = std::env::current_exe()?;
        // A rebuilt binary leaves /proc/self/exe pointing at "… (deleted)".
        Ok(match exe.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
            Some(b) => PathBuf::from(std::ffi::OsStr::from_bytes(b)),
            None => exe,
        })
    }

    pub fn runnable_exe() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        if exe.is_file() {
            return Some(exe);
        }
        exe_fallback(&exe, std::process::id())
    }

    pub fn exe_replaced() -> bool {
        // The kernel's name for an image whose file is gone, and a file there again.
        std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().ends_with(" (deleted)")) && current_exe().is_ok_and(|p| p.is_file())
    }

    pub fn reexec(exe: &Path, args: &[std::ffi::OsString]) -> std::io::Error {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(exe).args(args).exec()
    }

    pub(super) fn exe_fallback(exe: &Path, pid: u32) -> Option<PathBuf> {
        let s = exe.to_string_lossy();
        if let Some(p) = s.strip_suffix(" (deleted)").map(PathBuf::from).filter(|p| p.is_file()) {
            return Some(p);
        }
        let proc_exe = PathBuf::from(format!("/proc/{pid}/exe"));
        proc_exe.exists().then_some(proc_exe)
    }

    /// `(ppid, starttime ticks)` from `/proc/<pid>/stat` (the name may contain spaces
    /// and parentheses; fields after the last `)` are fixed).
    pub(super) fn parse_stat(stat: &str) -> Option<(u32, u64)> {
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

    pub fn user_processes() -> impl Iterator<Item = ProcEntry> {
        use std::os::unix::fs::MetadataExt;
        let uid = nix::unistd::getuid().as_raw();
        let me = std::process::id();
        let boot = boot_time_ms();
        let ticks = 100i64; // USER_HZ on Linux
        std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(move |e| {
            let pid = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok())?;
            if pid == me {
                return None;
            }
            let dir = e.path();
            let meta = std::fs::metadata(&dir).ok()?;
            if meta.uid() != uid {
                return None;
            }
            let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
            if cmdline.is_empty() {
                return None; // kernel thread or zombie
            }
            let command = cmdline.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>().join(" ");
            let name = std::fs::read_to_string(dir.join("comm")).map(|s| s.trim().to_string()).unwrap_or_default();
            let (ppid, start) = std::fs::read_to_string(dir.join("stat")).ok().and_then(|s| parse_stat(&s)).unwrap_or((0, 0));
            Some(ProcEntry { pid, ppid, name, command, started_at: boot.map(|b| b + start as i64 * 1000 / ticks) })
        })
    }

    pub fn ptrace_scope() -> Option<u8> {
        std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").ok()?.trim().parse().ok()
    }

    pub async fn debugger_attached(pid: u32) -> bool {
        let status = tokio::fs::read_to_string(format!("/proc/{pid}/status")).await.unwrap_or_default();
        let tracer = status.lines().find_map(|l| l.strip_prefix("TracerPid:")).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        tracer != 0
    }

    pub fn enable_ctrl_c() {}

    pub async fn shutdown_signal(_data_dir: &Path) {
        let ctrl_c = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        let term = async {
            if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                s.recv().await;
            }
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = term => {},
        }
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::ExitStatus;
    use std::sync::Arc;

    use tokio::process::Command;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_HANDLE, ERROR_MORE_DATA, GetLastError, HANDLE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::System::Diagnostics::Debug::CheckRemoteDebuggerPresent;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_ASSOCIATE_COMPLETION_PORT,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectAssociateCompletionPortInformation, JobObjectBasicProcessIdList,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateEventW, DETACHED_PROCESS, EVENT_MODIFY_STATE, OpenEventW, OpenProcess, PROCESS_QUERY_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, TerminateProcess,
        WaitForSingleObject,
    };

    use super::ProcEntry;
    use crate::util::os::win32::{Handle, Local, User, sid_string, wide};

    /// The exit code of processes ended by `TerminateJobObject` or `TerminateProcess`.
    const KILLED: u32 = 1;

    fn open_process(access: u32, pid: u32) -> Option<Handle> {
        // SAFETY: plain call; `Handle` owns the result.
        Handle::new(unsafe { OpenProcess(access, 0, pid) })
    }

    /// Whether the process behind `h` (opened with SYNCHRONIZE) still runs.
    fn running(h: &Handle) -> bool {
        // SAFETY: a valid process handle; a zero timeout only polls.
        unsafe { WaitForSingleObject(h.0, 0) == WAIT_TIMEOUT }
    }

    pub fn prepare(cmd: &mut Command) {
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    pub fn prepare_session(cmd: &mut Command) {
        // Not CREATE_NO_WINDOW: a hidden console is still one to prompt on. git passes
        // DETACHED_PROCESS on to its console children when it has no console itself.
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    #[derive(Clone, Default)]
    pub struct Group(Option<Arc<Job>>);

    struct Job {
        pid: u32,
        leader: Handle,
        /// `None` when the process could not join one: it is then ended alone.
        job: Option<Handle>,
    }

    /// A new job whose processes are killed when its last handle closes; with
    /// `breakaway_ok`, a process it holds may start one outside it.
    fn kill_on_close_job(breakaway_ok: bool) -> Option<Handle> {
        // SAFETY: no attributes and no name: an unnamed job with default security.
        let job = Handle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | if breakaway_ok { JOB_OBJECT_LIMIT_BREAKAWAY_OK } else { 0 };
        // SAFETY: `info` is the structure of this information class, with its own size.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        (ok != 0).then_some(job)
    }

    /// Makes `job` report its events to the completion port `port`, with `key`. Delivery is
    /// not guaranteed (Microsoft's documentation): whoever waits for a report also looks
    /// for itself now and then.
    fn report_to(job: &Handle, port: HANDLE, key: usize) {
        let info = JOBOBJECT_ASSOCIATE_COMPLETION_PORT { CompletionKey: key as *mut std::ffi::c_void, CompletionPort: port };
        // SAFETY: a job handle with all access rights; `info` is the structure of this
        // information class, with its own size; the key is only handed back, never read.
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectAssociateCompletionPortInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
            )
        };
    }

    impl Group {
        pub fn attach(pid: u32) -> Group {
            Group::attach_job(pid, false, None)
        }

        pub fn attach_job(pid: u32, breakaway_ok: bool, reports: Option<(HANDLE, usize)>) -> Group {
            let access = PROCESS_TERMINATE | PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
            let Some(leader) = open_process(access, pid) else { return Group(None) };
            let job = kill_on_close_job(breakaway_ok).filter(|job| {
                // While the job is still empty: it has no event to miss yet.
                if let Some((port, key)) = reports {
                    report_to(job, port, key);
                }
                // SAFETY: both handles are valid; the process handle has PROCESS_SET_QUOTA
                // and PROCESS_TERMINATE.
                unsafe { AssignProcessToJobObject(job.0, leader.0) != 0 }
            });
            Group(Some(Arc::new(Job { pid, leader, job })))
        }

        pub fn attach_single(pid: u32) -> Group {
            Group::attach(pid)
        }

        pub fn terminate(&self) {
            self.kill();
        }

        pub fn kill(&self) {
            let Some(g) = &self.0 else { return };
            match &g.job {
                // SAFETY: a job handle with all access rights (from CreateJobObjectW).
                Some(job) => unsafe { TerminateJobObject(job.0, KILLED) },
                // SAFETY: a process handle with PROCESS_TERMINATE.
                None => unsafe { TerminateProcess(g.leader.0, KILLED) },
            };
        }

        pub fn terminate_leader(&self) {
            self.kill_leader();
        }

        pub fn kill_leader(&self) {
            if let Some(g) = &self.0 {
                // SAFETY: a process handle with PROCESS_TERMINATE.
                unsafe { TerminateProcess(g.leader.0, KILLED) };
            }
        }

        pub fn members(&self) -> Vec<u32> {
            let Some(g) = &self.0 else { return vec![] };
            let Some(job) = &g.job else {
                return if running(&g.leader) { vec![g.pid] } else { vec![] };
            };
            // The ids follow two u32 counts; a buffer of usize keeps the structure aligned.
            let header = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList) / size_of::<usize>();
            let mut room = 64;
            let mut ids = vec![];
            for _ in 0..3 {
                let mut buf = vec![0usize; header + room];
                // SAFETY: `buf` holds the header and `room` ids, aligned for the structure.
                let ok = unsafe {
                    QueryInformationJobObject(job.0, JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), size_of_val(buf.as_slice()) as u32, std::ptr::null_mut())
                } != 0;
                // SAFETY: plain call, right after the failed one.
                if !ok && unsafe { GetLastError() } != ERROR_MORE_DATA {
                    return vec![];
                }
                // SAFETY: the buffer starts with the header, filled in or still zeroed.
                let list = unsafe { &*buf.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
                let assigned = list.NumberOfAssignedProcesses as usize;
                let listed = (list.NumberOfProcessIdsInList as usize).min(room);
                ids = buf[header..header + listed].iter().map(|p| *p as u32).collect();
                if ok && listed >= assigned {
                    break;
                }
                room = assigned.max(room) + 16;
            }
            ids
        }

        /// The job's limit flags (`JOB_OBJECT_LIMIT_*`); `None` without a job.
        #[cfg(test)]
        pub fn limit_flags(&self) -> Option<u32> {
            let job = self.0.as_ref()?.job.as_ref()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // SAFETY: `info` is the structure of this information class, with its own size.
            let ok = unsafe {
                QueryInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_mut(&mut info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            } != 0;
            ok.then_some(info.BasicLimitInformation.LimitFlags)
        }
    }

    pub fn pid_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        match open_process(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, pid) {
            Some(h) => running(&h),
            // It exists but is not ours to open (EPERM on Unix).
            None => std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
        }
    }

    pub fn own_pid_alive(pid: u32) -> bool {
        pid > 0 && open_process(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, pid).is_some_and(|h| running(&h))
    }

    #[cfg(test)]
    pub fn pid_running(pid: u32) -> bool {
        pid_alive(pid)
    }

    pub fn kill_pid(pid: u32) {
        let Some(h) = Some(pid).filter(|p| *p > 0).and_then(|p| open_process(PROCESS_TERMINATE, p)) else { return };
        // SAFETY: a process handle with PROCESS_TERMINATE.
        unsafe { TerminateProcess(h.0, KILLED) };
    }

    pub fn parent_of(pid: u32) -> Option<u32> {
        // SAFETY: plain call; `Handle` owns the snapshot.
        let snap = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })?;
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        // SAFETY: a valid snapshot and a PROCESSENTRY32W with its size set, as both calls need.
        let mut more = unsafe { Process32FirstW(snap.0, &mut e) } != 0;
        while more {
            if e.th32ProcessID == pid {
                return Some(e.th32ParentProcessID);
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap.0, &mut e) } != 0;
        }
        None
    }

    pub fn exit_signal(_st: &ExitStatus) -> Option<i32> {
        None
    }

    pub fn exit_text(st: &ExitStatus) -> String {
        match st.code() {
            // Codes with the high bit set (NTSTATUS 0xC0000005, an access violation; .NET's
            // 0x80131506) read better in hex.
            Some(c) if c < 0 => format!("exit code {:#010X}", c as u32),
            Some(c) => format!("exit code {c}"),
            None => "no exit status".into(),
        }
    }

    pub fn current_exe() -> std::io::Result<PathBuf> {
        std::env::current_exe().map(|p| dunce::simplified(&p).to_path_buf())
    }

    pub fn runnable_exe() -> Option<PathBuf> {
        // A running exe cannot be deleted; renamed aside by an upgrade, its path holds the new one.
        current_exe().ok().filter(|p| p.is_file())
    }

    pub fn exe_replaced() -> bool {
        false
    }

    pub fn reexec(_exe: &Path, _args: &[std::ffi::OsString]) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::Unsupported, "a running Workbench cannot be replaced in place on Windows")
    }

    pub fn user_processes() -> impl Iterator<Item = ProcEntry> {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let me = std::process::id();
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_user(UpdateKind::Always));
        let user = sys.process(Pid::from_u32(me)).and_then(|p| p.user_id()).cloned();
        // Command lines only for this user's processes (reading one opens the process).
        let mine: Vec<Pid> = match &user {
            Some(u) => sys.processes().iter().filter(|(pid, p)| pid.as_u32() != me && p.user_id() == Some(u)).map(|(pid, _)| *pid).collect(),
            None => vec![],
        };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&mine),
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always),
        );
        let out: Vec<ProcEntry> = mine
            .iter()
            .filter_map(|pid| sys.process(*pid))
            .map(|p| {
                let name = p.name().to_string_lossy().into_owned();
                let mut command = p.cmd().iter().filter(|a| !a.is_empty()).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
                if command.is_empty() {
                    command = p.exe().map(|e| e.display().to_string()).unwrap_or_else(|| name.clone());
                }
                let start = p.start_time();
                ProcEntry {
                    pid: p.pid().as_u32(),
                    ppid: p.parent().map_or(0, |pp| pp.as_u32()),
                    name,
                    command,
                    started_at: (start > 0).then(|| start as i64 * 1000),
                }
            })
            .collect();
        out.into_iter()
    }

    pub fn ptrace_scope() -> Option<u8> {
        None
    }

    pub async fn debugger_attached(pid: u32) -> bool {
        let Some(h) = open_process(PROCESS_QUERY_INFORMATION, pid) else { return false };
        let mut present = 0;
        // SAFETY: a process handle with PROCESS_QUERY_INFORMATION; `present` outlives the call.
        let ok = unsafe { CheckRemoteDebuggerPresent(h.0, &mut present) };
        ok != 0 && present != 0
    }

    pub fn enable_ctrl_c() {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        // SAFETY: with no handler, FALSE only clears this process's "ignore Ctrl-C"
        // attribute (what the processes it starts inherit); handlers stay as they are.
        unsafe { SetConsoleCtrlHandler(None, 0) };
    }

    pub async fn shutdown_signal(data_dir: &Path) {
        use tokio::signal::windows;
        let ctrl_c = async {
            match windows::ctrl_c() {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending().await,
            }
        };
        let ctrl_break = async {
            match windows::ctrl_break() {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending().await,
            }
        };
        // Windows ends the process a few seconds after a close; tokio's handler waits till then.
        let ctrl_close = async {
            match windows::ctrl_close() {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(_) => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = ctrl_break => {},
            _ = ctrl_close => {},
            _ = stop_event(data_dir) => {},
        }
    }

    /// `Local\workbench-<hash>`: the event that asks the server serving `data_dir` to stop.
    /// Every process computes the same name for a data dir: the hash is of its canonical,
    /// lowercased path.
    pub fn stop_event_name(data_dir: &Path) -> String {
        use sha2::Digest;
        let dir = dunce::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let hash = sha2::Sha256::digest(dir.to_string_lossy().to_lowercase().as_bytes());
        format!("Local\\workbench-{}", &hex::encode(hash)[..16])
    }

    /// Ask the server serving `data_dir` to stop (sets its stop event). `Ok(false)` when
    /// none is running.
    pub fn request_stop(data_dir: &Path) -> std::io::Result<bool> {
        Event::set(&stop_event_name(data_dir))
    }

    /// Whether a server serving `data_dir` runs: it holds the stop event, which it creates
    /// once it listens and keeps until its process ends, a graceful shutdown included (a crash
    /// leaves nothing stale).
    pub fn server_running(data_dir: &Path) -> bool {
        Event::exists(&stop_event_name(data_dir))
    }

    /// The Windows session process `pid` runs in: 0 for services and for what an SSH
    /// sign-in starts, 1 and up for the desktops users sign in to. `Local\` names, the stop
    /// events among them, belong to one session: another session's server holds its events
    /// where no process here can see them. `None` when the pid cannot be looked up.
    pub fn session_of(pid: u32) -> Option<u32> {
        use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
        let mut session = 0u32;
        // SAFETY: plain call; `session` is a valid out-pointer.
        (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
    }

    /// A named auto-reset event that only this user and SYSTEM may open, held while the
    /// value lives: a server's stop event, `workbench service`'s own.
    pub struct Event(Handle);

    impl Event {
        /// Creates the event `name`. `Ok(None)` when it exists already: another process holds
        /// it (or squats the name).
        pub fn create(name: &str) -> std::io::Result<Option<Event>> {
            let sd = owner_only_sd().ok_or_else(|| std::io::Error::other("cannot build the event's security descriptor"))?;
            let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
            let wname = wide(name);
            // Auto-reset: a waiter started after a request does not see the old one.
            // SAFETY: `sa` and its descriptor, and the NUL-terminated `wname`, outlive the
            // call; `Handle` owns the result.
            let event = Handle::new(unsafe { CreateEventW(&sa, 0, 0, wname.as_ptr()) });
            // SAFETY: plain call, right after CreateEventW (which sets it on success too).
            let err = unsafe { GetLastError() };
            match event {
                Some(event) if err != ERROR_ALREADY_EXISTS => Ok(Some(Event(event))),
                Some(_) => Ok(None),
                None => Err(std::io::Error::from_raw_os_error(err as i32)),
            }
        }

        /// Sets the event `name`. `Ok(false)` when no process holds it.
        pub fn set(name: &str) -> std::io::Result<bool> {
            let name = wide(name);
            // SAFETY: `name` is NUL-terminated and outlives the call; `Handle` owns the result.
            let Some(event) = Handle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) }) else {
                let e = std::io::Error::last_os_error();
                return if e.raw_os_error().is_some_and(no_event) { Ok(false) } else { Err(e) };
            };
            // SAFETY: an event handle with EVENT_MODIFY_STATE.
            if unsafe { SetEvent(event.0) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(true)
        }

        /// Whether a process holds the event `name`, one this user may open (a name squatted
        /// by another account does not count).
        pub fn exists(name: &str) -> bool {
            let name = wide(name);
            // SAFETY: `name` is NUL-terminated and outlives the call; `Handle` owns the result
            // and closes it at once.
            Handle::new(unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) }).is_some()
        }

        /// Waits up to `timeout` for the event to be set (which resets it). `Ok(false)` on
        /// timeout.
        pub fn wait(&self, timeout: std::time::Duration) -> std::io::Result<bool> {
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
            // SAFETY: a valid event handle, held by `self`.
            match unsafe { WaitForSingleObject(self.0.0, ms) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(std::io::Error::last_os_error()),
            }
        }
    }

    /// Whether `OpenEventW` failing with `code` means that no event has the name. Its docs name
    /// no code for that: ERROR_FILE_NOT_FOUND is the usual one; ERROR_INVALID_HANDLE is the
    /// documented one for a name that another kind of object holds (no event either), and the
    /// one Windows Server 2025 (10.0.26100, the CI runner) gave for names nothing held. Access
    /// denied (another account's event) and a bad name stay errors.
    fn no_event(code: i32) -> bool {
        code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_INVALID_HANDLE as i32
    }

    /// This process's user as a SID string (`S-1-5-21-…`).
    fn user_sid() -> Option<String> {
        let user = User::current().ok()?;
        // SAFETY: `user` keeps its SID valid for the call.
        unsafe { sid_string(user.sid()) }
    }

    /// A protected DACL granting only this user and SYSTEM (the owner-only DACL of os::perm,
    /// kept here so the stop event does not depend on it).
    fn owner_only_sd() -> Option<Local> {
        let sddl = wide(&format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", user_sid()?));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated; `sd` receives a LocalAlloc'd descriptor that
        // `Local` frees; the size is not asked for.
        let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } != 0;
        (ok && !sd.is_null()).then_some(Local(sd))
    }

    /// Creates the stop event of `data_dir`, for this user and SYSTEM only. `None` (logged)
    /// when it cannot be created or already exists: another server on this data dir, or a
    /// process squatting the name, would otherwise share its stop requests.
    pub(super) fn create_stop_event(data_dir: &Path) -> Option<Event> {
        let name = stop_event_name(data_dir);
        // Auto-reset: a server started right after a stop does not see the old request.
        match Event::create(&name) {
            Ok(Some(event)) => return Some(event),
            Ok(None) => tracing::warn!("the stop event {name} already exists (another Workbench on this data dir?); `workbench service stop` cannot stop this server"),
            Err(e) => tracing::warn!("cannot create the stop event {name} ({e}); `workbench service stop` cannot stop this server"),
        }
        None
    }

    /// Resolves once the stop event of `data_dir` is set; never when it cannot be created.
    async fn stop_event(data_dir: &Path) {
        let Some(event) = create_stop_event(data_dir) else {
            return std::future::pending().await;
        };
        // Never closed: the event goes with the process, so a server still shutting down (and
        // holding its port) counts as running (`server_running`). Setting it again is harmless.
        let event = std::mem::ManuallyDrop::new(event);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        // A thread of its own, not `spawn_blocking`: the runtime waits for blocking tasks
        // when it shuts down. It ends once a stop is requested or nobody listens.
        let waiter = std::thread::Builder::new().name("workbench-stop-event".into()).spawn(move || {
            // The whole `Event` moves here (not just its handle).
            let event = event;
            loop {
                match event.wait(std::time::Duration::from_millis(250)) {
                    Ok(true) => {
                        let _ = tx.send(());
                        return;
                    }
                    Ok(false) if !tx.is_closed() => {}
                    _ => return,
                }
            }
        });
        if waiter.is_err() || rx.await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;

    use super::*;

    /// A command that runs for a while in a child process of its own.
    fn parent_and_child() -> Command {
        #[cfg(unix)]
        let (program, args) = ("sh", ["-c", "sleep 30 & wait"]);
        #[cfg(windows)]
        let (program, args) = ("cmd", ["/c", "ping -n 30 127.0.0.1 >nul"]);
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        cmd
    }

    /// A command that runs for a while, alone.
    fn sleeper() -> std::process::Command {
        #[cfg(unix)]
        let mut cmd = std::process::Command::new("sleep");
        #[cfg(unix)]
        cmd.arg("30");
        #[cfg(windows)]
        let mut cmd = std::process::Command::new("ping");
        #[cfg(windows)]
        cmd.args(["-n", "30", "127.0.0.1"]);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd
    }

    async fn eventually(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_group_ends_with_what_it_started() {
        let mut cmd = parent_and_child();
        ProcGroup::prepare(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let group = ProcGroup::attach(&child);
        let leader = child.id().unwrap();
        let mut members = vec![];
        assert!(
            eventually(|| {
                members = group.members();
                members.len() >= 2
            })
            .await,
            "the leader and its child: {members:?}"
        );
        assert!(members.contains(&leader), "{members:?}");
        group.kill();
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap();
        assert!(!status.success());
        assert!(eventually(|| members.iter().all(|p| !pid_alive(*p))).await, "{members:?} outlived the group");
        assert!(group.members().is_empty());
        // An empty group (the child was gone) does nothing.
        ProcGroup::default().kill();
        assert!(ProcGroup::default().members().is_empty());
    }

    #[tokio::test]
    async fn one_process() {
        let mut child = sleeper().spawn().unwrap();
        let pid = child.id();
        assert!(pid_alive(pid));
        assert!(own_pid_alive(pid) && own_pid_alive(std::process::id()));
        assert_eq!(parent_of(pid), Some(std::process::id()));
        assert!(!debugger_attached(pid).await);
        assert!(pid_running(pid));
        kill_pid(pid);
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(!pid_alive(pid) && !pid_running(pid));
        assert!(!own_pid_alive(pid));
        assert!(!pid_alive(0));
        kill_pid(0); // ignored, not Workbench's own group
    }

    /// Unix: a pid above `i32::MAX` is none, and never reaches `kill` (which would read it
    /// as a process group, or as every process for `u32::MAX`, -1). Checked without calling
    /// `kill_pid` with one: a mistake would signal every process of this user.
    #[cfg(unix)]
    #[test]
    fn pids_kill_would_misread_are_none() {
        assert_eq!(imp::pid_t(u32::MAX), None);
        assert_eq!(imp::pid_t(1 << 31), None);
        assert_eq!(imp::pid_t(0), None);
        assert_eq!(imp::pid_t(42), Some(42));
        assert!(!pid_alive(u32::MAX) && !pid_alive(1 << 31));
    }

    /// A process started in a new process group ignores Ctrl-C (and would hand that down to
    /// every terminal it opens) until `enable_ctrl_c`. It runs in a child with a console of
    /// its own, the only process there that Ctrl-C reaches.
    #[cfg(windows)]
    #[test]
    fn ctrl_c_works_again_after_a_new_process_group() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
        if std::env::var_os("WB_CTRL_C_CHILD").is_some() {
            return ctrl_c_child();
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "--quiet", "--test-threads=1", "util::os::proc::tests::ctrl_c_works_again_after_a_new_process_group"])
            .env("WB_CTRL_C_CHILD", "1")
            .stdin(Stdio::null())
            .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && text.contains("1 passed"), "{text}");
    }

    #[cfg(windows)]
    fn ctrl_c_child() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use windows_sys::Win32::System::Console::{CTRL_C_EVENT, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler};
        static SEEN: AtomicU32 = AtomicU32::new(0);
        unsafe extern "system" fn count(kind: u32) -> windows_sys::core::BOOL {
            if kind == CTRL_C_EVENT {
                SEEN.fetch_add(1, Ordering::SeqCst);
            }
            1
        }
        let seen = || {
            for _ in 0..40 {
                if SEEN.load(Ordering::SeqCst) > 0 {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        // SAFETY: a handler that only counts and says it handled the event, so the process
        // stays; it stays registered for the process's life.
        assert_ne!(unsafe { SetConsoleCtrlHandler(Some(count), 1) }, 0);
        // SAFETY: Ctrl-C to every process on this process's own hidden console: itself.
        assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) }, 0);
        assert!(!seen(), "a new process group ignores Ctrl-C");
        enable_ctrl_c();
        // SAFETY: as above.
        assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) }, 0);
        assert!(seen(), "Ctrl-C reaches the process again");
    }

    #[cfg(windows)]
    #[test]
    fn only_a_terminals_job_lets_processes_leave() {
        use windows_sys::Win32::System::JobObjects::{JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
        let (mut a, mut b) = (sleeper().spawn().unwrap(), sleeper().spawn().unwrap());
        let (plain, terminal) = (ProcGroup::attach_pid(a.id()), ProcGroup::attach_terminal(b.id(), None));
        assert_eq!(plain.0.limit_flags(), Some(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE));
        assert_eq!(terminal.0.limit_flags(), Some(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK));
        plain.kill();
        terminal.kill();
        assert!(!a.wait().unwrap().success() && !b.wait().unwrap().success());
    }

    /// Windows: a terminal's job reports to the completion port it was given, with its key,
    /// once no process of it runs (what `os::session` closes the pseudoconsole on).
    #[cfg(windows)]
    #[test]
    fn a_terminals_job_reports_that_it_is_empty() {
        use std::time::Instant;
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::IO::{CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED};
        use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO, JOB_OBJECT_MSG_NEW_PROCESS};
        // SAFETY: a new completion port, tied to no file; `Handle` closes it.
        let port = crate::util::os::win32::Handle::new(unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, 1) }).unwrap();
        // The next report within `timeout`: `(message, key)`.
        let next = |timeout: Duration| {
            let (mut message, mut key, mut overlapped) = (0u32, 0usize, std::ptr::null_mut::<OVERLAPPED>());
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
            // SAFETY: a completion port `port` keeps open; the out-values are valid for the call.
            let got = unsafe { GetQueuedCompletionStatus(port.0, &mut message, &mut key, &mut overlapped, ms) } != 0;
            got.then_some((message, key))
        };
        let mut child = sleeper().spawn().unwrap();
        let group = ProcGroup::attach_terminal(child.id(), Some((port.0, 42)));
        // While it runs, the job reports its process joining (and maybe a console host):
        // never that it is empty.
        let mut seen = vec![];
        while let Some((message, key)) = next(Duration::from_millis(500)) {
            assert_eq!(key, 42);
            seen.push(message);
        }
        assert!(seen.contains(&JOB_OBJECT_MSG_NEW_PROCESS) && !seen.contains(&JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO), "{seen:?}");
        group.kill();
        assert!(!child.wait().unwrap().success());
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Some((message, key)) = next(deadline.saturating_duration_since(Instant::now())) {
            assert_eq!(key, 42);
            seen.push(message);
            if message == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO {
                return;
            }
        }
        panic!("no report that the job is empty: {seen:?}");
    }

    #[tokio::test]
    async fn a_process_left_in_our_group_is_ended_alone() {
        let mut child = sleeper().spawn().unwrap();
        let group = ProcGroup::attach_single(child.id());
        let members = group.members();
        // Windows: the job may also hold the console host of the child.
        assert!(members.contains(&child.id()), "{members:?}");
        #[cfg(unix)]
        assert_eq!(members, vec![child.id()]);
        group.kill();
        let status = child.wait().unwrap();
        assert!(!status.success());
        #[cfg(unix)]
        assert_eq!(exit_signal(&status), Some(9));
        assert!(eventually(|| group.members().is_empty()).await);
    }

    #[test]
    fn exit_codes_read_well() {
        #[cfg(unix)]
        let st = std::process::Command::new("sh").args(["-c", "exit 3"]).status().unwrap();
        #[cfg(windows)]
        let st = std::process::Command::new("cmd").args(["/c", "exit 3"]).status().unwrap();
        assert_eq!(exit_text(&st), "exit code 3");
        assert_eq!(exit_signal(&st), None);
    }

    #[cfg(unix)]
    #[test]
    fn signals_are_named() {
        let st = std::process::Command::new("sh").args(["-c", "kill -9 $$"]).status().unwrap();
        assert_eq!(exit_text(&st), "killed by SIGKILL");
        assert_eq!(exit_signal(&st), Some(9));
    }

    #[test]
    fn this_executable() {
        let exe = current_exe().unwrap();
        assert!(exe.is_file(), "{}", exe.display());
        assert_eq!(runnable_exe(), Some(exe));
    }

    #[cfg(unix)]
    #[test]
    fn helper_survives_a_replaced_binary() {
        let dir = tempfile::tempdir().unwrap();
        let new_bin = dir.path().join("workbench");
        std::fs::write(&new_bin, b"").unwrap();
        let deleted = PathBuf::from(format!("{} (deleted)", new_bin.display()));
        assert_eq!(imp::exe_fallback(&deleted, std::process::id()), Some(new_bin.clone()));
        std::fs::remove_file(&new_bin).unwrap();
        let me = std::process::id();
        assert_eq!(imp::exe_fallback(&deleted, me), Some(PathBuf::from(format!("/proc/{me}/exe"))));
    }

    #[cfg(unix)]
    #[test]
    fn stat_parsing_survives_odd_names() {
        let s = "4145729 (my (odd) prog) S 4145700 4145729 1 0 -1 4194304 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 100";
        assert_eq!(imp::parse_stat(s), Some((4145700, 987654)));
        assert_eq!(imp::parse_stat("garbage"), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_child_runs_in_its_parents_session() {
        let mine = session_of(std::process::id()).expect("this process's session");
        let mut child = sleeper().spawn().unwrap();
        assert_eq!(session_of(child.id()), Some(mine));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn the_stop_event_stops_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let name = stop_event_name(dir.path());
        assert!(name.starts_with("Local\\workbench-"), "{name}");
        assert_eq!(name, stop_event_name(&dir.path().join(".")));
        assert_ne!(name, stop_event_name(other.path()));
        assert!(!request_stop(dir.path()).unwrap(), "no server listens yet");
        assert!(!server_running(dir.path()));
        // A second server on the data dir (or a squatter of the name) is not listened to.
        let first = imp::create_stop_event(dir.path()).expect("the event is created");
        assert!(server_running(dir.path()) && !server_running(other.path()));
        assert!(imp::create_stop_event(dir.path()).is_none());
        drop(first);
        assert!(!server_running(dir.path()), "the event goes with its last handle");
        let path = dir.path().to_path_buf();
        let server = tokio::spawn(async move { shutdown_signal(&path).await });
        assert!(eventually(|| request_stop(dir.path()).unwrap()).await, "the server created its event");
        tokio::time::timeout(Duration::from_secs(5), server).await.expect("the server stopped").unwrap();
        assert!(server_running(dir.path()), "held through the shutdown, until the process ends");
    }

    #[cfg(windows)]
    #[test]
    fn named_events_are_held_set_and_waited_for() {
        let name = format!("Local\\workbench-test-{}", std::process::id());
        assert!(!Event::exists(&name) && !Event::set(&name).unwrap());
        let event = Event::create(&name).unwrap().expect("a new event");
        assert!(Event::create(&name).unwrap().is_none(), "held by `event`");
        assert!(Event::exists(&name));
        assert!(!event.wait(Duration::from_millis(10)).unwrap());
        assert!(Event::set(&name).unwrap());
        assert!(event.wait(Duration::from_secs(1)).unwrap());
        assert!(!event.wait(Duration::from_millis(10)).unwrap(), "auto-reset");
        drop(event);
        assert!(!Event::exists(&name));
        assert!(!Event::set(&name).unwrap(), "gone with its last handle");
    }

    /// A name that another kind of object holds is no event: nothing to set, and none of ours
    /// can be created under it.
    #[cfg(windows)]
    #[test]
    fn a_name_held_by_another_kind_of_object_is_no_event() {
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name = format!("Local\\workbench-test-mutex-{}", std::process::id());
        let wname = crate::util::os::win32::wide(&name);
        // SAFETY: no attributes; `wname` is NUL-terminated and outlives the call; `Handle`
        // owns the result.
        let mutex = crate::util::os::win32::Handle::new(unsafe { CreateMutexW(std::ptr::null(), 0, wname.as_ptr()) }).expect("a mutex");
        assert!(!Event::exists(&name));
        assert!(!Event::set(&name).unwrap());
        assert!(Event::create(&name).is_err());
        drop(mutex);
    }
}
