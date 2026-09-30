//! PTY sessions: the processes a terminal started, found, hung up and ended together, what
//! they hold open, and the PTY's output as its reader thread sees it.
//!
//! Unix: portable-pty starts a terminal's process in a session of its own (`setsid`), so
//! the leader's pid is the session id. Members are found by scanning `/proc`; the hang-up
//! is SIGHUP (and SIGCONT) to every process group of the session, SIGKILL comes after a
//! grace period.
//!
//! Windows: the leader joins a Job Object right after the spawn (what it starts joins too,
//! unless it asks to leave with `CREATE_BREAKAWAY_FROM_JOB`, as a daemon leaves a Unix
//! session), registered under the leader's pid, so the same `i32` session ids work (a
//! `Handle` keeps it registered, so its pid is not reused for a later session). The hang-up
//! closes the pseudoconsole (ConPTY sends CTRL_CLOSE_EVENT to every process attached to
//! it); `TerminateJobObject` ends what is left after the grace period. ConPTY gives the
//! reader no EOF when its processes exit: `leader_exited` closes the pseudoconsole once the
//! job is empty. A process that starts a grandchild before it joined its job (the moment
//! after the spawn) leaves that grandchild out (docs/windows-port.md §5).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::MasterPty;

/// Take charge of the session led by `pid`, a PTY's process that was just spawned.
/// `hang_up` closes the PTY (Windows: the pseudoconsole, which the session then owns until
/// it is over). Unix does nothing here: the session exists already, and the kernel hangs
/// it up when the PTY closes.
pub fn register(pid: i32, hang_up: impl FnOnce() + Send + 'static) -> Handle {
    Handle { sid: pid, _hold: imp::register(pid, Box::new(hang_up)).map(Arc::new) }
}

/// A session `register` took charge of. While a clone of it lives, its sid names this
/// session and no later one whose leader got the same pid: whoever follows a session after
/// its leader exited (lingering processes, their kill) keeps one. Unix: the kernel gives no
/// process the pid of a session that still has members, and allocates pids in turn. Windows
/// reuses a pid as soon as its process is gone: the session stays registered, its job
/// keeping a handle to the leader, until every clone is dropped and `leader_exited` is done.
#[derive(Clone)]
pub struct Handle {
    sid: i32,
    _hold: Option<Arc<imp::Hold>>,
}

impl Handle {
    pub fn sid(&self) -> i32 {
        self.sid
    }
}

/// How a PTY's leader ended (`wait`).
#[derive(Debug, Clone)]
pub struct LeaderExit {
    /// Its exit code as portable-pty reports it: 1 for a process a signal ended (Unix).
    pub code: u32,
    /// The signal that ended it, as portable-pty describes it (`strsignal`, which may be
    /// translated). Unix only.
    pub signal: Option<String>,
    /// A request to end it did, rather than its own exit or a crash: on Unix a hang-up (its
    /// terminal closed), terminate, kill or interrupt signal; not SIGSEGV, SIGABRT and the
    /// like. Always false on Windows, where a process ended from outside (`TerminateProcess`,
    /// as Task Manager's End task does) exits with the code it was given, 1 there, like one
    /// that exited with that code by itself.
    pub terminated: bool,
}

/// Blocking: wait for `child`, a PTY's leader (`SlavePty::spawn_command`), to exit. The code
/// and signal are portable-pty's, as its own `wait` reports them.
pub fn wait(child: &mut dyn portable_pty::Child) -> std::io::Result<LeaderExit> {
    imp::wait(child)
}

/// A `LeaderExit` with the code and signal of portable-pty's status.
fn leader_exit(s: portable_pty::ExitStatus, terminated: bool) -> LeaderExit {
    LeaderExit { code: s.exit_code(), signal: s.signal().map(str::to_owned), terminated }
}

/// Blocking, on the thread that saw the leader of session `sid` exit. Unix returns at once
/// (the reader sees EOF once no process has the PTY open). Windows waits until no process
/// of the session runs, then closes its pseudoconsole, so the reader sees EOF, and forgets
/// the session once no `Handle` of it is left.
pub fn leader_exited(sid: i32) {
    imp::leader_exited(sid);
}

