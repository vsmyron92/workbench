//! HTTP Client (JetBrains' `.http` / `.rest` files): parse a file, fill in
//! `{{variables}}` from `http-client.env.json` and `http-client.private.env.json`
//! (the nearest ones from the file's folder up to the project root; the private one
//! is a sensitive file, so its values never go to the browser), run one request on
//! the user's click, and answer with the response. Values from the private file are
//! masked in the request shown back.
//!
//! Supported: `###` separators (with a name), `# @name`, `@var = value` file
//! variables, `METHOD URL [HTTP/x]` (GET when only a URL), indented `?`/`&`
//! continuation lines, headers, a body (inline, or `< ./file`), comments `#`/`//`,
//! dynamic variables (`$uuid`, `$timestamp`, `$isoTimestamp`, `$randomInt`, and the
//! `$random.uuid` spelling). Response handler scripts (`> {% … %}`) and output
//! redirects (`>> file`) are skipped, not run.
//!
//! Routes: `GET …/http/envs?path=`, `POST …/http/run {path, line, env?}`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::files::Sensitive;

const PUBLIC_ENV: &str = "http-client.env.json";
const PRIVATE_ENV: &str = "http-client.private.env.json";
const MAX_FILE: u64 = 1024 * 1024;
const MAX_BODY_FILE: u64 = 16 * 1024 * 1024;
const MAX_RESPONSE: usize = 5 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);
const MASK: &str = "••••";
const METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

/// One request of a file, before variables are filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub name: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Body,
    /// 1-based lines: the request line, and the last line of the request's block.
    pub line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    None,
    Text(String),
    /// `< path`, relative to the file.
    File(String),
}

/// The requests of a file, and its `@name = value` variables.
pub fn parse(text: &str) -> (Vec<Request>, BTreeMap<String, String>) {
    let mut vars = BTreeMap::new();
    let mut out = vec![];
    let lines: Vec<&str> = text.lines().collect();
    // Blocks between `###` lines.
    let mut starts = vec![0usize];
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("###") {
            starts.push(i);
        }
    }
    starts.dedup();
    for (bi, &s) in starts.iter().enumerate() {
        let e = starts.get(bi + 1).copied().unwrap_or(lines.len());
        let mut name = lines.get(s).and_then(|l| l.trim_start().strip_prefix("###")).map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        let mut i = if lines.get(s).is_some_and(|l| l.trim_start().starts_with("###")) { s + 1 } else { s };
        // Before the request line: comments, `# @name`, file variables.
        let mut req: Option<Request> = None;
        while i < e {
            let l = lines[i].trim();
            if l.is_empty() {
                i += 1;
                continue;
            }
            if let Some(c) = l.strip_prefix('#').or_else(|| l.strip_prefix("//")) {
                if let Some(n) = c.trim().strip_prefix("@name") {
                    let n = n.trim().trim_start_matches('=').trim();
                    if !n.is_empty() {
                        name = Some(n.to_string());
                    }
                }
                i += 1;
                continue;
            }
            if let Some(v) = l.strip_prefix('@') {
                if let Some((k, val)) = v.split_once('=') {
                    let k = k.trim();
                    if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
                        vars.insert(k.to_string(), val.trim().to_string());
                    }
                }
                i += 1;
                continue;
            }
            // The request line.
            let (method, rest) = match l.split_once(char::is_whitespace) {
                Some((m, r)) if METHODS.contains(&m.to_ascii_uppercase().as_str()) => (m.to_ascii_uppercase(), r.trim()),
                _ => ("GET".to_string(), l),
            };
            let mut url = rest.to_string();
            if let Some((u, v)) = url.rsplit_once(' ') {
                if v.starts_with("HTTP/") {
                    url = u.trim_end().to_string();
                }
            }
            let line = i + 1;
            i += 1;
            // Continuation lines of a long URL: indented, starting with ? or &.
            while i < e {
                let raw = lines[i];
                let t = raw.trim();
                if raw.starts_with(char::is_whitespace) && (t.starts_with('?') || t.starts_with('&')) {
                    url.push_str(t);
                    i += 1;
                } else {
                    break;
                }
            }
            let mut headers = vec![];
            while i < e {
                let t = lines[i].trim();
                if t.is_empty() {
                    i += 1;
                    break;
                }
                if t.starts_with('#') || t.starts_with("//") {
                    i += 1;
                    continue;
                }
                if let Some((k, v)) = t.split_once(':') {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
                i += 1;
            }
            // The body, up to a response handler, a redirect or the next request.
            let mut body_lines: Vec<&str> = vec![];
            let mut in_script = false;
            while i < e {
                let raw = lines[i];
                let t = raw.trim();
                if in_script {
                    if t.contains("%}") {
                        in_script = false;
                    }
                    i += 1;
                    continue;
                }
                if t.starts_with("> {%") || t.starts_with(">{%") {
                    in_script = !t.contains("%}");
                    i += 1;
                    continue;
                }
                if t.starts_with(">>") || (t.starts_with("> ") && t.len() > 2) {
                    i += 1;
                    continue;
                }
                body_lines.push(raw);
                i += 1;
            }
            while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
                body_lines.pop();
            }
            let body = match body_lines.as_slice() {
                [] => Body::None,
                [one] if one.trim_start().starts_with("< ") => Body::File(one.trim_start()[2..].trim().to_string()),
                ls => Body::Text(ls.join("\n")),
            };
            req = Some(Request { name: name.clone(), method, url, headers, body, line, end_line: e });
            break;
        }
        if let Some(r) = req {
            out.push(r);
        }
    }
    (out, vars)
}

