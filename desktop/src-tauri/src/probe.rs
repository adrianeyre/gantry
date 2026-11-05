//! Checks that an address really is a Gantry server before the window goes
//! there, so the app never opens (and grants desktop Alerts to) some other
//! site by mistake.
use std::time::Duration;

use reqwest::StatusCode;

use crate::server_url::ServerUrl;

/// Why the address is not a Gantry server the app can use.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("could not reach {server}: {reason}")]
    Unreachable { server: String, reason: String },
    #[error("{server} answered, but it is not a Gantry server ({reason})")]
    NotGantry { server: String, reason: String },
}

/// An HTTP client for the check: short timeouts, no redirects (a Gantry
/// server answers both checks itself).
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("gantry-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("the HTTP client builds")
}

/// A Gantry server answers `GET /healthz` with `ok`, and refuses
/// `GET /api/session` with 401 to a client with no session.
pub async fn check_gantry(client: &reqwest::Client, server: &ServerUrl) -> Result<(), ProbeError> {
    let unreachable = |e: reqwest::Error| ProbeError::Unreachable {
        server: server.to_string(),
        reason: describe(&e),
    };
    let not_gantry = |reason: String| ProbeError::NotGantry {
        server: server.to_string(),
        reason,
    };

    let health = client
        .get(server.join("/healthz"))
        .send()
        .await
        .map_err(unreachable)?;
    let status = health.status();
    let body = health.text().await.map_err(unreachable)?;
    if status != StatusCode::OK || body.trim() != "ok" {
        return Err(not_gantry(format!("/healthz answered {status}")));
    }

    let session = client
        .get(server.join("/api/session"))
        .send()
        .await
        .map_err(unreachable)?;
    if session.status() != StatusCode::UNAUTHORIZED {
        return Err(not_gantry(format!(
            "/api/session answered {} instead of asking for a login",
            session.status()
        )));
    }
    Ok(())
}

/// The innermost cause, which says what actually went wrong (refused,
/// timed out, a certificate the system doesn't trust).
fn describe(error: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = error;
    while let Some(next) = source.source() {
        source = next;
    }
    if error.is_timeout() {
        "it did not answer in time".to_owned()
    } else {
        source.to_string()
    }
}
