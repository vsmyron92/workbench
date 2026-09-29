//! Git credentials end to end: the prompts real git passes to askpass, and remote ops against
//! a local HTTPS remote that requires authentication, with a logging credential helper.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;

use super::askpass::{Answer, match_prompt};
use super::tests::{commit_all, git, init_repo, no_hooks, op_done, write};
use crate::util::os::path::to_slash;

/// A `#!/bin/sh` script (Git for Windows runs these with its own sh).
fn script(path: &Path, body: &str) -> PathBuf {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    crate::util::os::perm::apply(path, 0o755).unwrap();
    path.to_path_buf()
}

/// An empty file for `GIT_CONFIG_GLOBAL`: the user's global and system git config
/// (their credential helpers above all) stay out of these tests.
fn no_user_config(dir: &Path) -> Vec<(String, String)> {
    let empty = dir.join("empty-gitconfig");
    std::fs::write(&empty, "").unwrap();
    vec![("GIT_CONFIG_GLOBAL".into(), empty.display().to_string()), ("GIT_CONFIG_NOSYSTEM".into(), "1".into())]
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// What git itself asks GIT_ASKPASS for URLs whose user name, host or path would move the
/// host, before the CVE-2024-50349 fix (`credential.sanitizePrompt=false`) and after it:
/// no prompt of a URL whose host is not the configured one gets an answer.
#[test]
fn prompts_of_real_git_never_steer_the_token_to_another_host() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("prompts.log");
    let askpass = script(
        &d.path().join("askpass"),
        &format!("printf '%s\\n' \"$1\" >> '{}'\ncase \"$1\" in Username*) echo oauth2 ;; *) echo x ;; esac", to_slash(&log)),
    );
    let env = no_user_config(d.path());
    let prompts = |url: &str, sanitize: bool, http_path: bool| -> Vec<String> {
        let _ = std::fs::remove_file(&log);
        let mut cmd = std::process::Command::new("git");
        cmd.args(["-c", &format!("credential.sanitizePrompt={sanitize}"), "-c", &format!("credential.useHttpPath={http_path}")])
            .args(["credential", "fill"])
            .current_dir(d.path())
            .envs(env.iter().map(|(k, v)| (k, v)))
            .env("GIT_ASKPASS", &askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn().expect("git runs");
        {
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            writeln!(stdin, "url={url}\n").unwrap();
        }
        child.wait().unwrap();
        read(&log).lines().map(String::from).collect()
    };
    let hostile = [
        "https://gitlab.com%2F@evil.example/r.git",
        "https://gitlab.com%2Fx@evil.example/r.git",
        "https://gitlab.com%3A443%2F@evil.example/r.git",
        "https://x%2F%40gitlab.com@evil.example/r.git",
        "https://x@gitlab.com%2F%40evil.example/r.git",
        "https://gitlab.com%2F%40evil.example/r.git",
        "https://gitlab.com@evil.example/r.git",
        "https://gitlab.com.evil.example/r.git",
        "http://gitlab.com/r.git",
    ];
    for url in hostile {
        for sanitize in [true, false] {
            for http_path in [false, true] {
                let asked = prompts(url, sanitize, http_path);
                assert!(!asked.is_empty(), "git asked nothing for {url}");
                for p in &asked {
                    assert_eq!(match_prompt(p, "gitlab.com"), None, "{url} (sanitizePrompt={sanitize}, useHttpPath={http_path}): {p}");
                }
            }
        }
    }
    // The configured host itself, however git prints the user name.
    let (user, pass) = (Some(Answer::Username), Some(Answer::Password));
    let legit = [
        ("https://gitlab.com/group/r.git", vec![user, pass]),
        ("https://oauth2@GitLab.com:443/group/r.git", vec![pass]),
        ("https://me%40corp.example@gitlab.com/group/r.git", vec![pass]),
    ];
    for (url, want) in legit {
        for sanitize in [true, false] {
            let got: Vec<Option<Answer>> = prompts(url, sanitize, false).iter().map(|p| match_prompt(p, "gitlab.com")).collect();
            assert_eq!(got, want, "{url} (sanitizePrompt={sanitize})");
        }
    }
    // A user name with `/` printed decoded reads like a path: refused; percent-encoded
    // (git with the fix, by default) it is answered.
    for sanitize in [true, false] {
        let asked = prompts("https://a%3Ab%2Fc@gitlab.com/group/r.git", sanitize, false);
        assert_eq!(asked.len(), 1, "{asked:?}");
        let want = if asked[0].contains("%2F") { pass } else { None };
        assert_eq!(match_prompt(&asked[0], "gitlab.com"), want, "{asked:?}");
    }
}

/// A self-signed certificate made with the openssl CLI (`None` when it is absent).
fn tls_acceptor(dir: &Path) -> Option<tokio_rustls::TlsAcceptor> {
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    let ok = std::process::Command::new("openssl")
        .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=localhost"])
        .arg("-keyout")
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("openssl not available; skipping");
        return None;
    }
    let cfg = crate::config::global::TlsConfig { cert: cert.display().to_string(), key: key.display().to_string() };
    Some(crate::platform::tls::acceptor(&cfg).unwrap())
}

