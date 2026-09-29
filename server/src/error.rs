//! The one error type every handler returns. Serialized as
//! `{"error": {"code": "not_found", "message": "..."}}` with a matching status
//! (`unsupported_platform` errors add `"feature"`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// The feature an `unsupported_platform` error is about (`util::os::support`).
    pub feature: Option<&'static str>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into(), feature: None }
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", m)
    }
    /// Optimistic-concurrency failure (file changed on disk, page version moved on…).
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", m)
    }
    /// The integration is not set up (no token, no site…). The UI shows setup help.
    pub fn not_configured(m: impl Into<String>) -> Self {
        Self::new(StatusCode::PRECONDITION_FAILED, "not_configured", m)
    }
    /// `feature` does not work on this OS (`util::os::support`; dev containers on Windows…).
    /// The UI shows it like setup help, without a way to set it up.
    pub fn unsupported(feature: &'static str, reason: impl Into<String>) -> Self {
        Self { feature: Some(feature), ..Self::new(StatusCode::NOT_IMPLEMENTED, "unsupported_platform", reason) }
    }
    /// A remote service (GitLab, Atlassian, ssh host) failed. Never include credentials.
    pub fn upstream(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "upstream", m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", m)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.code, self.status.as_u16(), self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // A feature this OS leaves out is expected, not a failure worth a warning.
        if self.status.is_server_error() && self.feature.is_none() {
            tracing::warn!("{self}");
        }
        let mut message = self.message;
        if message.len() > 4000 {
            message.truncate(4000);
            message.push('…');
        }
        let mut error = json!({ "code": self.code, "message": message });
        if let Some(f) = self.feature {
            error["feature"] = json!(f);
        }
        (self.status, Json(json!({ "error": error }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast::<ApiError>() {
            Ok(api) => api,
            Err(e) => Self::internal(format!("{e:#}")),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Self::not_found(e.to_string()),
            std::io::ErrorKind::PermissionDenied => Self::forbidden(e.to_string()),
            _ => Self::internal(e.to_string()),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors can carry the URL; ours never embed credentials in URLs.
        Self::upstream(e.without_url().to_string())
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::bad_request(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body(e: ApiError) -> (u16, serde_json::Value) {
        let resp = e.into_response();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn unsupported_names_the_feature() {
        let (status, v) = body(ApiError::unsupported("devcontainer", "not on this OS")).await;
        assert_eq!(status, 501);
        assert_eq!(v, json!({ "error": { "code": "unsupported_platform", "message": "not on this OS", "feature": "devcontainer" } }));
        // Other errors keep their shape.
        let (status, v) = body(ApiError::not_configured("no token")).await;
        assert_eq!(status, 412);
        assert_eq!(v, json!({ "error": { "code": "not_configured", "message": "no token" } }));
        // Through anyhow (handlers that use `?` on it) the feature stays.
        let e: ApiError = anyhow::Error::new(ApiError::unsupported("networkRoots", "x")).into();
        assert_eq!((e.code, e.feature), ("unsupported_platform", Some("networkRoots")));
    }
}
