//! Running the git CLI.
//!
//! Every invocation gets the same hygiene: `core.quotepath=false` (paths come back
//! verbatim), `LC_ALL=C` (stable, parseable messages), no terminal prompts, no
//! pager/colour/external diff, and the Claude session variables removed. Read-only
//! commands also set `GIT_OPTIONAL_LOCKS=0` so a status refresh never fights an
//! agent's `git commit` over `index.lock`.
//!
//! Paths from clients are passed as `:(literal)` pathspecs ([`literal`]), so a file
//! called `*.rs` or `[id].tsx` is that file, not a glob. This is deliberately *not*
//! done with `GIT_LITERAL_PATHSPECS=1`: git's own internals (`stash push -u` and
//! `--keep-index` pass `:/` to their clean/checkout steps) and hooks rely on pathspec
//! magic, and break silently under that variable.
//!
//! Output is captured as bytes with a cap, so a runaway `git log` or a huge blob
//! cannot exhaust memory.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use axum::http::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::error::ApiError;
use crate::util::proc;

/// Default cap for captured stdout (log pages, diffs, blame).
pub const DEFAULT_MAX_STDOUT: usize = 64 * 1024 * 1024;
const MAX_STDERR: usize = 256 * 1024;

pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(120);
/// Commits run hooks (pre-commit linters can be slow).
pub const COMMIT_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// stdout was cut at the cap.
    #[allow(dead_code)]
    pub truncated: bool,
}

impl GitOutput {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// What to show on failure: stderr, else stdout, trimmed.
    pub fn message(&self) -> String {
        let out = self.text();
        let s = if self.stderr.trim().is_empty() { out.as_str() } else { self.stderr.as_str() };
        clean_message(s)
    }
}

/// Tidy git's error text for display: drop `hint:` lines, strip `error: `/`fatal: `
/// prefixes, collapse blank lines.
pub fn clean_message(s: &str) -> String {
    let lines: Vec<&str> = s
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.starts_with("hint:") && !l.trim().is_empty())
        .map(|l| l.strip_prefix("error: ").or_else(|| l.strip_prefix("fatal: ")).unwrap_or(l))
        .collect();
    let out = lines.join("\n");
    if out.len() > 3000 {
        let mut cut = 3000;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &out[..cut])
    } else {
        out
    }
}

/// A git invocation.
#[derive(Debug, Clone)]
pub struct Git {
    cwd: PathBuf,
    args: Vec<String>,
    read_only: bool,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    max_stdout: usize,
    env: Vec<(String, String)>,
    /// `-c key=value` before the subcommand, after Workbench's own.
    config: Vec<(String, String)>,
}