struct Remote {
    /// The bare repository served at `/up.git/` (git's "dumb" HTTP protocol).
    bare: PathBuf,
    /// `user:password` of every authenticated request.
    logins: Mutex<Vec<String>>,
    expected: String,
}

fn serve(remote: &Remote, headers: &HeaderMap, uri: &Uri) -> Response {
    use base64::Engine;
    let login = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let Some(login) = login else {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic realm=\"GitLab\"")]).into_response();
    };
    remote.logins.lock().push(login.clone());
    if login != remote.expected {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic realm=\"GitLab\"")]).into_response();
    }
    let Some(rel) = uri.path().strip_prefix("/up.git/") else { return StatusCode::NOT_FOUND.into_response() };
    if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match std::fs::read(remote.bare.join(rel)) {
        Ok(body) => ([(header::CONTENT_TYPE, "text/plain")], body).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn https_remote(dir: &Path, remote: Arc<Remote>) -> Option<SocketAddr> {
    let acceptor = tls_acceptor(dir)?;
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = crate::platform::tls::TlsListener::new(tcp, acceptor).unwrap();
    let addr = axum::serve::Listener::local_addr(&listener).unwrap();
    let app = axum::Router::new().fallback(move |headers: HeaderMap, uri: Uri| {
        let remote = remote.clone();
        async move { serve(&remote, &headers, &uri) }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Some(addr)
}

/// Fetches from the configured GitLab host get Workbench's token from askpass without git's
/// credential helpers being asked for it or told to store it; another host keeps them.
/// Plain git on the same remote (no reset) hands the token to the helper: the case the
/// reset prevents.
#[tokio::test]
async fn remote_ops_keep_the_managed_token_from_credential_helpers() {
    const TOKEN: &str = "wb-test-token-7f3a";
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();

    // Upstream with two commits; `down` has only the first, so the fetch brings objects.
    let up = init_repo();
    write(up.path(), "a.txt", "1\n");
    commit_all(up.path(), "one");
    let down = t.join("down");
    git(t, &["clone", "-q", up.path().to_str().unwrap(), down.to_str().unwrap()]);
    no_hooks(&down);
    write(up.path(), "a.txt", "2\n");
    commit_all(up.path(), "two");
    let bare = t.join("srv").join("up.git");
    git(t, &["clone", "-q", "--bare", up.path().to_str().unwrap(), bare.to_str().unwrap()]);
    git(&bare, &["update-server-info"]);

    let remote = Arc::new(Remote { bare, logins: Mutex::new(vec![]), expected: format!("oauth2:{TOKEN}") });
    let Some(addr) = https_remote(t, remote.clone()).await else { return };
    let port = addr.port();

    // The user's helper: logs every call and knows nothing.
    let helper_log = t.join("helper.log");
    git(&down, &["config", "credential.helper", &format!("!f() {{ {{ echo \"op=$1\"; cat; }} >> '{}'; }}; f", to_slash(&helper_log))]);
    git(&down, &["config", "http.sslVerify", "false"]);
    git(&down, &["remote", "set-url", "origin", &format!("https://127.0.0.1:{port}/up.git")]);
    git(&down, &["remote", "add", "other", &format!("https://localhost:{port}/up.git")]);
    // Stands in for `workbench askpass` (the test binary is not Workbench): answers any host.
    let ask_log = t.join("askpass.log");
    let askpass = script(
        &t.join("askpass"),
        &format!(
            "printf '%s\\n' \"$1\" >> '{}'\ncase \"$1\" in Username*) echo oauth2 ;; Password*) echo {TOKEN} ;; *) exit 1 ;; esac",
            to_slash(&ask_log)
        ),
    );

    let token_file = t.join("gitlab_token");
    std::fs::write(&token_file, TOKEN).unwrap();
    let paths = crate::config::Paths { config_dir: t.join("config"), data_dir: t.join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.projects.include = vec![down.display().to_string()];
    cfg.gitlab = Some(crate::config::global::GitlabConfig { host: format!("https://127.0.0.1:{port}"), token: "gitlab".into() });
    cfg.secrets.insert("gitlab".into(), crate::config::SecretRef::File(token_file.display().to_string()));
    let state = crate::app::AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    state.git.askpass.set(vec![("GIT_ASKPASS".into(), askpass.display().to_string())]).unwrap();
    let pid = state.projects.list()[0].id.clone();
    let project = state.projects.require(&pid).unwrap();
    let repo = state.git.repo(&project).await.unwrap();

    let fetch = |name: &str| {
        let mut spec = super::remote::RemoteOpSpec::new("fetch", format!("Fetch {name}"), ["fetch", "--progress", name].map(String::from).to_vec(), false);
        spec.env = no_user_config(t);
        super::remote::start(&state, repo.clone(), spec, None).unwrap()
    };

    let op = op_done(&state, &fetch("origin"), std::time::Duration::from_secs(60)).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    assert_eq!(git(&down, &["rev-parse", "origin/main"]), git(up.path(), &["rev-parse", "HEAD"]));
    assert!(remote.logins.lock().iter().any(|l| l == &remote.expected), "askpass's token reached the remote");
    let asked = read(&ask_log);
    assert!(asked.contains(&format!("Username for 'https://127.0.0.1:{port}': ")), "{asked}");
    assert!(asked.contains(&format!("Password for 'https://oauth2@127.0.0.1:{port}': ")), "{asked}");
    assert_eq!(read(&helper_log), "", "no helper is asked for, or told to store, the managed host's token");

    // Another host (the same server under another name) still goes through the user's helper.
    let op = op_done(&state, &fetch("other"), std::time::Duration::from_secs(60)).await;
    assert_eq!(op.ok, Some(true), "{op:?}");
    let helper = read(&helper_log);
    assert!(helper.contains("op=get") && helper.contains(&format!("host=localhost:{port}")), "{helper}");
    assert!(!helper.contains("host=127.0.0.1"), "{helper}");

    // Without the reset, git hands the askpass answer to the helper for the managed host too.
    // (Async: the remote is served on this test's runtime.)
    std::fs::remove_file(&helper_log).unwrap();
    let out = tokio::process::Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(&down)
        .envs(no_user_config(t))
        .env("GIT_ASKPASS", &askpass)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let helper = read(&helper_log);
    assert!(helper.contains("op=store") && helper.contains(&format!("host=127.0.0.1:{port}")), "the helper sees the token without the reset: {helper}");
}
