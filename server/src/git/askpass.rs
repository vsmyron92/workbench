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

/// A `[[repository]] path` the way an op names its repository (`WORKBENCH_REPO_ID`):
/// `/`-separated, no `./` or trailing `/`.
fn repo_id_of(path: &str) -> String {
    crate::config::project::repository_key(path)
}

/// The GitLab hosts askpass can answer for, each with the secret reference of its token,
/// in the order they are tried: the project's `[repo.gitlab]` and the `[[repository]]`
/// GitLab sections that have a token of their own (the repository an op runs in, `repo`,
/// first, so two repositories on one host with different tokens each get theirs), then
/// the global `[gitlab]`. A prompt is answered by the first whose host it names; the
/// global token therefore only ever goes to the global host.
fn gitlab_credentials(paths: &Paths, cfg: &GlobalConfig, project: Option<(&Path, &str)>, repo: Option<&str>) -> Vec<(String, SecretRef)> {
    let mut out = vec![];
    if let Some((root, id)) = project {
        // Same trust rules as the server: `.workbench.toml` cannot define secrets or
        // hand config.toml's secrets to a host of its choosing.
        let site = cfg.atlassian.as_ref().map(|a| a.site.as_str());
        let layered = crate::config::project::load_layers(ProjectFile::default(), root, &paths.project_overlay(id), site);
        let mut entries: Vec<(String, &crate::config::project::GitLab)> = vec![];
        if let Some(g) = layered.config.repo.as_ref().and_then(|r| r.gitlab.as_ref()) {
            entries.push((crate::projects::ROOT_REPO.to_string(), g));
        }
        for r in &layered.config.repositories {
            if let Some(g) = r.repo.gitlab.as_ref() {
                entries.push((repo_id_of(&r.path), g));
            }
        }
        // Stable: the op's own repository first, the rest in config order.
        entries.sort_by_key(|(id, _)| Some(id.as_str()) != repo);
        for (_, g) in entries.into_iter().filter(|(_, g)| !g.token.is_empty()) {
            if let Some(r) = layered.secret_ref(&g.token, &cfg.secrets) {
                out.push((g.host.clone(), r));
            }
        }
    }
    if let Some(r) = cfg.gitlab.as_ref().and_then(|g| Some((g.host.clone(), cfg.secrets.get(&g.token)?.clone()))) {
        out.push(r);
    }
    out
}