/// `(pid, process group)` of every live process of session `sid`. Unix scans `/proc`,
/// which also catches jobs an interactive shell put into process groups of their own.
/// Windows: the processes in the session's job; the group is `sid`.
pub fn members(sid: i32) -> Vec<(i32, i32)> {
    imp::members(sid)
}

/// Hang up session `sid`, then end whatever is left of it after `grace` (SIGKILL; Windows:
/// `TerminateJobObject`). Returns once the session is empty and `leader_gone()` holds, or
/// right after that forced end.
pub async fn kill(sid: i32, grace: Duration, leader_gone: impl Fn() -> bool) {
    if sid <= 1 {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || imp::hang_up(sid)).await;
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        tokio::time::sleep(Duration::from_millis(40)).await;
        let exited = leader_gone();
        let members = tokio::task::spawn_blocking(move || members(sid)).await.unwrap_or_default();
        if exited && members.is_empty() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = tokio::task::spawn_blocking(move || imp::force_end(sid)).await;
            return;
        }
    }
}

/// Which of `paths` the processes of the sessions `sids` hold open (blocking): for each
/// path (as given) held by one of them, those sessions. Unix reads `/proc/<pid>/fd` of the
/// members; Windows asks the Restart Manager who holds each path.
pub fn holders(sids: &[i32], paths: &[PathBuf]) -> HashMap<PathBuf, Vec<i32>> {
    imp::holders(sids, paths)
}

/// Whether a process outside the sessions `ours` holds `path` open (blocking; Unix reads
/// every readable `/proc/<pid>/fd`, Windows asks the Restart Manager).
pub fn held_outside(path: &Path, ours: &HashSet<i32>) -> bool {
    imp::held_outside(path, ours)
}

/// Whether a process outside the sessions `ours`, working in the directory `cwd`, has a
/// command line (its arguments, each followed by a NUL) that `matches` accepts (blocking;
/// Unix reads `/proc`, Windows this user's processes through sysinfo).
pub fn runs_outside(cwd: &Path, ours: &HashSet<i32>, matches: impl Fn(&[u8]) -> bool) -> bool {
    imp::runs_outside(cwd, ours, &matches)
}

/// Whether the PTY paints what programs write from a screen of its own instead of passing
/// their bytes on: ConPTY (Windows) does, so escape sequences of its own (cursor moves,
/// hiding the cursor while it paints) can fall between the characters a program wrote in
/// one piece. Unix passes the bytes on.
pub const REPAINTS: bool = cfg!(windows);

/// Whether a new PTY asks for the cursor position before its program runs and waits for
/// the answer: ConPTY does (portable-pty creates it with INHERIT_CURSOR). Unix does not.
pub const ASKS_CURSOR: bool = cfg!(windows);

/// Variables that describe the terminal Workbench itself was started from on this OS only,
/// beyond those every OS shares (the caller's list): a terminal's session clears them, as
/// they are wrong for it. Windows: Windows Terminal's. Unix: none (these names are not its).
pub const PARENT_TERMINAL_VARS: &[&str] = if cfg!(windows) { &["WT_SESSION", "WT_PROFILE_ID"] } else { &[] };

/// A PTY's output, for the thread that reads it.
///
/// Windows: a thread of its own reads the pipe until it ends and hands the chunks over, so
/// `readable_within` is a wait on a channel, and the pipe keeps being drained after this
/// reader stopped (`ClosePseudoConsole` waits for the console host's last frame to be read).
pub struct Output(imp::Output);

impl Output {
    /// `reader` is `master`'s (`try_clone_reader`). `hold_back`: `readable_within` will be
    /// asked (Unix then polls a duplicate of the master's descriptor). `label` names the
    /// threads (the leader's pid).
    pub fn new(reader: Box<dyn Read + Send>, master: &dyn MasterPty, hold_back: bool, label: &str) -> std::io::Result<Output> {
        imp::Output::new(reader, master, hold_back, label).map(Output)
    }

    /// Blocks until output arrives; `Ok(0)` once it has ended.
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }

    /// Whether output arrives (or the output ends) within `timeout`. True at once when
    /// that cannot be told (made without `hold_back`, or no descriptor to poll on Unix).
    pub fn readable_within(&mut self, timeout: Duration) -> bool {
        self.0.readable_within(timeout)
    }
}

// ---------------------------------------------------------------- Unix

