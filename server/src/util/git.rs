//! Minimal read-only git queries shared by several slices (project summaries,
//! status bar, deploy gates). The full git feature lives in `crate::git`.
//!
//! Each query comes in three forms: the plain one answers `None` for any failure (callers
//! that only want an answer); `try_…` says why git gave none ([`Failure`]: git missing, a
//! timeout, a repository git refuses, git's message); `…_logged` (background work: forge
//! pollers and CI summaries) logs such a failure once per folder instead of dropping it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use axum::http::StatusCode;
use tokio::process::Command;

use super::proc;
use crate::error::ApiError;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Why git gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// There is no `git` on `PATH`.
    NotInstalled,
    /// Git did not answer within the timeout.
    TimedOut,
    /// Git refuses the repository because another user owns it ([`refuses`]): git's
    /// message verbatim, which names the command that trusts the folder.
    Refused(String),
    /// Any other failure, in git's words (not a repository, a damaged one…).
    Failed(String),
}

impl From<Failure> for ApiError {
    /// As the git slice answers the same failures (`git::cmd`): `not_configured`, `timeout`,
    /// `403 unsafe_repository`, else `422 git_error` with git's message.
    fn from(f: Failure) -> Self {
        match f {
            Failure::NotInstalled => ApiError::not_configured(format!(
                "git is not installed (no `git` on PATH){}",
                crate::util::os::exe::INSTALLED_SINCE
            )),
            Failure::TimedOut => {
                ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", format!("git timed out after {}s", TIMEOUT.as_secs()))
            }
            Failure::Refused(msg) => refused_error(msg),
            Failure::Failed(msg) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "git_error", msg),
        }
    }
}

/// Current branch name, or `None` when detached / not a repository.
pub async fn current_branch(root: &Path) -> Option<String> {
    try_current_branch(root).await.ok().flatten()
}

/// The current branch; `Ok(None)` when HEAD is detached.
pub async fn try_current_branch(root: &Path) -> Result<Option<String>, Failure> {
    query(command(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])).await
}

/// [`try_current_branch`] with a failure logged once per folder (see `log_once`).
pub async fn current_branch_logged(root: &Path) -> Option<String> {
    logged(root, try_current_branch(root).await)
}

/// Full sha of HEAD.
pub async fn head_sha(root: &Path) -> Option<String> {
    try_head_sha(root).await.ok().flatten()
}

/// Full sha of HEAD; `Ok(None)` when git ran and HEAD resolves to no commit (a repository
/// without commits yet).
pub async fn try_head_sha(root: &Path) -> Result<Option<String>, Failure> {
    query(command(root, &["rev-parse", "--verify", "--quiet", "HEAD"])).await
}

/// [`try_head_sha`] with a failure logged once per folder (see `log_once`).
pub async fn head_sha_logged(root: &Path) -> Option<String> {
    logged(root, try_head_sha(root).await)
}

/// Full sha of the commit `rev` names; `Ok(None)` when it names none. A `rev` that git
/// could read as an option names none.
pub async fn try_commit_sha(root: &Path, rev: &str) -> Result<Option<String>, Failure> {
    if rev.is_empty() || rev.starts_with('-') {
        return Ok(None);
    }
    let spec = format!("{rev}^{{commit}}");
    query(command(root, &["rev-parse", "--verify", "--quiet", &spec])).await
}

/// URL of a remote (`origin` by default).
pub async fn remote_url(root: &Path, remote: &str) -> Option<String> {
    query(command(root, &["remote", "get-url", remote])).await.ok().flatten()
}

