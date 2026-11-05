//! Login, logout and the session gate (PRD §9.2).
//!
//! Every route needs a session except the login page and the assets it loads,
//! `POST /api/login` and `/healthz`. API requests (and the WebSocket upgrade)
//! without one get `401`; page requests are sent to `/login`.
//!
//! Instead of a session, an API request may carry an API token as
//! `Authorization: Bearer gantry_…` (PRD §7.7). It acts as its owner, with
//! the owner's role as it is now, and everything it does is audited with
//! the token's name.
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::client::ClientInfo;
use crate::store::audit::{Action, Entry, Outcome};
use crate::store::users::{LoginCheck, SESSION_DAYS, SecondFactor, User};

/// A 429 with `Retry-After`, for a login that must wait.
fn too_many(wait: std::time::Duration, message: &str) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": message, "retry_after_secs": wait.as_secs().max(1) })),
    )
        .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from(wait.as_secs().max(1)),
    );
    response
}

/// The session cookie's name.
pub const COOKIE: &str = "gantry_session";

/// The session token from the request's cookies, if any.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_owned())
        .filter(|v| !v.is_empty())
}

/// The logged-in user, if the request carries a live session.
pub async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<User> {
    let token = session_token(headers)?;
    match state.store().session_user(&token).await {
        Ok(user) => user,
        Err(e) => {
            tracing::error!(error = %e, "could not look up a session");
            None
        }
    }
}

/// Whether the session cookie is marked `Secure`: whenever the client's
/// connection is HTTPS, to Gantry itself or to a trusted proxy (PRD §9.2),
/// or always when configured so.
fn over_https(state: &AppState, client: Option<&ClientInfo>) -> bool {
    state.secure_cookies() || client.is_some_and(|c| c.https)
}

fn cookie(state: &AppState, client: Option<&ClientInfo>, value: &str, max_age: i64) -> HeaderValue {
    let secure = if over_https(state, client) {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{secure}"
    ))
    .expect("the cookie is plain ASCII")
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "login required" })),
    )
        .into_response()
}

/// The bearer token in `Authorization`, if any.
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// Middleware for the API: refuses requests without a session or a valid API
/// token, and makes the [`User`] available to handlers as an extension.
pub async fn require_api_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let user = match current_user(&state, request.headers()).await {
        Some(user) => Some(user),
        None => match bearer_token(request.headers()) {
            Some(token) => match state.store().api_token_user(token).await {
                Ok(Some(user)) => Some(user),
                Ok(None) => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({ "error": "that API token isn't valid: it may have been revoked or have expired" })),
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!(error = %e, "could not look up an API token");
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            },
            None => None,
        },
    };
    match user {
        Some(user) => {
            if user.must_enrol_totp && !crate::authz::allowed_before_totp(&request) {
                return crate::authz::enrol_totp_first();
            }
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        None => unauthorized(),
    }
}

