//! GIT_ASKPASS: git asks `<helper> "Username for 'https://gitlab.com': "`; we answer
//! only for the configured GitLab host over https (user `oauth2`, password = the
//! token), and refuse everything else (other hosts, ssh passphrases, prompts whose
//! host is ambiguous) with a non-zero exit so git fails fast instead of hanging.
//!
//! Git runs GIT_ASKPASS without a shell and with the prompt as the only argument.
//! `util::os::helper::askpass_env` points it at `workbench askpass "<prompt>"` (the CLI
//! subcommand handled by `cli_askpass`): through a tiny wrapper script on Unix; on
//! Windows at the executable itself, with `WORKBENCH_HELPER=askpass` (ssh's
//! `SSH_ASKPASS` too, so a passphrase or host-key prompt fails instead of waiting).
//!
//! Git asks its credential helpers before askpass and hands them every credential that
//! worked (`store`). Remote ops therefore empty the helper list for the host askpass
//! answers for ([`reset_helpers_key`], on git's command line), so a Workbench-managed
//! token never lands in Git Credential Manager, `~/.git-credentials` or a keychain, and a
//! token stored there never answers in its place; other hosts keep the user's helpers.

use std::path::Path;

use crate::config::{GlobalConfig, Paths, ProjectFile, SecretRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Username,
    Password,
}

/// A plain `host[:port]`: a DNS name or IPv4 address (ASCII letters, digits, `-`, `.`, `_`)
/// or a bracketed IPv6 address, and an optional numeric port. Lowercased; `None` for
/// anything else (percent-escapes, `@`, `/`, quotes, spaces, `=`, an empty port).
fn plain_host_port(s: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (addr, after) = rest.split_once(']')?;
        if addr.is_empty() || !addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') {
            return None;
        }
        let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?) };
        (format!("[{addr}]"), port)
    } else {
        let (host, port) = match s.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        };
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')) {
            return None;
        }
        (host.to_string(), port)
    };
    let port = match port {
        None => None,
        Some(p) if !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => Some(p.parse().ok()?),
        Some(_) => return None,
    };
    Some((host.to_ascii_lowercase(), port))
}

/// The configured GitLab host (`gitlab.com`, `https://gitlab.com/`, `git.corp:8443`…)
/// as a plain `host[:port]`: scheme, user info, path and a trailing slash dropped.
fn config_host(h: &str) -> Option<(String, Option<u16>)> {
    let h = h.trim();
    let h = h.split_once("://").map(|(_, r)| r).unwrap_or(h);
    let h = h.split('/').next().unwrap_or(h);
    let h = h.rsplit_once('@').map(|(_, r)| r).unwrap_or(h);
    plain_host_port(h)
}

/// Same host, and the same port with https's 443 as the default.
fn same_host(a: &(String, Option<u16>), b: &(String, Option<u16>)) -> bool {
    a.0 == b.0 && a.1.unwrap_or(443) == b.1.unwrap_or(443)
}

/// Which answer (if any) `prompt` asks for, given the configured GitLab host.
///
/// Git asks `<Username|Password> for '<protocol>://[<user>@]<host>[/<path>]': ` (the path
/// only with `credential.useHttpPath`). Git with the CVE-2024-50349 fix (2.48.1, 2.47.2,
/// 2.40.4 and later maintenance releases) percent-encodes the user name and path; older
/// git, or `credential.sanitizePrompt=false`, prints them decoded, so a user name may hold
/// `/`, `:`, `@` or `'`: `https://gitlab.com%2F@evil.example/` asks
/// `Password for 'https://gitlab.com/@evil.example': `. A host git connects to holds none of
/// `/` and `@` (curl refuses such a host name, `%2F` and `%40` decoded or not), hence:
/// - an `@` after the first `/` is a user name with `/` or a path with `@`, which read
///   alike: refused;
/// - otherwise the host is what follows the last `@` before that `/` (a user name may hold
///   `@` and `:`), and it must be a plain `host[:port]` ([`plain_host_port`]);
/// - a Username prompt never names a user, so one that does is refused.
///
/// When in doubt this answers nothing: git then fails the op instead of sending the token.
pub fn match_prompt(prompt: &str, configured_host: &str) -> Option<Answer> {
    let p = prompt.trim();
    let (kind, rest) = if let Some(r) = p.strip_prefix("Username for '") {
        (Answer::Username, r)
    } else if let Some(r) = p.strip_prefix("Password for '") {
        (Answer::Password, r)
    } else {
        return None;
    };
    let url = rest.strip_suffix("':")?;
    // Never send the token over plain http.
    let rest = url.strip_prefix("https://")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if path.contains('@') {
        return None;
    }
    let host = match authority.rsplit_once('@') {
        Some(_) if kind == Answer::Username => return None,
        Some((_user, host)) => host,
        None => authority,
    };
    let host = plain_host_port(host)?;
    let want = config_host(configured_host)?;
    same_host(&host, &want).then_some(kind)
}

