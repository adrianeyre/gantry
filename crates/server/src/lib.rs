//! Gantry's server: axum HTTP and WebSocket, serving the embedded frontend.

pub mod alerts;
pub mod analytics;
pub mod api;
mod assets;
pub mod audit;
pub mod auth;
pub mod authz;
pub mod camera;
#[cfg(test)]
mod camera_http_tests;
#[cfg(test)]
mod camera_tests;
pub mod client;
pub mod compatibility;
pub mod config;
pub mod config_io;
pub mod crypto;
pub mod discovery;
pub mod dispatch;
pub mod history;
pub mod login_limit;
pub mod metrics;
pub mod print_files;
pub mod retention;
pub mod store;
pub mod timelapse;
pub mod tls;
pub mod totp;
pub mod webauthn;
pub mod webhooks;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing::{get, post};

use gantry_core::{DriverRegistry, Fleet};

use crate::config::Config;

/// Gantry's version: the release's, which the release workflow passes in
/// as `GANTRY_VERSION` when it builds (semantic-release decides it from the
/// commits), or else the workspace's.
pub const VERSION: &str = match option_env!("GANTRY_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};
use crate::crypto::SecretKey;
use crate::store::{KeyCheck, Store};

/// Server settings the state is built with.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub secure_cookies: bool,
    pub login: store::users::LoginLimits,
    /// Whose forwarded headers are believed.
    pub trusted_proxies: client::TrustedProxies,
    /// Where the Print File library keeps its files. A fresh temporary
    /// folder when not given (tests).
    pub data_dir: Option<std::path::PathBuf>,
    /// Waits between tries of a failed Alert delivery.
    pub alert_retry: Option<alerts::Retry>,
    /// Where people reach Gantry, for the absolute URLs in link previews.
    pub public_url: Option<String>,
    /// The bearer token `/metrics` needs, when metrics are on.
    pub metrics_token: Option<String>,
    /// Metrics have an address of their own, so `/metrics` is not served
    /// beside the dashboard.
    pub metrics_elsewhere: bool,
    /// Waits between tries of a failed webhook delivery.
    pub webhook_backoff: Option<webhooks::Backoff>,
    /// Listen for Printers on the LAN (when the Admin's setting allows).
    /// Only a real server does; tests feed announcements in directly.
    pub lan_discovery: bool,
}

/// Everything a request handler can reach. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    key: SecretKey,
    secure_cookies: bool,
    fleet: Fleet,
    login: store::users::LoginLimits,
    trusted_proxies: client::TrustedProxies,
    public_url: Option<String>,
    metrics_token: Option<String>,
    metrics_elsewhere: bool,
    clients: login_limit::ClientLimiter,
    ghosts: login_limit::Ghosts,
    /// Logins are checked one at a time, so concurrent attempts can't all
    /// read the same failure count and slip past the lockout.
    login_lock: tokio::sync::Mutex<()>,
    cameras: camera::CameraRelay,
    library: print_files::Library,
    alert_retry: alerts::Retry,
    desktop_alerts: alerts::DesktopAlerts,
    webhook_backoff: webhooks::Backoff,
    challenges: webauthn::Challenges,
    timelapses: timelapse::Timelapses,
    discovered: discovery::Discovered,
    discovery_changed: tokio::sync::Notify,
    /// Queued Jobs being started right now, so two starts (an Operator's
    /// and Auto-start's) never take the same one.
    claims: std::sync::Mutex<std::collections::HashSet<i64>>,
    /// Turns true when the server starts shutting down, so streams that never
    /// end on their own (the WebSocket, cameras) let graceful shutdown finish.
    stopping: tokio::sync::watch::Sender<bool>,
}

impl AppState {
    /// Opens the database named by `config` and checks the secret key against
    /// it. Fails, with a message saying how to fix it, when there is no key.
    /// Then starts every stored Printer with the Drivers in `drivers`.
    pub async fn start(config: &Config, drivers: DriverRegistry) -> anyhow::Result<Self> {
        let key = config.require_secret_key()?;
        let store = Store::open(&config.database_path()).await.map_err(|e| {
            anyhow::anyhow!(
                "could not open the database {}: {e}",
                config.database_path().display()
            )
        })?;
        match store.check_secret_key(&key).await? {
            KeyCheck::Recorded | KeyCheck::Matches => {}
            KeyCheck::Changed => tracing::warn!(
                "the secret key is not the one stored Printer credentials were encrypted with; \
                 those credentials must be re-entered"
            ),
        }
        let log = Arc::new(store::jobs::SqlJobLog(store.clone()));
        let state = Self::with_options(
            store,
            key,
            Fleet::new(drivers, log),
            Options {
                secure_cookies: config.secure_cookies,
                login: config.login,
                trusted_proxies: config.trusted_proxies.clone(),
                data_dir: Some(config.data_dir.clone()),
                alert_retry: None,
                public_url: config.public_url.clone(),
                metrics_token: config.metrics_token.clone(),
                metrics_elsewhere: config.metrics_bind.is_some(),
                webhook_backoff: None,
                lan_discovery: true,
            },
        );
        api::settings::apply(&state).await?;
        state.start_printers().await?;
        Ok(state)
    }

