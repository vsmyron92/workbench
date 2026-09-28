//! Push end to end: a real server (router, auth, the notifier's listener and the
//! queue) delivering to a local mock push service, which checks the VAPID
//! signature and headers and decrypts the payload with the device's key.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxPath, State as AxState};
use axum::http::{HeaderMap, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::SecretKey;
use p256::elliptic_curve::Generate;
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::*;
use crate::app::AppState;
use crate::config::Paths;

// ---------------------------------------------------------------- mock push service

#[derive(Clone, Debug)]
struct Received {
    id: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Clone, Default)]
struct MockState {
    received: Arc<Mutex<Vec<Received>>>,
    /// Answers to give, in order (status, Retry-After); 201 when empty.
    script: Arc<Mutex<VecDeque<(u16, Option<u64>)>>>,
}

struct Mock {
    addr: SocketAddr,
    st: MockState,
}

impl Mock {
    async fn start() -> Self {
        let st = MockState::default();
        let app = axum::Router::new()
            .route(
                "/push/{id}",
                axum::routing::post(|AxState(st): AxState<MockState>, AxPath(id): AxPath<String>, headers: HeaderMap, body: axum::body::Bytes| async move {
                    st.received.lock().push(Received { id, headers, body: body.to_vec() });
                    let (status, retry) = st.script.lock().pop_front().unwrap_or((201, None));
                    let mut resp = axum::response::IntoResponse::into_response(StatusCode::from_u16(status).unwrap());
                    if let Some(r) = retry {
                        resp.headers_mut().insert(header::RETRY_AFTER, r.to_string().parse().unwrap());
                    }
                    resp
                }),
            )
            .with_state(st.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Mock { addr, st }
    }

    fn script(&self, answers: &[(u16, Option<u64>)]) {
        self.st.script.lock().extend(answers.iter().copied());
    }

    fn received(&self) -> Vec<Received> {
        self.st.received.lock().clone()
    }

    async fn wait_for(&self, n: usize) -> Vec<Received> {
        for _ in 0..100 {
            let r = self.received();
            if r.len() >= n {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.received()
    }
}

/// A browser's side of a subscription.
struct Device {
    secret: SecretKey,
    auth: [u8; 16],
    endpoint: String,
}

impl Device {
    fn new(mock: &Mock, id: &str) -> Self {
        Device { secret: SecretKey::try_generate().unwrap(), auth: rand::random(), endpoint: format!("http://{}/push/{id}", mock.addr) }
    }
    fn subscription_json(&self) -> Value {
        json!({
            "endpoint": self.endpoint,
            "keys": { "p256dh": B64.encode(ece::uncompressed(&self.secret.public_key())), "auth": B64.encode(self.auth) },
        })
    }
    /// Decrypt what the mock received, as the browser would.
    fn open(&self, r: &Received) -> Value {
        let plain = ece::decrypt(&self.secret, &self.auth, &r.body).expect("decrypts with the device's key");
        serde_json::from_slice(&plain).unwrap()
    }
}

// ---------------------------------------------------------------- a served Workbench

struct Served {
    state: AppState,
    addr: SocketAddr,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

struct Session {
    cookie: String,
    key: String,
}

async fn served() -> Served {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths { config_dir: dir.path().join("config"), data_dir: dir.path().join("data") };
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots = vec![];
    cfg.notify.desktop = false;
    cfg.agents.restore_on_start = false;
    cfg.save(&paths).unwrap();
    let state = AppState::new(paths, cfg, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    crate::platform::start(&state).await;
    // Tests only: let the engine post to the mock on http://127.0.0.1.
    state.platform.push.engine().unwrap().test_mode.store(true, Ordering::Relaxed);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = crate::app::build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
    });
    Served { state, addr, http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap(), _dir: dir }
}

impl Served {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
    fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }
    fn engine(&self) -> Arc<Engine> {
        self.state.platform.push.engine().unwrap().clone()
    }

    /// Sign a browser in through `/auth?token=`.
    async fn sign_in(&self) -> Session {
        let r = self.http.get(self.url(&format!("/auth?token={}", self.state.auth.master_token()))).send().await.unwrap();
        let key = r.headers()[header::LOCATION].to_str().unwrap().strip_prefix("/#wbk=").unwrap().to_string();
        let cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
        Session { cookie, key }
    }

    async fn call(&self, s: &Session, method: reqwest::Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = self
            .http
            .request(method, self.url(path))
            .header("cookie", &s.cookie)
            .header("origin", self.origin())
            .header(crate::auth::KEY_HEADER, &s.key);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        let status = StatusCode::from_u16(r.status().as_u16()).unwrap();
        (status, r.json().await.unwrap_or(Value::Null))
    }

    async fn subscribe(&self, s: &Session, d: &Device) -> Value {
        let (status, body) = self.call(s, reqwest::Method::POST, "/api/push/subscriptions", Some(d.subscription_json())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    async fn push_info(&self, s: &Session) -> Value {
        self.call(s, reqwest::Method::GET, "/api/push", None).await.1
    }
}

fn note(topic: Topic, tag: &str, body: &str) -> PushNote {
    PushNote {
        topic,
        tag: tag.into(),
        title: "proj · Fix the login".into(),
        body: body.into(),
        level: "warning",
        urgency: Urgency::High,
        ttl_secs: 600,
        project_id: Some("proj".into()),
        open: Some(OpenTarget::terminal("t1")),
        agent: None,
    }
}

// ---------------------------------------------------------------- tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_push_is_encrypted_signed_and_decryptable() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    let device = Device::new(&mock, "phone");
    let sub = s.subscribe(&phone, &device).await;
    assert_eq!(sub["current"], true);
    assert_eq!(sub["service"], "127.0.0.1");
    assert!(sub.get("endpoint").is_none() && sub.get("p256dh").is_none(), "no endpoint or keys go back to the browser: {sub}");

    let info = s.push_info(&phone).await;
    let public_key = info["publicKey"].as_str().unwrap().to_string();
    assert_eq!(B64.decode(&public_key).unwrap().len(), 65);
    assert_eq!(info["subscriptions"].as_array().unwrap().len(), 1);

    let (status, out) = s.call(&phone, reqwest::Method::POST, "/api/push/test", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["results"][0]["outcome"], "sent", "{out}");

    let got = mock.wait_for(1).await;
    assert_eq!(got.len(), 1);
    let r = &got[0];
    assert_eq!(r.id, "phone");
    let h = |name: &str| r.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    assert_eq!(h("content-encoding"), "aes128gcm");
    assert_eq!(h("content-type"), "application/octet-stream");
    assert_eq!(h("ttl"), "120");
    assert_eq!(h("urgency"), "normal");
    let topic = h("topic");
    assert!(topic.len() <= 32 && topic.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{topic}");
    // RFC 8292: a JWT for this push service's origin, signed by the key devices got.
    let claims = vapid::verify_authorization(&h("authorization"), &public_key, &format!("http://{}", mock.addr), util::now_ms() / 1000).unwrap();
    assert!(claims.exp - util::now_ms() / 1000 <= 12 * 3600);
    assert_eq!(claims.sub, "mailto:workbench@localhost", "no https origin or public_url here");
    // RFC 8291: only the device decrypts it.
    assert_eq!(&r.body[16..20], &4096u32.to_be_bytes());
    let payload = device.open(r);
    assert_eq!(payload["title"], "Workbench");
    assert!(payload["body"].as_str().unwrap().contains("push notifications work"), "{payload}");
    assert_eq!(payload["open"]["kind"], "settings");
    assert_eq!(payload["tag"], "test");
    let other = Device::new(&mock, "x");
    assert!(ece::decrypt(&other.secret, &other.auth, &r.body).is_err());

    // Stored 0600 in data_dir/push.
    let dir = s.state.paths.data_dir.join("push");
    for f in ["subscriptions.json", "vapid.json"] {
        let mode = std::fs::metadata(dir.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{f}");
    }
    let stored = std::fs::read_to_string(dir.join("subscriptions.json")).unwrap();
    assert!(stored.contains("lastOkAt") && stored.contains(&device.endpoint));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribing_checks_the_caller_the_endpoint_and_the_keys() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    let d = Device::new(&mock, "p");
    let post = |body: Value| s.call(&phone, reqwest::Method::POST, "/api/push/subscriptions", Some(body));

    let mut bad = d.subscription_json();
    bad["endpoint"] = json!("http://10.0.0.1:7846/push/p");
    assert_eq!(post(bad).await.0, StatusCode::BAD_REQUEST, "http to a non-loopback host");
    let mut bad = d.subscription_json();
    bad["endpoint"] = json!("https://push.example.org/p");
    let (status, body) = post(bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("extra_endpoint_hosts"), "{body}");
    let mut bad = d.subscription_json();
    bad["keys"]["auth"] = json!(B64.encode([1u8; 8]));
    assert_eq!(post(bad).await.0, StatusCode::BAD_REQUEST, "short auth secret");
    let mut bad = d.subscription_json();
    let mut pk = ece::uncompressed(&d.secret.public_key());
    pk[40] ^= 0xff;
    bad["keys"]["p256dh"] = json!(B64.encode(pk));
    assert_eq!(post(bad).await.0, StatusCode::BAD_REQUEST, "a point off the curve");

    // The master token is not a device: nothing to tie the subscription to.
    let r = s
        .http
        .post(s.url("/api/push/subscriptions"))
        .header("authorization", format!("Bearer {}", s.state.auth.master_token()))
        .json(&d.subscription_json())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    // A cookie without the device key cannot write (another local port holding the cookie).
    let r = s.http.post(s.url("/api/push/subscriptions")).header("cookie", &phone.cookie).header("origin", s.origin()).json(&d.subscription_json()).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    assert!(s.engine().subs.read().is_empty());

    // Subscribing again replaces this device's subscription and keeps its topics.
    s.subscribe(&phone, &d).await;
    let id = s.engine().subs.read()[0].id.clone();
    let (status, _) = s
        .call(&phone, reqwest::Method::PATCH, &format!("/api/push/subscriptions/{id}"), Some(json!({ "topics": { "done": false }, "quietWhenActive": false })))
        .await;
    assert_eq!(status, StatusCode::OK);
    let d2 = Device::new(&mock, "p2");
    s.subscribe(&phone, &d2).await;
    let subs = s.engine().subs.read().clone();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, d2.endpoint);
    assert!(!subs[0].topics.done && subs[0].topics.attention && !subs[0].quiet_when_active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gone_subscriptions_are_dropped_and_errors_are_retried_then_recorded() {
    let mock = Mock::start().await;
    let s = served().await;
    let a = s.sign_in().await;
    let b = s.sign_in().await;
    let da = Device::new(&mock, "a");
    let db = Device::new(&mock, "b");
    s.subscribe(&a, &da).await;
    s.subscribe(&b, &db).await;
    let e = s.engine();
    let id_of = |ep: &str| e.subs.read().iter().find(|x| x.endpoint == ep).unwrap().id.clone();
    let (ida, idb) = (id_of(&da.endpoint), id_of(&db.endpoint));

    // 429 with Retry-After, then accepted.
    mock.script(&[(429, Some(1)), (201, None)]);
    let r = e.deliver(&s.state, note(Topic::Env, "env:p:prod", "down"), Some(&ida), true).await;
    assert_eq!(r[0].outcome, "sent", "{r:?}");
    assert_eq!(mock.received().len(), 2);

    // 5xx three times: given up, recorded, kept.
    mock.script(&[(503, Some(1)), (500, Some(1)), (502, Some(1))]);
    let r = e.deliver(&s.state, note(Topic::Env, "env:p:prod", "down"), Some(&ida), true).await;
    assert_eq!((r[0].outcome, r[0].status), ("failed", Some(502)), "{r:?}");
    assert_eq!(mock.received().len(), 5);
    let info = s.push_info(&a).await;
    let mine = info["subscriptions"].as_array().unwrap().iter().find(|x| x["id"] == ida.as_str()).unwrap().clone();
    assert!(mine["lastError"].as_str().unwrap().contains("503") || mine["lastError"].as_str().unwrap().contains("502"), "{mine}");

    // A VAPID refusal is not retried.
    mock.script(&[(403, None)]);
    let r = e.deliver(&s.state, note(Topic::Env, "env:p:prod", "down"), Some(&ida), true).await;
    assert_eq!(r[0].outcome, "failed");
    assert!(r[0].error.as_deref().unwrap().contains("VAPID"));
    assert_eq!(mock.received().len(), 6);

    // 410 Gone: the push service forgot it.
    mock.script(&[(410, None)]);
    let r = e.deliver(&s.state, note(Topic::Env, "env:p:prod", "down"), Some(&idb), true).await;
    assert_eq!(r[0].outcome, "gone");
    assert!(e.subs.read().iter().all(|x| x.id != idb));
    let stored = std::fs::read_to_string(s.state.paths.data_dir.join("push/subscriptions.json")).unwrap();
    assert!(!stored.contains(&db.endpoint));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_and_topics_decide_who_gets_a_push() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    let laptop = s.sign_in().await;
    let d = Device::new(&mock, "phone");
    s.subscribe(&phone, &d).await;
    let e = s.engine();
    async fn presence(s: &Served, who: &Session, visible: bool, active: bool) {
        let (status, _) = s.call(who, reqwest::Method::POST, "/api/push/presence", Some(json!({ "visible": visible, "active": active }))).await;
        assert_eq!(status, StatusCode::OK);
    }

    // The phone shows Workbench: no push.
    presence(&s, &phone, true, true).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(r[0].outcome, "skipped", "{r:?}");
    // Hidden again: pushed.
    presence(&s, &phone, false, false).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(r[0].outcome, "sent");
    // The laptop is in active use: the phone stays quiet…
    presence(&s, &laptop, true, true).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(r[0].outcome, "skipped");
    assert!(r[0].error.as_deref().unwrap().contains("another device"));
    // …but not when the laptop is merely open (nobody touched it for a while).
    presence(&s, &laptop, true, false).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(r[0].outcome, "sent");
    // Topics are per device.
    let id = e.subs.read()[0].id.clone();
    s.call(&phone, reqwest::Method::PATCH, &format!("/api/push/subscriptions/{id}"), Some(json!({ "topics": { "done": false } }))).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(r[0].outcome, "skipped");
    let r = e.deliver(&s.state, note(Topic::Attention, "agent:t1", "Needs your permission"), None, false).await;
    assert_eq!(r[0].outcome, "sent");
    assert_eq!(mock.received().len(), 3);
}

/// Every tab of a browser shares its device session: one tab going hidden (or
/// reloading, or closing) leaves the device present while another tab shows Workbench.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_is_per_tab() {
    let mock = Mock::start().await;
    let s = served().await;
    let desk = s.sign_in().await;
    let phone = s.sign_in().await;
    let (dd, dp) = (Device::new(&mock, "desk"), Device::new(&mock, "phone"));
    s.subscribe(&desk, &dd).await;
    s.subscribe(&phone, &dp).await;
    let e = s.engine();
    async fn report(s: &Served, who: &Session, tab: &str, visible: bool) {
        let body = json!({ "visible": visible, "active": visible, "tab": tab });
        let (status, _) = s.call(who, reqwest::Method::POST, "/api/push/presence", Some(body)).await;
        assert_eq!(status, StatusCode::OK);
    }
    let outcomes = |r: &[DeliveryResult]| {
        let mut v: Vec<(String, &'static str)> = r.iter().map(|x| (x.device.clone(), x.outcome)).collect();
        v.sort();
        v.into_iter().map(|(_, o)| o).collect::<Vec<_>>()
    };

    // Tab A shows Workbench on the desk and is in use; tab B of the same browser hides.
    report(&s, &desk, "tab-a", true).await;
    report(&s, &desk, "tab-b", true).await;
    report(&s, &desk, "tab-b", false).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(outcomes(&r), ["skipped", "skipped"], "the desk still looks, the phone holds: {r:?}");
    assert!(mock.received().is_empty());
    // Tab A hides too: both devices get it.
    report(&s, &desk, "tab-a", false).await;
    let r = e.deliver(&s.state, note(Topic::Done, "agent:t1", "Done."), None, false).await;
    assert_eq!(outcomes(&r), ["sent", "sent"], "{r:?}");
    // A page without a tab id (from before this change) is one tab of its own.
    report(&s, &desk, "tab-a", true).await;
    let (status, _) = s.call(&desk, reqwest::Method::POST, "/api/push/presence", Some(json!({ "visible": false }))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(e.visible(&e.subs.read().iter().find(|x| x.endpoint == dd.endpoint).unwrap().session_id));
    // Tab ids are bounded per device.
    let session = e.subs.read().iter().find(|x| x.endpoint == dd.endpoint).unwrap().session_id.clone();
    for i in 0..100 {
        e.report_presence(&session, &format!("t{i}"), true, false);
    }
    assert!(e.presence.lock()[&session].len() <= MAX_TABS);
}

/// A push service that answers 503 with `Retry-After` does not keep a send slot
/// while Workbench waits to try it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_waits_hold_no_send_slot() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    let d = Device::new(&mock, "phone");
    s.subscribe(&phone, &d).await;
    let e = s.engine();
    let id = e.subs.read()[0].id.clone();
    mock.script(&[(503, Some(2)), (201, None)]);
    let task = {
        let (e, state) = (e.clone(), s.state.clone());
        tokio::spawn(async move { e.deliver(&state, note(Topic::Env, "env:p:prod", "down"), Some(&id), true).await })
    };
    assert_eq!(mock.wait_for(1).await.len(), 1);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(mock.received().len(), 1, "still waiting for the retry");
    assert_eq!(e.sends.available_permits(), CONCURRENT_SENDS, "no slot held across the wait");
    let r = task.await.unwrap();
    assert_eq!(r[0].outcome, "sent", "{r:?}");
    assert_eq!(mock.received().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_a_device_session_drops_its_subscription() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    s.subscribe(&phone, &Device::new(&mock, "phone")).await;
    let session = s.engine().subs.read()[0].session_id.clone();
    let r = s
        .http
        .delete(s.url(&format!("/api/auth/devices/{session}")))
        .header("authorization", format!("Bearer {}", s.state.auth.master_token()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let file = s.state.paths.data_dir.join("push/subscriptions.json");
    for _ in 0..50 {
        if s.engine().subs.read().is_empty() && std::fs::read_to_string(&file).unwrap().trim() == "[]" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(s.engine().subs.read().is_empty(), "revoked with the session");
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "[]", "and saved");
}

/// The notifier's listener turns events into pushes (same events and limits as the
/// desktop), coalesced per tag.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_reach_devices_through_the_queue_coalesced() {
    let mock = Mock::start().await;
    let s = served().await;
    let phone = s.sign_in().await;
    let d = Device::new(&mock, "phone");
    s.subscribe(&phone, &d).await;

    s.state.events.emit("env.health", Some("shop"), json!({ "env": "production", "status": "down", "httpStatus": 502 }));
    let got = mock.wait_for(1).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].headers["urgency"], "high");
    let p = d.open(&got[0]);
    assert_eq!(p["topic"], "env");
    assert!(p["title"].as_str().unwrap().ends_with("production is down"), "{p}");
    assert!(p["body"].as_str().unwrap().contains("HTTP 502"));
    assert_eq!(p["projectId"], "shop");

    // Three notes with one tag inside the window: one push, the newest.
    for body in ["one", "two", "three"] {
        assert_eq!(s.state.platform.push.enqueue(note(Topic::Deploy, "deploy:t9", body)), "queued");
    }
    mock.wait_for(2).await;
    tokio::time::sleep(COALESCE + Duration::from_millis(300)).await;
    let got = mock.received();
    assert_eq!(got.len(), 2, "coalesced");
    assert_eq!(d.open(&got[1])["body"], "three");
}

// ---------------------------------------------------------------- agents

fn wait(state: &str, hint: Option<&str>) -> AgentWait {
    AgentWait { terminal_id: "t1".into(), state: state.into(), permission_hint: hint.map(str::to_string) }
}

fn pending(id: &str, tool: &str, summary: &str) -> PendingPermission {
    PendingPermission {
        id: id.into(),
        tool: tool.into(),
        summary: summary.into(),
        since: 1,
        session_rule: None,
        detail: String::new(),
        complete: false,
    }
}

/// A `TerminalInfo` as `terminal.updated` carries it, with the terminals'
/// `AgentInfo.pendingPermission`.
fn event_info(state: &str, perm: Option<(&str, &str, &str)>) -> Value {
    json!({
        "id": "t1",
        "agent": {
            "sessionId": "s1",
            "state": state,
            "unread": false,
            "remoteControl": false,
            "lastEventAt": 0,
            "pendingPermission": perm.map(|(id, tool, summary)| pending(id, tool, summary)),
        },
    })
}

/// The session's `AgentInfo`.
fn info(state: &str, perm: Option<(&str, &str, &str)>) -> Option<AgentInfo> {
    agent_of_event(&event_info(state, perm))
}

#[test]
fn agent_notes_go_out_only_while_the_agent_waits() {
    let perm = pending("p1", "Bash", "npm test");
    // Waiting on a permission Workbench can answer: Allow / Deny.
    assert_eq!(decide_agent(&wait("needs_permission", None), info("needs_permission", Some(("p1", "Bash", "npm test"))).as_ref(), None), Some(Some(perm.clone())));
    // Answered in the terminal meanwhile.
    assert_eq!(decide_agent(&wait("needs_permission", None), info("working", None).as_ref(), None), None);
    // Already pushed by the other path (attention event or terminal.updated).
    assert_eq!(decide_agent(&wait("needs_permission", Some("p1")), info("needs_permission", Some(("p1", "Bash", "x"))).as_ref(), Some("p1")), None);
    assert_eq!(decide_agent(&wait("needs_permission", None), info("needs_permission", Some(("p1", "Bash", "x"))).as_ref(), Some("p1")), None);
    // A new request after the last one was answered.
    assert!(decide_agent(&wait("needs_permission", Some("p2")), info("needs_permission", Some(("p2", "Edit", "src/main.rs"))).as_ref(), Some("p1")).is_some_and(|p| p.is_some()));
    // No request Workbench can answer: a plain notification.
    assert_eq!(decide_agent(&wait("needs_permission", None), info("needs_permission", None).as_ref(), None), Some(None));
    // A sighting whose request is gone.
    assert_eq!(decide_agent(&wait("needs_permission", Some("p1")), info("needs_permission", None).as_ref(), None), None);
    // Questions and finished turns carry no permission even if one is listed.
    assert_eq!(decide_agent(&wait("idle", None), info("idle", Some(("p1", "Bash", "x"))).as_ref(), None), Some(None));
    // No request, or no agent at all.
    assert_eq!(agent_wait_state(info("needs_input", None).as_ref()), (Some("needs_input".to_string()), None));
    assert!(agent_of_event(&json!({ "agent": null })).is_none());
    assert_eq!(agent_wait_state(None), (None, None));
}

#[test]
fn payloads_carry_the_permission_and_stay_small() {
    let perm = pending("perm-7", "Bash", "Permission to run `cargo publish --dry-run`");
    let mut n = note(Topic::Attention, "agent:t1", "Claude needs your permission to use Bash");
    n.agent = Some(wait("needs_permission", None));
    let v: Value = serde_json::from_slice(&Engine::payload(&n, Some(&perm), 5)).unwrap();
    assert_eq!(v["terminalId"], "t1");
    assert_eq!(v["permissionId"], "perm-7");
    assert_eq!(v["tool"], "Bash");
    // Not `complete`: the notification cannot show it whole, so no Allow.
    assert_eq!(v["body"], "Needs your permission\nBash: Permission to run `cargo publish --dry-run`");
    assert_eq!(v["allow"], false);
    // Whole and short: Allow, with the whole request in the body.
    let whole = PendingPermission { detail: "cargo publish --dry-run".into(), complete: true, ..perm.clone() };
    assert!(one_tap_allow(&whole));
    let v: Value = serde_json::from_slice(&Engine::payload(&n, Some(&whole), 5)).unwrap();
    assert_eq!(v["allow"], true);
    assert_eq!(v["body"], "Needs your permission · Bash\ncargo publish --dry-run");
    // Whole but longer than a notification shows (characters or lines): Deny and Review only.
    let long = PendingPermission { detail: "x".repeat(301), ..whole.clone() };
    assert!(!one_tap_allow(&long));
    let lines = PendingPermission { detail: "a\nb\nc\nd\ne".into(), ..whole.clone() };
    assert!(!one_tap_allow(&lines));
    assert!(one_tap_allow(&PendingPermission { detail: "a\nb\nc\nd".into(), ..whole.clone() }));
    let v: Value = serde_json::from_slice(&Engine::payload(&n, Some(&long), 5)).unwrap();
    assert_eq!(v["allow"], false);
    // Masked or cut (not `complete`), however short.
    assert!(!one_tap_allow(&PendingPermission { complete: false, ..whole.clone() }));
    assert_eq!(v["open"], json!({ "kind": "terminal", "id": "terminal:t1", "params": { "terminalId": "t1" } }));
    assert_eq!(v["topic"], "attention");
    // Long texts are cut, control characters dropped.
    let mut n = note(Topic::Done, "agent:t1", &"x\u{1b}[31m".repeat(5000));
    n.title = "t".repeat(5000);
    let raw = Engine::payload(&n, None, 5);
    assert!(raw.len() < 1200, "{}", raw.len());
    let v: Value = serde_json::from_slice(&raw).unwrap();
    assert!(!v["body"].as_str().unwrap().contains('\u{1b}'));
    assert_eq!(v["title"].as_str().unwrap().chars().count(), 100);
}

/// A permission request's push lives no longer than Workbench can answer it.
#[test]
fn permission_pushes_expire_with_the_request() {
    let now = 10_000_000;
    // The default wait (600 s), a request asked 100 s ago: 500 s left.
    assert_eq!(crate::config::GlobalConfig::default().agents.permission_wait, 600);
    assert_eq!(permission_ttl(3600, 600, Some(now - 100_000), now), 500);
    // Configured waits, within the terminals' bounds.
    assert_eq!(crate::terminals::permission_wait_secs(90), 90);
    assert_eq!(crate::terminals::permission_wait_secs(5), 30);
    assert_eq!(crate::terminals::permission_wait_secs(86400), 3600);
    assert_eq!(permission_ttl(3600, 3600, Some(now), now), 3600);
    // Never longer than the note's own TTL.
    assert_eq!(permission_ttl(120, 600, Some(now), now), 120);
    // Expired already: deliver now or not at all.
    assert_eq!(permission_ttl(3600, 600, Some(now - 700_000), now), 0);
    // No (or a nonsensical) `since`: the whole wait.
    assert_eq!(permission_ttl(3600, 600, None, now), 600);
    assert_eq!(permission_ttl(3600, 600, Some(now + 60_000), now), 600);
    // `since` is read from `pendingPermission`.
    let (_, perm) = agent_wait_state(info("needs_permission", Some(("p1", "Bash", "x"))).as_ref());
    assert_eq!(perm.unwrap().since, 1);
}

#[test]
fn subjects_and_topics() {
    assert!(valid_subject("mailto:me@example.com"));
    assert!(valid_subject("https://box.tail1234.ts.net"));
    assert!(!valid_subject("mailto:nobody"));
    assert!(!valid_subject("http://box.local"));
    assert!(!valid_subject("me@example.com"));
    let t = topic_header("agent:terminal-with-a-long-id-0123456789");
    assert_eq!(t.len(), 32);
    assert_ne!(t, topic_header("agent:other"));
    assert_eq!(b64_field("q+/w=="), b64_field("q-_w"));
}
