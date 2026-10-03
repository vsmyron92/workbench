//! Signing an account in: the CLI's own login, run in a terminal, and whether the account
//! holds one (`GET /api/agents/signin`, `POST /api/agents/signin/{provider}`).
//!
//! Workbench starts the CLI's login command and asks its status command. It never reads a login
//! file or a token, and the CLI's own text never reaches the browser: what leaves this module is
//! a yes or no, one of a few fixed words for how the account signs in, and the plan's short name.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};

use super::providers::ProviderKind;
use crate::util;

/// How long a status command may run. The CLIs start Node or a Rust binary and read one file.
const TIMEOUT: Duration = Duration::from_secs(8);

/// How long an answer is reused, so a settings page that is opened and refocused does not start
/// a CLI each time. A finished sign-in forgets it (`Cache::forget`).
const FRESH: Duration = Duration::from_secs(20);

/// How a CLI signs in and reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Method {
    /// The arguments after the program that start the sign-in. Empty: the CLI asks by itself when it
    /// is started (Gemini CLI, Kimi Code), so it is started as it is.
    pub login: &'static [&'static str],
    /// The arguments that report whether it is signed in. `None`: the CLI cannot say without a session.
    pub status: Option<&'static [&'static str]>,
}

/// The way `kind` signs in, or `None` when there is nothing to sign in to (Aider has API keys, a
/// custom CLI is unknown to Workbench). Fixed here, never taken from a repository or the browser.
pub fn method(kind: ProviderKind) -> Option<Method> {
    match kind {
        ProviderKind::Claude => Some(Method { login: &["auth", "login"], status: Some(&["auth", "status", "--json"]) }),
        ProviderKind::Codex => Some(Method { login: &["login"], status: Some(&["login", "status"]) }),
        ProviderKind::Gemini | ProviderKind::Kimi => Some(Method { login: &[], status: None }),
        ProviderKind::Aider | ProviderKind::Custom => None,
    }
}

/// The command line that signs the account in: the CLI's program, then its login arguments.
pub fn login_argv(program: &Path, kind: ProviderKind) -> Option<Vec<String>> {
    let m = method(kind)?;
    let mut argv = vec![program.display().to_string()];
    argv.extend(m.login.iter().map(|a| a.to_string()));
    Some(argv)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    SignedIn,
    SignedOut,
    /// The CLI cannot say, did not answer, or answered something unexpected.
    Unknown,
}

/// What the status command said, reduced to what the settings page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub state: State,
    /// How the account signs in: one of a few fixed words, never the CLI's text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    /// The plan's short name (`max`, `pro`), when the CLI names it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

impl Status {
    pub fn unknown() -> Self {
        Self { state: State::Unknown, method: None, plan: None }
    }
}

