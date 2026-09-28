//! Route tests (no database needed: permissions, the overlay, secrets, failures),
//! and `against_a_real_postgres`, ignored unless `WORKBENCH_TEST_PG_URL` names a
//! throwaway server (`postgres://user:pw@127.0.0.1:55432/postgres`; its password
//! never appears in any answer or error).

use axum::http::Method;
use serde_json::{Value, json};

use crate::mcp::{McpCtx, call_api};

struct Setup {
    t: crate::platform::testutil::TestApp,
    pid: String,
    _repo: tempfile::TempDir,
}

/// A project whose repository defines a source (`repo_db`, password named `gitlab`,
/// a secret only config.toml has) and whose overlay defines `local` and the secrets.
async fn setup(overlay: &str) -> Setup {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".workbench.toml"),
        "[[database]]\nname = \"repo_db\"\nhost = \"127.0.0.1\"\nport = 1\nuser = \"app\"\npassword = \"gitlab\"\n",
    )
    .unwrap();
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots.clear();
    cfg.projects.include = vec![repo.path().display().to_string()];
    cfg.notify.desktop = false;
    cfg.secrets.insert("gitlab".into(), crate::config::SecretRef::Env("WB_TEST_GLOBAL_TOKEN_UNSET".into()));
    let t = crate::platform::testutil::app_with(cfg).await;
    let pid = t.state.projects.list()[0].id.clone();
    let path = t.state.paths.project_overlay(&pid);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::util::fs::write_atomic(&path, overlay.as_bytes(), 0o600).unwrap();
    t.state.projects.reload(&t.state).await;
    Setup { t, pid, _repo: repo }
}

/// A request as the signed-in user (the master token), through the router.
async fn call(s: &Setup, method: Method, path: &str, body: Option<Value>) -> Result<Value, crate::error::ApiError> {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(format!("/api/projects/{}/db{path}", s.pid))
        .header("host", "127.0.0.1:7999")
        .header("authorization", format!("Bearer {}", s.t.state.auth.master_token()))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let resp = s.t.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    if status.is_success() {
        return Ok(v);
    }
    let code: &'static str = match v["error"]["code"].as_str() {
        Some("not_configured") => "not_configured",
        Some("upstream") => "upstream",
        Some("forbidden") => "forbidden",
        Some("conflict") => "conflict",
        _ => "error",
    };
    Err(crate::error::ApiError::new(status, code, v["error"]["message"].as_str().unwrap_or_default()))
}

