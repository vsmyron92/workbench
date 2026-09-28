//! Serves the built React app. Release builds embed `web/dist`; debug builds read
//! it from disk at request time, so `npm run build` is picked up without a restart.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist/"]
#[allow_missing = true]
struct Assets;

/// Second line of defence behind the HTML sanitizers: the app's origin can spawn
/// shells, so only its own bundled scripts may run. Styles stay inline-capable
/// (Monaco, xterm and dockview inject `<style>`); images and frames may be remote
/// (avatars, Confluence images, app previews on their own loopback port).
pub const CSP: &str = "default-src 'self'; script-src 'self'; worker-src 'self' blob:; connect-src 'self'; \
     style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data: blob: https: http:; \
     media-src 'self' data: blob:; frame-src 'self' blob: https: http:; object-src 'none'; base-uri 'none'; \
     form-action 'self'; frame-ancestors 'none'";

/// The service worker's own policy (a worker gets the CSP of its script): it only
/// talks to this origin and loads nothing else.
pub const SW_CSP: &str = "default-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; object-src 'none'; base-uri 'none'";

pub async fn handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "mcp" {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    if !path.is_empty() {
        if let Some(file) = Assets::get(path) {
            let cache = if path.starts_with("assets/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            // The service worker and the web app manifest (platform push / PWA): exact
            // types, always revalidated so an update reaches every device.
            let mimetype = match path {
                "sw.js" => "text/javascript; charset=utf-8".to_string(),
                "manifest.webmanifest" => "application/manifest+json; charset=utf-8".to_string(),
                _ => file.metadata.mimetype().to_string(),
            };
            let mut resp = (
                [
                    (header::CONTENT_TYPE, mimetype),
                    (header::CACHE_CONTROL, cache.to_string()),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                ],
                file.data,
            )
                .into_response();
            if path == "sw.js" {
                resp.headers_mut().insert(header::CONTENT_SECURITY_POLICY, header::HeaderValue::from_static(SW_CSP));
            }
            return resp;
        }
        // A missing hashed asset is a real 404, not a client route.
        if path.starts_with("assets/") {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    }
    match Assets::get("index.html") {
        Some(file) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
                (header::X_FRAME_OPTIONS, "DENY".to_string()),
                (header::REFERRER_POLICY, "no-referrer".to_string()),
                (header::CONTENT_SECURITY_POLICY, CSP.to_string()),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            ],
            file.data,
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "The web UI is not built. Run `npm ci && npm run build` in web/ (or use the Vite dev server on :5173).",
        )
            .into_response(),
    }
}
