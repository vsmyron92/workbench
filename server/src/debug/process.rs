//! Starting a debug adapter: on the host, or in the project's dev container through
//! `docker exec -i` (no TTY: DAP is a byte stream), over stdio or TCP. The adapter
//! leads its own process group so ending a session ends it and what it started;
//! in a container, the wrapper's pid file lets `kill_inside` do the same.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use super::adapters::{Adapter, AdapterKind, Transport};
use super::client::{DapClient, Incoming};
use crate::config::project::DebugRequest;
use crate::devcontainer::ExecTarget;

pub struct AdapterProc {
    pub child: Child,
    group: crate::util::os::proc::ProcGroup,
    /// `(docker, container)` when it runs in a dev container.
    inside: Option<(String, String)>,
    session_id: String,
}

pub struct Started {
    pub client: Arc<DapClient>,
    pub incoming: mpsc::Receiver<Incoming>,
    pub proc: AdapterProc,
    /// Output of the adapter process that is not DAP (stderr; stdout for TCP adapters).
    pub stderr: Option<ChildStderr>,
    pub stdout_log: Option<ChildStdout>,
    /// The loopback port of a TCP adapter (child sessions connect to it too).
    pub port: Option<u16>,
}

/// The directory an adapter process runs in.
#[derive(Debug, Clone, Copy)]
pub enum AdapterDir<'a> {
    /// The launch's working directory (see `runs_in_project`).
    Project(&'a Path),
    /// A directory of Workbench's own on the host (`/` in a dev container): nothing
    /// of the repository is on an interpreter's module path.
    Neutral(&'a Path),
}

/// Whether the adapter process runs in the launch's working directory. Only gdb
/// launches need it (before GDB 15 the launch request has no `cwd`: the program
/// inherits gdb's; gdb imports nothing from it) and delve launches (`go build`
/// finds the module from its working directory). Every other adapter, and every
/// attach, starts in a neutral directory: `python3 -m debugpy.adapter` puts its
/// working directory first on `sys.path`, so a repository's `debugpy/` or
/// `platform.py` would be imported by the adapter, and an attach to an unrelated
/// process would run repository code. The DAP `cwd` argument still sets the
/// program's working directory.
pub fn runs_in_project(kind: AdapterKind, request: DebugRequest) -> bool {
    request == DebugRequest::Launch && matches!(kind, AdapterKind::Gdb | AdapterKind::Delve)
}

/// `docker exec … -w <dir> …` with another working directory.
pub fn with_workdir(argv: Vec<String>, dir: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut in_options = true;
    let mut replace_next = false;
    for a in argv {
        if replace_next {
            out.push(dir.to_string());
            replace_next = false;
            continue;
        }
        if a == "/bin/sh" {
            in_options = false;
        }
        if in_options && a == "-w" {
            replace_next = true;
        }
        out.push(a);
    }
    out
}

/// Whether `host` names this machine's loopback interface (without resolving it).
pub fn loopback_host(host: &str) -> Option<std::net::IpAddr> {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    }
    h.parse::<std::net::IpAddr>().ok().filter(|ip| ip.is_loopback())
}

/// A new DAP connection to an adapter that serves on a loopback port: a child
/// session (`startDebugging`) of debugpy (`configuration.connect`) or of a TCP
/// adapter talks to the parent's adapter, which knows the child process.
pub async fn connect(host: &str, port: u16, timeout: Duration) -> Result<(Arc<DapClient>, mpsc::Receiver<Incoming>), String> {
    let ip = loopback_host(host).ok_or_else(|| format!("the debugger asked Workbench to connect to {host}:{port}, which is not this machine's loopback interface"))?;
    let stream = tokio::time::timeout(timeout, tokio::net::TcpStream::connect((ip, port)))
        .await
        .map_err(|_| format!("the debugger did not accept a connection on {host}:{port} within {}s", timeout.as_secs()))?
        .map_err(|e| format!("could not connect to the debugger on {host}:{port}: {e}"))?;
    let _ = stream.set_nodelay(true);
    let (r, w) = stream.into_split();
    Ok(DapClient::start(r, w))
}