impl Git {
    /// A read-only command (`GIT_OPTIONAL_LOCKS=0`).
    pub fn read(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            args: vec![],
            read_only: true,
            stdin: None,
            timeout: READ_TIMEOUT,
            max_stdout: DEFAULT_MAX_STDOUT,
            env: vec![],
            config: vec![],
        }
    }

    /// A command that may write the index, refs or the working tree.
    pub fn write(cwd: &Path) -> Self {
        Self { read_only: false, timeout: WRITE_TIMEOUT, ..Self::read(cwd) }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, it: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(it.into_iter().map(Into::into));
        self
    }

    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    pub fn env(mut self, k: &str, v: impl Into<String>) -> Self {
        self.env.push((k.to_string(), v.into()));
        self
    }

    /// Set config `key` for this command only (`-c key=value`, which git reads after every
    /// config file and passes on to the git commands it starts). An empty value resets a
    /// list such as `credential.<url>.helper`.
    pub fn config(mut self, key: &str, value: impl Into<String>) -> Self {
        self.config.push((key.to_string(), value.into()));
        self
    }

    /// The prepared `tokio::process::Command` (also used for streamed remote ops).
    pub fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=false", "-c", "core.pager=cat"]);
        for (k, v) in &self.config {
            cmd.arg("-c").arg(format!("{k}={v}"));
        }
        cmd.args(&self.args);
        cmd.current_dir(&self.cwd)
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            // Pathspec behaviour must not depend on whatever started Workbench
            // (user paths are protected with `:(literal)` instead; see `literal`).
            .env_remove("GIT_LITERAL_PATHSPECS")
            .env_remove("GIT_GLOB_PATHSPECS")
            .env_remove("GIT_NOGLOB_PATHSPECS")
            .env_remove("GIT_ICASE_PATHSPECS")
            .env("GIT_PAGER", "cat")
            // Continue/merge/revert must never wait for an editor.
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        if self.read_only {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        proc::clean_env(&mut cmd);
        cmd
    }

    /// Run and capture. A non-zero exit is *not* an error here; see `run_ok`.
    pub async fn run(self) -> Result<GitOutput, ApiError> {
        let mut cmd = self.command();
        cmd.stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::not_configured(format!("git is not installed (no `git` on PATH){}", crate::util::os::exe::INSTALLED_SINCE))
            } else {
                ApiError::internal(format!("cannot run git: {e}"))
            }
        })?;
        let stdin_task = match (self.stdin, child.stdin.take()) {
            (Some(data), Some(mut pipe)) => Some(tokio::spawn(async move {
                let _ = pipe.write_all(&data).await;
                let _ = pipe.shutdown().await;
            })),
            _ => None,
        };
        let mut stdout = child.stdout.take().ok_or_else(|| ApiError::internal("no stdout"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| ApiError::internal("no stderr"))?;
        let max = self.max_stdout;
        let work = async {
            let read_out = async {
                let mut buf = Vec::new();
                let mut chunk = vec![0u8; 64 * 1024];
                let mut truncated = false;
                loop {
                    let n = stdout.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    if buf.len() + n > max {
                        buf.extend_from_slice(&chunk[..max - buf.len()]);
                        truncated = true;
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Ok::<_, std::io::Error>((buf, truncated))
            };
            let read_err = async {
                let mut buf = Vec::new();
                (&mut stderr).take(MAX_STDERR as u64).read_to_end(&mut buf).await?;
                // Drain the rest so git never blocks on a full pipe.
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                Ok::<_, std::io::Error>(buf)
            };
            let (out, err) = tokio::join!(read_out, read_err);
            let (out, truncated) = out?;
            if truncated {
                // We stopped reading; do not wait for a writer blocked on the pipe.
                let _ = child.start_kill();
            }
            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, out, truncated, err?))
        };
        let res = tokio::time::timeout(self.timeout, work).await;
        if let Some(t) = stdin_task {
            t.abort();
        }
        match res {
            Ok(Ok((status, stdout, truncated, stderr))) => Ok(GitOutput {
                code: if truncated { Some(0) } else { status.code() },
                stdout,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                truncated,
            }),
            Ok(Err(e)) => Err(ApiError::internal(format!("git failed: {e}"))),
            Err(_) => Err(ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                format!("git {} timed out after {}s", self.args.first().map(String::as_str).unwrap_or(""), self.timeout.as_secs()),
            )),
        }
    }

    /// Run and require exit 0; failures become a `git_error` (422) with git's message.
    pub async fn run_ok(self) -> Result<GitOutput, ApiError> {
        let out = self.run().await?;
        if out.ok() { Ok(out) } else { Err(git_error(&out)) }
    }

    /// `run_ok`, retrying briefly while another git process holds `index.lock`
    /// (agents commit in the same worktree).
    pub async fn run_ok_retry_lock(self) -> Result<GitOutput, ApiError> {
        let mut attempt = 0;
        loop {
            let out = self.clone().run().await?;
            if out.ok() {
                return Ok(out);
            }
            if attempt < 4 && is_lock_error(&out.stderr) {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(150 * attempt)).await;
                continue;
            }
            return Err(git_error(&out));
        }
    }
}

fn is_lock_error(stderr: &str) -> bool {
    stderr.contains(".lock': File exists") || stderr.contains(".lock': file exists")
}

/// A failed git command as an API error. Recognizes the common cases so the UI
/// can react (offer a smart checkout, a force delete…).
pub fn git_error(out: &GitOutput) -> ApiError {
    if let Some(e) = unsafe_repository(out) {
        return e;
    }
    let raw = format!("{}\n{}", out.stderr, out.text());
    let msg = out.message();
    if is_lock_error(&raw) {
        return ApiError::new(
            StatusCode::CONFLICT,
            "locked",
            "Another git process is running in this repository (index.lock exists). Try again in a moment.",
        );
    }
    if let Some(files) = overwritten_files(&raw) {
        let n = files.len();
        let shown: Vec<&str> = files.iter().take(5).map(String::as_str).collect();
        let more = if n > 5 { format!(" and {} more", n - 5) } else { String::new() };
        return ApiError::new(
            StatusCode::CONFLICT,
            "dirty_tree",
            format!(
                "Your local changes to {n} file{} would be overwritten: {}{more}. Commit, stash or roll them back first (or use Smart checkout).",
                if n == 1 { "" } else { "s" },
                shown.join(", ")
            ),
        );
    }
    if raw.contains("is not fully merged") {
        return ApiError::new(StatusCode::CONFLICT, "not_merged", msg);
    }
    if raw.contains("untracked working tree files would be overwritten") {
        return ApiError::new(StatusCode::CONFLICT, "untracked_overwritten", msg);
    }
    if raw.contains("not a git repository") {
        return ApiError::new(StatusCode::NOT_FOUND, "not_a_repo", msg);
    }
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "git_error", if msg.is_empty() { "git failed".into() } else { msg })
}

/// Git refusing a repository that another user owns (the `safe.directory` check: "detected
/// dubious ownership", "unsafe repository" in older gits; common on Windows for folders an
/// administrator created and on drives without owners): 403 `unsafe_repository` with git's
/// message verbatim, which names the owners and the command that trusts the folder. Only
/// where such folders are common (`os::fs::FOREIGN_OWNERS`, Windows); elsewhere the refusal
/// reads as it always did (reporting it there too would be a Linux change for the owner to
/// decide). The check is `util::git::refuses`, which the project summaries, forge pollers
/// and deploys share.
pub fn unsafe_repository(out: &GitOutput) -> Option<ApiError> {
    let text = out.stderr.trim();
    crate::util::git::refuses(text).then(|| crate::util::git::refused_error(text))
}

