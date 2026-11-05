//! No password, password hash or session token ever reaches a log (PRD §9.2).
//!
//! Its own test binary, so it can install a global subscriber that sees every
//! event, at every level, from every thread.
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gantry_server::AppState;
use gantry_server::crypto::SecretKey;
use gantry_server::store::Store;
use gantry_server::store::users::Role;
use tower::ServiceExt;

const PASSWORD: &str = "correct-horse-battery";
const WRONG: &str = "a-wrong-password-123";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn captured() -> &'static Captured {
    static CAPTURED: OnceLock<Captured> = OnceLock::new();
    CAPTURED.get_or_init(|| {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .init();
        captured
    })
}

fn login_request(password: &str) -> Request<Body> {
    Request::post("/api/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({ "username": "adrian", "password": password }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn no_password_hash_or_token_reaches_a_log() {
    let captured = captured();

    let store = Store::in_memory().await.unwrap();
    store
        .create_user("adrian", PASSWORD, Role::Admin)
        .await
        .unwrap();
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_one(store.pool())
        .await
        .unwrap();
    let key = SecretKey::from_config("a secret key for tests only").unwrap();
    let fleet = gantry_core::Fleet::new(
        gantry_core::DriverRegistry::new(),
        std::sync::Arc::new(gantry_core::MemoryJobLog::new()),
    );
    let app = gantry_server::app(AppState::new(store, key, false, fleet));

    let failed = app.clone().oneshot(login_request(WRONG)).await.unwrap();
    assert_eq!(failed.status(), StatusCode::UNAUTHORIZED);
    let ok = app.clone().oneshot(login_request(PASSWORD)).await.unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let cookie = ok.headers()["set-cookie"].to_str().unwrap().to_owned();
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned();
    let session = Request::get("/api/session")
        .header("cookie", format!("gantry_session={token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(session).await.unwrap().status(),
        StatusCode::OK
    );

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("login failed"), "logs were captured: {logs}");
    assert!(logs.contains("logged in"), "logs were captured: {logs}");
    for secret in [PASSWORD, WRONG, "$argon2", hash.as_str(), token.as_str()] {
        assert!(
            !logs.contains(secret),
            "`{secret}` reached the log:\n{logs}"
        );
    }
}
