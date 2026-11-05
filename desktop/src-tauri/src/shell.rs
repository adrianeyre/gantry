//! The Tauri side: the window, its menu, the commands the bundled "choose a
//! server" page calls, and the runtime capability that lets the chosen
//! server (and only it) show native desktop Alerts.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::CapabilityBuilder;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::desktop_config::{DesktopConfig, Mode};
use crate::embedded::{Embedded, EmbeddedOptions};
use crate::navigation::{self, Navigation};
use crate::probe;
use crate::secret_key::{self, OsKeychain};
use crate::server_url::ServerUrl;

const WINDOW: &str = "main";
const CHANGE_SERVER: &str = "change-server";
const RELOAD: &str = "reload";

/// What the app holds while it runs.
struct Shell {
    config_dir: PathBuf,
    data_dir: PathBuf,
    /// The server the window may show, read by the navigation handler.
    server: Arc<Mutex<Option<ServerUrl>>>,
    /// Origins already given the desktop Alert capability.
    granted: Mutex<HashSet<String>>,
    embedded: tokio::sync::Mutex<Option<Embedded>>,
    http: reqwest::Client,
}

/// What the bundled page shows next.
#[derive(Debug, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
enum Step {
    /// Choose a mode, with what was chosen before and why it didn't open.
    Choose {
        server: Option<String>,
        embedded: bool,
        error: Option<String>,
    },
    /// The embedded server has no account yet.
    FirstAdmin,
    /// The window is on its way to the dashboard.
    Opened,
}

pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            resume,
            choices,
            connect_remote,
            start_embedded,
            create_first_admin
        ])
        .setup(|app| {
            let paths = app.path();
            let shell = Shell {
                config_dir: paths.app_config_dir()?,
                data_dir: crate::embedded::data_dir(&paths.app_data_dir()?),
                server: Arc::new(Mutex::new(None)),
                granted: Mutex::new(HashSet::new()),
                embedded: tokio::sync::Mutex::new(None),
                http: probe::client(),
            };
            let server = shell.server.clone();
            app.manage(shell);
            build_window(app.handle(), server)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the desktop app builds");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let shell = app.state::<Shell>();
            tauri::async_runtime::block_on(async {
                if let Some(embedded) = shell.embedded.lock().await.take() {
                    embedded.stop().await;
                }
            });
        }
    });
}

fn build_window(app: &AppHandle, server: Arc<Mutex<Option<ServerUrl>>>) -> tauri::Result<()> {
    let menu = menu(app)?;
    let outside = app.clone();
    let popups = app.clone();
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
        .title("Gantry")
        .inner_size(1280.0, 840.0)
        .min_inner_size(400.0, 500.0)
        .menu(menu)
        .on_navigation(move |url| {
            let current = server.lock().expect("server lock").clone();
            match navigation::decide(url, current.as_ref()) {
                Navigation::Allow => true,
                Navigation::OpenOutside => {
                    open_outside(&outside, url);
                    false
                }
                Navigation::Block => {
                    tracing::warn!(url = %url, "blocked a navigation");
                    false
                }
            }
        })
        .on_new_window(move |url, _features| {
            // Links that open a new window (target=_blank) go to the
            // system browser.
            open_outside(&popups, &url);
            NewWindowResponse::Deny
        })
        .build()?;
    window.on_menu_event(|window, event| match event.id().as_ref() {
        CHANGE_SERVER => {
            if let Err(e) = show_choices(window.app_handle()) {
                tracing::error!(error = %e, "could not show the server choice");
            }
        }
        RELOAD => {
            if let Ok(webview) = main_window(window.app_handle()) {
                let _ = webview.reload();
            }
        }
        _ => {}
    });
    Ok(())
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    let change = MenuItem::with_id(
        app,
        CHANGE_SERVER,
        "Change Server…",
        true,
        Some("CmdOrCtrl+Shift+S"),
    )?;
    let reload = MenuItem::with_id(app, RELOAD, "Reload", true, Some("CmdOrCtrl+R"))?;
    let separator = PredefinedMenuItem::separator(app)?;
    let gantry = Submenu::with_items(app, "Server", true, &[&change, &separator, &reload])?;
    menu.append(&gantry)?;
    Ok(menu)
}

fn open_outside(app: &AppHandle, url: &Url) {
    if matches!(url.scheme(), "http" | "https" | "mailto")
        && let Err(e) = app.opener().open_url(url.as_str(), None::<&str>)
    {
        tracing::warn!(error = %e, "could not open a link in the system browser");
    }
}

fn main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    app.get_webview_window(WINDOW)
        .ok_or_else(|| "the window is gone".to_owned())
}

/// Back to the bundled page, to choose another server.
fn show_choices(app: &AppHandle) -> Result<(), String> {
    let window = main_window(app)?;
    let mut page = window.url().map_err(|e| e.to_string())?;
    if !navigation::is_app_page(&page) {
        page = app_page_url();
    }
    page.set_path("/index.html");
    page.set_query(Some("choose"));
    window.navigate(page).map_err(|e| e.to_string())
}

/// The bundled page's address on this platform.
fn app_page_url() -> Url {
    let base = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };
    Url::parse(base).expect("a valid URL")
}