/// Files listed after "Your local changes to the following files would be overwritten by …:".
pub fn overwritten_files(text: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    lines.by_ref().find(|l| l.contains("Your local changes to the following files would be overwritten"))?;
    let files: Vec<String> = lines
        .take_while(|l| l.starts_with('\t') || l.starts_with("        "))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Some(files)
}

/// A repository-relative path as a pathspec that matches exactly that path (no
/// globbing, no other magic), for argv after `--`.
pub fn literal(repo_path: &str) -> String {
    format!(":(literal){repo_path}")
}

/// Paths as `--pathspec-from-file=- --pathspec-file-nul` input (no ARG_MAX
/// surprises), each one a [`literal`] pathspec.
pub fn pathspec_stdin<S: AsRef<str>>(paths: &[S]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in paths {
        v.extend_from_slice(literal(p.as_ref()).as_bytes());
        v.push(0);
    }
    v
}

/// Split `-z` output into records (trailing empty record dropped).
pub fn split_z(bytes: &[u8]) -> Vec<String> {
    let mut v: Vec<String> = bytes.split(|b| *b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    if v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    v
}

/// Reject revision arguments that git could read as options, or that are not
/// plausible revision expressions.
pub fn check_rev(rev: &str) -> Result<&str, ApiError> {
    let r = rev.trim();
    if r.is_empty() {
        return Err(ApiError::bad_request("empty revision"));
    }
    if r.starts_with('-') || r.len() > 512 || r.chars().any(|c| c.is_control() || c == ' ') {
        return Err(ApiError::bad_request(format!("invalid revision {r:?}")));
    }
    Ok(r)
}

/// Validate a new branch / tag name with git itself.
pub async fn check_ref_name(cwd: &Path, name: &str, kind: &str) -> Result<(), ApiError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(ApiError::bad_request(format!("invalid {kind} name {name:?}")));
    }
    let full = match kind {
        "tag" => format!("refs/tags/{name}"),
        _ => format!("refs/heads/{name}"),
    };
    let out = Git::read(cwd).args(["check-ref-format", &full]).run().await?;
    if out.ok() { Ok(()) } else { Err(ApiError::bad_request(format!("{name:?} is not a valid {kind} name"))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_git_messages() {
        let s = "error: pathspec 'x' did not match\nhint: try this\n\nfatal: bad";
        assert_eq!(clean_message(s), "pathspec 'x' did not match\nbad");
    }

    #[test]
    fn finds_overwritten_files() {
        let s = "error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\n\tdir/b.txt\nPlease commit your changes or stash them before you switch branches.\nAborting\n";
        assert_eq!(overwritten_files(s), Some(vec!["a.txt".to_string(), "dir/b.txt".to_string()]));
        let out = GitOutput { code: Some(1), stdout: vec![], stderr: s.into(), truncated: false };
        let e = git_error(&out);
        assert_eq!(e.code, "dirty_tree");
        assert!(e.message.contains("2 files"));
    }

    #[test]
    fn rejects_option_like_revisions() {
        assert!(check_rev("--output=/tmp/x").is_err());
        assert!(check_rev("a b").is_err());
        assert!(check_rev("main~2").is_ok());
        assert!(check_rev("origin/feature/x").is_ok());
    }

    #[test]
    fn user_paths_become_literal_pathspecs() {
        assert_eq!(literal("app/[id]/*.tsx"), ":(literal)app/[id]/*.tsx");
        assert_eq!(pathspec_stdin(&["a b", "*.rs"]), b":(literal)a b\0:(literal)*.rs\0".to_vec());
        // The environment of whatever started Workbench must not change pathspec
        // semantics (GIT_LITERAL_PATHSPECS breaks `stash -u` / `--keep-index`).
        let cmd = Git::read(Path::new("/")).command();
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        for k in ["GIT_LITERAL_PATHSPECS", "GIT_GLOB_PATHSPECS", "GIT_NOGLOB_PATHSPECS", "GIT_ICASE_PATHSPECS"] {
            assert!(envs.iter().any(|(n, v)| *n == k && v.is_none()), "{k} must be removed");
        }
    }

    #[test]
    fn config_goes_before_the_subcommand() {
        let cmd = Git::write(Path::new("/")).args(["fetch", "origin"]).config("credential.https://gitlab.com.helper", "").command();
        let args: Vec<String> = cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let i = args.iter().position(|a| a == "fetch").unwrap();
        assert_eq!(args[i - 2..], ["-c", "credential.https://gitlab.com.helper=", "fetch", "origin"]);
    }

    #[test]
    fn splits_nul_records() {
        assert_eq!(split_z(b"a\0b\0"), vec!["a", "b"]);
        assert_eq!(split_z(b""), Vec::<String>::new());
    }
}
