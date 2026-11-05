//! Who may do what (PRD §7.6, §9.2). Enforced here, on every API route,
//! whatever the UI shows: the UI only hides what a role can't use.
//!
//! The rule is one table. Reads need a Viewer; everything that changes state
//! needs an Admin unless the table says an Operator may do it. A new route is
//! therefore Admin-only until someone decides otherwise, never open by
//! accident. The WebSocket only ever sends to the browser: messages from the
//! browser are ignored, so it needs nothing beyond a session.
use axum::Json;
use axum::extract::{MatchedPath, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::store::users::{Role, User};

/// The routes that differ from the default (reads: Viewer; changes: Admin).
/// Paths are as routed under `/api`.
pub const POLICY: &[(Method, &str, Role)] = &[
    // Anyone may end their own session.
    (Method::POST, "/logout", Role::Viewer),
    // Operators control Printers and manage Jobs and the Queue.
    (Method::POST, "/printers/{id}/commands", Role::Operator),
    (Method::POST, "/printers/{id}/mark-clear", Role::Operator),
    (Method::POST, "/bulk/commands", Role::Operator),
    // The Print File library, and sending a Plate to a Printer.
    (Method::POST, "/print-files", Role::Operator),
    (Method::PUT, "/print-files/{id}", Role::Operator),
    (Method::DELETE, "/print-files/{id}", Role::Operator),
    (Method::POST, "/printers/{id}/start", Role::Operator),
    // The Queue.
    (Method::POST, "/queue", Role::Operator),
    (Method::PUT, "/queue/order", Role::Operator),
    (Method::DELETE, "/queue/{id}", Role::Operator),
    (Method::POST, "/queue/{id}/start", Role::Operator),
    // The filament inventory.
    (Method::POST, "/spools", Role::Operator),
    (Method::PUT, "/spools/{id}", Role::Operator),
    (Method::DELETE, "/spools/{id}", Role::Operator),
    // Every user manages their own API tokens, two-factor authentication
    // and passkeys; the handlers check whose they are.
    (Method::POST, "/tokens", Role::Viewer),
    (Method::DELETE, "/tokens/{id}", Role::Viewer),
    (Method::POST, "/account/totp/setup", Role::Viewer),
    (Method::POST, "/account/totp/enable", Role::Viewer),
    (Method::POST, "/account/totp/disable", Role::Viewer),
    (Method::POST, "/account/totp/recovery-codes", Role::Viewer),
    (
        Method::POST,
        "/account/passkeys/register/start",
        Role::Viewer,
    ),
    (
        Method::POST,
        "/account/passkeys/register/finish",
        Role::Viewer,
    ),
    (Method::DELETE, "/account/passkeys/{id}", Role::Viewer),
    // Removing a Job's timelapse (the Job stays).
    (Method::DELETE, "/jobs/{id}/timelapse", Role::Operator),
    // Reads only Admins may make.
    (Method::GET, "/webhooks", Role::Admin),
    (Method::GET, "/discovery", Role::Admin),
    (Method::GET, "/users/{id}/passkeys", Role::Admin),
    (Method::GET, "/users", Role::Admin),
    (Method::GET, "/audit", Role::Admin),
    (Method::GET, "/config/export", Role::Admin),
    (Method::GET, "/printers/{id}/simulation", Role::Admin),
    (Method::GET, "/alerts/channels", Role::Admin),
    (Method::GET, "/alerts/rules", Role::Admin),
    (Method::GET, "/alerts/channel-kinds", Role::Admin),
];

/// The least role that may call `method` on `route` (an axum route pattern,
/// with or without the `/api` prefix).
pub fn required_role(method: &Method, route: &str) -> Role {
    let route = route.strip_prefix("/api").unwrap_or(route);
    if let Some((_, _, role)) = POLICY
        .iter()
        .find(|(m, path, _)| m == method && *path == route)
    {
        return *role;
    }
    if method == Method::GET || method == Method::HEAD {
        Role::Viewer
    } else {
        Role::Admin
    }
}

/// Refuses, with 403, a request the user's role doesn't allow. Runs after the
/// session gate, which has put the [`User`] in the request.
pub async fn authorize(request: Request, next: Next) -> Response {
    let Some(user) = request.extensions().get::<User>() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let route = request.extensions().get::<MatchedPath>().map_or_else(
        || request.uri().path().to_owned(),
        |p| p.as_str().to_owned(),
    );
    let needed = required_role(request.method(), &route);
    if user.role < needed {
        tracing::info!(
            username = %user.username,
            role = user.role.as_str(),
            method = %request.method(),
            route = %route,
            "refused: role too low"
        );
        return forbidden(user.role, needed);
    }
    next.run(request).await
}

/// The routes a user who must still set up two-factor authentication may
/// call: their session, logging out, and enrolling.
const BEFORE_TOTP: &[&str] = &[
    "/session",
    "/logout",
    "/account/totp/setup",
    "/account/totp/enable",
];

/// Whether `request` is one a user who must enrol in TOTP may still make.
pub fn allowed_before_totp(request: &Request) -> bool {
    let path = request.uri().path();
    let path = path.strip_prefix("/api").unwrap_or(path);
    BEFORE_TOTP.contains(&path)
}

/// The response for anything else, until they have enrolled.
pub fn enrol_totp_first() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "an Admin requires two-factor authentication: set it up under Account first",
            "enrol_totp": true,
        })),
    )
        .into_response()
}

/// The response for a role that can't do something.
pub fn forbidden(role: Role, needed: Role) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": format!(
                "this needs the {} role; you are {} {}",
                needed.label(),
                if role == Role::Admin { "an" } else { "a" },
                role.label()
            ),
            "needed": needed,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_need_a_viewer_and_changes_an_admin_unless_listed() {
        assert_eq!(required_role(&Method::GET, "/api/printers"), Role::Viewer);
        assert_eq!(required_role(&Method::POST, "/api/printers"), Role::Admin);
        assert_eq!(
            required_role(&Method::DELETE, "/api/printers/{id}"),
            Role::Admin
        );
        assert_eq!(
            required_role(&Method::POST, "/api/printers/{id}/commands"),
            Role::Operator
        );
        assert_eq!(required_role(&Method::GET, "/api/users"), Role::Admin);
        // A route nobody listed is Admin-only if it changes anything.
        assert_eq!(
            required_role(&Method::PATCH, "/api/something/new"),
            Role::Admin
        );
    }
}
