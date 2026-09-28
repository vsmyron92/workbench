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

/// Run a prepared command (stdin closed), capture output, kill on timeout.
pub async fn run_cmd(mut cmd: Command, timeout: Duration) -> Result<Output, ApiError> {
    clean_env(&mut cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| ApiError::internal(format!("spawn failed: {e}")))?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(ApiError::internal(format!("process failed: {e}"))),
        Err(_) => Err(ApiError::new(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "timeout",
            format!("command timed out after {}s", timeout.as_secs()),
        )),
    }
}