/// GitLab host and the secret reference for its token: `project`'s `[repo.gitlab]` (its
/// root and id) or the global `[gitlab]`.
fn gitlab_credentials(paths: &Paths, cfg: &GlobalConfig, project: Option<(&Path, &str)>) -> Option<(String, SecretRef)> {
    if let Some((root, id)) = project {
        // Same trust rules as the server: `.workbench.toml` cannot define secrets or
        // hand config.toml's secrets to a host of its choosing.
        let site = cfg.atlassian.as_ref().map(|a| a.site.as_str());
        let layered = crate::config::project::load_layers(ProjectFile::default(), root, &paths.project_overlay(id), site);
        if let Some(g) = layered.config.repo.as_ref().and_then(|r| r.gitlab.as_ref()).filter(|g| !g.token.is_empty()) {
            if let Some(r) = layered.secret_ref(&g.token, &cfg.secrets) {
                return Some((g.host.clone(), r));
            }
        }
    }
    let g = cfg.gitlab.as_ref()?;
    Some((g.host.clone(), cfg.secrets.get(&g.token)?.clone()))
}

/// The config key that, set to an empty value on git's command line (read after every
/// config file), empties git's credential helper list for the host askpass answers for in
/// `project` (`WORKBENCH_PROJECT_ROOT`/`_ID` of the op): `credential.https://<host>.helper`.
/// Git then neither asks a helper (Git Credential Manager, `store`, `cache`, a keychain)
/// for that host nor hands it Workbench's token to store; URLs of other hosts, and plain
/// http, keep the user's helpers. `None` when askpass answers nothing (no GitLab
/// credentials, or a host that is not a plain `host[:port]`).
pub fn reset_helpers_key(paths: &Paths, cfg: &GlobalConfig, project: Option<(&Path, &str)>) -> Option<String> {
    let (host, _) = gitlab_credentials(paths, cfg, project)?;
    let (host, port) = config_host(&host)?;
    let port = port.filter(|p| *p != 443).map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("credential.https://{host}{port}.helper"))
}

