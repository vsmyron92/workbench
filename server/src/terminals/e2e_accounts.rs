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
        c.local = Some(LocalModelConfig { server: "ollama".into(), url: "http://127.0.0.1:9".into(), context: Some(32768) });
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
    for want in ["ANTHROPIC_BASE_URL=http://127.0.0.1:9", "ANTHROPIC_AUTH_TOKEN=ollama", "ANTHROPIC_API_KEY=<unset>", "ANTHROPIC_MODEL=qwen3-coder", "ANTHROPIC_DEFAULT_HAIKU_MODEL=qwen3-coder", "CLAUDE_CODE_SUBAGENT_MODEL=qwen3-coder", "CLAUDE_CODE_MAX_CONTEXT_TOKENS=32768"] {
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

// ---------------------------------------------------------------- conversation transfer

use super::conversation::Transfer;
use super::transcript;

/// Claude Code and a second account of it, and a Codex account, all fakes with their own folders and logs.
async fn transfer_accounts(dir: &Path) -> (AppState, std::net::SocketAddr, String, Vec<Account>, PathBuf) {
    let fake = fake_cli(dir, "claude");
    let (a, ca) = account(dir, &fake, "claude", |c| {
        c.kind = None;
        c.fallback = vec!["claude-b".into(), "codex-x".into()];
    });
    let (b, cb) = account(dir, &fake, "claude-b", |_| {});
    let fake_codex = fake_cli(dir, "codex");
    let codex_home = dir.join("codex-home");
    std::fs::create_dir_all(&codex_home).unwrap();
    let codex_log = dir.join("codex.log");
    let cx = ProviderConfig {
        kind: Some("codex".into()),
        command: Some(fake_codex.display().to_string()),
        env: [
            ("CODEX_HOME".to_string(), codex_home.display().to_string()),
            ("FAKE_CODEX_LOG".to_string(), codex_log.display().to_string()),
            ("FAKE_CODEX_TURN_SECS".to_string(), "0.2".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let (state, addr, pid) = served_with(dir, &fake, |cfg| {
        for (id, c) in [("claude", ca), ("claude-b", cb), ("codex-x", cx)] {
            cfg.agents.providers.insert(id.into(), c);
        }
    })
    .await;
    (state, addr, pid, vec![a, b], codex_log)
}

/// A Claude Code transcript of `turns` (user, assistant, user, …) where `claude --resume` of the account at `home` looks for it.
fn write_transcript(home: &Path, cwd: &str, id: &str, turns: &[String]) -> PathBuf {
    let path = transcript::transcript_path(home, Path::new(cwd), id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = String::new();
    for (i, t) in turns.iter().enumerate() {
        let rec = if i % 2 == 0 {
            json!({"type": "user", "isSidechain": false, "message": {"role": "user", "content": t}, "sessionId": id})
        } else {
            json!({"type": "assistant", "isSidechain": false, "message": {"role": "assistant", "content": [{"type": "text", "text": t}]}, "sessionId": id})
        };
        out.push_str(&rec.to_string());
        out.push('\n');
    }
    std::fs::write(&path, out).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_resumes_on_another_account_of_the_same_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, accts, _codex) = transfer_accounts(dir.path()).await;
    let t = &state.terminals;
    let a = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    let sid = agent_of(&state, &a.id).session_id;
    let cwd = t.info(&a.id).unwrap().cwd;
    let src = write_transcript(&dir.path().join("home-claude"), &cwd, &sid, &["rename the module".into(), "Renamed it.".into(), "and the tests".into()]);

    let moved = t.switch_account(&state, &a.id, Some("claude-b"), None).await.unwrap();
    // The other account's folder now holds the conversation, and the new session resumes it.
    let dest = transcript::transcript_path(&dir.path().join("home-claude-b"), Path::new(&cwd), &sid);
    assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&src).unwrap());
    assert_eq!(moved.meta["transfer"]["mode"], "resume");
    assert_eq!(moved.agent.as_ref().map(|g| (g.provider_id.as_deref(), g.session_id.as_str())), Some((Some("claude-b"), sid.as_str())));
    wait_long("its start", 10, || !log_lines(&accts[1].log, "ARGS ").is_empty()).await;
    let args = log_lines(&accts[1].log, "ARGS ")[0].clone();
    assert!(args.contains(&format!("--resume {sid}")) && args.contains("same conversation"), "{args}");
    assert!(!args.contains("--session-id"), "{args}");
    assert_eq!(t.info(&a.id).unwrap().meta["movedTo"], moved.id.as_str());
    // The old session keeps its own file.
    assert!(src.is_file());
    // The same move again finds the session already there.
    assert_eq!(t.switch_account(&state, &a.id, Some("claude-b"), None).await.unwrap().id, moved.id);
    // A conversation the target already runs is not copied over.
    t.update(&t.get(&a.id).unwrap(), |rec| {
        rec.info.meta.as_object_mut().unwrap().remove("movedTo");
        true
    });
    let err = t.switch_account(&state, &a.id, Some("claude-b"), None).await.unwrap_err();
    assert!(err.to_string().contains("already runs this conversation"), "{err}");
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_goes_to_another_cli_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _addr, pid, _accts, codex_log) = transfer_accounts(dir.path()).await;
    let t = &state.terminals;

    // A long one: a file the new session is allowed to read.
    let a = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    let sid = agent_of(&state, &a.id).session_id;
    let cwd = t.info(&a.id).unwrap().cwd;
    let turns: Vec<String> = (0..40).map(|i| format!("{} {i} {}", if i % 2 == 0 { "question" } else { "answer" }, "word ".repeat(400))).collect();
    write_transcript(&dir.path().join("home-claude"), &cwd, &sid, &turns);
    let moved = t.switch_account(&state, &a.id, Some("codex-x"), None).await.unwrap();
    assert_eq!(moved.meta["transfer"]["mode"], "digest");
    assert!(moved.meta["transfer"]["turns"].as_u64().unwrap() < moved.meta["transfer"]["of"].as_u64().unwrap(), "{}", moved.meta["transfer"]);
    wait_long("codex to start", 10, || std::fs::read_to_string(&codex_log).is_ok_and(|l| l.contains("conversation.md"))).await;
    let line = std::fs::read_to_string(&codex_log).unwrap();
    let file = line.split_whitespace().find(|w| w.ends_with("conversation.md")).expect("the file in the prompt").to_string();
    let dir_of_file = Path::new(&file).parent().unwrap().display().to_string();
    assert!(line.contains(&format!("--add-dir {dir_of_file}")), "the new session may read it: {line}");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("## User\nquestion 0 ") && text.contains("answer 39 ") && text.contains("earlier turns left out"), "{}", &text[..300.min(text.len())]);
    assert!(text.len() <= conversation_budget() + 2000, "{}", text.len());
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777, 0o600);

    // A short one: in the prompt itself, no file.
    std::fs::write(&codex_log, "").unwrap();
    let b = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the second session", 10, || agent_of(&state, &b.id).state == AgentState::Idle).await;
    let sid_b = agent_of(&state, &b.id).session_id;
    write_transcript(&dir.path().join("home-claude"), &cwd, &sid_b, &["rename the module".into(), "Renamed it.".into()]);
    let moved = t.switch_account(&state, &b.id, Some("codex-x"), None).await.unwrap();
    assert_eq!(moved.meta["transfer"]["mode"], "digest");
    wait_long("codex to start again", 10, || std::fs::read_to_string(&codex_log).is_ok_and(|l| l.contains("rename the module"))).await;
    let line = std::fs::read_to_string(&codex_log).unwrap();
    assert!(line.contains("Renamed it.") && !line.contains("conversation.md"), "{line}");
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

