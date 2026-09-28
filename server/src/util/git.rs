//! Minimal read-only git queries shared by several slices (project summaries,
//! status bar, deploy gates). The full git feature lives in `crate::git`.

use std::path::Path;
use std::time::Duration;

use super::proc;

/// Current branch name, or `None` when detached / not a repository.
pub async fn current_branch(root: &Path) -> Option<String> {
    let out = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await?;
    let b = out.trim();
    (!b.is_empty()).then(|| b.to_string())
}

/// Full sha of HEAD.
pub async fn head_sha(root: &Path) -> Option<String> {
    let out = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await?;
    let s = out.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// URL of a remote (`origin` by default).
pub async fn remote_url(root: &Path, remote: &str) -> Option<String> {
    let out = git(root, &["remote", "get-url", remote]).await?;
    let s = out.trim();
    (!s.is_empty()).then(|| s.to_string())
}

async fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut all = vec!["-c", "core.quotepath=false"];
    all.extend_from_slice(args);
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(&all).current_dir(root).env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0");
    let out = proc::run_cmd(cmd, Duration::from_secs(10)).await.ok()?;
    out.ok().then_some(out.stdout)
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
    use super::parse_remote;

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
}
