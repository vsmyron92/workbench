//! End-to-end tests of accounts: which account a new session runs as when one is at its
//! usage limit, continuing a session on the next account, and local models, through the
//! real PTY path with the fake Claude Code of `testdata/fake_cli.py`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::AgentState;
use super::e2e::{agent_of, fake_cli, log_lines, wait_long};
use super::e2e_agents::{served_with, sign_in};
use crate::app::AppState;
use crate::config::global::{LocalModelConfig, ProviderConfig};

/// A Claude Code account: the log of how its sessions were started.
struct Account {
    log: PathBuf,
}

fn account(dir: &Path, fake: &Path, id: &'static str, extra: impl FnOnce(&mut ProviderConfig)) -> (Account, ProviderConfig) {
    let home = dir.join(format!("home-{id}"));
    std::fs::create_dir_all(&home).unwrap();
    let log = dir.join(format!("{id}.log"));
    let mut c = ProviderConfig {
        kind: Some("claude".into()),
        command: Some(fake.display().to_string()),
        env: [("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string()), ("FAKE_CLAUDE_LOG".to_string(), log.display().to_string())].into(),
        ..Default::default()
    };
    extra(&mut c);
    (Account { log }, c)
}

fn ask(pid: &str, provider: Option<&str>) -> super::agent::AgentRequest {
    super::agent::AgentRequest { project_id: pid.into(), provider: provider.map(str::to_string), ..Default::default() }
}

fn provider_of(state: &AppState, id: &str) -> String {
    agent_of(state, id).provider_id.expect("a provider")
}

/// Three accounts: `claude` falls back to `claude-b`, then to a local model.
async fn three_accounts(dir: &Path, failover: Option<&str>) -> (AppState, std::net::SocketAddr, String, Vec<Account>) {
    let fake = fake_cli(dir, "claude");
    let (a, ca) = account(dir, &fake, "claude", |c| {
        c.kind = None;
        c.fallback = vec!["claude-b".into(), "claude-local".into()];
    });
    let (b, cb) = account(dir, &fake, "claude-b", |_| {});
    let (l, cl) = account(dir, &fake, "claude-local", |c| {
        c.model = Some("qwen3-coder".into());
        c.local = Some(LocalModelConfig { server: "ollama".into(), url: "http://127.0.0.1:9".into() });
    });
    let failover = failover.map(str::to_string);
    let (state, addr, pid) = served_with(dir, &fake, |cfg| {
        cfg.agents.failover = failover;
        for (id, c) in [("claude", ca), ("claude-b", cb), ("claude-local", cl)] {
            cfg.agents.providers.insert(id.into(), c);
        }
    })
    .await;
    (state, addr, pid, vec![a, b, l])
}

fn env_line(log: &Path) -> String {
    log_lines(log, "ENV ").first().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_session_skips_accounts_at_their_limit_and_ends_on_a_local_model() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, accts) = three_accounts(dir.path(), None).await;
    let t = &state.terminals;
    let now = crate::util::now_ms();
    let mut events = state.events.subscribe();

    // Nothing is limited: the account asked for.
    let a = t.spawn_agent(&state, ask(&pid, None)).await.unwrap();
    assert_eq!(provider_of(&state, &a.id), "claude");
    assert!(a.meta.get("failover").is_none());
    wait_long("the first session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    // (The vendor's account keeps whatever environment the server has.)
    assert!(!env_line(&accts[0].log).contains("127.0.0.1:9"), "{}", env_line(&accts[0].log));

    // The first account is at its limit: the second, with its own folder.
    t.usage.mark_limited("claude", Some(now + 3_600_000), "session limit reached", now);
    let b = t.spawn_agent(&state, ask(&pid, None)).await.unwrap();
    assert_eq!(provider_of(&state, &b.id), "claude-b");
    assert_eq!(b.meta["failover"]["from"], "claude");
    assert_eq!(b.meta["failover"]["reason"], "session limit reached");
    wait_long("the second session", 10, || log_lines(&accts[1].log, "ENV ").len() == 1).await;
    assert!(env_line(&accts[1].log).contains("home-claude-b"), "{}", env_line(&accts[1].log));
    // The notice reached the devices.
    let mut seen = None;
    while let Ok(ev) = events.try_recv() {
        if ev.kind == "agent.failover" {
            seen = Some(ev.data.clone());
        }
    }
    let seen = seen.expect("agent.failover");
    assert_eq!((seen["from"].as_str(), seen["to"].as_str(), seen["terminalId"].as_str()), (Some("claude"), Some("claude-b"), Some(b.id.as_str())));

    // Both are: the local model, with the environment that sends nothing to the vendor.
    t.usage.mark_limited("claude-b", Some(now + 3_600_000), "session limit reached", now);
    let mut req = ask(&pid, Some("claude"));
    req.model = Some("opus".into());
    req.effort = Some("high".into());
    let l = t.spawn_agent(&state, req).await.unwrap();
    assert_eq!(provider_of(&state, &l.id), "claude-local");
    let got = agent_of(&state, &l.id);
    // The model asked for belongs to the vendor; the local one is used instead.
    assert_eq!((got.model.as_deref(), got.effort.as_deref()), (Some("qwen3-coder"), None));
    wait_long("the local session", 10, || log_lines(&accts[2].log, "ENV ").len() == 1).await;
    let env = env_line(&accts[2].log);
    for want in ["ANTHROPIC_BASE_URL=http://127.0.0.1:9", "ANTHROPIC_AUTH_TOKEN=ollama", "ANTHROPIC_API_KEY=<unset>", "ANTHROPIC_MODEL=qwen3-coder", "ANTHROPIC_DEFAULT_HAIKU_MODEL=qwen3-coder"] {
        assert!(env.contains(want), "{want} in {env}");
    }
    assert!(log_lines(&accts[2].log, "ARGS ")[0].contains("--model qwen3-coder"), "{:?}", log_lines(&accts[2].log, "ARGS "));
    assert!(!got.remote_control);

    // Every account is out: the one asked for, which shows its own limit.
    t.usage.mark_limited("claude-local", Some(now + 3_600_000), "x", now);
    let z = t.spawn_agent(&state, ask(&pid, None)).await.unwrap();
    assert_eq!(provider_of(&state, &z.id), "claude");
    assert!(z.meta.get("failover").is_none());
    // A conversation stays on its account, whatever its usage.
    let r = t.restart(&state, &a.id).await.unwrap();
    assert_eq!(r.agent.unwrap().provider_id.as_deref(), Some("claude"));
    for id in [&a.id, &b.id, &l.id, &z.id] {
        let _ = t.kill(id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failover_off_keeps_the_account_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, _accts) = three_accounts(dir.path(), Some("off")).await;
    let now = crate::util::now_ms();
    state.terminals.usage.mark_limited("claude", Some(now + 3_600_000), "x", now);
    let a = state.terminals.spawn_agent(&state, ask(&pid, None)).await.unwrap();
    assert_eq!(provider_of(&state, &a.id), "claude");
    let _ = state.terminals.kill(&a.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_hits_its_limit_is_announced_and_continued_on_the_next_account() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, accts) = three_accounts(dir.path(), Some("session")).await;
    let t = &state.terminals;
    let mut events = state.events.subscribe();
    let a = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    t.send_text(&a.id, "touch one.txt", true).await.unwrap();
    wait_long("a request", 10, || agent_of(&state, &a.id).pending_permission.is_some()).await;

    // The turn is refused for the account's usage; its message is on the screen.
    t.claude_refused(&state, &a.id, "rate_limit", "API Error\nYou've hit your session limit · resets 11:59pm").await;

    let limited = t.usage.limited("claude", crate::util::now_ms()).expect("the account is out of use");
    assert!(limited.until > crate::util::now_ms() && limited.reason.contains("session limit"), "{limited:?}");
    // A new session on the next account, told where this one stopped.
    wait_long("the continued session", 10, || t.list().iter().any(|i| i.agent.as_ref().and_then(|g| g.provider_id.as_deref()) == Some("claude-b"))).await;
    let moved = t.list().into_iter().find(|i| i.agent.as_ref().and_then(|g| g.provider_id.as_deref()) == Some("claude-b")).unwrap();
    assert_eq!(t.info(&a.id).unwrap().meta["movedTo"], moved.id.as_str());
    assert!(moved.title.ends_with("· claude-b"), "{}", moved.title);
    wait_long("its start", 10, || !log_lines(&accts[1].log, "ARGS ").is_empty()).await;
    let args = log_lines(&accts[1].log, "ARGS ")[0].clone();
    assert!(args.contains("usage limit") && args.contains("git status"), "{args}");
    // The old session was not touched.
    assert_eq!(t.info(&a.id).unwrap().status, super::TerminalStatus::Running);
    let mut limit_event = None;
    while let Ok(ev) = events.try_recv() {
        if ev.kind == "agent.limit" {
            limit_event = Some(ev.data.clone());
        }
    }
    let ev = limit_event.expect("agent.limit");
    assert_eq!((ev["providerId"].as_str(), ev["fallback"]["id"].as_str(), ev["movedTo"]["providerId"].as_str()), (Some("claude"), Some("claude-b"), Some("claude-b")));

    // The same refusal again (the screen and a hook) announces and moves nothing more.
    let before = t.list().len();
    t.claude_refused(&state, &a.id, "rate_limit", "You've hit your session limit · resets 11:59pm").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(t.list().len(), before);

    // One model's limit leaves the account alone.
    t.usage.clear_limited("claude", crate::util::now_ms());
    let c = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    t.claude_refused(&state, &c.id, "rate_limit", "You've hit your Opus limit · resets 11:59pm").await;
    assert!(t.usage.limited("claude", crate::util::now_ms()).is_none());
    // A plain 429 with no full window neither.
    t.claude_refused(&state, &c.id, "rate_limit", "API Error: Request rejected (429) · this may be a temporary capacity issue").await;
    assert!(t.usage.limited("claude", crate::util::now_ms()).is_none());
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn usage_comes_from_the_status_line_and_a_person_can_correct_it() {
    let dir = tempfile::tempdir().unwrap();
    let (state, addr, pid, _accts) = three_accounts(dir.path(), Some("new")).await;
    let t = &state.terminals;
    let a = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    let (cookie, key) = sign_in(&state, addr).await;
    let http = reqwest::Client::new();
    let origin = format!("http://{addr}");
    let call = |method: reqwest::Method, path: &str, body: Option<Value>| {
        let mut r = http.request(method, format!("{origin}{path}")).header("cookie", &cookie).header("origin", &origin).header(crate::auth::KEY_HEADER, &key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
    };
    let reset = (crate::util::now_ms() / 1000) + 7200;
    let full = json!({"model": {"display_name": "Fake"}, "rate_limits": {"five_hour": {"used_percentage": 100, "resets_at": reset}, "seven_day": {"used_percentage": 40, "resets_at": reset + 100000}}});
    t.note_status_usage(&a.id, &full);
    let u: Value = call(reqwest::Method::GET, "/api/agents/usage", None).await.unwrap().json().await.unwrap();
    assert_eq!(u["usage"]["claude"]["limited"], true);
    assert_eq!(u["usage"]["claude"]["windows"][0]["label"], "5-hour");
    assert_eq!(u["usage"]["claude"]["limitedUntil"], reset * 1000);
    assert_eq!(u["failover"], "new");
    // The new-session form sees it, and a new session goes to the next account.
    let d: Value = call(reqwest::Method::GET, &format!("/api/agents/defaults?projectId={pid}"), None).await.unwrap().json().await.unwrap();
    let claude = d["providers"].as_array().unwrap().iter().find(|p| p["id"] == "claude").unwrap();
    assert_eq!((claude["usage"]["limited"].clone(), claude["fallback"][0].clone()), (json!(true), json!("claude-b")));
    let local = d["providers"].as_array().unwrap().iter().find(|p| p["id"] == "claude-local").unwrap();
    assert_eq!((local["local"]["server"].as_str(), local["local"]["url"].as_str()), (Some("ollama"), Some("http://127.0.0.1:9")));
    let n = t.spawn_agent(&state, ask(&pid, None)).await.unwrap();
    assert_eq!(provider_of(&state, &n.id), "claude-b");
    // The status line shows room again (a window reset): the account is usable.
    t.note_status_usage(&a.id, &json!({"rate_limits": {"five_hour": {"used_percentage": 3, "resets_at": reset + 18000}}}));
    let u: Value = call(reqwest::Method::GET, "/api/agents/usage", None).await.unwrap().json().await.unwrap();
    assert_eq!(u["usage"]["claude"]["limited"], false);
    // A person says an account is at its limit, and that it is not.
    let until = crate::util::now_ms() + 3_600_000;
    let r = call(reqwest::Method::PUT, "/api/agents/usage/claude-b", Some(json!({"limitedUntil": until}))).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(t.usage.limited("claude-b", crate::util::now_ms()).is_some());
    let r = call(reqwest::Method::PUT, "/api/agents/usage/claude-b", Some(json!({"limitedUntil": null}))).await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(t.usage.limited("claude-b", crate::util::now_ms()).is_none());
    assert_eq!(call(reqwest::Method::PUT, "/api/agents/usage/claude-b", Some(json!({"limitedUntil": 5}))).await.unwrap().status(), 400);
    // An account that does not exist.
    assert_eq!(call(reqwest::Method::PUT, "/api/agents/usage/ghost", Some(json!({"limitedUntil": null}))).await.unwrap().status(), 412);
    // A person moves a session to an account by hand.
    let r = call(reqwest::Method::POST, &format!("/api/agents/{}/switch", a.id), Some(json!({"provider": "claude-b"}))).await.unwrap();
    assert_eq!(r.status(), 200);
    let moved: Value = r.json().await.unwrap();
    assert_eq!(moved["agent"]["providerId"], "claude-b");
    assert_eq!(t.info(&a.id).unwrap().meta["movedTo"], moved["id"]);
    // The same move offered on another device finds the session the first one started.
    let again = call(reqwest::Method::POST, &format!("/api/agents/{}/switch", a.id), Some(json!({"provider": "claude-b"}))).await.unwrap();
    assert_eq!(again.status(), 200);
    let again: Value = again.json().await.unwrap();
    assert_eq!(again["id"], moved["id"]);
    // …not onto the account it already runs on.
    let same = call(reqwest::Method::POST, &format!("/api/agents/{}/switch", a.id), Some(json!({"provider": "claude"}))).await.unwrap();
    assert_eq!(same.status(), 409);
    // An agent's own credentials may not decide about accounts.
    let agent_token = state.auth.issue_agent_token(&a.id);
    for (method, path, body) in [
        (reqwest::Method::POST, format!("/api/agents/{}/switch", a.id), json!({"provider": "claude-b"})),
        (reqwest::Method::PUT, "/api/agents/usage/claude".to_string(), json!({"limitedUntil": null})),
        (reqwest::Method::POST, "/api/agents/local-models".to_string(), json!({"server": "ollama"})),
    ] {
        let r = http.request(method, format!("{origin}{path}")).header("authorization", format!("Bearer {agent_token}")).json(&body).send().await.unwrap();
        assert!(matches!(r.status().as_u16(), 401 | 403), "{path}: {}", r.status());
    }
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_windows_and_a_refused_turn_mark_the_account() {
    use super::usage::Reported;
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, _accts) = three_accounts(dir.path(), Some("new")).await;
    let t = &state.terminals;
    // Any hosted session stands in for the Codex one: only its provider name is used.
    let a = t.spawn_agent(&state, ask(&pid, Some("claude-b"))).await.unwrap();
    let entry = t.get(&a.id).unwrap();
    let now = crate::util::now_ms();
    let window = |used: f64, minutes: i64| Reported { name: "primary", used_pct: used, window_minutes: Some(minutes), resets_at: Some(now + 3_600_000), resets_in_ms: None };

    t.note_codex_usage(&state, &entry, vec![window(40.0, 300)], None).await;
    assert!(t.usage.limited("claude-b", now).is_none());
    // A full window is a limit of its own, and the turn it refused is announced once.
    t.note_codex_usage(&state, &entry, vec![window(100.0, 300)], Some("You\u{2019}ve hit your usage limit. Try again later.".into())).await;
    let l = t.usage.limited("claude-b", now).expect("limited");
    assert_eq!(l.until, now + 3_600_000);
    assert_eq!(t.info(&a.id).unwrap().meta["limit"]["until"], l.until);
    // A limit that belongs to one named model leaves the account alone.
    t.usage.clear_limited("claude-b", now);
    t.note_codex_usage(&state, &entry, vec![], Some("You\u{2019}ve hit your usage limit for GPT-5-Codex. Switch to another model now.".into())).await;
    assert!(t.usage.limited("claude-b", now).is_none());
    let _ = t.kill(&a.id).await;
}