    /// Builds the state from parts already open, with default login limits.
    /// No Printer is started.
    pub fn new(store: Store, key: SecretKey, secure_cookies: bool, fleet: Fleet) -> Self {
        Self::with_options(
            store,
            key,
            fleet,
            Options {
                secure_cookies,
                ..Options::default()
            },
        )
    }

    /// Builds the state from parts already open, and starts the Alert task.
    /// No Printer is started. Must be called within a tokio runtime.
    pub fn with_options(store: Store, key: SecretKey, fleet: Fleet, options: Options) -> Self {
        let data_dir = options.data_dir.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("gantry-{}", uuid::Uuid::new_v4()))
        });
        let state = Self {
            inner: Arc::new(Inner {
                store,
                key,
                secure_cookies: options.secure_cookies,
                fleet,
                login: options.login,
                trusted_proxies: options.trusted_proxies,
                public_url: options.public_url,
                metrics_token: options.metrics_token,
                metrics_elsewhere: options.metrics_elsewhere,
                clients: login_limit::ClientLimiter::new(),
                ghosts: login_limit::Ghosts::new(),
                login_lock: tokio::sync::Mutex::new(()),
                cameras: camera::CameraRelay::new(),
                alert_retry: options.alert_retry.clone().unwrap_or_default(),
                desktop_alerts: tokio::sync::broadcast::Sender::new(64),
                webhook_backoff: options.webhook_backoff.clone().unwrap_or_default(),
                challenges: webauthn::Challenges::default(),
                library: print_files::Library::new(&data_dir),
                timelapses: timelapse::Timelapses::new(&data_dir),
                discovered: discovery::Discovered::default(),
                discovery_changed: tokio::sync::Notify::new(),
                claims: std::sync::Mutex::new(std::collections::HashSet::new()),
                stopping: tokio::sync::watch::Sender::new(false),
            }),
        };
        alerts::spawn(&state);
        dispatch::spawn(&state);
        history::spawn(&state);
        retention::spawn(&state);
        timelapse::spawn(&state);
        if options.lan_discovery {
            discovery::spawn(&state);
        }
        state
    }

    /// Starts every stored Printer. A Printer whose Driver isn't compiled in
    /// is skipped with a warning rather than stopping Gantry.
    pub async fn start_printers(&self) -> anyhow::Result<()> {
        for id in self.store().printer_ids().await? {
            if let Err(e) = api::printers::start(self, &id).await {
                tracing::warn!(printer = %id, error = %e, "could not start a Printer");
            }
        }
        Ok(())
    }

    /// Resolves once the server has started shutting down.
    pub async fn stopping(&self) {
        let mut rx = self.inner.stopping.subscribe();
        let _ = rx.wait_for(|stopping| *stopping).await;
    }

    /// Tells every long-lived stream to end.
    pub fn begin_shutdown(&self) {
        self.inner.stopping.send_replace(true);
    }

    /// The bearer token `/metrics` needs, when one is set.
    pub fn metrics_token(&self) -> Option<&str> {
        self.inner.metrics_token.as_deref()
    }

    /// Claims a queued Job for starting: `None` if something else is
    /// starting it. The claim ends when the guard is dropped.
    pub fn claim(&self, queued: i64) -> Option<Claim> {
        let mut claims = self.inner.claims.lock().expect("claims lock");
        claims.insert(queued).then(|| Claim {
            state: self.clone(),
            queued,
        })
    }

    /// The queued Jobs being started right now.
    pub fn claimed(&self) -> std::collections::HashSet<i64> {
        self.inner.claims.lock().expect("claims lock").clone()
    }

    /// The cameras being relayed.
    pub fn cameras(&self) -> &camera::CameraRelay {
        &self.inner.cameras
    }

    /// How failed Alert deliveries are retried.
    pub fn alert_retry(&self) -> &alerts::Retry {
        &self.inner.alert_retry
    }

    /// The Printers found on the LAN.
    pub fn discovered(&self) -> &discovery::Discovered {
        &self.inner.discovered
    }

    /// Resolves when the discovery setting may have changed.
    pub async fn discovery_changed(&self) {
        self.inner.discovery_changed.notified().await;
    }

    /// Says the discovery setting may have changed.
    pub fn notify_discovery(&self) {
        self.inner.discovery_changed.notify_waiters();
    }

    /// Where timelapses' frames are kept.
    pub fn timelapses(&self) -> &timelapse::Timelapses {
        &self.inner.timelapses
    }

    /// Passkey challenges waiting for an answer.
    pub fn challenges(&self) -> &webauthn::Challenges {
        &self.inner.challenges
    }

    /// How failed webhook deliveries are retried.
    pub fn webhook_backoff(&self) -> &webhooks::Backoff {
        &self.inner.webhook_backoff
    }

    /// Where desktop-channel Alerts are handed to the open desktop apps.
    pub fn desktop_alerts(&self) -> &alerts::DesktopAlerts {
        &self.inner.desktop_alerts
    }

    /// The Print File library's files.
    pub fn library(&self) -> &print_files::Library {
        &self.inner.library
    }

    /// The running Fleet.
    pub fn fleet(&self) -> &Fleet {
        &self.inner.fleet
    }

    /// The database.
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// The key that encrypts Printer credentials.
    pub fn key(&self) -> &SecretKey {
        &self.inner.key
    }

    /// The login rate limits and lockout thresholds.
    pub fn login_limits(&self) -> &store::users::LoginLimits {
        &self.inner.login
    }

    /// The proxies whose forwarded headers are believed.
    pub fn trusted_proxies(&self) -> &client::TrustedProxies {
        &self.inner.trusted_proxies
    }

    /// The configured address people reach Gantry at, if any.
    pub fn public_url(&self) -> Option<&str> {
        self.inner.public_url.as_deref()
    }

    /// Login attempts per client.
    pub fn clients(&self) -> &login_limit::ClientLimiter {
        &self.inner.clients
    }

    /// Failed logins for names that aren't accounts.
    pub fn ghosts(&self) -> &login_limit::Ghosts {
        &self.inner.ghosts
    }

    /// Held while one login is checked and its result recorded.
    pub async fn login_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.login_lock.lock().await
    }

    /// Whether session cookies are always `Secure`.
    pub fn secure_cookies(&self) -> bool {
        self.inner.secure_cookies
    }
}

