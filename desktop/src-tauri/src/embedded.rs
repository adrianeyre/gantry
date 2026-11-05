//! Embedded mode: Gantry's core and server inside the desktop app, for a
//! single machine (ADR-0008). It is the same server the `gantry` binary runs,
//! listening only on 127.0.0.1, with its data in the app's data folder.
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::Context;
use gantry_core::DriverRegistry;
use gantry_server::AppState;
use gantry_server::config::Config;
use gantry_server::store::users::Role;

/// The server settings file in the data folder. The desktop app sets the
/// bind address, data folder and secret key itself; anything else in it
/// (login limits, for example) applies as it would for `gantry`.
pub const CONFIG_FILE: &str = "gantry.toml";

const CONFIG_TEMPLATE: &str = "\
# Server settings for Gantry's embedded mode, read when the desktop app starts.
# The app sets the bind address (127.0.0.1 only), the data folder and the
# secret key (kept in the system keychain) itself, so leave those out.
";

/// How to start the embedded server.
pub struct EmbeddedOptions {
    /// Where the database, Print Files and server settings live.
    pub data_dir: PathBuf,
    /// Encrypts Printer credentials (ADR-0005).
    pub secret_key: String,
    /// A port to try first, so the address stays the same between runs. Any
    /// free port is used when it is taken.
    pub preferred_port: Option<u16>,
    pub drivers: DriverRegistry,
}

/// A running embedded server.
pub struct Embedded {
    state: AppState,
    address: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
    first_admin: tokio::sync::Mutex<()>,
}

/// Why the first Admin could not be created.
#[derive(Debug, thiserror::Error)]
pub enum FirstAdminError {
    #[error("an account already exists: log in with it instead")]
    AccountsExist,
    #[error(transparent)]
    Create(#[from] gantry_server::store::users::CreateUserError),
    #[error("could not read the accounts: {0}")]
    Database(String),
}

impl Embedded {
    /// Starts the core and server. Fails, as `gantry` does, if the secret
    /// key is missing or does not suit the database.
    pub async fn start(options: EmbeddedOptions) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&options.data_dir).with_context(|| {
            format!(
                "could not create the data folder {}",
                options.data_dir.display()
            )
        })?;
        let config_file = options.data_dir.join(CONFIG_FILE);
        if !config_file.exists() {
            std::fs::write(&config_file, CONFIG_TEMPLATE)
                .with_context(|| format!("could not write {}", config_file.display()))?;
        }
        let listener = bind(options.preferred_port).await?;
        let address = listener.local_addr()?;

        // Only these three come from the app; nothing is read from the
        // process environment, so a GANTRY_BIND meant for a server elsewhere
        // can't make the embedded one listen beyond this machine.
        let data_dir = options.data_dir.to_string_lossy().into_owned();
        let secret_key = options.secret_key;
        let config = Config::load(Some(&config_file), move |name| match name {
            "GANTRY_BIND" => Some(address.to_string()),
            "GANTRY_DATA_DIR" => Some(data_dir.clone()),
            "GANTRY_SECRET_KEY" => Some(secret_key.clone()),
            _ => None,
        })?;
        let state = AppState::start(&config, options.drivers).await?;

        let (shutdown, stop) = tokio::sync::oneshot::channel::<()>();
        let app =
            gantry_server::app(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let stopping = state.clone();
        let server = tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop.await;
                    stopping.begin_shutdown();
                })
                .await;
            if let Err(e) = result {
                tracing::error!(error = %e, "the embedded server stopped");
            }
        });
        tracing::info!(address = %address, data_dir = %config.data_dir.display(), "embedded Gantry listening");
        Ok(Self {
            state,
            address,
            shutdown: Some(shutdown),
            server,
            first_admin: tokio::sync::Mutex::new(()),
        })
    }

    /// Where the server listens: always 127.0.0.1.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The dashboard's address, such as `http://127.0.0.1:51234`.
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Whether there is no account yet, so the first Admin must be created.
    pub async fn needs_first_admin(&self) -> Result<bool, FirstAdminError> {
        let users = self
            .state
            .store()
            .users()
            .await
            .map_err(|e| FirstAdminError::Database(e.to_string()))?;
        Ok(users.is_empty())
    }

    /// Creates the first account, an Admin, as `gantry user create --admin`
    /// does. Only while there is no account at all: the server never offers
    /// this (PRD §9.2), and the desktop app only does because its server
    /// listens on 127.0.0.1 alone.
    pub async fn create_first_admin(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(), FirstAdminError> {
        let _one_at_a_time = self.first_admin.lock().await;
        if !self.needs_first_admin().await? {
            return Err(FirstAdminError::AccountsExist);
        }
        let user = self
            .state
            .store()
            .create_user(username, password, Role::Admin)
            .await?;
        tracing::info!(username = %user.username, "created the first Admin account");
        Ok(())
    }

    /// Stops the server and every Printer's connection.
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = (&mut self.server).await;
        self.state.fleet().stop_all().await;
    }
}

/// Listens on 127.0.0.1, on `preferred` if it is free, else any free port.
async fn bind(preferred: Option<u16>) -> anyhow::Result<tokio::net::TcpListener> {
    if let Some(port) = preferred.filter(|p| *p != 0)
        && let Ok(listener) = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await
    {
        return Ok(listener);
    }
    tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("could not listen on 127.0.0.1")
}

/// The data folder inside the app's data folder.
pub fn data_dir(app_data: &Path) -> PathBuf {
    app_data.join("server")
}