/// The config keys that, set to an empty value on git's command line (read after every
/// config file), empty git's credential helper list for the hosts askpass answers for in
/// `project` (`WORKBENCH_PROJECT_ROOT`/`_ID` of the op): `credential.https://<host>.helper`.
/// Git then neither asks a helper (Git Credential Manager, `store`, `cache`, a keychain)
/// for those hosts nor hands them Workbench's token to store; URLs of other hosts, and plain
/// http, keep the user's helpers. Empty when askpass answers nothing (no GitLab
/// credentials, or hosts that are not a plain `host[:port]`).
pub fn reset_helpers_keys(paths: &Paths, cfg: &GlobalConfig, project: Option<(&Path, &str)>) -> Vec<String> {
    let mut keys: Vec<String> = vec![];
    for (host, _) in gitlab_credentials(paths, cfg, project, None) {
        let Some((host, port)) = config_host(&host) else { continue };
        let port = port.filter(|p| *p != 443).map(|p| format!(":{p}")).unwrap_or_default();
        let key = format!("credential.https://{host}{port}.helper");
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// `workbench askpass "<prompt>"`. Prints the answer on stdout, or fails.
pub fn cli_askpass(prompt: &str) -> anyhow::Result<()> {
    let paths = Paths::from_env()?;
    let cfg = GlobalConfig::load_or_init(&paths)?;
    let root = std::env::var_os("WORKBENCH_PROJECT_ROOT").map(std::path::PathBuf::from);
    let id = std::env::var("WORKBENCH_PROJECT_ID").ok();
    let project = root.as_deref().zip(id.as_deref());
    let repo = std::env::var("WORKBENCH_REPO_ID").ok();
    let credentials = gitlab_credentials(&paths, &cfg, project, repo.as_deref());
    if credentials.is_empty() {
        anyhow::bail!("workbench askpass: no GitLab credentials configured");
    }
    let Some((kind, secret_ref)) = credentials.iter().find_map(|(host, r)| match_prompt(prompt, host).map(|k| (k, r))) else {
        anyhow::bail!("workbench askpass: not answering this prompt");
    };
    match kind {
        Answer::Username => {
            println!("oauth2");
            Ok(())
        }
        Answer::Password => {
            let mut warnings = vec![];
            let v = crate::secrets::resolve(secret_ref, &mut warnings)?;
            use secrecy::ExposeSecret;
            println!("{}", v.expose_secret());
            Ok(())
        }
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
        let key = |cfg: &GlobalConfig, project: Option<(&Path, &str)>| reset_helpers_keys(&paths, cfg, project).into_iter().next();

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

    /// A project with a repository on another GitLab host (and one on the same host with its
    /// own token): askpass answers for each host with the token of the repository the op runs
    /// in, and never with a token the repository's own config chose from config.toml.
    #[test]
    fn every_gitlab_host_of_a_project_is_answered_for() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths { config_dir: d.path().join("config"), data_dir: d.path().join("data") };
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let overlay = paths.project_overlay("p1");
        std::fs::create_dir_all(overlay.parent().unwrap()).unwrap();
        std::fs::write(
            &overlay,
            r#"
            [secrets]
            root_tok = { file = "~/.root_token" }
            api_tok = { file = "~/.api_token" }
            web_tok = { file = "~/.web_token" }
            [repo.gitlab]
            host = "gitlab.com"
            path = "g/shop"
            token = "root_tok"
            [[repository]]
            path = "./services//api/"
            [repository.gitlab]
            host = "git.api.example"
            path = "g/api"
            token = "api_tok"
            [[repository]]
            path = "web"
            [repository.gitlab]
            host = "gitlab.com"
            path = "g/web"
            token = "web_tok"
            [[repository]]
            path = "docs"
            [repository.gitlab]
            host = "gitlab.com"
            path = "g/docs"
            "#,
        )
        .unwrap();
        let cfg = cfg_with_gitlab("gitlab.com");
        let project = Some((root.as_path(), "p1"));
        let file = |n: &str| SecretRef::File(format!("~/.{n}"));
        let global = SecretRef::File("~/.gitlab_token".into());
        let creds = |repo: Option<&str>| gitlab_credentials(&paths, &cfg, project, repo);
        fn hosts(c: &[(String, SecretRef)]) -> Vec<&str> {
            c.iter().map(|(h, _)| h.as_str()).collect()
        }

        // Config order, then the global token (a repository without a token of its own adds nothing).
        let c = creds(None);
        assert_eq!(hosts(&c), ["gitlab.com", "git.api.example", "gitlab.com", "gitlab.com"]);
        assert_eq!(c[3].1, global);
        assert_eq!(c[..3].iter().map(|(_, r)| r.clone()).collect::<Vec<_>>(), [file("root_token"), file("api_token"), file("web_token")]);
        // The repository an op runs in comes first: two repositories on one host, two tokens.
        let c = creds(Some("web"));
        assert_eq!((c[0].0.as_str(), &c[0].1), ("gitlab.com", &SecretRef::File("~/.web_token".into())));
        let c = creds(Some("services/api"));
        assert_eq!((c[0].0.as_str(), &c[0].1), ("git.api.example", &SecretRef::File("~/.api_token".into())));
        // The first credential whose host a prompt names answers it.
        let answer = |c: &[(String, SecretRef)], prompt: &str| c.iter().find_map(|(h, r)| match_prompt(prompt, h).map(|k| (k, r.clone())));
        let c = creds(Some("services/api"));
        assert_eq!(answer(&c, "Password for 'https://oauth2@git.api.example': "), Some((Answer::Password, SecretRef::File("~/.api_token".into()))));
        assert_eq!(answer(&c, "Password for 'https://oauth2@gitlab.com': "), Some((Answer::Password, SecretRef::File("~/.root_token".into()))));
        assert_eq!(answer(&c, "Password for 'https://oauth2@evil.example': "), None);
        // Git's credential helpers are reset for every host askpass answers for, once each.
        assert_eq!(
            reset_helpers_keys(&paths, &cfg, project),
            ["credential.https://gitlab.com.helper", "credential.https://git.api.example.helper"]
        );

        // `.workbench.toml` of the repository cannot name config.toml's secrets for a host of its choosing.
        let other = d.path().join("evil");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join(".workbench.toml"),
            "[[repository]]\npath = \"web\"\n[repository.gitlab]\nhost = \"evil.example\"\npath = \"g/r\"\ntoken = \"gitlab\"\n",
        )
        .unwrap();
        let c = gitlab_credentials(&paths, &cfg, Some((other.as_path(), "p9")), Some("web"));
        assert_eq!(hosts(&c), ["gitlab.com"], "only the global host: {c:?}");
        assert_eq!(repo_id_of(" ./services//api/ "), "services/api");
    }
}