/// Serves the app's pages: the login page and assets to anyone, everything
/// else only with a session.
pub async fn pages(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path();
    if path == "/login" || crate::assets::is_asset(path) {
        return crate::assets::serve(
            &state,
            request.uri(),
            request.headers(),
            request.extensions().get::<ClientInfo>(),
        )
        .await;
    }
    if path.starts_with("/api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    if current_user(&state, request.headers()).await.is_none() {
        let next = request
            .uri()
            .path_and_query()
            .map_or("/", |p| p.as_str())
            .to_owned();
        let target = if next == "/" {
            "/login".to_owned()
        } else {
            format!("/login?next={}", urlencode(&next))
        };
        return Redirect::to(&target).into_response();
    }
    crate::assets::serve(
        &state,
        request.uri(),
        request.headers(),
        request.extensions().get::<ClientInfo>(),
    )
    .await
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The login form.
#[derive(Deserialize)]
pub struct LoginForm {
    username: String,
    password: String,
    /// A TOTP code or a recovery code, for an account with TOTP on.
    #[serde(default)]
    code: Option<String>,
}

impl std::fmt::Debug for LoginForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginForm")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// `POST /api/login`.
pub async fn login(
    State(state): State<AppState>,
    info: Option<axum::Extension<ClientInfo>>,
    Json(form): Json<LoginForm>,
) -> Response {
    let info = info.map(|i| i.0);
    let client = info.map_or_else(|| "unknown".to_owned(), |i| i.label());
    // One at a time, from the limit check to recording the result.
    let turn = state.login_turn().await;
    // A failed login, with the name tried (which may not be an account) and
    // never the password.
    let failed = |account: Option<&User>, message: &str| {
        let mut entry = match account {
            Some(user) => crate::audit::by(user, Action::LoginFailed),
            None => {
                let mut entry = Entry::new(Action::LoginFailed);
                entry.username = Some(form.username.trim().to_owned());
                entry
            }
        };
        entry.outcome = Outcome::Refused;
        entry.message = Some(message.to_owned());
        entry.detail = Some(json!({ "client": client }));
        entry
    };

    if let Err(wait) = state.clients().check(
        &client,
        state.login_limits().client_attempts_per_minute,
        std::time::Instant::now(),
    ) {
        tracing::warn!(client = %client, "failed logins from one client rate-limited");
        crate::audit::record(
            &state,
            failed(None, "too many failed logins from this client"),
        )
        .await;
        return too_many(
            wait,
            "Too many failed logins from here. Wait a minute and try again.",
        );
    }

    let at = time::OffsetDateTime::now_utc();
    let check = state
        .store()
        .login(
            &form.username,
            &form.password,
            SecondFactor {
                key: Some(state.key()),
                code: form.code.as_deref(),
            },
            state.login_limits(),
            at,
        )
        .await
        .map(|check| match check {
            // Not an account: refused by the same rules as one.
            LoginCheck::Wrong { account: None, .. } => {
                state
                    .ghosts()
                    .failed(&form.username, state.login_limits(), at)
            }
            other => other,
        });
    // Every refusal below counts against the client; a success doesn't.
    if !matches!(
        check,
        Ok(LoginCheck::Ok(_) | LoginCheck::RecoveryCode(_) | LoginCheck::NeedsCode { .. }) | Err(_)
    ) {
        state.clients().failed(&client, std::time::Instant::now());
    }
    drop(turn);
    let user = match check {
        Ok(LoginCheck::Ok(user)) => user,
        Ok(LoginCheck::RecoveryCode(user)) => {
            tracing::info!(username = %user.username, "a recovery code was used to log in");
            crate::audit::record(&state, crate::audit::by(&user, Action::RecoveryCodeUsed)).await;
            user
        }
        // Not a failure: the password was right, and the UI asks for the code.
        Ok(LoginCheck::NeedsCode { .. }) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "enter the code from your authenticator app",
                    "second_factor": "totp",
                })),
            )
                .into_response();
        }
        Ok(LoginCheck::WrongCode { account, locked }) => {
            tracing::warn!(username = %account.username, "login failed: wrong code");
            crate::audit::record(&state, failed(Some(&account), "wrong two-factor code")).await;
            if locked {
                let mut entry = Entry::new(Action::AccountLocked);
                entry.target = Some(account.username.clone());
                entry.detail = Some(json!({
                    "minutes": state.login_limits().lockout.as_secs() / 60,
                    "failures": state.login_limits().max_failures,
                    "client": client,
                }));
                crate::audit::record(&state, entry).await;
            }
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "that code isn't right, or has already been used",
                    "second_factor": "totp",
                })),
            )
                .into_response();
        }
        Ok(LoginCheck::Wrong { account, locked }) => {
            tracing::warn!(username = %form.username, "login failed");
            crate::audit::record(
                &state,
                failed(account.as_ref(), "wrong username or password"),
            )
            .await;
            if let (Some(account), true) = (&account, locked) {
                tracing::warn!(username = %account.username, "account locked after failed logins");
                // Gantry did this, not the person logging in.
                let mut entry = Entry::new(Action::AccountLocked);
                entry.target = Some(account.username.clone());
                entry.detail = Some(json!({
                    "minutes": state.login_limits().lockout.as_secs() / 60,
                    "failures": state.login_limits().max_failures,
                    "client": client,
                }));
                crate::audit::record(&state, entry).await;
            }
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "wrong username or password" })),
            )
                .into_response();
        }
        Ok(LoginCheck::Locked { account, until }) => {
            crate::audit::record(&state, failed(account.as_ref(), "account locked")).await;
            let wait = (until - time::OffsetDateTime::now_utc()).unsigned_abs();
            return too_many(
                wait,
                "This account is locked after too many failed logins. Try again later, or ask an \
                 Admin to unlock it.",
            );
        }
        Ok(LoginCheck::TooSoon { account, wait }) => {
            crate::audit::record(&state, failed(account.as_ref(), "too soon after a failure"))
                .await;
            return too_many(
                wait,
                &format!(
                    "Too many failed logins. Wait {} s and try again.",
                    wait.as_secs().max(1)
                ),
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "login could not reach the database");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    start_session(&state, info.as_ref(), user, json!({ "client": client })).await
}