#[tokio::test]
async fn only_the_user_connects_and_the_overlay_is_edited_in_place() {
    // SAFETY: tests in this binary do not read this variable concurrently.
    unsafe { std::env::set_var("WB_TEST_DB_PW", "hunter2-db") };
    let s = setup("# mine\n[secrets]\ndbpw = { env = \"WB_TEST_DB_PW\" }\n\n[[database]]\nname = \"local\"\nhost = \"127.0.0.1\"\nport = 1\nuser = \"app\"\npassword = \"dbpw\"\n").await;

    let v = call(&s, Method::GET, "", None).await.unwrap();
    let names: Vec<&str> = v["sources"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["repo_db", "local"]);
    assert_eq!(v["sources"][0]["origin"], "repository");
    assert_eq!(v["sources"][1]["origin"], "overlay");
    assert_eq!(v["sources"][1]["password"], "dbpw", "the secret's name");
    assert!(!v.to_string().contains("hunter2"), "{v}");
    assert!(v["secretNames"].as_array().unwrap().iter().any(|n| n == "dbpw"));

    // Nothing answers on port 1: an upstream error that never carries the password.
    let e = call(&s, Method::POST, "/local/test", None).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 502, "{e:?}");
    assert!(!e.message.contains("hunter2"), "{e:?}");
    // The repository names `gitlab`, which only config.toml has: never used.
    let e = call(&s, Method::POST, "/repo_db/test", None).await.unwrap_err();
    assert_eq!(e.code, "not_configured", "{e:?}");
    assert!(e.message.contains("machine overlay"), "{e:?}");

    // Agents list, and do nothing else.
    let agent = McpCtx { terminal_id: Some("t1".into()), project_id: Some(s.pid.clone()) };
    for (m, path, body) in [
        (Method::POST, "/local/test", None),
        (Method::GET, "/local/schema", None),
        (Method::GET, "/local/table?schema=public&table=x", None),
        (Method::POST, "/local/query", Some(json!({ "sql": "select 1", "console": "c1" }))),
        (Method::POST, "/local/consoles/c1/cancel", None),
        (Method::PUT, "/_sources/x", Some(json!({ "source": { "name": "x" } }))),
        (Method::DELETE, "/_sources/local", None),
    ] {
        let e = call_api(&s.t.state, m, &format!("/api/projects/{}/db{path}", s.pid), body, &agent).await.unwrap_err();
        assert_eq!(e.status.as_u16(), 403, "{path}");
    }
    assert!(call_api(&s.t.state, Method::GET, &format!("/api/projects/{}/db", s.pid), None, &agent).await.is_ok());

    // Add, rename and remove through the overlay; comments stay, the file stays private.
    call(&s, Method::PUT, "/_sources/reports", Some(json!({ "source": { "name": "reports", "host": "db.example.com", "database": "r", "user": "ro", "url": "", "readOnly": true, "read_only": true } })))
        .await
        .unwrap();
    call(&s, Method::PUT, "/_sources/reporting", Some(json!({ "source": { "name": "reporting", "host": "db.example.com" }, "previousName": "reports" }))).await.unwrap();
    let path = s.t.state.paths.project_overlay(&s.pid);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# mine") && text.contains("name = \"reporting\"") && !text.contains("\"reports\""), "{text}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let e = call(&s, Method::PUT, "/_sources/bad", Some(json!({ "source": { "name": "bad", "password": "hunter2 plain" } }))).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 400, "a password value is refused");
    call(&s, Method::DELETE, "/_sources/reporting", None).await.unwrap();
    let e = call(&s, Method::DELETE, "/_sources/repo_db", None).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 409, "the repository's own entry");
    let v = call(&s, Method::GET, "", None).await.unwrap();
    assert_eq!(v["sources"].as_array().unwrap().len(), 2);

    let e = call(&s, Method::POST, "/local/query", Some(json!({ "sql": "select 1", "console": "_meta" }))).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 400, "the tree's session is not a console");
}