/// `docker exec -i -t …` → without `-t`: a TTY would mangle the byte stream
/// (CRLF translation, echo) and merge stderr into stdout.
pub fn without_tty(argv: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    // Docker's own options end where the in-container wrapper (`/bin/sh -c …`) starts.
    let mut in_options = true;
    for a in argv {
        if in_options && (a == "-t" || a.starts_with("--detach-keys")) {
            continue;
        }
        if a == "/bin/sh" {
            in_options = false;
        }
        out.push(a);
    }
    out
}

/// A free loopback port for a TCP adapter (bound and released: another process may
/// take it in between, then the adapter fails to listen and we report that).
fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(l.local_addr()?.port())
}

/// Ports handed out recently: between picking a free port and the server binding it,
/// another session must not be given the same one.
static ISSUED_PORTS: parking_lot::Mutex<Vec<(u16, std::time::Instant)>> = parking_lot::Mutex::new(Vec::new());

/// Where debug servers' ports are picked: below every system's ephemeral range (Linux
/// 32768.., Windows and macOS 49152..). A port the system hands out for `bind(0)` or an
/// outgoing connection may be given away again a moment after it was released (1% of
/// the time on Linux, measured), and OpenOCD binds its gdb port seconds after it started.
const SERVER_PORTS: std::ops::Range<u32> = 20000..30000;

/// `n` distinct free loopback ports for a debug server's listeners, none that this process
/// handed out in the last two minutes. Whoever uses them binds them soon: they are only
/// checked, not held.
pub fn free_ports(n: usize) -> std::io::Result<Vec<u16>> {
    pick_ports(n, bindable)
}

/// Whether this computer lets us listen on `port`. Windows keeps port ranges for itself
/// (Hyper-V, WinNAT: `netsh int ipv4 show excludedportrange`) and refuses to bind them with
/// "forbidden by its access permissions" (WSAEACCES, 10013): a debug server told to listen
/// there fails to start, and which ranges are kept differs from computer to computer. Nothing
/// in our range is reserved elsewhere, and there a probe that bound the port would be a
/// listener for a moment, which a forking thread copies until it execs (see `pick_ports`).
#[cfg(windows)]
fn bindable(port: u16) -> bool {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok()
}

#[cfg(not(windows))]
fn bindable(_port: u16) -> bool {
    true
}

/// `n` ports of `SERVER_PORTS` that nothing listens on, that `usable` accepts and that this
/// process has not issued lately.
fn pick_ports(n: usize, usable: impl Fn(u16) -> bool) -> std::io::Result<Vec<u16>> {
    use rand::RngCore;
    let mut issued = ISSUED_PORTS.lock();
    issued.retain(|(_, at)| at.elapsed() < Duration::from_secs(120));
    let mut out: Vec<u16> = vec![];
    let mut rng = rand::rng();
    for _ in 0..2000 {
        if out.len() == n {
            break;
        }
        let port = (SERVER_PORTS.start + rng.next_u32() % (SERVER_PORTS.end - SERVER_PORTS.start)) as u16;
        // (`listening_on` asks the kernel; a probe that bound the port would be a listener
        // for a moment, and one that a forking thread copies stays one until it execs.)
        let taken = out.contains(&port) || issued.iter().any(|(p, _)| *p == port) || listening_on(port).is_some() || !usable(port);
        if !taken {
            out.push(port);
        }
    }
    // A crowded range (or one that is not ours to bind): whatever the system offers.
    let mut held = vec![];
    while out.len() < n {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = l.local_addr()?.port();
        held.push(l);
        if !out.contains(&port) && !issued.iter().any(|(p, _)| *p == port) {
            out.push(port);
        }
        if held.len() > 64 {
            return Err(std::io::Error::other("no free port"));
        }
    }
    issued.extend(out.iter().map(|p| (*p, std::time::Instant::now())));
    Ok(out)
}