/// Starts a session for a user who has just logged in (by password or
/// passkey): audits it, and answers with the user and the session cookie.
pub async fn start_session(
    state: &AppState,
    info: Option<&ClientInfo>,
    mut user: User,
    detail: serde_json::Value,
) -> Response {
    let token = match state.store().create_session(&user).await {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(error = %e, "could not create a session");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    tracing::info!(username = %user.username, "logged in");
    let mut entry = crate::audit::by(&user, Action::LoginSucceeded);
    entry.detail = Some(detail);
    crate::audit::record(state, entry).await;
    // The UI sends someone who must still set up TOTP straight to it.
    user.must_enrol_totp = matches!(state.store().totp_required().await, Ok(true))
        && matches!(state.store().totp_enabled(user.id).await, Ok(false));
    let mut response = Json(&user).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie(state, info, &token.0, SESSION_DAYS * 24 * 60 * 60),
    );
    response
}

/// `POST /api/logout`: ends the session server side and clears the cookie.
pub async fn logout(
    State(state): State<AppState>,
    info: Option<axum::Extension<ClientInfo>>,
    headers: HeaderMap,
) -> Response {
    if let Some(token) = session_token(&headers)
        && let Err(e) = state.store().delete_session(&token).await
    {
        tracing::error!(error = %e, "could not end a session");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie(&state, info.as_ref().map(|i| &i.0), "", 0),
    );
    response
}