/// `workbench askpass "<prompt>"`. Prints the answer on stdout, or fails.
pub fn cli_askpass(prompt: &str) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    let cfg = GlobalConfig::load_or_init(&paths)?;
    let root = std::env::var_os("WORKBENCH_PROJECT_ROOT").map(std::path::PathBuf::from);
    let id = std::env::var("WORKBENCH_PROJECT_ID").ok();
    let project = root.as_deref().zip(id.as_deref());
    let Some((host, secret_ref)) = gitlab_credentials(&paths, &cfg, project) else {
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
        // What ssh asks through SSH_ASKPASS (Windows) is refused too.
        let host_key = "The authenticity of host 'gitlab.com (1.2.3.4)' can't be established.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert_eq!(match_prompt(host_key, "gitlab.com"), None);
        assert_eq!(match_prompt("(git@gitlab.com) Password: ", "gitlab.com"), None);
        assert_eq!(match_prompt("Enter passphrase for key 'C:\\Users\\u/.ssh/id_ed25519': ", "gitlab.com"), None);
        // An empty user name (`https://@gitlab.com/`) asks for the password alone.
        assert_eq!(match_prompt("Password for 'https://gitlab.com': ", "gitlab.com"), Some(Answer::Password));
        // With `credential.useHttpPath` the prompt names the path.
        assert_eq!(match_prompt("Password for 'https://oauth2@gitlab.com/group/repo.git': ", "gitlab.com"), Some(Answer::Password));
        assert_eq!(match_prompt("Username for 'https://gitlab.com/group/repo.git': ", "gitlab.com"), Some(Answer::Username));
    }

    #[test]
    fn matches_ports() {
        assert_eq!(match_prompt("Username for 'https://git.corp:8443': ", "git.corp:8443"), Some(Answer::Username));
        assert_eq!(match_prompt("Username for 'https://git.corp:8443': ", "git.corp"), None);
        assert_eq!(match_prompt("Username for 'https://git.corp:443': ", "git.corp"), Some(Answer::Username));
        assert_eq!(match_prompt("Username for 'https://git.corp': ", "git.corp:9000"), None);
        assert_eq!(match_prompt("Username for 'https://git.corp': ", "https://git.corp:443/"), Some(Answer::Username));
        assert_eq!(match_prompt("Password for 'https://oauth2@[::1]:8443': ", "[::1]:8443"), Some(Answer::Password));
        assert_eq!(match_prompt("Password for 'https://oauth2@[::1]': ", "[::1]:8443"), None);
        assert_eq!(match_prompt("Username for 'https://git.corp:': ", "git.corp"), None);
        assert_eq!(match_prompt("Username for 'https://git.corp:99999': ", "git.corp"), None);
    }

    /// Prompts for URLs whose user name (or host, or path) holds `/`, `@`, `:`, `'` or
    /// spaces, as git prints them decoded (before CVE-2024-50349 was fixed, or with
    /// `credential.sanitizePrompt=false`) and percent-encoded (after).
    #[test]
    fn a_user_name_never_picks_the_host() {
        let host = "gitlab.com";
        let refused = [
            // https://gitlab.com%2F@evil.example/r.git: the token would go to evil.example.
            "Password for 'https://gitlab.com/@evil.example': ",
            "Password for 'https://gitlab.com%2F@evil.example': ",
            "Password for 'https://gitlab.com/@evil.example/r.git': ",
            "Password for 'https://gitlab.com:443/@evil.example': ",
            "Password for 'https://gitlab.com/x@evil.example': ",
            "Password for 'https://x@gitlab.com/@evil.example': ",
            "Password for 'https://x/@gitlab.com@evil.example': ",
            "Password for 'https://x%2F%40gitlab.com@evil.example': ",
            "Password for 'https://gitlab.com@evil.example': ",
            "Password for 'https://gitlab.com': x@evil.example': ",
            // A host git shows decoded (`%2F`, `%40` in the URL's host), which curl refuses.
            "Username for 'https://gitlab.com/@evil.example': ",
            "Password for 'https://oauth2@gitlab.com/@evil.example': ",
            "Username for 'https://gitlab.com%2F%40evil.example': ",
            "Password for 'https://oauth2@evil%40gitlab.com': ",
            "Username for 'https://oauth2@gitlab.com': ",
            // A path with `@` reads like a user name with `/`.
            "Password for 'https://oauth2@gitlab.com/g/@r.git': ",
            // Odd host characters.
            "Username for 'https://gitlab.com%2e': ",
            "Username for 'https://gitlab.com x': ",
            "Username for 'https://gitlab.com'x': ",
            "Username for 'https://': ",
            "Username for 'https://gitlab.com'",
        ];
        for p in refused {
            assert_eq!(match_prompt(p, host), None, "{p}");
        }
        let answered = [
            // User names with `@`, `:`, `'` or a space, decoded and encoded.
            ("Password for 'https://a@b@gitlab.com': ", Answer::Password),
            ("Password for 'https://a%40b@gitlab.com': ", Answer::Password),
            ("Password for 'https://a:b@gitlab.com': ", Answer::Password),
            ("Password for 'https://a%3Ab@gitlab.com/r.git': ", Answer::Password),
            ("Password for 'https://x'y@gitlab.com': ", Answer::Password),
            ("Password for 'https://a b@gitlab.com': ", Answer::Password),
            ("Password for 'https://oauth2@gitlab.com/g/%40r.git': ", Answer::Password),
            ("Username for 'https://GitLab.COM:443': ", Answer::Username),
        ];
        for (p, kind) in answered {
            assert_eq!(match_prompt(p, host), Some(kind), "{p}");
        }
        // Whatever the user name says, another host is another host.
        assert_eq!(match_prompt("Password for 'https://gitlab.com/@evil.example': ", "evil.example"), None);
        assert_eq!(match_prompt("Password for 'https://gitlab.com@evil.example': ", "evil.example"), Some(Answer::Password));
    }

    #[test]
    fn configured_hosts_are_plain() {
        assert_eq!(config_host("https://GitLab.com/"), Some(("gitlab.com".into(), None)));
        assert_eq!(config_host("gitlab.com"), Some(("gitlab.com".into(), None)));
        assert_eq!(config_host("https://corp.example/gitlab"), Some(("corp.example".into(), None)));
        assert_eq!(config_host("git.corp:8443"), Some(("git.corp".into(), Some(8443))));
        assert_eq!(config_host("https://[::1]:8443"), Some(("[::1]".into(), Some(8443))));
        for bad in ["", "https://", "gitlab.com=x", "x.helper=!sh", "git lab", "gitlab.com:x", "gitlab.com'", "[::1", "gitlab%2ecom"] {
            assert_eq!(config_host(bad), None, "{bad}");
        }
    }

    fn cfg_with_gitlab(host: &str) -> GlobalConfig {
        let mut cfg = GlobalConfig::default();
        cfg.gitlab = Some(crate::config::global::GitlabConfig { host: host.into(), token: "gitlab".into() });
        cfg.secrets.insert("gitlab".into(), SecretRef::File("~/.gitlab_token".into()));
        cfg
    }

    #[test]
    fn helpers_are_reset_for_the_host_askpass_answers_for() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths { config_dir: d.path().join("config"), data_dir: d.path().join("data") };
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let key = |cfg: &GlobalConfig, project: Option<(&Path, &str)>| reset_helpers_key(&paths, cfg, project);

        // No GitLab credentials: askpass answers nothing, the user's helpers stay.
        assert_eq!(key(&GlobalConfig::default(), None), None);
        let mut no_secret = cfg_with_gitlab("gitlab.com");
        no_secret.secrets.clear();
        assert_eq!(key(&no_secret, None), None);

        let cfg = cfg_with_gitlab("https://GitLab.com/");
        assert_eq!(key(&cfg, None).as_deref(), Some("credential.https://gitlab.com.helper"));
        assert_eq!(key(&cfg, Some((&root, "p1"))).as_deref(), Some("credential.https://gitlab.com.helper"));
        assert_eq!(key(&cfg_with_gitlab("git.corp:443"), None).as_deref(), Some("credential.https://git.corp.helper"));
        assert_eq!(key(&cfg_with_gitlab("git.corp:8443"), None).as_deref(), Some("credential.https://git.corp:8443.helper"));
        assert_eq!(key(&cfg_with_gitlab("[::1]:8443"), None).as_deref(), Some("credential.https://[::1]:8443.helper"));
        // A host that is not plain never reaches git's command line.
        assert_eq!(key(&cfg_with_gitlab("x.helper=!sh -c id #"), None), None);

        // A project's own GitLab token (machine overlay) names its host.
        let overlay = paths.project_overlay("p1");
        std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        std::fs::write(&overlay, "[repo.gitlab]\nhost = \"git.team.example\"\npath = \"g/r\"\ntoken = \"team\"\n\n[secrets]\nteam = { file = \"~/.team_token\" }\n").unwrap();
        assert_eq!(key(&cfg, Some((&root, "p1"))).as_deref(), Some("credential.https://git.team.example.helper"));
        // A repository's `.workbench.toml` cannot pick the host with config.toml's token.
        std::fs::write(root.join(".workbench.toml"), "[repo.gitlab]\nhost = \"evil.example\"\npath = \"g/r\"\ntoken = \"gitlab\"\n").unwrap();
        assert_eq!(key(&cfg, Some((&root, "p2"))).as_deref(), Some("credential.https://gitlab.com.helper"));
    }
}
