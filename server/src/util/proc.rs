//! Running child processes: environment hygiene and captured runs with timeouts.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::ApiError;

/// Variables that identify the Claude Code session Workbench itself was started
/// from. Leaking them into hosted sessions makes a nested `claude` believe it runs
/// inside another one (and can route its messaging to the wrong socket).
pub const SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "AI_AGENT",
];

/// Remove session-identity variables from this process's own environment. Called
/// once at startup, before any threads exist.
pub fn scrub_own_env() {
    for k in SESSION_ENV_VARS {
        // SAFETY: called from `main` before the runtime or any other thread starts.
        unsafe { std::env::remove_var(k) };
    }
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("WORKBENCH_AGENT_") {
            unsafe { std::env::remove_var(&k) };
        }
    }
}

/// Apply the same hygiene to a child command.
pub fn clean_env(cmd: &mut Command) -> &mut Command {
    for k in SESSION_ENV_VARS {
        cmd.env_remove(k);
    }
    cmd
}

#[derive(Debug, Clone)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
    /// stderr if present, else stdout — what to show the user on failure.
    pub fn message(&self) -> String {
        let s = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        s.trim().to_string()
    }
}

/// Run `program args…` in `cwd`, capture output, and kill it after `timeout`.
pub async fn run(program: &str, args: &[&str], cwd: &Path, timeout: Duration) -> Result<Output, ApiError> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd);
    run_cmd(cmd, timeout).await
}

/// Run a prepared command (stdin closed, `os::exe::child_env` added), capture output, kill
/// on timeout. stderr is `os::shell::readable_stderr` (on Windows, PowerShell's CLIXML
/// records as text).
pub async fn run_cmd(cmd: Command, timeout: Duration) -> Result<Output, ApiError> {
    try_run_cmd(cmd, timeout).await.map_err(|e| match e {
        RunError::Spawn(e) => ApiError::internal(format!("spawn failed: {e}")),
        RunError::Wait(e) => ApiError::internal(format!("process failed: {e}")),
        RunError::TimedOut => ApiError::new(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            format!("command timed out after {}s", timeout.as_secs()),
        ),
    })
}

/// Why [`try_run_cmd`] has no output to give.
#[derive(Debug)]
pub enum RunError {
    /// The program did not start (`NotFound`: it is not installed).
    Spawn(std::io::Error),
    /// Waiting for it failed.
    Wait(std::io::Error),
    /// It ran past the timeout and was killed.
    TimedOut,
}

/// [`run_cmd`] for a caller that tells a missing program or a timeout apart from other
/// failures (`util::git`).
pub async fn try_run_cmd(mut cmd: Command, timeout: Duration) -> Result<Output, RunError> {
    clean_env(&mut cmd);
    cmd.envs(crate::util::os::exe::child_env().iter().copied());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(RunError::Spawn)?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: crate::util::os::shell::readable_stderr(&out.stderr),
        }),
        Ok(Err(e)) => Err(RunError::Wait(e)),
        Err(_) => Err(RunError::TimedOut),
    }
}
