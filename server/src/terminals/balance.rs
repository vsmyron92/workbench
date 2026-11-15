//! A hosted API account's balance, when its service publishes one and the account asked
//! for it (`api.balance = true`). DeepSeek's `GET /user/balance` is read-only and takes the
//! same key the sessions use. Nothing else here asks a vendor: the request is made only
//! while the usage view is open, its answer is cached for a minute, and only the figures
//! (never the key or the raw body) reach the browser.

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::secrets::Secret;
use crate::util;

/// One account's balance, as its service reported it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    /// `false`: the balance is gone (or below the service's minimum) and calls will fail.
    pub available: bool,
    pub currency: String,
    pub total: String,
    pub granted: String,
    pub topped_up: String,
    /// When it was read (ms).
    pub fetched_at: i64,
}

impl Balance {
    /// Whether an answer is old enough to ask again.
    pub fn stale(&self, now: i64) -> bool {
        now.saturating_sub(self.fetched_at) >= 60_000
    }
}

/// The last answer per provider, kept in memory (a balance is not config).
#[derive(Default)]
pub struct Cache {
    map: Mutex<HashMap<String, Balance>>,
}

impl Cache {
    pub fn get(&self, id: &str) -> Option<Balance> {
        self.map.lock().get(id).cloned()
    }
    pub fn put(&self, id: &str, b: Balance) {
        self.map.lock().insert(id.to_string(), b);
    }
}

/// The address a balance is read from: the service's origin, whatever path its API uses
/// (`https://api.deepseek.com/anthropic` → `https://api.deepseek.com`).
pub fn endpoint(service: &str, url: &str) -> Option<String> {
    if service.trim().to_ascii_lowercase() != "deepseek" {
        return None;
    }
    let u = url.trim();
    if u.is_empty() {
        return Some("https://api.deepseek.com/user/balance".into());
    }
    // The key goes there: the address must be one `api_setup` would accept.
    if !super::providers::valid_api_url(u) {
        return None;
    }
    let (scheme, rest) = u.strip_prefix("https://").map(|r| ("https", r)).or_else(|| u.strip_prefix("http://").map(|r| ("http", r)))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!host.is_empty()).then(|| format!("{scheme}://{host}/user/balance"))
}

/// Read the balance. One bounded GET, no redirects, and only the figures leave the
/// response: the body's other fields (and the key) never reach the caller.
pub async fn fetch(endpoint: &str, key: &Secret) -> Result<Balance, String> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = http.get(endpoint).bearer_auth(key.expose()).header("accept", "application/json").send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("the service answered {}", resp.status().as_u16()));
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > 64 * 1024 {
        return Err("the answer is too large".into());
    }
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    parse(&v, util::now_ms())
}

/// `{is_available, balance_infos: [{currency, total_balance, granted_balance,
/// topped_up_balance}]}` → the first currency's figures. An answer without figures keeps
/// the availability but shows no amount.
pub fn parse(v: &Value, now: i64) -> Result<Balance, String> {
    let available = v.get("is_available").and_then(Value::as_bool).ok_or("the answer has no is_available")?;
    let info = v.get("balance_infos").and_then(Value::as_array).and_then(|a| a.first());
    let text = |k: &str| {
        info.and_then(|i| i.get(k))
            .and_then(|x| x.as_str().map(str::to_string).or_else(|| x.as_f64().map(|n| format!("{n:.2}"))))
            .unwrap_or_default()
    };
    Ok(Balance {
        available,
        currency: info.and_then(|i| i.get("currency")).and_then(Value::as_str).unwrap_or_default().to_string(),
        total: text("total_balance"),
        granted: text("granted_balance"),
        topped_up: text("topped_up_balance"),
        fetched_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoint_is_the_services_origin() {
        assert_eq!(endpoint("deepseek", "").as_deref(), Some("https://api.deepseek.com/user/balance"));
        assert_eq!(endpoint("deepseek", "https://api.deepseek.com/anthropic").as_deref(), Some("https://api.deepseek.com/user/balance"));
        assert_eq!(endpoint("DeepSeek", "http://127.0.0.1:8080/anthropic").as_deref(), Some("http://127.0.0.1:8080/user/balance"));
        assert_eq!(endpoint("openrouter", "https://openrouter.ai/api"), None);
        // The key must not go to a plain-HTTP host or an address with credentials.
        assert_eq!(endpoint("deepseek", "http://example.com/anthropic"), None);
        assert_eq!(endpoint("deepseek", "https://u:p@api.deepseek.com/anthropic"), None);
    }

    #[test]
    fn only_figures_are_kept() {
        let v = json!({
            "is_available": true,
            "balance_infos": [{"currency": "CNY", "total_balance": "110.00", "granted_balance": "0.00", "topped_up_balance": "110.00"}],
            "email": "nobody@example.com"
        });
        let b = parse(&v, 7).unwrap();
        assert_eq!((b.available, b.currency.as_str(), b.total.as_str(), b.topped_up.as_str(), b.granted.as_str()), (true, "CNY", "110.00", "110.00", "0.00"));
        assert_eq!(b.fetched_at, 7);
        assert!(parse(&json!({"balance_infos": []}), 0).is_err());
    }
}