/// `GET /api/session`: who is logged in.
pub async fn session(axum::Extension(user): axum::Extension<User>) -> Json<User> {
    Json(user)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::store::users::Role;
    use crate::test_support;

    const PASSWORD: &str = "correct-horse-battery";

    async fn app_with_user() -> axum::Router {
        let state = test_support::state().await;
        state
            .store()
            .create_user("adrian", PASSWORD, Role::Admin)
            .await
            .unwrap();
        crate::app(state)
    }

    async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
        app.clone().oneshot(request).await.unwrap()
    }

    fn login_request(password: &str) -> Request<Body> {
        Request::post("/api/login")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "username": "adrian", "password": password }).to_string(),
            ))
            .unwrap()
    }

    async fn login(app: &axum::Router) -> String {
        let response = send(app, login_request(PASSWORD)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers()["set-cookie"].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    fn get(path: &str, cookie: Option<&str>) -> Request<Body> {
        let mut builder = Request::get(path);
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", cookie);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn the_api_refuses_requests_without_a_session() {
        let app = app_with_user().await;
        let response = send(&app, get("/api/session", None)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = send(&app, get("/api/session", Some("gantry_session=forged"))).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_websocket_upgrade_needs_a_session() {
        let app = app_with_user().await;
        let request = Request::get("/api/ws")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, request).await.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn pages_send_the_browser_to_the_login_page() {
        let app = app_with_user().await;
        let response = send(&app, get("/printers?x=1", None)).await;
        assert!(response.status().is_redirection());
        assert_eq!(
            response.headers()["location"],
            "/login?next=/printers%3Fx%3D1"
        );
        let response = send(&app, get("/", None)).await;
        assert_eq!(response.headers()["location"], "/login");
        // The login page itself, and /healthz, are open.
        assert_eq!(
            send(&app, get("/login", None)).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&app, get("/healthz", None)).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_wrong_password_is_refused() {
        let app = app_with_user().await;
        let response = send(&app, login_request("not the password")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get("set-cookie").is_none());
    }

    #[tokio::test]
    async fn the_session_cookie_is_http_only_and_same_site() {
        let app = app_with_user().await;
        let response = send(&app, login_request(PASSWORD)).await;
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        assert!(!cookie.contains("Secure"), "plain HTTP: {cookie}");

        // A forged X-Forwarded-Proto from a client that isn't a trusted
        // proxy changes nothing.
        let mut request = login_request(PASSWORD);
        request
            .headers_mut()
            .insert("x-forwarded-proto", "https".parse().unwrap());
        let response = send(&app, request).await;
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(!cookie.contains("Secure"), "forged: {cookie}");
    }

    fn from_peer(mut request: Request<Body>, peer: &str) -> Request<Body> {
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo::<std::net::SocketAddr>(
                peer.parse().unwrap(),
            ));
        request
    }

    #[tokio::test]
    async fn a_trusted_proxy_saying_https_gets_a_secure_cookie() {
        let state = test_support::state_with(crate::Options {
            trusted_proxies: crate::client::TrustedProxies::parse_list("127.0.0.1").unwrap(),
            ..Default::default()
        })
        .await;
        state
            .store()
            .create_user("adrian", PASSWORD, Role::Admin)
            .await
            .unwrap();
        let app = crate::app(state);
        let https = |peer: &str| {
            let mut request = from_peer(login_request(PASSWORD), peer);
            request
                .headers_mut()
                .insert("x-forwarded-proto", "https".parse().unwrap());
            request
        };
        let response = send(&app, https("127.0.0.1:4000")).await;
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(cookie.contains("; Secure"), "via the proxy: {cookie}");
        // The same header from anywhere else is ignored.
        let response = send(&app, https("192.168.1.50:4000")).await;
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(!cookie.contains("Secure"), "not the proxy: {cookie}");
    }

    #[tokio::test]
    async fn secure_cookies_can_be_forced_by_config() {
        let state = test_support::state().await;
        state
            .store()
            .create_user("adrian", PASSWORD, Role::Admin)
            .await
            .unwrap();
        let state = crate::AppState::new(
            state.store().clone(),
            state.key().clone(),
            true,
            state.fleet().clone(),
        );
        let response = send(&crate::app(state), login_request(PASSWORD)).await;
        assert!(
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("; Secure")
        );
    }

    #[tokio::test]
    async fn logged_in_requests_reach_the_api_and_the_app() {
        let app = app_with_user().await;
        let cookie = login(&app).await;
        let response = send(&app, get("/api/session", Some(&cookie))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let user: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(user["username"], "adrian");
        assert_eq!(user["role"], "admin");
        assert_eq!(
            send(&app, get("/", Some(&cookie))).await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn logout_ends_the_session_server_side() {
        let app = app_with_user().await;
        let cookie = login(&app).await;
        let request = Request::post("/api/logout")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap();
        let response = send(&app, request).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .contains("Max-Age=0")
        );
        // The old cookie no longer works, even if the browser kept it.
        let response = send(&app, get("/api/session", Some(&cookie))).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