/// `git args…` in `root`. `LC_ALL=C`, as the git slice runs it: [`refuses`] reads git's
/// English message.
fn command(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.quotepath=false"])
        .args(args)
        .current_dir(root)
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

async fn query(cmd: Command) -> Result<Option<String>, Failure> {
    answer(proc::try_run_cmd(cmd, TIMEOUT).await)
}

/// What a query's run says: stdout trimmed, `None` when empty or when git answered "no"
/// (exit 1 and nothing on stderr: `--quiet` for a detached HEAD, a rev that names nothing),
/// else why git gave no answer.
fn answer(run: Result<proc::Output, proc::RunError>) -> Result<Option<String>, Failure> {
    let out = match run {
        Ok(out) => out,
        Err(proc::RunError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => return Err(Failure::NotInstalled),
        Err(proc::RunError::TimedOut) => return Err(Failure::TimedOut),
        Err(proc::RunError::Spawn(e) | proc::RunError::Wait(e)) => return Err(Failure::Failed(format!("cannot run git: {e}"))),
    };
    if out.ok() {
        let s = out.stdout.trim();
        return Ok((!s.is_empty()).then(|| s.to_string()));
    }
    let stderr = out.stderr.trim();
    if out.code == Some(1) && stderr.is_empty() {
        return Ok(None);
    }
    if refuses(stderr) {
        return Err(Failure::Refused(stderr.to_string()));
    }
    let msg = out.message();
    if !msg.is_empty() {
        return Err(Failure::Failed(msg));
    }
    Err(Failure::Failed(match out.code {
        Some(c) => format!("git exited with code {c}"),
        None => "git was killed".into(),
    }))
}

/// Whether git's `stderr` is git refusing a repository that another user owns (the
/// `safe.directory` check: "detected dubious ownership", "unsafe repository" in older
/// gits), where such folders are common (`os::fs::FOREIGN_OWNERS`, Windows: folders an
/// administrator created, drives without owners). Elsewhere the refusal reads as git's
/// other failures do: reporting it there too would be a Linux change for the owner to
/// decide. The git slice's `unsafe_repository` answers use this check too.
pub fn refuses(stderr: &str) -> bool {
    crate::util::os::fs::FOREIGN_OWNERS && is_refusal(stderr)
}

fn is_refusal(stderr: &str) -> bool {
    stderr.contains("detected dubious ownership") || stderr.contains("fatal: unsafe repository")
}

/// `403 unsafe_repository` with git's refusal verbatim (it names the owners and the command
/// that trusts the folder).
pub fn refused_error(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "unsafe_repository", message)
}

/// The `git config --global --add safe.directory …` line of git's refusal, which trusts
/// the folder (git quotes the path for a shell when it needs quoting).
pub fn trust_command(message: &str) -> Option<&str> {
    message.lines().map(str::trim).find(|l| l.starts_with("git config --global --add safe.directory "))
}

/// A project warning for a repository git refuses: the folder and the command that
/// trusts it.
pub fn refused_warning(root: &Path, message: &str) -> String {
    let fix = match trust_command(message) {
        Some(c) => format!("If you trust it, run: {c}"),
        None => format!("Git says: {}", message.lines().next().unwrap_or_default().trim()),
    };
    format!(
        "Git refuses {} because another user owns the folder (safe.directory), so its branch, changes and history are not shown. {fix}",
        crate::config::contract_tilde(root)
    )
}

/// Folders whose failure has been logged, until git answers there again.
static LOGGED: LazyLock<parking_lot::Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

/// The answer, with a failure logged once per folder ([`log_once`]).
fn logged(root: &Path, r: Result<Option<String>, Failure>) -> Option<String> {
    match r {
        Ok(v) => {
            LOGGED.lock().remove(root);
            v
        }
        Err(f) => {
            log_once(root, &f);
            None
        }
    }
}

/// Log a failure that needs the owner (git refusing the repository, git missing, a timeout)
/// unless one was logged for `root` since git last answered there, so a poller does not log
/// it every round; whether it logged. A folder that is no repository, or a damaged one, is
/// left to the git tool windows.
fn log_once(root: &Path, f: &Failure) -> bool {
    let why = match f {
        Failure::Refused(msg) => format!(
            "git refuses the repository because another user owns it (safe.directory); to trust it, run: {}",
            trust_command(msg).unwrap_or_else(|| msg.lines().next().unwrap_or_default())
        ),
        Failure::NotInstalled => "git is not installed (no `git` on PATH)".into(),
        Failure::TimedOut => format!("git timed out after {}s", TIMEOUT.as_secs()),
        Failure::Failed(_) => return false,
    };
    if !LOGGED.lock().insert(root.to_path_buf()) {
        return false;
    }
    tracing::warn!("CI status leaves out the branch of {}: {why}", crate::config::contract_tilde(root));
    true
}

/// Parse `https://host/ns/proj(.git)`, `git@host:ns/proj.git`, `ssh://git@host[:port]/ns/proj`
/// into `(host, "ns/proj")`.
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:https?://(?:[^@/]+@)?|ssh://(?:[^@/]+@)?|[^@/]+@)([^/:]+)(?::\d+)?[:/](.+?)(?:\.git)?/?$").unwrap()
    });
    let c = RE.captures(url.trim())?;
    Some((c[1].to_string(), c[2].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_remote_forms() {
        let want = Some(("gitlab.com".to_string(), "acme/shop".to_string()));
        assert_eq!(parse_remote("https://gitlab.com/acme/shop.git"), want);
        assert_eq!(parse_remote("https://oauth2@gitlab.com/acme/shop"), want);
        assert_eq!(parse_remote("git@gitlab.com:acme/shop.git"), want);
        assert_eq!(parse_remote("ssh://git@gitlab.com:2222/acme/shop.git"), want);
        assert_eq!(
            parse_remote("https://gitlab.com/group/sub/proj.git"),
            Some(("gitlab.com".into(), "group/sub/proj".into()))
        );
    }

    /// Git's refusal of a folder another user owns, as git 2.36+ and Git for Windows word it.
    const REFUSAL: &str = "fatal: detected dubious ownership in repository at 'C:/work/shop'\n\
        'C:/work/shop' is owned by:\n\tBUILTIN/Administrators (S-1-5-32-544)\n\
        but the current user is:\n\tPC/me (S-1-5-21-1-2-3-1001)\n\
        To add an exception for this directory, call:\n\n\
        \tgit config --global --add safe.directory C:/work/shop";

    fn out(code: i32, stdout: &str, stderr: &str) -> Result<proc::Output, proc::RunError> {
        Ok(proc::Output { code: Some(code), stdout: stdout.into(), stderr: stderr.into() })
    }

    #[test]
    fn tells_why_git_gave_no_answer() {
        assert_eq!(answer(out(0, "main\n", "")), Ok(Some("main".into())));
        assert_eq!(answer(out(0, "\n", "")), Ok(None));
        // `--quiet`: a detached HEAD, a HEAD without commits.
        assert_eq!(answer(out(1, "", "")), Ok(None));
        let not_repo = "fatal: not a git repository (or any of the parent directories): .git\n";
        assert_eq!(answer(out(128, "", not_repo)), Err(Failure::Failed(not_repo.trim().into())));
        assert_eq!(answer(out(1, "", "error: short object ID 4b8e is ambiguous")), Err(Failure::Failed("error: short object ID 4b8e is ambiguous".into())));
        assert_eq!(answer(out(129, "", "")), Err(Failure::Failed("git exited with code 129".into())));
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(answer(Err(proc::RunError::Spawn(missing))), Err(Failure::NotInstalled));
        assert_eq!(answer(Err(proc::RunError::TimedOut)), Err(Failure::TimedOut));
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(answer(Err(proc::RunError::Spawn(denied))), Err(Failure::Failed(m)) if m.starts_with("cannot run git")));
    }

    /// The same check as the git slice's `unsafe_repository` answers: on Windows only
    /// (`os::fs::FOREIGN_OWNERS`); elsewhere git's refusal is one of its other failures.
    #[test]
    fn a_refusal_is_told_apart_where_such_folders_are_common() {
        assert!(is_refusal(REFUSAL));
        assert!(is_refusal("fatal: unsafe repository ('/srv/x' is owned by someone else)"));
        assert!(!is_refusal("fatal: not a git repository (or any of the parent directories): .git"));
        assert_eq!(refuses(REFUSAL), crate::util::os::fs::FOREIGN_OWNERS);
        let got = answer(out(128, "", &format!("{REFUSAL}\n")));
        if crate::util::os::fs::FOREIGN_OWNERS {
            assert_eq!(got, Err(Failure::Refused(REFUSAL.into())));
            let e = ApiError::from(got.unwrap_err());
            assert_eq!((e.status.as_u16(), e.code, e.message.as_str()), (403, "unsafe_repository", REFUSAL));
        } else {
            assert_eq!(got, Err(Failure::Failed(REFUSAL.into())));
            assert_eq!(ApiError::from(got.unwrap_err()).code, "git_error");
        }
    }

    #[test]
    fn a_refusal_names_the_command_that_trusts_the_folder() {
        assert_eq!(trust_command(REFUSAL), Some("git config --global --add safe.directory C:/work/shop"));
        let quoted = "fatal: detected dubious ownership in repository at '/srv/my repo'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory '/srv/my repo'\n";
        assert_eq!(trust_command(quoted), Some("git config --global --add safe.directory '/srv/my repo'"));
        assert_eq!(trust_command("fatal: not a git repository"), None);

        let w = refused_warning(Path::new("/srv/shop"), REFUSAL);
        assert!(w.contains("/srv/shop") && w.contains("safe.directory"), "{w}");
        assert!(w.ends_with("If you trust it, run: git config --global --add safe.directory C:/work/shop"), "{w}");
        assert!(!w.contains('\n'), "one line: {w}");
        let w = refused_warning(Path::new("/srv/shop"), "fatal: unsafe repository ('/srv/shop' is owned by someone else)\n");
        assert!(w.ends_with("Git says: fatal: unsafe repository ('/srv/shop' is owned by someone else)"), "{w}");
    }

    #[test]
    fn failures_answer_as_the_git_slice_does() {
        assert_eq!(ApiError::from(Failure::NotInstalled).code, "not_configured");
        let e = ApiError::from(Failure::TimedOut);
        assert_eq!((e.status.as_u16(), e.code), (504, "timeout"));
        let e = ApiError::from(Failure::Failed("fatal: bad object HEAD".into()));
        assert_eq!((e.status.as_u16(), e.code, e.message.as_str()), (422, "git_error", "fatal: bad object HEAD"));
    }

    #[test]
    fn a_failure_is_logged_once_per_folder_until_git_answers() {
        let root = tempfile::tempdir().unwrap();
        let (p, other) = (root.path().join("a"), root.path().join("b"));
        assert!(!log_once(&p, &Failure::Failed("fatal: not a git repository".into())), "a folder's normal state");
        assert!(log_once(&p, &Failure::Refused(REFUSAL.into())));
        assert!(!log_once(&p, &Failure::Refused(REFUSAL.into())), "not every round");
        assert!(!log_once(&p, &Failure::TimedOut));
        assert!(log_once(&other, &Failure::NotInstalled), "per folder");
        assert_eq!(logged(&p, Err(Failure::TimedOut)), None);
        assert_eq!(logged(&p, Ok(Some("main".into()))), Some("main".into()));
        assert!(log_once(&p, &Failure::TimedOut), "logged again once git answered in between");
    }

    fn init(dir: &Path) {
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    }

    /// Real git in scratch folders: answers, "no" answers and failures stay apart.
    #[tokio::test]
    async fn queries_say_why_git_gave_no_answer() {
        let d = tempfile::tempdir().unwrap();
        let (repo, empty, plain) = (d.path().join("repo"), d.path().join("empty"), d.path().join("plain"));
        for p in [&repo, &empty, &plain] {
            std::fs::create_dir(p).unwrap();
        }
        init(&repo);
        assert_eq!(try_current_branch(&repo).await, Ok(Some("main".into())));
        let head = try_head_sha(&repo).await.unwrap().unwrap();
        assert_eq!(head.len(), 40);
        assert_eq!(try_commit_sha(&repo, &head[..8]).await, Ok(Some(head.clone())));
        assert_eq!(try_commit_sha(&repo, "deadbeef").await, Ok(None));
        assert_eq!(try_commit_sha(&repo, "--all").await, Ok(None));

        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&repo).status().unwrap().success());
        git(&["checkout", "-q", "--detach"]);
        assert_eq!(try_current_branch(&repo).await, Ok(None), "detached");
        assert_eq!(current_branch(&repo).await, None);

        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&empty).status().unwrap().success());
        assert_eq!(try_head_sha(&empty).await, Ok(None), "no commits yet");

        // A folder that is no repository: git's words, not "no commits". (The scratch folder
        // sits outside any repository; GIT_CEILING_DIRECTORIES keeps it so.)
        let mut c = command(&plain, &["rev-parse", "--verify", "--quiet", "HEAD"]);
        c.env("GIT_CEILING_DIRECTORIES", d.path());
        match query(c).await {
            Err(Failure::Failed(m)) => assert!(m.contains("not a git repository"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(head_sha(&plain).await, None);

        let mut missing = Command::new("workbench-test-no-such-program");
        missing.current_dir(&plain);
        assert_eq!(query(missing).await, Err(Failure::NotInstalled));
    }

    /// Git's own switch for testing its ownership check, and an empty global config, so no
    /// `safe.directory` of this computer lets the folder pass.
    #[tokio::test]
    async fn a_repository_git_refuses_is_reported_where_such_folders_are_common() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        let cfg = tempfile::tempdir().unwrap();
        let empty = cfg.path().join("gitconfig");
        std::fs::write(&empty, "").unwrap();
        let mut c = command(d.path(), &["symbolic-ref", "--quiet", "--short", "HEAD"]);
        c.env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1").env("GIT_CONFIG_GLOBAL", &empty).env("GIT_CONFIG_NOSYSTEM", "1");
        match query(c).await {
            Err(Failure::Refused(m)) => {
                assert!(crate::util::os::fs::FOREIGN_OWNERS);
                assert!(trust_command(&m).is_some(), "{m}");
            }
            Err(Failure::Failed(m)) => {
                assert!(!crate::util::os::fs::FOREIGN_OWNERS, "{m}");
                assert!(m.contains("dubious ownership"), "git's words: {m}");
            }
            other => panic!("{other:?}"),
        }
    }
}