#[cfg(unix)]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::io::Read;
    use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use portable_pty::MasterPty;

    /// Nothing to hold: the kernel keeps a session's pid while it has members.
    pub enum Hold {}

    pub fn register(_pid: i32, _hang_up: Box<dyn FnOnce() + Send>) -> Option<Hold> {
        None
    }

    pub fn leader_exited(_sid: i32) {}

    /// portable-pty's status names the signal but not its number, and reports a code of 1
    /// with it. Its children are std's (`std::process::Child`), whose status has the number.
    pub fn wait(child: &mut dyn portable_pty::Child) -> std::io::Result<super::LeaderExit> {
        use std::os::unix::process::ExitStatusExt;
        let Some(c) = child.downcast_mut::<std::process::Child>() else {
            return Ok(super::leader_exit(child.wait()?, false));
        };
        let st = c.wait()?;
        Ok(super::leader_exit(st.into(), st.signal().is_some_and(terminating)))
    }

    /// Signals that ask a process to end: a hang-up (its terminal closed, or Workbench's
    /// kill), a terminate or kill request, an interrupt (Ctrl-C). Not those of a crash
    /// (SIGSEGV, SIGABRT, SIGBUS…) or SIGQUIT's core dump.
    pub(super) fn terminating(signal: i32) -> bool {
        matches!(signal, libc::SIGHUP | libc::SIGTERM | libc::SIGKILL | libc::SIGINT)
    }

    /// SIGHUP, and SIGCONT so stopped jobs can receive it, to every process group of the
    /// session. portable-pty's own kill signals the leader only, which leaves background
    /// and HUP-immune jobs behind.
    pub fn hang_up(sid: i32) {
        signal_session(sid, libc::SIGHUP);
        signal_session(sid, libc::SIGCONT);
    }

    pub fn force_end(sid: i32) {
        signal_session(sid, libc::SIGKILL);
    }

    /// `(pid, pgrp)` of every live process whose session id is `sid` (scans `/proc/*/stat`).
    pub fn members(sid: i32) -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        let Ok(rd) = std::fs::read_dir("/proc") else { return v };
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            if let Some((state, pgrp, session)) = parse_stat(&stat) {
                if state != "Z" && session == sid {
                    v.push((pid, pgrp));
                }
            }
        }
        v
    }

    /// Live processes as `(pid, session id)` (zombies left out).
    fn processes() -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        let Ok(rd) = std::fs::read_dir("/proc") else { return v };
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            if let Some((state, _, session)) = parse_stat(&stat) {
                if state != "Z" {
                    v.push((pid, session));
                }
            }
        }
        v
    }

    /// `(state, pgrp, session)` from a `/proc/<pid>/stat` line. The command name may
    /// contain spaces and parentheses, so fields are parsed after the *last* `)`.
    pub(super) fn parse_stat(stat: &str) -> Option<(&str, i32, i32)> {
        let rest = stat.rsplit_once(')')?.1;
        let f: Vec<&str> = rest.split_whitespace().collect();
        // f[0]=state f[1]=ppid f[2]=pgrp f[3]=session
        Some((f.first()?, f.get(2)?.parse().ok()?, f.get(3)?.parse().ok()?))
    }

    fn signal_session(sid: i32, sig: i32) {
        let mut pgrps: Vec<i32> = members(sid).into_iter().map(|(_, g)| g).filter(|g| *g > 1).collect();
        pgrps.push(sid);
        pgrps.sort_unstable();
        pgrps.dedup();
        for g in pgrps {
            // SAFETY: plain syscall; a negative pid signals the process group.
            unsafe { libc::kill(-g, sig) };
        }
    }

    pub fn holders(sids: &[i32], paths: &[PathBuf]) -> HashMap<PathBuf, Vec<i32>> {
        // `/proc/<pid>/fd` links are canonical paths.
        let wanted: HashMap<PathBuf, &PathBuf> = paths.iter().map(|p| (super::super::path::canonicalize(p).unwrap_or_else(|_| p.clone()), p)).collect();
        let mut out: HashMap<PathBuf, Vec<i32>> = HashMap::new();
        if wanted.is_empty() {
            return out;
        }
        for sid in sids {
            if *sid <= 1 {
                continue;
            }
            for (pid, _) in members(*sid) {
                let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { continue };
                for fd in rd.flatten() {
                    let Ok(target) = std::fs::read_link(fd.path()) else { continue };
                    if let Some(orig) = wanted.get(&target) {
                        let v = out.entry((*orig).clone()).or_default();
                        if !v.contains(sid) {
                            v.push(*sid);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn held_outside(path: &Path, ours: &HashSet<i32>) -> bool {
        let want = super::super::path::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        processes().into_iter().filter(|(_, sid)| !ours.contains(sid)).any(|(pid, _)| {
            std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|rd| rd.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == want)))
        })
    }

    pub fn runs_outside(cwd: &Path, ours: &HashSet<i32>, matches: &dyn Fn(&[u8]) -> bool) -> bool {
        let cwd = super::super::path::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        processes().into_iter().filter(|(_, sid)| !ours.contains(sid)).any(|(pid, _)| {
            std::fs::read_link(format!("/proc/{pid}/cwd")).is_ok_and(|d| d == cwd) && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| matches(&c))
        })
    }

    pub struct Output {
        reader: Box<dyn Read + Send>,
        /// A duplicate of the master's descriptor to poll, closed with the reader.
        poll_fd: Option<OwnedFd>,
    }

    impl Output {
        pub fn new(reader: Box<dyn Read + Send>, master: &dyn MasterPty, hold_back: bool, _label: &str) -> std::io::Result<Output> {
            let poll_fd = match master.as_raw_fd() {
                // SAFETY: the master's descriptor stays open for the call (`master` is borrowed).
                Some(fd) if hold_back => unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned().ok(),
                _ => None,
            };
            Ok(Output { reader, poll_fd })
        }

        pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reader.read(buf)
        }

        pub fn readable_within(&mut self, timeout: Duration) -> bool {
            let Some(fd) = &self.poll_fd else { return true };
            let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for the duration of the call.
            let r = unsafe { libc::poll(&mut p, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
            r != 0
        }
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::sync::{Arc, LazyLock};
    use std::time::Duration;

    use parking_lot::Mutex;
    use portable_pty::MasterPty;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::RestartManager::{
        CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
    };

    use crate::util::os::path;
    use crate::util::os::proc::{ProcGroup, own_pid_alive};
    use crate::util::os::win32::wide_path;

    /// A terminal's session: the job its leader joined, and what closes its pseudoconsole.
    struct Session {
        group: ProcGroup,
        hang_up: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// What keeps it registered: its `Hold` (the `Handle`s) and the waiter, until
        /// `leader_exited` is done.
        holds: AtomicUsize,
    }

    /// Sessions by their leader's pid. The job keeps a handle to the leader, so that pid
    /// is not given to another process while its session is registered.
    static SESSIONS: LazyLock<Mutex<HashMap<i32, Arc<Session>>>> = LazyLock::new(Default::default);

    fn session(sid: i32) -> Option<Arc<Session>> {
        SESSIONS.lock().get(&sid).cloned()
    }

    /// The `Handle`s' part in keeping a session registered; dropped with the last of them.
    pub struct Hold {
        sid: i32,
        session: Arc<Session>,
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            release(self.sid, &self.session);
        }
    }

    /// One of what keeps `s` registered lets go; the last one forgets it.
    fn release(sid: i32, s: &Arc<Session>) {
        if s.holds.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut map = SESSIONS.lock();
            if map.get(&sid).is_some_and(|x| Arc::ptr_eq(x, s)) {
                map.remove(&sid);
            }
        }
    }

    pub fn register(pid: i32, hang_up: Box<dyn FnOnce() + Send>) -> Option<Hold> {
        let p = u32::try_from(pid).ok().filter(|p| *p > 1)?;
        let s = Arc::new(Session { group: ProcGroup::attach_terminal(p), hang_up: Mutex::new(Some(hang_up)), holds: AtomicUsize::new(2) });
        SESSIONS.lock().insert(pid, s.clone());
        Some(Hold { sid: pid, session: s })
    }

    /// Whether a session is registered under `sid` (tests).
    #[cfg(test)]
    pub fn registered(sid: i32) -> bool {
        session(sid).is_some()
    }

    /// The session's live processes. A job lists a process until its object is gone, and a
    /// listed pid cannot have been reused, so "it still runs" is the whole check.
    fn live(s: &Session) -> Vec<u32> {
        s.group.members().into_iter().filter(|p| own_pid_alive(*p)).collect()
    }

    pub fn leader_exited(sid: i32) {
        let Some(s) = session(sid) else { return };
        // A job signals "empty" only through a completion port; its members are few, so poll.
        let mut pause = Duration::from_millis(20);
        while !live(&s).is_empty() {
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(500));
        }
        // Already on a thread of its own: the close may wait for the last frame to be read.
        let close = s.hang_up.lock().take();
        if let Some(close) = close {
            close();
        }
        release(sid, &s);
    }

    /// Never `terminated`: Task Manager's End task (`TerminateProcess`) leaves exit code 1
    /// and nothing else, so it cannot be told from a process that exited with 1 itself. Only
    /// the ends Workbench causes are known as such (the terminals slice notes them).
    pub fn wait(child: &mut dyn portable_pty::Child) -> std::io::Result<super::LeaderExit> {
        Ok(super::leader_exit(child.wait()?, false))
    }

    pub fn members(sid: i32) -> Vec<(i32, i32)> {
        session(sid).map(|s| live(&s).into_iter().map(|p| (p as i32, sid)).collect()).unwrap_or_default()
    }

    pub fn hang_up(sid: i32) {
        let Some(close) = session(sid).and_then(|s| s.hang_up.lock().take()) else { return };
        // ClosePseudoConsole can wait until the console host's last frame was read (the
        // reader drains it): not on the caller's thread.
        let _ = std::thread::Builder::new().name(format!("pty-close-{sid}")).spawn(close);
    }

    pub fn force_end(sid: i32) {
        if let Some(s) = session(sid) {
            s.group.kill();
        }
    }

    /// Pids of the processes in the sessions `sids`.
    fn pids_of(sids: impl IntoIterator<Item = i32>) -> HashSet<u32> {
        sids.into_iter().filter_map(session).flat_map(|s| live(&s)).collect()
    }

    /// Ends a Restart Manager session when dropped.
    struct RmSession(u32);

    impl Drop for RmSession {
        fn drop(&mut self) {
            // SAFETY: a session handle from RmStartSession, ended once.
            unsafe { RmEndSession(self.0) };
        }
    }

    /// The pids of the processes that hold `path` open, from the Restart Manager; `None`
    /// when it cannot tell.
    fn holding(path: &Path) -> Option<Vec<u32>> {
        let name = wide_path(path).ok()?;
        let mut handle = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        // SAFETY: `handle` and `key` (CCH_RM_SESSION_KEY + 1 units, as documented) outlive the call.
        if unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) } != ERROR_SUCCESS {
            return None;
        }
        let rm = RmSession(handle);
        let names = [name.as_ptr()];
        // SAFETY: one NUL-terminated path that outlives the call; no applications, no services.
        let r = unsafe { RmRegisterResources(rm.0, 1, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) };
        if r != ERROR_SUCCESS {
            return None;
        }
        let mut room = 8usize;
        for _ in 0..4 {
            let mut infos = vec![RM_PROCESS_INFO::default(); room];
            let (mut needed, mut count, mut reasons) = (0u32, room as u32, 0u32);
            // SAFETY: `infos` has room for `count` entries; the counts and reasons outlive the call.
            let r = unsafe { RmGetList(rm.0, &mut needed, &mut count, infos.as_mut_ptr(), &mut reasons) };
            match r {
                ERROR_SUCCESS => return Some(infos.iter().take(count as usize).map(|i| i.Process.dwProcessId).collect()),
                // More processes than room (they can change between calls): again, larger.
                ERROR_MORE_DATA => room = needed as usize + 4,
                _ => return None,
            }
        }
        None
    }

    pub fn holders(sids: &[i32], paths: &[PathBuf]) -> HashMap<PathBuf, Vec<i32>> {
        let mut out: HashMap<PathBuf, Vec<i32>> = HashMap::new();
        let owners: Vec<(i32, HashSet<u32>)> =
            sids.iter().filter(|s| **s > 1).map(|s| (*s, pids_of([*s]))).filter(|(_, pids)| !pids.is_empty()).collect();
        if owners.is_empty() {
            return out;
        }
        for p in paths {
            let Some(pids) = holding(p) else { continue };
            for (sid, own) in &owners {
                if pids.iter().any(|pid| own.contains(pid)) {
                    let v = out.entry(p.clone()).or_default();
                    if !v.contains(sid) {
                        v.push(*sid);
                    }
                }
            }
        }
        out
    }

    pub fn held_outside(path: &Path, ours: &HashSet<i32>) -> bool {
        let Some(pids) = holding(path).filter(|p| !p.is_empty()) else { return false };
        let own = pids_of(ours.iter().copied());
        pids.iter().any(|p| !own.contains(p))
    }

    /// Whether the directories `a` and `b` are one: the same names without regard to case
    /// or a trailing separator (a process's current directory ends with `\`), else the
    /// same canonical path (8.3 short names, links).
    fn same_dir(a: &Path, b: &Path) -> bool {
        let equal = |a: &Path| path::strip_prefix(a, b).is_some_and(|rest| rest.as_os_str().is_empty());
        if equal(a) {
            return true;
        }
        // Canonicalizing opens the directory: only for a name that can be `b`'s.
        let may_be = match (a.file_name(), b.file_name()) {
            (Some(x), Some(y)) => x.eq_ignore_ascii_case(y) || x.to_string_lossy().contains('~'),
            _ => false,
        };
        may_be && path::canonicalize(a).is_ok_and(|c| equal(&c))
    }

    pub fn runs_outside(cwd: &Path, ours: &HashSet<i32>, matches: &dyn Fn(&[u8]) -> bool) -> bool {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let want = path::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let own = pids_of(ours.iter().copied());
        let me = std::process::id();
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_user(UpdateKind::Always));
        let user = sys.process(Pid::from_u32(me)).and_then(|p| p.user_id()).cloned();
        // Directories and command lines of this user's processes only (reading one opens the process).
        let mine: Vec<Pid> = sys
            .processes()
            .iter()
            .filter(|(pid, p)| pid.as_u32() != me && !own.contains(&pid.as_u32()) && user.is_some() && p.user_id() == user.as_ref())
            .map(|(pid, _)| *pid)
            .collect();
        if mine.is_empty() {
            return false;
        }
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&mine),
            true,
            ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always).with_cmd(UpdateKind::Always),
        );
        mine.iter().filter_map(|pid| sys.process(*pid)).any(|p| {
            p.cwd().is_some_and(|d| same_dir(d, &want)) && {
                let mut line: Vec<u8> = vec![];
                for a in p.cmd() {
                    line.extend_from_slice(a.to_string_lossy().as_bytes());
                    line.push(0);
                }
                matches(&line)
            }
        })
    }

    /// Chunks the pump thread read; it ends (the channel closes) with the output.
    pub struct Output {
        rx: Receiver<Vec<u8>>,
        chunk: Vec<u8>,
        at: usize,
    }

    impl Output {
        pub fn new(mut reader: Box<dyn Read + Send>, _master: &dyn MasterPty, _hold_back: bool, label: &str) -> std::io::Result<Output> {
            // Bounded: a reader that falls behind slows the pump, not the memory.
            let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
            std::thread::Builder::new().name(format!("pty-pump-{label}")).spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                let mut tx = Some(tx);
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) => break, // the pseudoconsole closed (a broken pipe reads as 0)
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    // Once nobody takes the output, it is still read and dropped until it
                    // ends: ClosePseudoConsole waits for the pipe to be drained.
                    if tx.as_ref().is_some_and(|t| t.send(buf[..n].to_vec()).is_err()) {
                        tx = None;
                    }
                }
            })?;
            Ok(Output { rx, chunk: vec![], at: 0 })
        }

        pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.at >= self.chunk.len() {
                match self.rx.recv() {
                    Ok(c) => (self.chunk, self.at) = (c, 0),
                    Err(_) => return Ok(0),
                }
            }
            let n = (self.chunk.len() - self.at).min(buf.len());
            buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }

        pub fn readable_within(&mut self, timeout: Duration) -> bool {
            if self.at < self.chunk.len() {
                return true;
            }
            match self.rx.recv_timeout(timeout) {
                Ok(c) => {
                    (self.chunk, self.at) = (c, 0);
                    true
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn parses_proc_stat_with_odd_command_names() {
        let line = "1234 (we(ird) name) S 1 1200 1100 34816 1234 4194560 0 0";
        assert_eq!(imp::parse_stat(line), Some(("S", 1200, 1100)));
        assert_eq!(imp::parse_stat("garbage"), None);
    }

    /// Signals that ask a process to end make its exit `terminated`; a crash's do not (they
    /// dump core, so `pty::tests` does not raise them).
    #[cfg(unix)]
    #[test]
    fn only_requests_to_end_are_terminating_signals() {
        for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGKILL, libc::SIGINT] {
            assert!(imp::terminating(sig), "{sig}");
        }
        for sig in [libc::SIGSEGV, libc::SIGABRT, libc::SIGBUS, libc::SIGFPE, libc::SIGILL, libc::SIGQUIT, libc::SIGUSR1, libc::SIGPIPE] {
            assert!(!imp::terminating(sig), "{sig}");
        }
    }

    #[test]
    fn nothing_is_known_of_sessions_never_registered() {
        // Pid 1 is never a terminal's (and 0 and below are ignored).
        assert!(holders(&[0, 1], &[PathBuf::from("x")]).is_empty());
        #[cfg(windows)]
        assert!(members(i32::MAX - 7).is_empty());
    }

    #[test]
    fn windows_terminal_variables_are_cleared_only_on_windows() {
        let wt = ["WT_SESSION", "WT_PROFILE_ID"];
        assert_eq!(wt.iter().all(|v| PARENT_TERMINAL_VARS.contains(v)), cfg!(windows));
        #[cfg(unix)]
        assert!(PARENT_TERMINAL_VARS.is_empty());
    }

    /// Windows: a session is a job that holds what its leader starts (a grandchild
    /// included), whose files the Restart Manager names, and which a kill ends: the
    /// counterpart of the HUP-immune session test (`pty::tests`).
    #[cfg(windows)]
    #[tokio::test]
    async fn a_session_is_a_job_that_holds_its_files_and_ends_together() {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        async fn eventually(mut f: impl FnMut() -> bool) -> bool {
            for _ in 0..400 {
                if f() {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            false
        }

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("held.txt");
        let other = dir.path().join("free.txt");
        std::fs::write(&other, "x").unwrap();
        // The first ping gives the leader time to join its job; the second (a grandchild of
        // this test) keeps `file` open through cmd's redirection.
        // As written (`raw_arg`): cmd.exe does not read CommandLineToArgvW's `\"` escapes.
        let script = format!("ping -n 2 127.0.0.1 >nul & ping -n 60 127.0.0.1 > \"{}\"", file.display());
        let mut child = std::process::Command::new("cmd")
            .args(["/d", "/c"])
            .raw_arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sid = child.id() as i32;
        let hung_up = Arc::new(AtomicBool::new(false));
        let h = hung_up.clone();
        let handle = register(sid, move || h.store(true, Ordering::Release));
        assert_eq!(handle.sid(), sid);
        assert!(eventually(|| members(sid).len() >= 2).await, "the leader and its ping: {:?}", members(sid));
        assert!(members(sid).iter().all(|(_, g)| *g == sid));
        assert!(eventually(|| holders(&[sid], std::slice::from_ref(&file)).get(&file) == Some(&vec![sid])).await, "the job holds the file");
        assert!(!holders(&[sid], std::slice::from_ref(&other)).contains_key(&other));
        assert!(!held_outside(&file, &[sid].into()));
        assert!(held_outside(&file, &HashSet::new()));
        assert!(!held_outside(&other, &HashSet::new()));
        let pids: Vec<u32> = members(sid).into_iter().map(|(p, _)| p as u32).collect();
        // cmd ignores CTRL_CLOSE_EVENT here (no pseudoconsole): the grace period ends it.
        kill(sid, Duration::from_millis(300), || false).await;
        assert!(eventually(|| hung_up.load(Ordering::Acquire)).await, "the hang-up ran");
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(eventually(|| pids.iter().all(|p| !crate::util::os::proc::pid_alive(*p as i32))).await, "{pids:?} outlived the kill");
        assert!(members(sid).is_empty());
        // The waiter's part: the pseudoconsole closes. The session is forgotten only once
        // no handle is left: until then, its pid cannot be another session's.
        leader_exited(sid);
        assert!(holders(&[sid], std::slice::from_ref(&file)).is_empty());
        let copy = handle.clone();
        drop(handle);
        assert!(imp::registered(sid), "a handle is left");
        drop(copy);
        assert!(!imp::registered(sid));
    }
}