/// Lets `server`'s pages call the notification plugin, and nothing else, so
/// the dashboard can show desktop Alerts. Any other site gets no IPC at all.
fn grant(app: &AppHandle, shell: &Shell, server: &ServerUrl) -> Result<(), String> {
    let origin = server.origin();
    let mut granted = shell.granted.lock().expect("granted lock");
    if granted.contains(&origin) {
        return Ok(());
    }
    let identifier: String = origin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    app.add_capability(
        CapabilityBuilder::new(format!("server-{identifier}"))
            .remote(server.capability_pattern())
            .local(false)
            .window(WINDOW)
            .permission("notification:allow-is-permission-granted")
            .permission("notification:allow-notify"),
    )
    .map_err(|e| format!("could not allow desktop Alerts from {origin}: {e}"))?;
    granted.insert(origin);
    Ok(())
}

/// Sends the window to `server`'s dashboard, which runs its own login page.
fn open(app: &AppHandle, shell: &Shell, server: &ServerUrl) -> Result<Step, String> {
    grant(app, shell, server)?;
    *shell.server.lock().expect("server lock") = Some(server.clone());
    main_window(app)?
        .navigate(server.home())
        .map_err(|e| e.to_string())?;
    Ok(Step::Opened)
}

fn load_config(shell: &Shell) -> DesktopConfig {
    DesktopConfig::load(&shell.config_dir).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "starting with no saved choice");
        DesktopConfig::default()
    })
}

fn choose(config: &DesktopConfig, error: Option<String>) -> Step {
    let server = match &config.mode {
        Some(Mode::Remote { server }) => Some(server.clone()),
        _ => None,
    };
    Step::Choose {
        server,
        embedded: config.mode == Some(Mode::Embedded),
        error,
    }
}

/// Called when the bundled page opens: goes straight to the last choice, or
/// asks for one.
#[tauri::command]
async fn resume(app: AppHandle, shell: State<'_, Shell>) -> Result<Step, String> {
    let config = load_config(&shell);
    let result = match &config.mode {
        None => return Ok(choose(&config, None)),
        Some(Mode::Remote { server }) => connect(&app, &shell, server).await,
        Some(Mode::Embedded) => embedded(&app, &shell).await,
    };
    Ok(result.unwrap_or_else(|e| choose(&config, Some(e))))
}

/// What was chosen before, for the "change server" page.
#[tauri::command]
async fn choices(shell: State<'_, Shell>) -> Result<Step, String> {
    Ok(choose(&load_config(&shell), None))
}

#[tauri::command]
async fn connect_remote(
    app: AppHandle,
    shell: State<'_, Shell>,
    server: String,
) -> Result<Step, String> {
    connect(&app, &shell, &server).await
}

#[tauri::command]
async fn start_embedded(app: AppHandle, shell: State<'_, Shell>) -> Result<Step, String> {
    embedded(&app, &shell).await
}

#[tauri::command]
async fn create_first_admin(
    app: AppHandle,
    shell: State<'_, Shell>,
    username: String,
    password: String,
) -> Result<Step, String> {
    let guard = shell.embedded.lock().await;
    let embedded = guard
        .as_ref()
        .ok_or("Gantry is not running on this computer")?;
    embedded
        .create_first_admin(&username, &password)
        .await
        .map_err(|e| e.to_string())?;
    let server = ServerUrl::parse(&embedded.url()).map_err(|e| e.to_string())?;
    drop(guard);
    open(&app, &shell, &server)
}

async fn connect(app: &AppHandle, shell: &Shell, input: &str) -> Result<Step, String> {
    let server = ServerUrl::parse(input).map_err(|e| e.to_string())?;
    probe::check_gantry(&shell.http, &server)
        .await
        .map_err(|e| e.to_string())?;
    // A remote server replaces the embedded one.
    if let Some(embedded) = shell.embedded.lock().await.take() {
        embedded.stop().await;
    }
    let mut config = load_config(shell);
    config.mode = Some(Mode::Remote {
        server: server.to_string(),
    });
    config.save(&shell.config_dir).map_err(|e| e.to_string())?;
    open(app, shell, &server)
}

async fn embedded(app: &AppHandle, shell: &Shell) -> Result<Step, String> {
    let mut config = load_config(shell);
    let mut guard = shell.embedded.lock().await;
    if guard.is_none() {
        let from_env = std::env::var("GANTRY_SECRET_KEY").ok();
        let key = tauri::async_runtime::spawn_blocking(move || {
            secret_key::load_or_create(&OsKeychain, from_env)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let started = Embedded::start(EmbeddedOptions {
            data_dir: shell.data_dir.clone(),
            secret_key: key,
            preferred_port: config.embedded_port,
            drivers: crate::drivers::registry(),
        })
        .await
        .map_err(|e| format!("{e:#}"))?;
        *guard = Some(started);
    }
    let embedded = guard.as_ref().expect("started above");
    config.mode = Some(Mode::Embedded);
    config.embedded_port = Some(embedded.address().port());
    config.save(&shell.config_dir).map_err(|e| e.to_string())?;
    if embedded
        .needs_first_admin()
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(Step::FirstAdmin);
    }
    let server = ServerUrl::parse(&embedded.url()).map_err(|e| e.to_string())?;
    drop(guard);
    open(app, shell, &server)
}