/// Variables of one environment: `$shared`, then the named one; the private file's
/// win. Also returns the values that came from the private file (to mask).
fn env_vars(public: Option<&Value>, private: Option<&Value>, env: Option<&str>) -> (BTreeMap<String, String>, Vec<String>) {
    let mut vars = BTreeMap::new();
    let mut secret = vec![];
    for (file, is_private) in [(public, false), (private, true)] {
        let Some(Value::Object(o)) = file else { continue };
        for section in std::iter::once("$shared").chain(env) {
            if let Some(Value::Object(vs)) = o.get(section) {
                for (k, v) in vs {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => continue,
                    };
                    if is_private && s.len() >= 4 {
                        secret.push(s.clone());
                    }
                    vars.insert(k.clone(), s);
                }
            }
        }
    }
    (vars, secret)
}

/// Names of the environments in the env files (`$shared` is not one).
fn env_names(public: Option<&Value>, private: Option<&Value>) -> Vec<String> {
    let mut names: Vec<String> = [public, private]
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_object())
        .flat_map(|o| o.keys().filter(|k| !k.starts_with('$')).cloned().collect::<Vec<_>>())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Fill in `{{name}}`s: file variables (which may use env ones), env variables and
/// dynamic ones. Unknown names are an error that lists them.
fn substitute(text: &str, vars: &BTreeMap<String, String>, missing: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(a) = rest.find("{{") {
        out.push_str(&rest[..a]);
        let after = &rest[a + 2..];
        let Some(b) = after.find("}}") else {
            out.push_str(&rest[a..]);
            return out;
        };
        let name = after[..b].trim();
        match dynamic(name).or_else(|| vars.get(name).cloned()) {
            Some(v) => out.push_str(&v),
            None => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_string());
                }
                out.push_str(&rest[a..a + 2 + b + 2]);
            }
        }
        rest = &after[b + 2..];
    }
    out.push_str(rest);
    out
}

fn dynamic(name: &str) -> Option<String> {
    let n = name.strip_prefix('$')?;
    Some(match n {
        "uuid" | "random.uuid" => uuid_v4(),
        "timestamp" => (crate::util::now_ms() / 1000).to_string(),
        "isoTimestamp" => chrono_iso(),
        "randomInt" | "random.integer" => (rand_u64() % 1000).to_string(),
        _ => return None,
    })
}

