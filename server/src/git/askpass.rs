//! GIT_ASKPASS: git asks `<helper> "Username for 'https://gitlab.com': "`; we answer
//! only for the configured GitLab host over https (user `oauth2`, password = the
//! token), and refuse everything else (other hosts, ssh passphrases) with a
//! non-zero exit so git fails fast instead of hanging.
//!
//! Git runs GIT_ASKPASS without a shell and with the prompt as the only argument,
//! so the server points it at a tiny wrapper script that calls
//! `workbench askpass "<prompt>"` (the CLI subcommand handled by `cli_askpass`).

use std::path::{Path, PathBuf};

use crate::config::{GlobalConfig, Paths, ProjectFile, SecretRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Username,
    Password,
}

/// `host[:port]` without scheme, userinfo, path or trailing slash, lowercased.
fn normalize_host(h: &str) -> String {
    let h = h.trim();
    let h = h.split_once("://").map(|(_, r)| r).unwrap_or(h);
    let h = h.split('/').next().unwrap_or(h);
    let h = h.rsplit_once('@').map(|(_, r)| r).unwrap_or(h);
    h.to_ascii_lowercase()
}

fn split_port(h: &str) -> (&str, Option<&str>) {
    match h.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => (host, Some(port)),
        _ => (h, None),
    }
}

/// Which answer (if any) `prompt` asks for, given the configured GitLab host.
pub fn match_prompt(prompt: &str, configured_host: &str) -> Option<Answer> {
    let p = prompt.trim();
    let kind = if p.starts_with("Username for ") {
        Answer::Username
    } else if p.starts_with("Password for ") {
        Answer::Password
    } else {
        return None;
    };
    let start = p.find('\'')?;
    let end = p.rfind('\'')?;
    if end <= start {
        return None;
    }
    let url = &p[start + 1..end];
    // Never send the token over plain http.
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = normalize_host(authority);
    let want = normalize_host(configured_host);
    if want.is_empty() {
        return None;
    }
    let (h, port) = split_port(&host);
    let (wh, wport) = split_port(&want);
    let port_ok = match (port, wport) {
        (_, None) => port.is_none() || port == Some("443"),
        (Some(a), Some(b)) => a == b,
        (None, Some(b)) => b == "443",
    };
    (h == wh && port_ok).then_some(kind)
}

/// GitLab host and the secret reference for its token: the project's
/// `[repo.gitlab]` (when `WORKBENCH_PROJECT_ROOT`/`_ID` are set) or the global `[gitlab]`.
fn gitlab_credentials(paths: &Paths, cfg: &GlobalConfig) -> Option<(String, SecretRef)> {
    let root = std::env::var_os("WORKBENCH_PROJECT_ROOT").map(PathBuf::from);
    let id = std::env::var("WORKBENCH_PROJECT_ID").ok();
    if let (Some(root), Some(id)) = (root, id) {
        // Same trust rules as the server: `.workbench.toml` cannot define secrets or
        // hand config.toml's secrets to a host of its choosing.
        let site = cfg.atlassian.as_ref().map(|a| a.site.as_str());
        let layered = crate::config::project::load_layers(ProjectFile::default(), &root, &paths.project_overlay(&id), site);
        if let Some(g) = layered.config.repo.as_ref().and_then(|r| r.gitlab.as_ref()).filter(|g| !g.token.is_empty()) {
            if let Some(r) = layered.secret_ref(&g.token, &cfg.secrets) {
                return Some((g.host.clone(), r));
            }
        }
    }
    let g = cfg.gitlab.as_ref()?;
    Some((g.host.clone(), cfg.secrets.get(&g.token)?.clone()))
}

/// `workbench askpass "<prompt>"`. Prints the answer on stdout, or fails.
pub fn cli_askpass(prompt: &str) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    let cfg = GlobalConfig::load_or_init(&paths)?;
    let Some((host, secret_ref)) = gitlab_credentials(&paths, &cfg) else {
        anyhow::bail!("workbench askpass: no GitLab credentials configured");
    };
    match match_prompt(prompt, &host) {
        Some(Answer::Username) => {
            println!("oauth2");
            Ok(())
        }
        Some(Answer::Password) => {
            let mut warnings = vec![];
            let v = crate::secrets::resolve(&secret_ref, &mut warnings)?;
            use secrecy::ExposeSecret;
            println!("{}", v.expose_secret());
            Ok(())
        }
        None => anyhow::bail!("workbench askpass: not answering this prompt"),
    }
}

/// Write `<data_dir>/git-askpass` (0700) that runs this binary's askpass helper.
pub fn write_wrapper(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    // A rebuilt binary leaves /proc/self/exe pointing at "… (deleted)".
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    let quoted = format!("'{}'", exe.replace('\'', "'\\''"));
    let script = format!("#!/bin/sh\n# Written by Workbench: answers git credential prompts for the configured GitLab host.\nexec {quoted} askpass \"$1\"\n");
    let path = data_dir.join("git-askpass");
    crate::util::fs::write_atomic(&path, script.as_bytes(), 0o700)?;
    crate::util::fs::set_mode(&path, 0o700);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_only_for_the_configured_https_host() {
        assert_eq!(match_prompt("Username for 'https://gitlab.com': ", "gitlab.com"), Some(Answer::Username));
        assert_eq!(match_prompt("Password for 'https://oauth2@gitlab.com': ", "gitlab.com"), Some(Answer::Password));
        assert_eq!(match_prompt("Password for 'https://oauth2@GitLab.com': ", "https://gitlab.com/"), Some(Answer::Password));
        assert_eq!(match_prompt("Username for 'https://github.com': ", "gitlab.com"), None);
        assert_eq!(match_prompt("Username for 'https://gitlab.com.evil.io': ", "gitlab.com"), None);
        assert_eq!(match_prompt("Username for 'http://gitlab.com': ", "gitlab.com"), None);
        assert_eq!(match_prompt("Enter passphrase for key '/home/u/.ssh/id_ed25519': ", "gitlab.com"), None);
        assert_eq!(match_prompt("Username for 'https://gitlab.com': ", ""), None);
    }

    #[test]
    fn matches_ports() {
        assert_eq!(match_prompt("Username for 'https://git.corp:8443': ", "git.corp:8443"), Some(Answer::Username));
        assert_eq!(match_prompt("Username for 'https://git.corp:8443': ", "git.corp"), None);
        assert_eq!(match_prompt("Username for 'https://git.corp:443': ", "git.corp"), Some(Answer::Username));
        assert_eq!(match_prompt("Username for 'https://git.corp': ", "git.corp:9000"), None);
    }

    #[test]
    fn wrapper_script_quotes_the_binary_path() {
        let d = tempfile::tempdir().unwrap();
        let p = write_wrapper(d.path()).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains(" askpass \"$1\""));
        crate::util::os::perm::assert_mode(&p, 0o700);
    }
}