/// A plan name is a short word. Anything else (the CLI's text, a path) is not passed on.
fn valid_plan(p: &str) -> bool {
    !p.is_empty() && p.len() <= 24 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `claude auth status --json`: `{"loggedIn": bool, "authMethod": …, "apiProvider": …, "subscriptionType": …}`.
fn parse_claude(stdout: &str) -> Status {
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else { return Status::unknown() };
    let Some(logged_in) = v.get("loggedIn").and_then(Value::as_bool) else { return Status::unknown() };
    if !logged_in {
        return Status { state: State::SignedOut, method: None, plan: None };
    }
    let method = if v.get("apiProvider").and_then(Value::as_str).is_some_and(|p| p != "firstParty") {
        Some("Cloud provider")
    } else {
        match v.get("authMethod").and_then(Value::as_str) {
            Some("claude.ai") => Some("Claude subscription"),
            Some("console" | "api_key" | "apiKey") => Some("API key"),
            Some("oauth_token") => Some("Access token"),
            _ => None,
        }
    };
    let plan = v.get("subscriptionType").and_then(Value::as_str).filter(|p| valid_plan(p)).map(str::to_lowercase);
    Status { state: State::SignedIn, method, plan }
}

/// `codex login status` says `Logged in using ChatGPT`, `Logged in using an API key` or `Not logged
/// in`, on stderr or stdout depending on the version.
fn parse_codex(text: &str) -> Status {
    let t = text.to_lowercase();
    if t.contains("not logged in") {
        return Status { state: State::SignedOut, method: None, plan: None };
    }
    if t.contains("logged in") {
        let method = if t.contains("chatgpt") {
            Some("ChatGPT account")
        } else if t.contains("api key") {
            Some("API key")
        } else {
            None
        };
        return Status { state: State::SignedIn, method, plan: None };
    }
    Status::unknown()
}

/// Read what a status command printed.
pub fn parse(kind: ProviderKind, stdout: &str, stderr: &str) -> Status {
    match kind {
        ProviderKind::Claude => parse_claude(stdout),
        ProviderKind::Codex => parse_codex(&format!("{stdout}\n{stderr}")),
        _ => Status::unknown(),
    }
}

/// Ask the CLI whether it is signed in, as the account's own environment sees it. Any failure
/// (it does not start, it is too slow, it prints something else) is `Unknown`: not signed out.
pub async fn probe(program: &Path, kind: ProviderKind, env: &[(String, Option<String>)]) -> Status {
    let Some(args) = method(kind).and_then(|m| m.status) else { return Status::unknown() };
    let resolved = util::os::exe::classify(program.to_path_buf());
    let mut cmd = util::os::exe::command(&resolved);
    cmd.args(args).current_dir(dirs::home_dir().unwrap_or_else(std::env::temp_dir));
    for (k, v) in env {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    match util::proc::try_run_cmd(cmd, TIMEOUT).await {
        // Signed out is often a failing exit code with the answer printed all the same.
        Ok(out) => parse(kind, &out.stdout, &out.stderr),
        Err(_) => Status::unknown(),
    }
}

/// A status and when it was asked.
#[derive(Debug, Clone)]
pub struct Checked {
    pub status: Status,
    pub at_ms: i64,
    at: Instant,
}

impl Checked {
    pub fn describe(&self) -> Value {
        let mut v = serde_json::to_value(&self.status).unwrap_or_else(|_| json!({}));
        v["checkedAt"] = json!(self.at_ms);
        v
    }
}

/// The last answer per account.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<String, Checked>>);

impl Cache {
    /// The answer for `id` if it is younger than `FRESH`.
    pub fn fresh(&self, id: &str) -> Option<Checked> {
        self.0.lock().get(id).filter(|c| c.at.elapsed() < FRESH).cloned()
    }

    pub fn put(&self, id: &str, status: Status) -> Checked {
        let checked = Checked { status, at_ms: util::now_ms(), at: Instant::now() };
        self.0.lock().insert(id.to_string(), checked.clone());
        checked
    }

    /// Forget the answer for `id`: its sign-in just ended, or it was started again.
    pub fn forget(&self, id: &str) {
        self.0.lock().remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cli_with_a_login_has_a_fixed_way_to_sign_in() {
        assert_eq!(method(ProviderKind::Claude).unwrap().login, ["auth", "login"]);
        assert_eq!(method(ProviderKind::Codex).unwrap().status, Some(&["login", "status"][..]));
        // Started plain, these ask by themselves; they cannot report without a session.
        for k in [ProviderKind::Gemini, ProviderKind::Kimi] {
            let m = method(k).unwrap();
            assert!(m.login.is_empty() && m.status.is_none(), "{k:?}");
        }
        // Aider has keys, a custom CLI is unknown.
        assert!(method(ProviderKind::Aider).is_none() && method(ProviderKind::Custom).is_none());
        assert_eq!(login_argv(Path::new("/opt/bin/claude"), ProviderKind::Claude).unwrap(), ["/opt/bin/claude", "auth", "login"]);
        assert_eq!(login_argv(Path::new("/opt/bin/gemini"), ProviderKind::Gemini).unwrap(), ["/opt/bin/gemini"]);
        assert!(login_argv(Path::new("aider"), ProviderKind::Aider).is_none());
    }

    #[test]
    fn reads_the_claude_status() {
        let out = parse_claude(r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"me@example.com","subscriptionType":"Max"}"#);
        assert_eq!(out, Status { state: State::SignedIn, method: Some("Claude subscription"), plan: Some("max".into()) });
        let key = parse_claude(r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty"}"#);
        assert_eq!((key.state, key.method, key.plan), (State::SignedIn, Some("API key"), None));
        let token = parse_claude(r#"{"loggedIn":true,"authMethod":"oauth_token","apiProvider":"firstParty"}"#);
        assert_eq!(token.method, Some("Access token"));
        let cloud = parse_claude(r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"bedrock"}"#);
        assert_eq!(cloud.method, Some("Cloud provider"));
        assert_eq!(parse_claude(r#"{"loggedIn":false}"#), Status { state: State::SignedOut, method: None, plan: None });
        // A method this version has no word for, and a plan that is not a short word, are dropped.
        let odd = parse_claude(r#"{"loggedIn":true,"authMethod":"something new","subscriptionType":"max; rm -rf /"}"#);
        assert_eq!(odd, Status { state: State::SignedIn, method: None, plan: None });
        for junk in ["", "Not logged in", "<html>", r#"{"loggedIn":"yes"}"#, "[]"] {
            assert_eq!(parse_claude(junk).state, State::Unknown, "{junk:?}");
        }
    }

    #[test]
    fn what_leaves_is_never_the_clis_own_text_or_the_email() {
        let status = parse_claude(r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@example.com","orgName":"Acme Ltd","subscriptionType":"pro"}"#);
        let sent = serde_json::to_string(&status).unwrap();
        assert!(!sent.contains("example.com") && !sent.contains("Acme"), "{sent}");
        assert_eq!(sent, r#"{"state":"signedIn","method":"Claude subscription","plan":"pro"}"#);
    }

    #[test]
    fn reads_the_codex_status() {
        let chat = parse_codex("Logged in using ChatGPT\n");
        assert_eq!((chat.state, chat.method), (State::SignedIn, Some("ChatGPT account")));
        assert_eq!(parse_codex("\nLogged in using an API key - sk-proj-***\n").method, Some("API key"));
        assert_eq!(parse_codex("Logged in\n").method, None);
        // "Not logged in" contains "logged in".
        assert_eq!(parse_codex("Not logged in\n").state, State::SignedOut);
        assert_eq!(parse_codex("error: unrecognized subcommand 'status'").state, State::Unknown);
        assert_eq!(parse_codex("").state, State::Unknown);
        // Either stream may carry it.
        assert_eq!(parse(ProviderKind::Codex, "", "Logged in using ChatGPT").state, State::SignedIn);
        assert_eq!(parse(ProviderKind::Codex, "Not logged in", "").state, State::SignedOut);
        // The others cannot be asked.
        assert_eq!(parse(ProviderKind::Gemini, "Logged in", "").state, State::Unknown);
    }

    #[test]
    fn an_answer_is_reused_until_it_is_forgotten() {
        let cache = Cache::default();
        assert!(cache.fresh("claude").is_none());
        cache.put("claude", Status { state: State::SignedOut, method: None, plan: None });
        assert_eq!(cache.fresh("claude").unwrap().status.state, State::SignedOut);
        assert!(cache.fresh("claude-work").is_none(), "another account has its own answer");
        cache.forget("claude");
        assert!(cache.fresh("claude").is_none());
        let described = cache.put("claude", Status::unknown()).describe();
        assert_eq!(described["state"], "unknown");
        assert!(described["checkedAt"].as_u64().unwrap() > 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn asks_the_program_with_the_accounts_environment() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("claude");
        // Signed in only where CLAUDE_CONFIG_DIR names a folder holding a marker, as a real login would.
        std::fs::write(
            &fake,
            "#!/bin/sh\ncase \"$*\" in 'auth status --json') ;; *) echo \"unexpected: $*\" >&2; exit 2;; esac\n\
             if [ -f \"$CLAUDE_CONFIG_DIR/signed-in\" ]; then echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"subscriptionType\":\"max\"}'\n\
             else echo '{\"loggedIn\":false}'; exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let account = dir.path().join("work");
        std::fs::create_dir_all(&account).unwrap();
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), Some(account.display().to_string()))];
        // A freshly written script can be busy while another test's child holds it open.
        let ask = || async {
            for _ in 0..200 {
                let s = probe(&fake, ProviderKind::Claude, &env).await;
                if s.state != State::Unknown {
                    return s;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            probe(&fake, ProviderKind::Claude, &env).await
        };
        assert_eq!(ask().await.state, State::SignedOut, "a failing exit code still carries the answer");
        std::fs::write(account.join("signed-in"), "").unwrap();
        assert_eq!(ask().await, Status { state: State::SignedIn, method: Some("Claude subscription"), plan: Some("max".into()) });
        // A CLI that is not there, and one that cannot be asked, are not "signed out".
        assert_eq!(probe(&dir.path().join("missing"), ProviderKind::Claude, &env).await.state, State::Unknown);
        assert_eq!(probe(&fake, ProviderKind::Gemini, &env).await.state, State::Unknown);
    }
}
