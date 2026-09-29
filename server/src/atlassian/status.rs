//! `GET /api/atlassian/status`: is Atlassian configured, do the credentials work,
//! and which products does the site have. Jira switches itself on only when
//! `GET /rest/api/3/serverInfo` succeeds (sites without Jira answer 404).
//! Cached for 10 minutes per site, account and credentials (30 s after a network
//! failure), so a corrected token is checked afresh; `refresh=1` skips the cache.
//! "Not set up" is an answer, not a failure: the endpoint returns 200 with
//! `configured: false` and the setup help in `error` (the UI probes on every load).

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{Api, Product, resolve_site};
use crate::app::AppState;
use crate::config::contract_tilde;
use crate::error::{ApiError, ApiResult};

const TTL: Duration = Duration::from_secs(600);
const TTL_ERROR: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserOut {
    pub account_id: String,
    pub display_name: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusOut {
    /// False when site, email or token are missing or unreadable; `error` says what to set.
    pub configured: bool,
    pub site: String,
    pub user: Option<UserOut>,
    pub confluence: bool,
    pub jira: bool,
    /// Jira's `serverTitle`, when it has one.
    pub jira_title: Option<String>,
    /// The credentials were rejected.
    pub auth_failed: bool,
    pub error: Option<String>,
    pub checked_at: i64,
    /// Where this server's config.toml is (`~`-contracted), for the setup help.
    pub config_file: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Me {
    account_id: String,
    display_name: String,
    public_name: String,
    email: Option<String>,
    email_address: Option<String>,
    #[serde(rename = "type")]
    kind: String,
}

impl Me {
    fn user(self) -> UserOut {
        UserOut {
            display_name: if self.display_name.is_empty() { self.public_name } else { self.display_name },
            account_id: self.account_id,
            email: self.email.or(self.email_address).filter(|e| !e.is_empty()),
        }
    }
}

/// Status for the site `project_id` resolves to (cached unless `refresh`).
pub async fn check(state: &AppState, project_id: Option<&str>, refresh: bool) -> Result<StatusOut, ApiError> {
    let site = resolve_site(state, project_id, Product::Confluence)?;
    let key = site.key();
    if !refresh {
        if let Some(s) = state.atlassian.cached_status(&key) {
            return Ok(s);
        }
    }
    // One upstream check at a time: several panels ask for the status at startup.
    let _guard = state.atlassian.status_lock.lock().await;
    if !refresh {
        if let Some(s) = state.atlassian.cached_status(&key) {
            return Ok(s);
        }
    }
    let conf = Api::new(state.http.clone(), site.clone(), Product::Confluence);
    let jira = Api::new(state.http.clone(), site.clone(), Product::Jira);
    let conf_url = conf.wiki("/rest/api/user/current");
    let jira_url = jira.url("/rest/api/3/serverInfo");
    let (me, info) = tokio::join!(conf.send(Method::GET, &conf_url, None), jira.send(Method::GET, &jira_url, None));

    let mut out = StatusOut {
        configured: true,
        site: site.base.clone(),
        user: None,
        confluence: false,
        jira: false,
        jira_title: None,
        auth_failed: false,
        error: None,
        checked_at: crate::util::now_ms(),
        config_file: contract_tilde(&state.paths.config_file()),
    };
    let mut transient = false;
    match me {
        Ok(r) if r.status().is_success() => match conf.json::<Me>(r).await {
            Ok(me) if me.kind != "anonymous" && !me.account_id.is_empty() => {
                out.confluence = true;
                out.user = Some(me.user());
            }
            Ok(_) => {
                out.auth_failed = true;
                out.error = Some(format!("Confluence treated {} as anonymous: check the API token", site.email));
            }
            Err(e) => {
                transient = true;
                out.error = Some(e.message);
            }
        },
        Ok(r) if matches!(r.status().as_u16(), 401 | 403) => {
            out.auth_failed = true;
            out.error = Some(format!(
                "Atlassian rejected the credentials for {} (HTTP {}). Check [atlassian] email and the API token.",
                site.email,
                r.status().as_u16()
            ));
        }
        Ok(r) if r.status().as_u16() == 404 => {} // no Confluence on this site
        Ok(r) => {
            transient = true;
            out.error = Some(conf.error_from(r).await.message);
        }
        Err(e) => {
            transient = true;
            out.error = Some(e.message);
        }
    }
    match info {
        Ok(r) if r.status().is_success() => {
            let v: Value = jira.json(r).await.unwrap_or(Value::Null);
            out.jira = true;
            out.jira_title = v.get("serverTitle").and_then(Value::as_str).map(str::to_string);
        }
        Ok(r) if matches!(r.status().as_u16(), 401 | 403) && !out.confluence => out.auth_failed = true,
        _ => {}
    }
    if out.jira && out.user.is_none() && !out.auth_failed {
        let url = jira.url("/rest/api/3/myself");
        match jira.get::<Me>(&url).await {
            Ok(me) => out.user = Some(me.user()),
            Err(e) if e.code == "not_configured" => {
                out.auth_failed = true;
                out.jira = false;
                out.error = Some(e.message);
            }
            Err(_) => {}
        }
    }
    if !out.confluence && !out.jira && out.error.is_none() {
        out.error = Some(format!("{} has neither Confluence nor Jira (or the account cannot see them)", site.host()));
    }
    state.atlassian.store_status(&key, out.clone(), if transient { TTL_ERROR } else { TTL });
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusQuery {
    project_id: Option<String>,
    #[serde(default)]
    refresh: Option<String>,
}

impl StatusOut {
    /// The answer when Atlassian is not set up (`message` is the setup help).
    fn unconfigured(state: &AppState, message: String) -> Self {
        StatusOut {
            configured: false,
            site: String::new(),
            user: None,
            confluence: false,
            jira: false,
            jira_title: None,
            auth_failed: false,
            error: Some(message),
            checked_at: crate::util::now_ms(),
            config_file: contract_tilde(&state.paths.config_file()),
        }
    }
}

pub async fn handler(State(state): State<AppState>, Query(q): Query<StatusQuery>) -> ApiResult<Json<StatusOut>> {
    let refresh = q.refresh.as_deref().is_some_and(|v| v == "1" || v == "true");
    match check(&state, q.project_id.as_deref(), refresh).await {
        Ok(s) => Ok(Json(s)),
        Err(e) if e.code == "not_configured" => Ok(Json(StatusOut::unconfigured(&state, e.message))),
        Err(e) => Err(e),
    }
}

/// A status cache entry.
pub struct Entry {
    pub at: Instant,
    pub ttl: Duration,
    pub status: StatusOut,
}