/// A queued Job being started; see [`AppState::claim`].
pub struct Claim {
    state: AppState,
    queued: i64,
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut claims) = self.state.inner.claims.lock() {
            claims.remove(&self.queued);
        }
    }
}

/// Builds the HTTP application.
pub fn app(state: AppState) -> Router {
    // Everything here needs a session, the WebSocket upgrade included.
    let api = Router::new()
        .route("/session", get(auth::session))
        .route("/logout", post(auth::logout))
        .route("/ws", get(ws::upgrade))
        .merge(api::routes())
        // Layers run outside in: the session gate first, then the role check.
        .route_layer(middleware::from_fn(authz::authorize))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_session,
        ))
        // Added after the layer, so they are the API routes open to all.
        .route("/login", post(auth::login))
        .route("/login/passkey/start", post(api::passkeys::login_start))
        .route("/login/passkey/finish", post(api::passkeys::login_finish))
        .route("/openapi.json", get(api::openapi::serve));

    let metrics_here = state.inner.metrics_token.is_some() && !state.inner.metrics_elsewhere;
    let router = Router::new().nest("/api", api);
    // Off by default; beside the dashboard only with a token (PRD §9.2).
    let router = if metrics_here {
        router.route("/metrics", get(metrics::serve))
    } else {
        router
    };
    router
        .route("/healthz", get(healthz))
        .fallback(auth::pages)
        // Outermost, so every handler and the session gate know the client.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            client::identify,
        ))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

/// Serves the app on `listener` until the process is told to stop, then ends
/// the long-lived streams so in-flight requests can finish.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let stopping = state.clone();
    // The peer's address is how login attempts are counted per client.
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        stopping.begin_shutdown();
    })
    .await
}

pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    pub const PASSWORD: &str = "a password for tests";

    /// A state over a fresh in-memory database, with these options.
    pub async fn state_with(options: crate::Options) -> AppState {
        let store = Store::in_memory().await.unwrap();
        let key = SecretKey::from_config("a secret key for tests only").unwrap();
        let log = Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
        AppState::with_options(store, key, Fleet::new(drivers(), log), options)
    }

    /// A state over a fresh in-memory database.
    pub async fn state() -> AppState {
        let store = Store::in_memory().await.unwrap();
        let key = SecretKey::from_config("a secret key for tests only").unwrap();
        let log = Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
        AppState::new(store, key, false, Fleet::new(drivers(), log))
    }

    /// A state whose registry also has these real Drivers (`bambu`,
    /// `moonraker`, `octoprint`), whose Printers tests never start.
    pub async fn state_with_drivers(ids: &[&str]) -> AppState {
        let store = Store::in_memory().await.unwrap();
        let key = SecretKey::from_config("a secret key for tests only").unwrap();
        let log = Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
        let mut registry = drivers();
        for id in ids {
            match *id {
                "bambu" => registry.register(gantry_driver_bambu::BambuDriver),
                "moonraker" => registry.register(gantry_driver_moonraker::MoonrakerDriver),
                "octoprint" => registry.register(gantry_driver_octoprint::OctoPrintDriver),
                other => panic!("no test Driver `{other}`"),
            };
        }
        AppState::with_options(
            store,
            key,
            Fleet::new(registry, log),
            crate::Options::default(),
        )
    }

    /// The Drivers tests run with.
    pub fn drivers() -> DriverRegistry {
        let mut drivers = DriverRegistry::new();
        drivers
            .register(gantry_driver_simulated::SimulatedDriver)
            .register(InertDriver);
        drivers
    }

    /// The id of [`InertDriver`].
    pub const INERT: &str = "inert";

    /// A test double: a Driver that is not the Simulated Driver, whose
    /// Printers connect and then do nothing.
    pub struct InertDriver;

    struct InertHandle;

    impl gantry_core::Driver for InertDriver {
        fn id(&self) -> &'static str {
            INERT
        }
        fn name(&self) -> &'static str {
            "Inert"
        }
        fn fields(&self) -> Vec<gantry_core::FieldSpec> {
            vec![]
        }
        fn connect(
            &self,
            _: gantry_core::PrinterConfig,
            events: gantry_core::EventSender,
        ) -> Box<dyn gantry_core::PrinterHandle> {
            let mut status =
                gantry_core::PrinterStatus::disconnected(gantry_core::DisconnectReason::Stopped);
            status.connection = gantry_core::Connection::Connected;
            status.state = Some(gantry_core::PrinterState::Idle);
            events.status(status);
            Box::new(InertHandle)
        }
    }

    #[async_trait::async_trait]
    impl gantry_core::PrinterHandle for InertHandle {
        async fn pause(&self) -> Result<(), gantry_core::CommandError> {
            Err(gantry_core::CommandError::Unsupported)
        }
        async fn resume(&self) -> Result<(), gantry_core::CommandError> {
            Err(gantry_core::CommandError::Unsupported)
        }
        async fn cancel(&self) -> Result<(), gantry_core::CommandError> {
            Err(gantry_core::CommandError::Unsupported)
        }
        async fn shutdown(&self) {}
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// A client with no session, as a visitor who hasn't logged in.
    pub fn anonymous(state: &AppState) -> Client {
        Client {
            state: state.clone(),
            app: app(state.clone()),
            cookie: String::new(),
            bearer: None,
        }
    }

    /// A client with no session that sends `token` as a bearer API token.
    pub fn with_token(state: &AppState, token: &str) -> Client {
        Client {
            bearer: Some(format!("Bearer {token}")),
            ..anonymous(state)
        }
    }

    /// The app, with a logged-in Admin.
    pub struct Client {
        pub state: AppState,
        app: Router,
        cookie: String,
        bearer: Option<String>,
    }

    impl Client {
        pub async fn new() -> Self {
            Self::with_state(state().await).await
        }

        pub async fn with_state(state: AppState) -> Self {
            Self::with_user(state, "admin", crate::store::users::Role::Admin).await
        }

        /// A client logged in as `username`, created with `role` if new.
        pub async fn with_user(
            state: AppState,
            username: &str,
            role: crate::store::users::Role,
        ) -> Self {
            let existing = state
                .store()
                .authenticate(username, PASSWORD)
                .await
                .unwrap();
            if existing.is_none() {
                state
                    .store()
                    .create_user(username, PASSWORD, role)
                    .await
                    .unwrap();
            }
            let app = app(state.clone());
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/login")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::json!({ "username": username, "password": PASSWORD })
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "login as {username}");
            let cookie = response.headers()["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            Self {
                state,
                app,
                cookie,
                bearer: None,
            }
        }

        /// Adds a Simulated Printer with an access code (which the Simulated
        /// Driver ignores) and returns its id.
        pub async fn add_simulated_with_secret(&self, name: &str, access_code: &str) -> String {
            let (status, body) = self
                .post(
                    "/api/printers",
                    serde_json::json!({ "name": name, "driver": "simulated", "settings": {},
                                        "secrets": { "access_code": access_code } }),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            body["id"].as_str().unwrap().to_owned()
        }

        /// Another client on the same server, logged in with `role`.
        pub async fn as_role(&self, role: crate::store::users::Role) -> Self {
            Self::with_user(self.state.clone(), role.as_str(), role).await
        }

        pub async fn send(
            &self,
            method: &str,
            path: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", &self.cookie);
            if let Some(bearer) = &self.bearer {
                request = request.header("authorization", bearer);
            }
            if body.is_some() {
                request = request.header("content-type", "application/json");
            }
            let request = request
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let response = self.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json = if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    serde_json::Value::String(String::from_utf8_lossy(&bytes).into())
                })
            };
            (status, json)
        }

        /// Logs in with `body` as `POST /api/login` would be sent, and
        /// returns the answer and the session cookie, if one was set.
        pub async fn login_with(
            state: &AppState,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value, Option<String>) {
            let response = app(state.clone())
                .oneshot(
                    Request::post("/api/login")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let cookie = response
                .headers()
                .get("set-cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(str::to_owned);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, json, cookie)
        }

        /// A client using a session cookie already made.
        pub fn with_cookie(state: &AppState, cookie: &str) -> Self {
            Self {
                cookie: cookie.to_owned(),
                ..anonymous(state)
            }
        }

        /// A GET whose response body is left for the test to read.
        pub async fn raw_get(&self, path: &str) -> axum::response::Response {
            let request = Request::get(path)
                .header("cookie", &self.cookie)
                .body(Body::empty())
                .unwrap();
            self.app.clone().oneshot(request).await.unwrap()
        }

        pub async fn get(&self, path: &str) -> (StatusCode, serde_json::Value) {
            self.send("GET", path, None).await
        }

        pub async fn post(
            &self,
            path: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            self.send("POST", path, Some(body)).await
        }

        pub async fn put(
            &self,
            path: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            self.send("PUT", path, Some(body)).await
        }

        pub async fn delete(&self, path: &str) -> (StatusCode, serde_json::Value) {
            self.send("DELETE", path, None).await
        }

        /// A POST whose body is raw bytes, as a file upload sends.
        pub async fn upload(&self, path: &str, bytes: &[u8]) -> (StatusCode, serde_json::Value) {
            let request = Request::post(path)
                .header("cookie", &self.cookie)
                .header("content-type", "application/octet-stream")
                .body(Body::from(bytes.to_vec()))
                .unwrap();
            let response = self.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, json)
        }

        /// Waits up to 5 s for a Printer's view to satisfy `pred`.
        pub async fn until(
            &self,
            id: &str,
            pred: impl Fn(&gantry_core::PrinterView) -> bool,
        ) -> gantry_core::PrinterView {
            (*self
                .state
                .fleet()
                .wait_for(id, std::time::Duration::from_secs(5), pred)
                .await
                .unwrap())
            .clone()
        }

        /// Drives a Simulated Printer through the simulation endpoint.
        pub async fn simulate(&self, id: &str, action: serde_json::Value) {
            let (status, body) = self
                .post(&format!("/api/printers/{id}/simulation"), action)
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        }

        /// Adds a Simulated Printer and waits for it to connect.
        pub async fn connected_simulated(&self, name: &str, settings: serde_json::Value) -> String {
            let id = self.add_simulated(name, settings).await;
            self.until(&id, |v| v.status.connection.is_connected())
                .await;
            id
        }

        /// Adds a Simulated Printer and returns its id.
        pub async fn add_simulated(&self, name: &str, settings: serde_json::Value) -> String {
            let (status, body) = self
                .post(
                    "/api/printers",
                    serde_json::json!({ "name": name, "driver": "simulated", "settings": settings }),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            body["id"].as_str().unwrap().to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_is_ok() {
        let response = app(test_support::state().await)
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_login_page_serves_the_frontend() {
        let response = app(test_support::state().await)
            .oneshot(Request::get("/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("<title>Gantry · 3D printer fleet dashboard</title>"));
        // Every link-preview URL is filled in before the page leaves the server.
        assert!(!html.contains(crate::assets::ORIGIN_PLACEHOLDER));
    }

    #[tokio::test]
    async fn starting_without_a_secret_key_fails_with_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap().to_owned();
        let config = Config::load(None, move |k| {
            (k == "GANTRY_DATA_DIR").then(|| data.clone())
        })
        .unwrap();
        let err = AppState::start(&config, DriverRegistry::new())
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("GANTRY_SECRET_KEY"), "{err}");
        assert!(!config.database_path().exists());
    }
}