fn conversation_budget() -> usize {
    super::conversation::FILE_BUDGET
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notes_only_moves_no_conversation_and_a_bad_setting_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (state, addr, pid, accts, _codex) = transfer_accounts(dir.path()).await;
    let t = &state.terminals;
    let a = t.spawn_agent(&state, ask(&pid, Some("claude"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    let sid = agent_of(&state, &a.id).session_id;
    let cwd = t.info(&a.id).unwrap().cwd;
    write_transcript(&dir.path().join("home-claude"), &cwd, &sid, &["secret plans".into(), "ok".into()]);

    let moved = t.switch_account(&state, &a.id, Some("claude-b"), Some(Transfer::Notes)).await.unwrap();
    assert_eq!(moved.meta["transfer"]["mode"], "notes");
    assert!(!transcript::transcript_path(&dir.path().join("home-claude-b"), Path::new(&cwd), &sid).exists(), "nothing was copied");
    wait_long("its start", 10, || !log_lines(&accts[1].log, "ARGS ")[..].is_empty()).await;
    let args = log_lines(&accts[1].log, "ARGS ")[0].clone();
    assert!(!args.contains("--resume") && !args.contains("secret plans"), "{args}");

    // The REST call refuses a transfer it does not know.
    let (cookie, key) = sign_in(&state, addr).await;
    let origin = format!("http://{addr}");
    let r = reqwest::Client::new()
        .post(format!("{origin}/api/agents/{}/switch", a.id))
        .header("cookie", &cookie)
        .header("origin", &origin)
        .header(crate::auth::KEY_HEADER, &key)
        .json(&json!({"provider": "claude-b", "transfer": "everything"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

// ---------------------------------------------------------------- hosted APIs

use crate::config::global::ApiConfig;
use crate::config::project::SecretRef;

const DEEPSEEK_KEY: &str = "sk-test-0123456789abcdefghijklmnopqrstuvwxyz";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hosted_api_key_comes_from_secrets_reaches_only_the_environment_and_is_masked() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "claude");
    // The key lives in a private file the config only names.
    let key_file = dir.path().join("deepseek.key");
    std::fs::write(&key_file, format!("{DEEPSEEK_KEY}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&key_file, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let (ds, c) = account(dir.path(), &fake, "claude-ds", |c| {
        c.model = Some("deepseek-v4-pro".into());
        c.api = Some(ApiConfig { service: "deepseek".into(), url: String::new(), key: "deepseek".into(), context: Some(131072) });
    });
    let (missing, cm) = account(dir.path(), &fake, "claude-nokey", |c| {
        c.model = Some("m".into());
        c.api = Some(ApiConfig { service: "deepseek".into(), url: String::new(), key: "not-defined".into(), context: None });
    });
    let _ = &missing;
    let (state, addr, pid) = served_with(dir.path(), &fake, |cfg| {
        cfg.secrets.insert("deepseek".into(), SecretRef::File(key_file.display().to_string()));
        cfg.agents.providers.insert("claude-ds".into(), c);
        cfg.agents.providers.insert("claude-nokey".into(), cm);
    })
    .await;
    let t = &state.terminals;

    let a = t.spawn_agent(&state, ask(&pid, Some("claude-ds"))).await.unwrap();
    wait_long("the session", 10, || agent_of(&state, &a.id).state == AgentState::Idle).await;
    let env = env_line(&ds.log);
    // In the environment of the process, as the service wants it…
    for want in [
        format!("ANTHROPIC_AUTH_TOKEN={DEEPSEEK_KEY}"),
        "ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic".to_string(),
        "ANTHROPIC_MODEL=deepseek-v4-pro".to_string(),
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS=131072".to_string(),
    ] {
        assert!(env.contains(&want), "{want} in {env}");
    }
    // …and nowhere else: not the command line, the record, or the logged argv.
    let info = t.info(&a.id).unwrap();
    assert!(!serde_json::to_string(&info).unwrap().contains(DEEPSEEK_KEY));
    assert!(!log_lines(&ds.log, "ARGS ")[0].contains(DEEPSEEK_KEY));
    assert!(!info.agent.as_ref().unwrap().remote_control, "Remote Control needs claude.ai");

    // A session that prints the key does not show it.
    t.send_text(&a.id, &format!("echo {DEEPSEEK_KEY}"), true).await.unwrap();
    wait_long("the dialog", 10, || t.screen_text(&a.id, 30).is_some_and(|s| s.contains("echo")) ).await;
    let screen = t.screen_text(&a.id, 30).unwrap();
    assert!(!screen.contains(DEEPSEEK_KEY) && !screen.contains("0123456789abcdef"), "{screen}");
    assert!(screen.contains("echo ••••"), "the key was printed, and masked: {screen}");

    // What the UI is told names the secret, never its value.
    let (cookie, key) = sign_in(&state, addr).await;
    let origin = format!("http://{addr}");
    let get = |path: String| {
        reqwest::Client::new().get(format!("{origin}{path}")).header("cookie", &cookie).header("origin", &origin).header(crate::auth::KEY_HEADER, &key).send()
    };
    let defaults = get(format!("/api/agents/defaults?projectId={pid}")).await.unwrap().text().await.unwrap();
    let settings = get("/api/settings".to_string()).await.unwrap().text().await.unwrap();
    for body in [&defaults, &settings] {
        assert!(!body.contains(DEEPSEEK_KEY) && body.contains("\"deepseek\""));
    }
    let d: Value = serde_json::from_str(&defaults).unwrap();
    let p = d["providers"].as_array().unwrap().iter().find(|p| p["id"] == "claude-ds").unwrap();
    assert_eq!((p["api"]["key"].as_str(), p["api"]["serviceLabel"].as_str(), p["apiError"].is_null()), (Some("deepseek"), Some("DeepSeek"), true));

    // A key that is not defined stops the start, naming the secret and nothing else.
    let err = t.spawn_agent(&state, ask(&pid, Some("claude-nokey"))).await.err().expect("no key, no session");
    assert!(err.to_string().contains("not-defined") && err.to_string().contains("API key"), "{err}");
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}

// ---------------------------------------------------------------- signing an account in

/// A Claude Code account on the fake CLI whose folder does not exist yet: a sign-in makes it.
fn signin_config(dir: &Path, fake: &Path, id: &str, label: &str) -> (ProviderConfig, PathBuf, PathBuf) {
    let home = dir.join(format!("login-{id}"));
    let log = dir.join(format!("{id}.log"));
    let c = ProviderConfig {
        kind: Some("claude".into()),
        command: Some(fake.display().to_string()),
        label: Some(label.into()),
        env: [("CLAUDE_CONFIG_DIR".to_string(), home.display().to_string()), ("FAKE_CLAUDE_LOG".to_string(), log.display().to_string())].into(),
        ..Default::default()
    };
    (c, home, log)
}

/// Run the fake once so it is not busy when the server starts it: a script written a moment ago can be
/// held open by a child another test forked meanwhile, and starting it then fails with ETXTBSY.
#[cfg(unix)]
fn warm_up(fake: &Path, scratch: &Path) {
    for _ in 0..200 {
        match std::process::Command::new(fake).args(["auth", "status"]).env("CLAUDE_CONFIG_DIR", scratch).output() {
            Err(e) if e.raw_os_error() == Some(26) => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => return,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_is_signed_in_from_a_terminal_and_the_status_follows() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_cli(dir.path(), "claude");
    #[cfg(unix)]
    warm_up(&fake, &dir.path().join("warm-up"));
    // The default account is signed in already; the second has no folder yet.
    let (mut default, default_home, _) = signin_config(dir.path(), &fake, "claude", "Claude Code");
    default.kind = None;
    std::fs::create_dir_all(&default_home).unwrap();
    std::fs::write(default_home.join("signed-in"), "").unwrap();
    let (work, work_home, work_log) = signin_config(dir.path(), &fake, "claude-work", "Claude · Work");
    let (mut local, ..) = signin_config(dir.path(), &fake, "claude-local", "Local");
    local.model = Some("qwen3-coder".into());
    local.local = Some(LocalModelConfig { server: "ollama".into(), url: "http://127.0.0.1:9".into(), context: None });
    let (state, addr, _pid) = served_with(dir.path(), &fake, |cfg| {
        for (id, c) in [("claude", default), ("claude-work", work), ("claude-local", local)] {
            cfg.agents.providers.insert(id.into(), c);
        }
        // The other CLIs are not here: never ask a real one of the computer the test runs on.
        for id in ["codex", "kimi", "gemini", "aider"] {
            let missing = dir.path().join("missing").join(id).display().to_string();
            cfg.agents.providers.insert(id.into(), ProviderConfig { command: Some(missing), ..Default::default() });
        }
    })
    .await;
    let t = &state.terminals;
    let (cookie, key) = sign_in(&state, addr).await;
    let http = reqwest::Client::new();
    let origin = format!("http://{addr}");
    let call = |method: reqwest::Method, path: &str| {
        http.request(method, format!("{origin}{path}")).header("cookie", &cookie).header("origin", &origin).header(crate::auth::KEY_HEADER, &key).json(&json!({})).send()
    };
    let get = |path: &'static str| async move { call(reqwest::Method::GET, path).await.unwrap().json::<Value>().await.unwrap() };

    // Who can say, and what they say. A model of your own has no login, and a CLI that is not
    // installed has no answer (not "signed out").
    let s = get("/api/agents/signin").await;
    let mut names: Vec<&String> = s["accounts"].as_object().unwrap().keys().collect();
    names.sort();
    assert_eq!(names, ["claude", "claude-work"]);
    assert_eq!(
        (s["accounts"]["claude"]["state"].clone(), s["accounts"]["claude"]["method"].clone(), s["accounts"]["claude"]["plan"].clone()),
        (json!("signedIn"), json!("Claude subscription"), json!("max"))
    );
    assert_eq!(s["accounts"]["claude-work"]["state"], "signedOut");
    assert!(s["accounts"]["claude-work"].get("method").is_none());
    assert!(s["accounts"]["claude-work"]["checkedAt"].as_i64().unwrap() > 0);
    // The email the CLI prints is not passed on.
    assert!(!s.to_string().contains("person@example.com"), "{s}");

    // Sign the work account in: a terminal runs the CLI's own login, with that account's folder.
    let r = call(reqwest::Method::POST, "/api/agents/signin/claude-work").await.unwrap();
    assert_eq!(r.status(), 200);
    let term: Value = r.json().await.unwrap();
    let id = term["id"].as_str().unwrap().to_string();
    assert_eq!(term["kind"], "command");
    assert_eq!(term["title"], "Sign in · Claude · Work");
    assert_eq!(term["meta"], json!({"signIn": true, "provider": "claude-work", "restartable": true}));
    let argv: Vec<&str> = term["argv"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(argv[argv.len() - 2..], ["auth", "login"]);
    assert!(work_home.is_dir(), "the account's folder is made, for a CLI that refuses a missing one");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&work_home).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let logins = || log_lines(&work_log, "AUTH login");
    wait_long("the login to start", 10, || logins().len() == 1).await;
    assert_eq!(logins()[0], format!("AUTH login config={}", work_home.display()), "the login ran for the account's folder, not the default one");
    // A second click shows the sign-in that is open.
    let again: Value = call(reqwest::Method::POST, "/api/agents/signin/claude-work").await.unwrap().json().await.unwrap();
    assert_eq!(again["id"], id.as_str());
    assert_eq!(logins().len(), 1);
    // The CLI waits for the code the person pastes; then it ends, and the account is signed in.
    assert!(!work_home.join("signed-in").exists());
    t.send_text(&id, "code-123", true).await.unwrap();
    wait_long("the login to end", 10, || t.info(&id).is_some_and(|i| i.status == super::TerminalStatus::Exited)).await;
    assert_eq!(t.info(&id).unwrap().exit.and_then(|e| e.code), Some(0));
    let s: Value = get("/api/agents/signin?refresh=1").await;
    assert_eq!(s["accounts"]["claude-work"]["state"], "signedIn");
    assert_eq!(s["accounts"]["claude-work"]["method"], "Claude subscription");
    assert_eq!(s["accounts"]["claude"]["state"], "signedIn");
    assert!(!s.to_string().contains("person@example.com"), "{s}");

    // Restarting the sign-in runs it again for the same account, not the default one.
    let r = call(reqwest::Method::POST, &format!("/api/terminals/{id}/restart")).await.unwrap();
    assert_eq!(r.status(), 200);
    wait_long("the login to start again", 10, || logins().len() == 2).await;
    assert_eq!(logins()[1], logins()[0]);
    t.send_text(&id, "code-456", true).await.unwrap();
    wait_long("the second login to end", 10, || t.info(&id).is_some_and(|i| i.status == super::TerminalStatus::Exited)).await;
    // Once it has ended a click starts a new one.
    let third: Value = call(reqwest::Method::POST, "/api/agents/signin/claude-work").await.unwrap().json().await.unwrap();
    assert_ne!(third["id"], id.as_str());

    // What cannot be signed in to says why.
    let refused = |path: &'static str, status: u16, word: &'static str| {
        let call = &call;
        async move {
            let r = call(reqwest::Method::POST, path).await.unwrap();
            assert_eq!(r.status().as_u16(), status, "{path}");
            let body = r.text().await.unwrap();
            assert!(body.contains(word), "{path}: {body}");
        }
    };
    refused("/api/agents/signin/claude-local", 400, "model server").await;
    refused("/api/agents/signin/aider", 400, "no login").await;
    refused("/api/agents/signin/gemini", 412, "was not found").await;
    refused("/api/agents/signin/ghost", 412, "no agent provider").await;

    // An agent's own credentials may not sign accounts in or ask about them.
    let agent_token = state.auth.issue_agent_token(&id);
    for (method, path) in [(reqwest::Method::GET, "/api/agents/signin"), (reqwest::Method::POST, "/api/agents/signin/claude-work")] {
        let r = http.request(method, format!("{origin}{path}")).header("authorization", format!("Bearer {agent_token}")).json(&json!({})).send().await.unwrap();
        assert!(matches!(r.status().as_u16(), 401 | 403), "{path}: {}", r.status());
    }
    for i in t.list() {
        let _ = t.kill(&i.id).await;
    }
}