/// The loopback address something listens on at `port`, found without connecting: a
/// gdb stub that serves one connection (`st-util`, `gdbserver`, pyOCD, QEMU) would take
/// a probe for its client. Linux asks the kernel's table of listening sockets; elsewhere
/// binding the port is tried (it fails while anything holds it).
pub fn listening_on(port: u16) -> Option<std::net::IpAddr> {
    #[cfg(target_os = "linux")]
    if let Some(found) = proc_listener(port) {
        return found;
    }
    bind_probe(port)
}

fn bind_probe(port: u16) -> Option<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener};
    [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)]
        .into_iter()
        .find(|ip| matches!(TcpListener::bind((*ip, port)), Err(e) if e.kind() == std::io::ErrorKind::AddrInUse))
}

/// `/proc/net/tcp` and `tcp6`: a socket in state LISTEN (0A) on `port` that a loopback
/// client reaches. `None`: the tables cannot be read.
#[cfg(target_os = "linux")]
fn proc_listener(port: u16) -> Option<Option<std::net::IpAddr>> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let mut readable = false;
    let mut found = None;
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        readable = true;
        for line in text.lines().skip(1) {
            let mut cols = line.split_whitespace();
            let (Some(local), Some(state)) = (cols.nth(1), cols.nth(1)) else { continue };
            let Some((addr, p)) = local.rsplit_once(':') else { continue };
            if state != "0A" || u16::from_str_radix(p, 16) != Ok(port) {
                continue;
            }
            // The address as the kernel prints it: IPv4 as one little-endian word, IPv6 as
            // four of them.
            let ip = if v6 {
                match addr {
                    "00000000000000000000000000000000" => Some(IpAddr::V6(Ipv6Addr::LOCALHOST)), // `::`: dual-stack or v6-only, ::1 is reached either way
                    "00000000000000000000000001000000" => Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
                    // ::ffff:127.0.0.1 and ::ffff:0.0.0.0: an IPv4 listener of a dual-stack socket
                    a if a == "0000000000000000FFFF00000100007F" || a == "0000000000000000FFFF000000000000" => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    _ => None,
                }
            } else {
                match addr {
                    "00000000" => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    a if a.ends_with("7F") && a.len() == 8 => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    _ => None,
                }
            };
            if ip.is_some() && (found.is_none() || ip == Some(IpAddr::V4(Ipv4Addr::LOCALHOST))) {
                found = ip;
            }
        }
    }
    readable.then_some(found)
}

pub fn substitute_port(args: &[String], port: u16) -> Vec<String> {
    args.iter().map(|a| a.replace("{port}", &port.to_string())).collect()
}