fn rand_u64() -> u64 {
    use rand::RngCore;
    rand::rng().next_u64()
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn chrono_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// An error with its causes ("error sending request: connection refused").
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        let t = s.to_string();
        if !msg.contains(&t) {
            msg.push_str(": ");
            msg.push_str(&t);
        }
        src = s.source();
    }
    msg
}

fn mask(text: &str, secrets: &[String]) -> String {
    let mut s = text.to_string();
    for v in secrets {
        if !v.is_empty() {
            s = s.replace(v.as_str(), MASK);
        }
    }
    s
}

/// The env files nearest to `file`, from its folder up to the project root.
fn find_env(root: &Path, file: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = file.parent();
    while let Some(d) = dir {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    None
}

fn read_json(p: Option<PathBuf>) -> ApiResult<Option<Value>> {
    let Some(p) = p else { return Ok(None) };
    let text = std::fs::read_to_string(&p)?;
    serde_json::from_str(&text).map(Some).map_err(|e| ApiError::bad_request(format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
}

struct Loaded {
    requests: Vec<Request>,
    file_vars: BTreeMap<String, String>,
    public: Option<Value>,
    private: Option<Value>,
    root: PathBuf,
    sensitive: Sensitive,
}

fn load(state: &AppState, pid: &str, path: &str) -> ApiResult<Loaded> {
    let project = state.projects.require(pid)?;
    let abs = crate::util::paths::resolve_in_root(&project.root, path)?;
    let sensitive = Sensitive::new(&project.config.project.sensitive);
    if sensitive.matches(path) {
        return Err(ApiError::forbidden(format!("{path} is marked sensitive")));
    }
    let md = std::fs::metadata(&abs)?;
    if md.len() > MAX_FILE {
        return Err(ApiError::bad_request("the .http file is too large"));
    }
    let text = std::fs::read_to_string(&abs).map_err(|_| ApiError::bad_request("the .http file is not UTF-8 text"))?;
    let (requests, file_vars) = parse(&text);
    let public = read_json(find_env(&project.root, &abs, PUBLIC_ENV))?;
    let private = read_json(find_env(&project.root, &abs, PRIVATE_ENV))?;
    Ok(Loaded { requests, file_vars, public, private, root: project.root.clone(), sensitive })
}

#[derive(Deserialize)]
pub struct EnvsQuery {
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Envs {
    pub envs: Vec<String>,
    /// The requests' lines (for the editor's Send Request marks).
    pub requests: Vec<RequestLine>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLine {
    pub line: usize,
    pub end_line: usize,
    pub method: String,
    pub name: Option<String>,
}

/// `GET /api/projects/{pid}/http/envs?path=`: the environment names and the requests.
pub async fn envs(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, Query(q): Query<EnvsQuery>) -> ApiResult<Json<Envs>> {
    let st = state.clone();
    let l = tokio::task::spawn_blocking(move || load(&st, &pid, &q.path)).await.map_err(|e| ApiError::internal(e.to_string()))??;
    Ok(Json(Envs {
        envs: env_names(l.public.as_ref(), l.private.as_ref()),
        requests: l.requests.iter().map(|r| RequestLine { line: r.line, end_line: r.end_line, method: r.method.clone(), name: r.name.clone() }).collect(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunBody {
    path: String,
    /// Any 1-based line of the request's block.
    line: usize,
    #[serde(default)]
    env: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SentRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The body as sent (text), masked; `null` without one.
    pub body: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpResult {
    pub path: String,
    pub line: usize,
    pub name: Option<String>,
    pub env: Option<String>,
    pub request: SentRequest,
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Not valid UTF-8 (shown lossily).
    pub binary: bool,
    pub truncated: bool,
    pub size: usize,
    pub elapsed_ms: u64,
    /// After redirects.
    pub final_url: String,
    pub content_type: Option<String>,
    pub at: i64,
}

/// `POST /api/projects/{pid}/http/run {path, line, env?}`: send the request at `line`.
pub async fn run(State(state): State<AppState>, UrlPath(pid): UrlPath<String>, headers: HeaderMap, Json(b): Json<RunBody>) -> ApiResult<Json<HttpResult>> {
    // A user's click: devices only (an agent has its own shell for requests).
    if state.auth.agent_from_headers(&headers).is_some() {
        return Err(ApiError::forbidden("agent sessions cannot send requests through the HTTP Client"));
    }
    let st = state.clone();
    let path = b.path.clone();
    let l = tokio::task::spawn_blocking(move || load(&st, &pid, &path)).await.map_err(|e| ApiError::internal(e.to_string()))??;
    let r = l
        .requests
        .iter()
        .find(|r| b.line >= r.line && b.line <= r.end_line)
        .or_else(|| l.requests.iter().rev().find(|r| b.line >= r.line))
        .or_else(|| l.requests.first())
        .ok_or_else(|| ApiError::bad_request("no request in this file"))?
        .clone();
    let env = b.env.clone().filter(|e| !e.is_empty());
    let (env_vars, secrets) = env_vars(l.public.as_ref(), l.private.as_ref(), env.as_deref());
    // File variables may use env ones; request text may use both.
    let mut missing = vec![];
    let mut vars = env_vars.clone();
    for (k, v) in &l.file_vars {
        let filled = substitute(v, &env_vars, &mut missing);
        vars.insert(k.clone(), filled);
    }
    missing.clear();
    let url = substitute(&r.url, &vars, &mut missing);
    let hdrs: Vec<(String, String)> = r.headers.iter().map(|(k, v)| (k.clone(), substitute(v, &vars, &mut missing))).collect();
    let body_bytes: Option<Vec<u8>> = match &r.body {
        Body::None => None,
        Body::Text(t) => Some(substitute(t, &vars, &mut missing).into_bytes()),
        Body::File(f) => {
            // Relative to the .http file, inside the project.
            let dir = Path::new(&b.path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
            let rel = if dir.is_empty() { f.trim_start_matches("./").to_string() } else { format!("{dir}/{}", f.trim_start_matches("./")) };
            let p = crate::util::paths::resolve_in_root(&l.root, &rel)?;
            if l.sensitive.matches(&rel) {
                return Err(ApiError::forbidden(format!("{f} is marked sensitive")));
            }
            if std::fs::metadata(&p)?.len() > MAX_BODY_FILE {
                return Err(ApiError::bad_request(format!("{f} is too large to send")));
            }
            Some(std::fs::read(&p)?)
        }
    };
    if !missing.is_empty() {
        let hint = if env.is_none() { " (no environment is selected)" } else { "" };
        return Err(ApiError::bad_request(format!("unknown variable{}: {}{hint}", if missing.len() == 1 { "" } else { "s" }, missing.join(", "))));
    }
    let parsed = reqwest::Url::parse(&url).map_err(|e| ApiError::bad_request(format!("{}: {e}", mask(&url, &secrets))))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ApiError::bad_request("only http and https URLs can be sent"));
    }
    let method = reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|_| ApiError::bad_request("bad method"))?;
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent(concat!("workbench-http-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut req = client.request(method, parsed);
    for (k, v) in &hdrs {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(bytes) = &body_bytes {
        req = req.body(bytes.clone());
    }
    let started = Instant::now();
    let resp = req.send().await.map_err(|e| ApiError::upstream(mask(&format!("request failed: {}", error_chain(&e)), &secrets)))?;
    let status = resp.status();
    let final_url = mask(resp.url().as_str(), &secrets);
    let resp_headers: Vec<(String, String)> = resp.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut body = Vec::new();
    let mut truncated = false;
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::upstream(format!("reading the response failed: {e}")))? {
        if body.len() + chunk.len() > MAX_RESPONSE {
            body.extend_from_slice(&chunk[..MAX_RESPONSE - body.len()]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let size = body.len();
    let (text, binary) = match String::from_utf8(body) {
        Ok(t) => (t, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    };
    let sent_body = body_bytes.as_ref().map(|b| mask(&String::from_utf8_lossy(b), &secrets));
    Ok(Json(HttpResult {
        path: b.path,
        line: r.line,
        name: r.name,
        env,
        request: SentRequest { method: r.method, url: mask(&url, &secrets), headers: hdrs.into_iter().map(|(k, v)| (k, mask(&v, &secrets))).collect(), body: sent_body },
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        headers: resp_headers,
        body: text,
        binary,
        truncated,
        size,
        elapsed_ms,
        final_url,
        content_type,
        at: crate::util::now_ms(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FILE: &str = r#"@host = {{base}}/api
@limit = 10

### List things
GET {{host}}/things
    ?limit={{limit}}
    &sort=name
Accept: application/json

###
# @name create
POST {{host}}/things HTTP/1.1
Content-Type: application/json
Authorization: Bearer {{token}}

{"name": "x", "id": "{{$uuid}}"}

> {%
  client.global.set("id", response.body.id);
%}

### Upload
PUT {{host}}/files

< ./payload.json

###
https://example.com/plain
"#;

    #[test]
    fn parses_requests_names_variables_and_bodies() {
        let (reqs, vars) = parse(FILE);
        assert_eq!(vars.get("host").map(String::as_str), Some("{{base}}/api"));
        assert_eq!(vars.get("limit").map(String::as_str), Some("10"));
        assert_eq!(reqs.len(), 4, "{reqs:#?}");
        assert_eq!(reqs[0].name.as_deref(), Some("List things"));
        assert_eq!(reqs[0].method, "GET");
        assert_eq!(reqs[0].url, "{{host}}/things?limit={{limit}}&sort=name");
        assert_eq!(reqs[0].headers, vec![("Accept".into(), "application/json".into())]);
        assert_eq!(reqs[0].body, Body::None);
        assert_eq!(reqs[1].name.as_deref(), Some("create"));
        assert_eq!(reqs[1].url, "{{host}}/things");
        assert_eq!(reqs[1].body, Body::Text(r#"{"name": "x", "id": "{{$uuid}}"}"#.into()), "the handler script is not part of the body");
        assert_eq!(reqs[2].body, Body::File("./payload.json".into()));
        assert_eq!((reqs[3].method.as_str(), reqs[3].url.as_str()), ("GET", "https://example.com/plain"));
        assert!(reqs[2].end_line < reqs[3].line);
    }

    #[test]
    fn a_bare_url_is_a_get() {
        let (reqs, _) = parse("https://example.com/health\n");
        assert_eq!(reqs.len(), 1);
        assert_eq!((reqs[0].method.as_str(), reqs[0].url.as_str(), reqs[0].line), ("GET", "https://example.com/health", 1));
    }

    #[test]
    fn env_files_merge_shared_named_and_private_and_mark_secrets() {
        let public = json!({ "$shared": { "base": "http://localhost:8080" }, "dev": { "token": "public-token" }, "prod": { "base": "https://api.example.com" } });
        let private = json!({ "dev": { "token": "s3cret-token" } });
        let (vars, secrets) = env_vars(Some(&public), Some(&private), Some("dev"));
        assert_eq!(vars["base"], "http://localhost:8080");
        assert_eq!(vars["token"], "s3cret-token", "the private file wins");
        assert_eq!(secrets, vec!["s3cret-token".to_string()]);
        assert_eq!(env_names(Some(&public), Some(&private)), vec!["dev".to_string(), "prod".to_string()]);
        let mut missing = vec![];
        let url = substitute("{{base}}/x?t={{token}}&u={{nope}}&id={{$uuid}}", &vars, &mut missing);
        assert!(url.starts_with("http://localhost:8080/x?t=s3cret-token&u={{nope}}&id="), "{url}");
        assert_eq!(missing, vec!["nope".to_string()]);
        assert_eq!(mask(&url, &secrets).contains("s3cret"), false);
    }

    #[test]
    fn dynamic_values_have_their_shapes() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        let iso = chrono_iso();
        assert!(iso.len() == 24 && iso.ends_with('Z') && &iso[4..5] == "-", "{iso}");
        assert!(dynamic("$randomInt").unwrap().parse::<u64>().unwrap() < 1000);
        assert_eq!(dynamic("$nope"), None);
    }

    /// Through the route: env files, masking, a body file, a mock server echoing back.
    #[tokio::test(flavor = "multi_thread")]
    async fn runs_a_request_against_a_server_and_masks_private_values() {
        use axum::http::Method;
        let echo = axum::Router::new().fallback(|req: axum::extract::Request| async move {
            let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let path = req.uri().to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
            axum::Json(json!({ "path": path, "auth": auth, "body": String::from_utf8_lossy(&body) }))
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, echo).await.unwrap() });

        let (cfg, data, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = proj.path().canonicalize().unwrap().join("api");
        std::fs::create_dir_all(root.join("http")).unwrap();
        std::fs::write(root.join("http/api.http"), "@base = http://127.0.0.1:{{port}}\n\n### Get\nGET {{base}}/things?q={{token}}\nAuthorization: Bearer {{token}}\n\n### Put\nPUT {{base}}/files\n\n< ./body.json\n\n### Broken\nGET {{base}}/{{nope}}\n").unwrap();
        std::fs::write(root.join("http/body.json"), "{\"a\": 1}").unwrap();
        std::fs::write(root.join("http-client.env.json"), json!({ "dev": { "port": port.to_string() } }).to_string()).unwrap();
        std::fs::write(root.join("http-client.private.env.json"), json!({ "dev": { "token": "tok-SECRET-1" } }).to_string()).unwrap();
        let mut config = crate::config::GlobalConfig::default();
        config.projects.roots = vec![];
        config.projects.include = vec![root.display().to_string()];
        config.notify.desktop = false;
        let paths = crate::config::Paths { config_dir: cfg.path().to_path_buf(), data_dir: data.path().to_path_buf() };
        let state = AppState::new(paths, config, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        let _ = crate::app::build_router(state.clone());
        let pid = state.projects.list()[0].id.clone();
        let api = |m: Method, p: String, b: Option<Value>| {
            let st = state.clone();
            async move { crate::mcp::call_api(&st, m, &p, b, &crate::mcp::McpCtx::default()).await }
        };

        let envs = api(Method::GET, format!("/api/projects/{pid}/http/envs?path=http/api.http"), None).await.unwrap();
        assert_eq!(envs["envs"], json!(["dev"]));
        assert_eq!(envs["requests"].as_array().unwrap().len(), 3);

        let r = api(Method::POST, format!("/api/projects/{pid}/http/run"), Some(json!({ "path": "http/api.http", "line": 4, "env": "dev" }))).await.unwrap();
        assert_eq!(r["status"], 200, "{r}");
        let body: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["auth"], "Bearer tok-SECRET-1", "the server got the real value");
        assert_eq!(body["path"], "/things?q=tok-SECRET-1");
        assert!(!r["request"].to_string().contains("tok-SECRET-1"), "the echoed request masks it: {}", r["request"]);
        assert!(r["request"]["url"].as_str().unwrap().ends_with("q=••••"));

        let r = api(Method::POST, format!("/api/projects/{pid}/http/run"), Some(json!({ "path": "http/api.http", "line": 9, "env": "dev" }))).await.unwrap();
        let body: Value = serde_json::from_str(r["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["body"], "{\"a\": 1}", "the body came from the file next to the .http file");

        let err = api(Method::POST, format!("/api/projects/{pid}/http/run"), Some(json!({ "path": "http/api.http", "line": 14, "env": "dev" }))).await.unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
        let err = api(Method::POST, format!("/api/projects/{pid}/http/run"), Some(json!({ "path": "http/api.http", "line": 4 }))).await.unwrap_err();
        assert!(err.to_string().contains("no environment is selected"), "{err}");
        // The private env file itself stays hidden.
        assert!(api(Method::GET, format!("/api/projects/{pid}/files/read?path=http-client.private.env.json"), None).await.unwrap()["content"].is_null());
    }
}
