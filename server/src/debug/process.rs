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
use tokio::sync::mpsc;

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
