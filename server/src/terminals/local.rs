//! Ask a model server of your own which models it serves (`POST /api/agents/local-models`),
//! so the account form can offer them and say whether the server answers at all.
//!
//! One GET to a fixed path (`/api/tags` for Ollama, `/v1/models` for the others), no
//! redirects, a few seconds, a small answer; only model names leave this module.

use std::time::Duration;

use serde_json::Value;

use super::providers::{default_local_url, valid_local_url, valid_model};

const MAX_BODY: usize = 1024 * 1024;
const MAX_MODELS: usize = 300;

/// The servers `[agents.providers.<name>.local]` can name.
pub const SERVERS: &[&str] = &["ollama", "lmstudio", "openai", "anthropic"];

/// Where a server lists its models.
pub fn models_url(server: &str, url: &str) -> Option<String> {
    let base = url.trim().trim_end_matches('/');
    let root = base.strip_suffix("/v1").unwrap_or(base);
    match server {
        "ollama" => Some(format!("{root}/api/tags")),
        "lmstudio" | "openai" | "anthropic" => Some(format!("{root}/v1/models")),
        _ => None,
    }
}

/// The model names in a server's answer: Ollama `models[].name`, the rest `data[].id`.
pub fn parse_models(server: &str, body: &[u8]) -> Option<Vec<String>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let (list, key) = if server == "ollama" { (v.get("models")?, "name") } else { (v.get("data")?, "id") };
    let mut out: Vec<String> = list
        .as_array()?
        .iter()
        .filter_map(|m| m.get(key).and_then(Value::as_str))
        .filter(|m| valid_model(m))
        .map(str::to_string)
        .take(MAX_MODELS)
        .collect();
    out.sort();
    out.dedup();
    Some(out)
}

/// The models `server` at `url` (empty: its usual address) serves, or why not.
pub async fn probe(server: &str, url: &str) -> Result<Vec<String>, String> {
    let server = server.trim().to_lowercase();
    if !SERVERS.contains(&server.as_str()) {
        return Err(format!("server must be one of {}", SERVERS.join(", ")));
    }
    let url = Some(url.trim()).filter(|u| !u.is_empty()).map_or_else(|| default_local_url(&server).to_string(), str::to_string);
    if url.is_empty() {
        return Err(format!("enter the address of the {server} server"));
    }
    if !valid_local_url(&url) {
        return Err("the address must be http:// or https:// and carry no user name or password".into());
    }
    let target = models_url(&server, &url).ok_or("unknown server")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(&target).header("accept", "application/json").send().await.map_err(|e| {
        if e.is_timeout() { "the server did not answer in time".to_string() } else { "could not connect: is the server running?".to_string() }
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("the server answered {} (is it a {server} server?)", status.as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| "the answer was cut off".to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err("the answer is too large to be a model list".into());
        }
    }
    parse_models(&server, &body).ok_or_else(|| format!("that is not a model list (is it a {server} server?)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_list_of_each_server() {
        assert_eq!(models_url("ollama", "http://localhost:11434/").as_deref(), Some("http://localhost:11434/api/tags"));
        assert_eq!(models_url("lmstudio", "http://localhost:1234/v1").as_deref(), Some("http://localhost:1234/v1/models"));
        assert_eq!(models_url("openai", "http://gpu:8080").as_deref(), Some("http://gpu:8080/v1/models"));
        assert_eq!(models_url("gemini", "http://x"), None);
    }

    #[test]
    fn reads_model_names_only() {
        let ollama = br#"{"models":[{"name":"qwen3:8b","size":1,"details":{}},{"name":"llama3.2:latest"},{"name":"bad name; rm"},{"nope":1}]}"#;
        assert_eq!(parse_models("ollama", ollama).unwrap(), ["llama3.2:latest", "qwen3:8b"]);
        let openai = br#"{"object":"list","data":[{"id":"openai/gpt-oss-20b","object":"model"},{"id":"openai/gpt-oss-20b"},{"id":"qwen2.5-coder-7b"}]}"#;
        assert_eq!(parse_models("lmstudio", openai).unwrap(), ["openai/gpt-oss-20b", "qwen2.5-coder-7b"]);
        assert!(parse_models("ollama", openai).is_none(), "an OpenAI list is not Ollama's");
        assert!(parse_models("openai", b"<html>").is_none());
        assert_eq!(parse_models("openai", br#"{"data":[]}"#).unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn asks_a_server_and_explains_a_failure() {
        use axum::{Router, routing::get};
        let app = Router::new()
            .route("/api/tags", get(|| async { axum::Json(serde_json::json!({"models":[{"name":"qwen3:8b"}]})) }))
            .route("/v1/models", get(|| async { (axum::http::StatusCode::NOT_FOUND, "no") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let url = format!("http://{addr}");
        assert_eq!(probe("ollama", &url).await.unwrap(), ["qwen3:8b"]);
        assert!(probe("openai", &url).await.unwrap_err().contains("404"));
        assert!(probe("openai", "ftp://x").await.unwrap_err().contains("http://"));
        assert!(probe("openai", "http://u:p@host").await.unwrap_err().contains("user name"));
        assert!(probe("openai", "").await.unwrap_err().contains("enter the address"));
        assert!(probe("gemini", &url).await.unwrap_err().contains("server must be"));
        // Nothing listens there.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert!(probe("ollama", &dead).await.unwrap_err().contains("could not connect"));
    }
}