/// Everything against a real server. Run with a throwaway one:
/// `docker run -d --rm --name wb-pg-test -e POSTGRES_PASSWORD=… -p 127.0.0.1:55432:5432 postgres:16-alpine`,
/// then `WORKBENCH_TEST_PG_URL=postgres://postgres:…@127.0.0.1:55432/postgres cargo test against_a_real_postgres -- --ignored`.
#[tokio::test]
#[ignore = "needs WORKBENCH_TEST_PG_URL (a throwaway PostgreSQL)"]
async fn against_a_real_postgres() {
    let url = std::env::var("WORKBENCH_TEST_PG_URL").expect("WORKBENCH_TEST_PG_URL");
    let password = url.split_once("://").and_then(|(_, r)| r.split_once('@')).and_then(|(ui, _)| ui.split_once(':')).map(|(_, p)| p.to_string()).unwrap_or_default();
    let sslmode = std::env::var("WORKBENCH_TEST_PG_SSLMODE").unwrap_or_else(|_| "prefer".into());
    let s = setup(&format!(
        "[secrets]\npg = {{ env = \"WORKBENCH_TEST_PG_URL\" }}\n\n[[database]]\nname = \"t\"\nurl = \"pg\"\nsslmode = \"{sslmode}\"\n\n[[database]]\nname = \"ro\"\nurl = \"pg\"\nread_only = true\nsslmode = \"{sslmode}\"\n\n[[database]]\nname = \"vf\"\nurl = \"pg\"\nsslmode = \"verify-full\"\n"
    ))
    .await;
    let clean = |v: &Value| assert!(password.is_empty() || !v.to_string().contains(&password), "the password leaked: {v}");
    let q = |console: &'static str, sql: String, max: Option<usize>| {
        let s = &s;
        async move {
            let mut body = json!({ "sql": sql, "console": console });
            if let Some(m) = max {
                body["maxRows"] = json!(m);
            }
            call(s, Method::POST, "/t/query", Some(body)).await.unwrap()
        }
    };

    if std::env::var("WORKBENCH_TEST_PG_SELFSIGNED").is_ok() {
        // verify-full checks the certificate: a self-signed one is refused.
        let e = call(&s, Method::POST, "/vf/test", None).await.unwrap_err();
        assert_eq!(e.status.as_u16(), 502, "{e:?}");
        assert!(e.message.to_lowercase().contains("certificate"), "{e:?}");
        assert!(password.is_empty() || !e.message.contains(&password));
    }
    let v = call(&s, Method::POST, "/t/test", None).await.unwrap();
    assert!(v["version"].as_str().unwrap().starts_with("PostgreSQL"), "{v}");
    if sslmode == "require" {
        assert_eq!(v["ssl"], true, "{v}");
    }
    clean(&v);

    let schema = format!("wbtest_{}", uuid::Uuid::new_v4().simple().to_string().get(..8).unwrap());
    let v = q("c1", format!("CREATE SCHEMA {schema}; CREATE TABLE {schema}.item (id int PRIMARY KEY, name text NOT NULL DEFAULT 'x'); INSERT INTO {schema}.item SELECT g, 'n' || g FROM generate_series(1, 1200) g"), None).await;
    assert!(v["error"].is_null(), "{v}");
    assert_eq!(v["results"].as_array().unwrap().len(), 3, "one result per statement: {v}");
    assert_eq!(v["results"][2]["rowsAffected"], 1200);

    // The cap: 500 rows by default, the query stopped there, not an error.
    let v = q("c1", format!("SELECT id, name, NULL AS nothing FROM {schema}.item ORDER BY id"), None).await;
    assert!(v["error"].is_null() && v["stopped"] == true, "{v}");
    let r = &v["results"][0];
    assert_eq!(r["columns"], json!(["id", "name", "nothing"]));
    assert_eq!(r["rows"].as_array().unwrap().len(), 500);
    assert_eq!((r["truncated"].clone(), r["rows"][0].clone()), (json!(true), json!(["1", "n1", null])));
    let v = q("c1", format!("SELECT count(*) FROM {schema}.item"), Some(10)).await;
    assert_eq!(v["results"][0]["rows"][0][0], "1200", "the session survives the cancel: {v}");
    // A huge result is cancelled after the drain window (rows), a slow one after its time;
    // neither cancel reaches the next query.
    let v = q("c1", "SELECT g FROM generate_series(1, 5000000) g".into(), Some(10)).await;
    assert!(v["error"].is_null() && v["stopped"] == true, "{v}");
    assert_eq!(v["results"][0]["rows"].as_array().unwrap().len(), 10);
    let t = std::time::Instant::now();
    let v = q("c1", "SELECT g, pg_sleep(0.02) FROM generate_series(1, 1000) g".into(), Some(5)).await;
    assert!(v["error"].is_null() && v["stopped"] == true, "{v}");
    // 1000 × 20 ms runs 20 s; the server sends rows in 8 KB batches (the first after
    // ~8 s here), and the cancel follows the cap by the 2 s drain window.
    assert!(t.elapsed() < std::time::Duration::from_secs(16), "cancelled after the drain time, not run to the end: {:?}", t.elapsed());
    for _ in 0..3 {
        let v = q("c1", "SELECT 1".into(), None).await;
        assert!(v["error"].is_null(), "a late cancel hit the next query: {v}");
    }

    // Errors with SQLSTATE and position; notices; a session keeps its state.
    let v = q("c1", "SELECT nope FROM pg_class".into(), None).await;
    assert_eq!(v["error"]["code"], "42703", "{v}");
    assert_eq!(v["error"]["position"], 8);
    let v = q("c1", "DO $$ BEGIN RAISE NOTICE 'hello %', 42; END $$".into(), None).await;
    assert_eq!(v["notices"], json!(["NOTICE: hello 42"]), "{v}");
    q("c1", "SET application_name = 'wb-console'".into(), None).await;
    let v = q("c1", "SHOW application_name".into(), None).await;
    assert_eq!(v["results"][0]["rows"][0][0], "wb-console");
    let v = q("c2", "SHOW application_name".into(), None).await;
    assert_eq!(v["results"][0]["rows"][0][0], "Workbench", "another console, another session");
    q("c1", format!("BEGIN; DELETE FROM {schema}.item WHERE id > 10"), None).await;
    let v = q("c2", format!("SELECT count(*) FROM {schema}.item"), None).await;
    assert_eq!(v["results"][0]["rows"][0][0], "1200", "uncommitted in c1");
    q("c1", "ROLLBACK".into(), None).await;

    // Cancel from another request.
    let slow = {
        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .method(Method::POST)
            .uri(format!("/api/projects/{}/db/t/query", s.pid))
            .header("host", "127.0.0.1:7999")
            .header("authorization", format!("Bearer {}", s.t.state.auth.master_token()))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(json!({ "sql": "SELECT pg_sleep(30)", "console": "c3" }).to_string()))
            .unwrap();
        let router = s.t.router.clone();
        tokio::spawn(async move {
            let resp = router.oneshot(req).await.unwrap();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let busy = call(&s, Method::POST, "/t/query", Some(json!({ "sql": "select 1", "console": "c3" }))).await.unwrap_err();
    assert_eq!(busy.status.as_u16(), 409);
    let c = call(&s, Method::POST, "/t/consoles/c3/cancel", None).await.unwrap();
    assert_eq!(c["running"], true);
    let v = tokio::time::timeout(std::time::Duration::from_secs(10), slow).await.unwrap().unwrap();
    assert_eq!(v["error"]["code"], "57014", "{v}");

    // The read-only source refuses writes (a guard, not a permission).
    let v = call(&s, Method::POST, "/ro/query", Some(json!({ "sql": format!("INSERT INTO {schema}.item VALUES (0)"), "console": "r" }))).await.unwrap();
    assert_eq!(v["error"]["code"], "25006", "{v}");

    // The tree.
    let v = call(&s, Method::GET, "/t/schema", None).await.unwrap();
    let sch = v["schemas"].as_array().unwrap().iter().find(|x| x["name"] == schema.as_str()).unwrap().clone();
    assert_eq!(sch["relations"][0]["name"], "item");
    assert_eq!(sch["relations"][0]["kind"], "table");
    let v = call(&s, Method::GET, &format!("/t/table?schema={schema}&table=item"), None).await.unwrap();
    assert_eq!(v["columns"][0]["name"], "id");
    assert_eq!(v["columns"][0]["primaryKey"], true);
    assert_eq!(v["columns"][1]["dataType"], "text");
    assert_eq!(v["columns"][1]["nullable"], false);
    assert_eq!(v["columns"][1]["default"], "'x'::text");
    assert!(v["indexes"][0]["definition"].as_str().unwrap().contains("UNIQUE INDEX"), "{v}");
    clean(&v);

    let v = q("c1", format!("DROP SCHEMA {schema} CASCADE"), None).await;
    assert!(v["error"].is_null(), "{v}");
    call(&s, Method::DELETE, "/t/consoles/c1", None).await.unwrap();
}
