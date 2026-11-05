//! The React frontend, embedded in the binary (ADR-0002).
//!
//! In a release build the files in `web/dist` are compiled into the binary, so
//! nothing but the binary is needed at runtime. Unknown paths fall back to
//! `index.html`, because the React app does its own routing.
//!
//! `index.html` names [`ORIGIN_PLACEHOLDER`] wherever a link preview needs an
//! absolute URL (`og:image`, `og:url`, the canonical link). Social sites won't
//! follow a relative one, and a self-hosted Gantry has no address known at
//! build time, so it is filled in as the page is served.
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::AppState;
use crate::client::ClientInfo;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Frontend;

/// What `index.html` says where it needs Gantry's own address.
pub const ORIGIN_PLACEHOLDER: &str = "__GANTRY_ORIGIN__";

/// Serves one embedded file, or `index.html` for any path that isn't a file.
/// `client` (from [`crate::client::identify`]) says whether the visitor used
/// HTTPS and whether a trusted proxy forwarded the request.
pub async fn serve(
    state: &AppState,
    uri: &Uri,
    headers: &HeaderMap,
    client: Option<&ClientInfo>,
) -> Response {
    let path = uri.path().trim_start_matches('/');
    match Frontend::get(path) {
        Some(file) if !path.is_empty() && path != "index.html" => file_response(path, file),
        _ => index(state, headers, client),
    }
}

/// The app's entry page, with its absolute URLs filled in.
pub fn index(state: &AppState, headers: &HeaderMap, client: Option<&ClientInfo>) -> Response {
    let Some(file) = Frontend::get("index.html") else {
        return (StatusCode::NOT_FOUND, "frontend not built").into_response();
    };
    let origin = origin(
        state.public_url(),
        headers,
        state.secure_cookies() || client.is_some_and(|c| c.https),
        client.is_some_and(|c| c.via_proxy),
    );
    let html = with_origin(&String::from_utf8_lossy(&file.data), &origin);
    let mut response = Response::new(Body::from(html));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// `html` with every [`ORIGIN_PLACEHOLDER`] replaced by `origin`.
pub fn with_origin(html: &str, origin: &str) -> String {
    html.replace(ORIGIN_PLACEHOLDER, origin)
}

/// The scheme and host people reach Gantry at, such as
/// `https://gantry.example.com`: the configured public URL, or else worked out
/// from the request. `https` is whether the visitor's own connection is HTTPS;
/// `X-Forwarded-Host` is believed only when a trusted proxy sent the request.
/// Empty when the host isn't a plain host name, which leaves the page's URLs
/// relative rather than echoing odd input.
pub fn origin(
    public_url: Option<&str>,
    headers: &HeaderMap,
    https: bool,
    via_trusted_proxy: bool,
) -> String {
    if let Some(url) = public_url {
        return url.to_owned();
    }
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let host = via_trusted_proxy
        .then(|| header("x-forwarded-host"))
        .flatten()
        .or_else(|| header("host"));
    let Some(host) = host.filter(|h| is_plain_host(h)) else {
        return String::new();
    };
    format!("{}://{host}", if https { "https" } else { "http" })
}

/// A host name or address, with an optional port, and nothing else.
fn is_plain_host(host: &str) -> bool {
    host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// Whether `path` is one of the embedded static files other than the entry
/// page. The login page needs them before anyone has a session.
pub fn is_asset(path: &str) -> bool {
    let path = path.trim_start_matches('/');
    !path.is_empty() && path != "index.html" && Frontend::get(path).is_some()
}

fn file_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from(file.data.into_owned()));
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    // Vite fingerprints everything under assets/, so those never change.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn the_configured_public_url_wins() {
        let h = headers(&[("host", "192.168.1.5:8080")]);
        assert_eq!(
            origin(Some("https://gantry.example.com"), &h, false, false),
            "https://gantry.example.com"
        );
    }

    #[test]
    fn the_origin_comes_from_the_host_and_the_connection() {
        let h = headers(&[("host", "pi.local:8080")]);
        assert_eq!(origin(None, &h, false, false), "http://pi.local:8080");
        let h = headers(&[("host", "gantry.example.com")]);
        assert_eq!(origin(None, &h, true, false), "https://gantry.example.com");
        assert_eq!(
            origin(None, &headers(&[("host", "[::1]:8080")]), true, false),
            "https://[::1]:8080"
        );
    }

    #[test]
    fn a_forwarded_host_is_used_only_from_a_trusted_proxy() {
        let h = headers(&[
            ("host", "127.0.0.1:8080"),
            ("x-forwarded-host", "gantry.example.com"),
        ]);
        assert_eq!(origin(None, &h, false, false), "http://127.0.0.1:8080");
        assert_eq!(origin(None, &h, false, true), "http://gantry.example.com");
    }

    #[test]
    fn an_odd_host_leaves_the_urls_relative() {
        let h = headers(&[("host", "evil.example\"><script>")]);
        assert_eq!(origin(None, &h, false, false), "");
        assert_eq!(origin(None, &HeaderMap::new(), false, false), "");
    }

    #[test]
    fn every_placeholder_is_filled_in() {
        let html = format!(
            r#"<meta property="og:image" content="{ORIGIN_PLACEHOLDER}/og-image.png"><link rel="canonical" href="{ORIGIN_PLACEHOLDER}/">"#
        );
        let filled = with_origin(&html, "https://g.example");
        assert!(!filled.contains(ORIGIN_PLACEHOLDER));
        assert!(filled.contains(r#"content="https://g.example/og-image.png""#));
        assert!(filled.contains(r#"href="https://g.example/""#));
    }
}