/// Start `adapter` for session `session_id` in `dir`, with `extra_args` after its
/// configured ones.
pub async fn spawn(adapter: &Adapter, session_id: &str, dir: AdapterDir<'_>, target: Option<(&ExecTarget, String)>, extra_args: &[String]) -> Result<Started, String> {
    let cwd = match dir {
        AdapterDir::Project(p) | AdapterDir::Neutral(p) => p,
    };
    let port = match adapter.transport {
        Transport::Tcp => Some(free_port().map_err(|e| format!("no free port for the adapter: {e}"))?),
        Transport::Stdio => None,
    };
    let mut args = match port {
        Some(p) => substitute_port(&adapter.args, p),
        None => adapter.args.clone(),
    };
    args.extend(extra_args.iter().cloned());
    // The Python launcher's `-3` (debugpy through `py` on Windows) goes before them all.
    args.splice(0..0, super::adapters::launcher_args(adapter));
    let mut cmd;
    let mut inside = None;
    match target {
        Some((t, path)) => {
            if adapter.transport == Transport::Tcp {
                return Err(format!(
                    "{} talks DAP over TCP, which Workbench cannot reach inside the dev container yet: use a stdio adapter there (gdb, lldb-dap, debugpy) or turn off \"Run terminals and runs in the container\"",
                    adapter.label
                ));
            }
            let mut argv = vec![path];
            argv.extend(args);
            let env: Vec<(String, Option<String>)> = adapter.env.iter().map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
            let (docker_argv, host_env) = t.wrap(session_id, &argv, cwd, &env);
            let mut docker_argv = without_tty(docker_argv);
            if let AdapterDir::Neutral(_) = dir {
                // Not the workspace folder (where `wrap` falls back to).
                docker_argv = with_workdir(docker_argv, "/");
            }
            cmd = Command::new(&docker_argv[0]);
            cmd.args(&docker_argv[1..]);
            for (k, v) in host_env {
                match v {
                    Some(v) => cmd.env(k, v),
                    None => cmd.env_remove(k),
                };
            }
            inside = Some((t.docker.clone(), t.container_id.clone()));
        }
        None => {
            use crate::util::os::exe;
            let r = exe::resolve(&adapter.command).ok_or_else(|| format!("`{}` was not found on PATH. {}", adapter.command, adapter.install_hint))?;
            // A batch file (Windows) gets its arguments through cmd.exe, which would
            // reparse paths and names that hold its metacharacters.
            if r.kind == exe::Kind::Batch && !exe::batch_args_safe(&args) {
                return Err(format!(
                    "{} is a batch file ({}), and cmd.exe would misread an argument with % ! ^ & | < > \" or a line break: point [debug.adapters.{}] command at the program itself",
                    adapter.label,
                    r.program.display(),
                    adapter.id
                ));
            }
            cmd = exe::command(&r);
            cmd.args(&args);
            for (k, v) in &adapter.env {
                cmd.env(k, v);
            }
        }
    }
    crate::util::proc::clean_env(&mut cmd);
    // Workbench's own agent token never reaches a debug adapter.
    cmd.env_remove("WORKBENCH_AGENT_TOKEN");
    cmd.current_dir(if cwd.is_dir() { cwd } else { Path::new("/") })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::util::os::proc::ProcGroup::prepare(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", adapter.label))?;
    let group = crate::util::os::proc::ProcGroup::attach(&child);
    let stderr = child.stderr.take();
    let proc_ = |child| AdapterProc { child, group: group.clone(), inside: inside.clone(), session_id: session_id.to_string() };
    match port {
        None => {
            let stdin = child.stdin.take().ok_or("no stdin")?;
            let stdout = child.stdout.take().ok_or("no stdout")?;
            let (client, incoming) = DapClient::start(stdout, stdin);
            Ok(Started { client, incoming, proc: proc_(child), stderr, stdout_log: None, port: None })
        }
        Some(port) => {
            let stdout = child.stdout.take();
            let deadline = Instant::now() + adapter.connect_timeout;
            let stream = loop {
                match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                    Ok(s) => break s,
                    Err(e) => {
                        if let Ok(Some(status)) = child.try_wait() {
                            return Err(format!("{} exited before it listened on port {port} ({status})", adapter.label));
                        }
                        if Instant::now() > deadline {
                            let mut p = proc_(child);
                            p.kill().await;
                            return Err(format!("{} did not listen on 127.0.0.1:{port} within {}s: {e}", adapter.label, adapter.connect_timeout.as_secs()));
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            };
            let _ = stream.set_nodelay(true);
            let (r, w) = stream.into_split();
            let (client, incoming) = DapClient::start(r, w);
            Ok(Started { client, incoming, proc: proc_(child), stderr, stdout_log: stdout, port: Some(port) })
        }
    }
}

impl AdapterProc {
    /// End the adapter and its process group: SIGTERM, then SIGKILL after a grace
    /// period; in a container, the process group inside too.
    pub async fn kill(&mut self) {
        if let Some((docker, container)) = self.inside.take() {
            crate::devcontainer::kill_inside(&docker, &container, &self.session_id).await;
        }
        if let Ok(Some(_)) = self.child.try_wait() {
            // The leader is gone; children in its group may not be.
            self.group.kill();
            return;
        }
        self.group.terminate();
        if tokio::time::timeout(Duration::from_millis(1500), self.child.wait()).await.is_err() {
            self.group.kill();
            let _ = self.child.kill().await;
        } else {
            self.group.kill();
        }
    }

    /// Wait up to `d` for the adapter to exit by itself.
    pub async fn wait_exit(&mut self, d: Duration) -> bool {
        tokio::time::timeout(d, self.child.wait()).await.is_ok()
    }

    /// How the adapter ended, waiting up to `d` for it.
    pub async fn exit_status(&mut self, d: Duration) -> Option<std::process::ExitStatus> {
        tokio::time::timeout(d, self.child.wait()).await.ok().and_then(Result::ok)
    }
}

/// A debug server (OpenOCD, J-Link GDB Server…) of one session: its own process group,
/// output forwarded line by line, its end observable.
#[derive(Clone)]
pub struct ServerProc {
    group: crate::util::os::proc::ProcGroup,
    exit: watch::Receiver<Option<String>>,
}

impl ServerProc {
    /// How the process ended, once it has.
    pub fn exited(&self) -> Option<String> {
        self.exit.borrow().clone()
    }

    /// A receiver that turns `Some(how it ended)` when the process ends.
    pub fn watch(&self) -> watch::Receiver<Option<String>> {
        self.exit.clone()
    }

    /// End the server and what it started: SIGTERM, then SIGKILL after a grace period.
    pub async fn kill(&self) {
        if self.exited().is_none() {
            self.group.terminate();
            let mut rx = self.exit.clone();
            if tokio::time::timeout(Duration::from_millis(2000), rx.wait_for(|e| e.is_some())).await.is_err() {
                self.group.kill();
            }
        }
        // The leader is gone; members of its group may not be.
        self.group.kill();
    }
}

/// Start `server` in `cwd` with `args`: stdin closed, stdout and stderr (merged into
/// `on_line`, line by line). The environment is the child environment of Workbench's
/// other processes, the server's own `env`, then the launch configuration's.
pub fn spawn_server(server: &super::servers::Server, args: &[String], cwd: &Path, env: &[(String, String)], on_line: impl Fn(String) + Send + Sync + 'static) -> Result<ServerProc, String> {
    use crate::util::os::exe;
    let path = super::servers::locate(&server.command).ok_or_else(|| format!("`{}` was not found on PATH. {}", server.command, server.install_hint))?;
    let r = exe::classify(path);
    if r.kind == exe::Kind::Batch && !exe::batch_args_safe(args) {
        return Err(format!(
            "{} is a batch file ({}), and cmd.exe would misread an argument with % ! ^ & | < > \" or a line break: point [debug.servers.{}] command at the program itself",
            server.label,
            r.program.display(),
            server.id
        ));
    }
    let mut cmd = exe::command(&r);
    cmd.args(args);
    for (k, v) in server.env.iter().chain(env) {
        cmd.env(k, v);
    }
    crate::util::proc::clean_env(&mut cmd);
    cmd.env_remove("WORKBENCH_AGENT_TOKEN");
    cmd.current_dir(if cwd.is_dir() { cwd } else { Path::new("/") }).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    crate::util::os::proc::ProcGroup::prepare(&mut cmd);
    // Nothing tells a debug server that Workbench is gone: left running it would hold its probe.
    crate::util::os::proc::ProcGroup::prepare_dies_with_parent(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", server.label))?;
    let group = crate::util::os::proc::ProcGroup::attach(&child);
    let on_line = Arc::new(on_line);
    if let Some(o) = child.stdout.take() {
        let f = on_line.clone();
        forward_lines(o, move |l| f(l));
    }
    if let Some(e) = child.stderr.take() {
        let f = on_line.clone();
        forward_lines(e, move |l| f(l));
    }
    let (tx, exit) = watch::channel(None);
    tokio::spawn(async move {
        let how = match child.wait().await {
            Ok(st) => crate::util::os::proc::exit_text(&st),
            Err(e) => format!("lost: {e}"),
        };
        // The readers deliver the last lines a moment later: whoever reports the end waits for them.
        tx.send_replace(Some(how));
    });
    Ok(ServerProc { group, exit })
}

/// Lines of a reader, capped (a chatty adapter cannot flood the console).
pub fn forward_lines<R: tokio::io::AsyncRead + Unpin + Send + 'static>(r: R, mut f: impl FnMut(String) + Send + 'static) {
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(r).lines();
        let mut count = 0usize;
        while let Ok(Some(line)) = lines.next_line().await {
            count += 1;
            if count <= 5000 {
                f(line);
            } else if count == 5001 {
                f("… (further adapter output dropped)".into());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows refuses to bind the port ranges it keeps for itself: a port the computer will
    /// not let us listen on is never handed to a debug server, however often it is drawn.
    #[test]
    fn ports_the_computer_will_not_let_us_bind_are_never_issued() {
        // Half the range is "reserved": even ports.
        for _ in 0..20 {
            let ports = pick_ports(6, |p| p % 2 == 1).unwrap();
            assert_eq!(ports.len(), 6);
            assert!(ports.iter().all(|p| p % 2 == 1 && SERVER_PORTS.contains(&u32::from(*p))), "{ports:?}");
            let mut sorted = ports.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 6, "no port twice: {ports:?}");
        }
        // Everything in our range is reserved: the system's own choice, like a crowded range.
        let ports = pick_ports(2, |_| false).unwrap();
        assert_eq!(ports.len(), 2);
        assert!(ports.iter().all(|p| !SERVER_PORTS.contains(&u32::from(*p))), "{ports:?}");
    }

    #[test]
    fn docker_exec_loses_its_tty() {
        let argv: Vec<String> = ["docker", "exec", "-i", "-t", "--detach-keys=ctrl-^,ctrl-^,ctrl-^", "-u", "vscode", "-w", "/w", "-e", "WB_PIDFILE", "abc", "/bin/sh", "-c", "W", "wb-exec", "gdb", "-t", "x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = without_tty(argv);
        assert_eq!(out[..4], ["docker", "exec", "-i", "-u"]);
        assert!(!out[..out.len() - 3].contains(&"-t".to_string()));
        // Arguments of the adapter itself stay untouched.
        assert_eq!(out[out.len() - 3..], ["gdb", "-t", "x"]);
        assert_eq!(substitute_port(&["--port".into(), "{port}".into(), "127.0.0.1:{port}".into()], 4711), vec!["--port", "4711", "127.0.0.1:4711"]);
        // A neutral working directory inside the container; the adapter's own `-w` stays.
        let argv: Vec<String> = ["docker", "exec", "-i", "-w", "/workspaces/app", "abc", "/bin/sh", "-c", "W", "wb-exec", "tool", "-w", "x"].iter().map(|s| s.to_string()).collect();
        let out = with_workdir(argv, "/");
        assert_eq!(out[3..5], ["-w", "/"]);
        assert_eq!(out[out.len() - 2..], ["-w", "x"]);
    }

    #[test]
    fn adapters_start_outside_the_project_unless_they_need_it() {
        use AdapterKind::*;
        assert!(runs_in_project(Gdb, DebugRequest::Launch));
        assert!(runs_in_project(Delve, DebugRequest::Launch));
        for k in [Debugpy, Lldb, Codelldb, Generic] {
            assert!(!runs_in_project(k, DebugRequest::Launch), "{k:?}");
        }
        for k in [Gdb, Delve, Debugpy, Lldb, Codelldb, Generic] {
            assert!(!runs_in_project(k, DebugRequest::Attach), "attach {k:?}");
        }
    }

    #[test]
    fn listeners_are_found_without_connecting_to_them() {
        let ports = free_ports(3).unwrap();
        assert_eq!(ports.len(), 3);
        assert!(ports[0] != ports[1] && ports[1] != ports[2] && ports[0] != ports[2], "{ports:?}");
        if cfg!(target_os = "linux") {
            assert_eq!(listening_on(ports[0]), None, "nothing listens yet");
        }
        let l = std::net::TcpListener::bind(("127.0.0.1", ports[0])).unwrap();
        assert_eq!(listening_on(ports[0]), Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
        // The probe did not take the listener's one connection.
        let c = std::net::TcpStream::connect(("127.0.0.1", ports[0])).unwrap();
        assert!(l.accept().is_ok());
        drop((c, l));
        // Gone for good, though not at the very instant: a thread of this process that forks
        // meanwhile holds a copy of the socket until it execs.
        if cfg!(target_os = "linux") {
            let end = std::time::Instant::now() + Duration::from_secs(3);
            while listening_on(ports[0]).is_some() {
                assert!(std::time::Instant::now() < end, "still listening on {} three seconds after it closed", ports[0]);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// The server is told to die with Workbench: the system ends it after a crash or a
    /// SIGKILL, when no graceful `finish` runs (an OpenOCD left behind holds its probe).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_debug_server_dies_with_workbench() {
        if !crate::util::which("python3") {
            eprintln!("skipped: Python 3 is not installed");
            return;
        }
        let mut server = crate::debug::servers::find(&Default::default(), "openocd").unwrap();
        server.command = "python3".into();
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(vec![]));
        let sink = seen.clone();
        // PR_GET_PDEATHSIG (2) reads the signal the kernel will send when the parent goes.
        let code = "import ctypes; x = ctypes.c_int(); ctypes.CDLL(None).prctl(2, ctypes.byref(x)); print('pdeathsig', x.value)";
        let p = spawn_server(&server, &["-c".into(), code.into()], Path::new("/"), &[], move |l| sink.lock().push(l)).unwrap();
        let mut gone = p.watch();
        tokio::time::timeout(Duration::from_secs(10), gone.wait_for(|e| e.is_some())).await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(*seen.lock(), ["pdeathsig 15"], "SIGTERM when the parent goes");
    }

    /// Server ports come from below the ephemeral range, which `bind(0)` and outgoing
    /// connections draw from, and are never given twice.
    #[test]
    fn server_ports_stay_below_the_ephemeral_range_and_are_never_repeated() {
        let mut all = std::collections::HashSet::new();
        for _ in 0..20 {
            for p in free_ports(3).unwrap() {
                assert!(SERVER_PORTS.contains(&u32::from(p)), "{p}");
                assert!(all.insert(p), "{p} was handed out twice");
            }
        }
        assert_eq!(all.len(), 60);
    }

    /// A socket that merely holds a port (a client's end of a connection) is not a
    /// listener: it would make a stub look ready that is not.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_listening_sockets_count() {
        let server = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = std::net::TcpStream::connect(server.local_addr().unwrap()).unwrap();
        let held = client.local_addr().unwrap().port();
        assert_ne!(held, server.local_addr().unwrap().port());
        assert_eq!(listening_on(held), None, "the client's own port");
        // An IPv6-only listener is found, and a client reaches it through ::1.
        if let Ok(v6) = std::net::TcpListener::bind(("::1", 0)) {
            assert_eq!(listening_on(v6.local_addr().unwrap().port()), Some(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
        }
        // A listener on all interfaces is reached through loopback.
        let all = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        assert_eq!(listening_on(all.local_addr().unwrap().port()), Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn child_connections_stay_on_loopback() {
        assert!(loopback_host("127.0.0.1").is_some());
        assert!(loopback_host("localhost").is_some());
        assert!(loopback_host("::1").is_some());
        assert!(loopback_host("[::1]").is_some());
        assert!(loopback_host("127.0.0.2").is_some());
        assert!(loopback_host("10.0.0.5").is_none());
        assert!(loopback_host("example.com").is_none());
        assert!(loopback_host("0.0.0.0").is_none());
    }
}
