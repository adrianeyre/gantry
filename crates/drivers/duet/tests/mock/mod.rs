//! A mock Duet, written from Duet3D's documentation of RepRapFirmware 3's
//! standalone HTTP API and object model: an in-process HTTP server on
//! `127.0.0.1` that answers `rr_connect` (with a password and RRF 3.5's
//! session key), `rr_model` (the whole model, or one `key` of it),
//! `rr_gcode`, `rr_upload` and `rr_disconnect`. It acts on the G-code that
//! matters here (`M25`, `M24`, `M0`, `M32`) the way the firmware does, and
//! records everything else. The test moves the print along and reads back
//! what the board was sent.
//!
//! It can be stopped and started again on the same port, closing every open
//! connection, so a test sees the Printer go away and come back.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Debug)]
pub struct Board {
    /// `None` when the board has no password (any is taken).
    pub password: Option<String>,
    sessions: HashSet<String>,
    next_session: u32,
    pub status: String,
    pub file: Option<String>,
    pub size: u64,
    pub position: u64,
    pub layer: Option<u32>,
    pub duration: u64,
    pub last_file: Option<String>,
    pub last_cancelled: bool,
    pub last_aborted: bool,
    pub files: BTreeMap<String, Vec<u8>>,
    /// Every G-code sent, in order.
    pub gcodes: Vec<String>,
    /// The path of every request, in order (the query left out).
    pub requests: Vec<String>,
    pub connects: usize,
}

type Shared = Arc<Mutex<Board>>;

pub struct MockDuet {
    address: SocketAddr,
    board: Shared,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl MockDuet {
    pub async fn start(password: Option<&str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let board = Arc::new(Mutex::new(Board {
            password: password.map(str::to_owned),
            sessions: HashSet::new(),
            next_session: 4242,
            status: "idle".into(),
            file: None,
            size: 0,
            position: 0,
            layer: None,
            duration: 0,
            last_file: None,
            last_cancelled: false,
            last_aborted: false,
            files: BTreeMap::new(),
            gcodes: Vec::new(),
            requests: Vec::new(),
            connects: 0,
        }));
        let task = tokio::spawn(serve(listener, board.clone()));
        Self {
            address,
            board,
            task: Mutex::new(Some(task)),
        }
    }

    pub fn host(&self) -> String {
        self.address.ip().to_string()
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Board) -> T) -> T {
        f(&mut self.board.lock().unwrap())
    }

    pub fn gcodes(&self) -> Vec<String> {
        self.with(|b| b.gcodes.clone())
    }

    /// A print started at the printer's own screen.
    pub fn start_print(&self, path: &str) {
        self.with(|b| {
            b.files.insert(path.into(), vec![b';'; 4000]);
            start(b, path);
        });
    }

    /// The print reaches `percent` of its file, on `layer`.
    pub fn advance(&self, percent: u64, layer: u32) {
        self.with(|b| {
            b.position = b.size * percent / 100;
            b.layer = Some(layer);
            b.duration = percent * 6;
        });
    }

    /// The print runs to its end.
    pub fn finish(&self) {
        self.with(|b| {
            b.last_file = b.file.take();
            b.last_cancelled = false;
            b.last_aborted = false;
            b.status = "idle".into();
        });
    }

    /// Forgets every session, as the board does after its session timeout.
    pub fn expire_sessions(&self) {
        self.with(|b| b.sessions.clear());
    }

    /// Closes the listener and every connection.
    pub fn go_down(&self) {
        if let Some(task) = self.task.lock().unwrap().take() {
            task.abort();
        }
    }

    /// Listens again on the same port.
    pub async fn come_back(&self) {
        let listener = loop {
            match TcpListener::bind(self.address).await {
                Ok(l) => break l,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        };
        *self.task.lock().unwrap() = Some(tokio::spawn(serve(listener, self.board.clone())));
    }
}

impl Drop for MockDuet {
    fn drop(&mut self) {
        self.go_down();
    }
}

fn start(b: &mut Board, path: &str) {
    b.size = b.files.get(path).map_or(0, |f| f.len() as u64);
    b.file = Some(path.into());
    b.status = "processing".into();
    b.position = 0;
    b.layer = Some(0);
    b.duration = 0;
}

/// Serves until aborted; aborting drops every connection with it.
async fn serve(listener: TcpListener, board: Shared) {
    let app = Router::new().fallback(handle).with_state(board);
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let service = hyper_util::service::TowerToHyperService::new(app.clone());
        connections.spawn(async move {
            let _ =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
        });
        while connections.try_join_next().is_some() {}
    }
}

fn model(b: &Board) -> Value {
    json!({
        "state": { "status": b.status, "upTime": 1234 },
        "heat": {
            "bedHeaters": [0, -1, -1],
            "chamberHeaters": [-1, -1],
            "heaters": [
                { "current": 59.8, "active": 60.0, "standby": 0.0, "state": "active" },
                { "current": 205.24, "active": 210.0, "standby": 0.0, "state": "active" }
            ]
        },
        "tools": [ { "number": 0, "heaters": [1], "state": "active" } ],
        "fans": [
            { "actualValue": 0.35, "requestedValue": 0.35, "name": "", "rpm": -1,
              "thermostatic": { "heaters": [], "highTemperature": null, "lowTemperature": null } },
            { "actualValue": 1.0, "requestedValue": 1.0, "name": "Hotend", "rpm": -1,
              "thermostatic": { "heaters": [1], "highTemperature": 45.0, "lowTemperature": 45.0 } }
        ],
        "job": job(b)
    })
}

fn job(b: &Board) -> Value {
    json!({
        "file": { "fileName": b.file, "size": b.size, "numLayers": 20 },
        "filePosition": b.position,
        "duration": b.file.as_ref().map(|_| b.duration),
        "layer": b.file.as_ref().and(b.layer),
        "timesLeft": { "slicer": b.file.as_ref().map(|_| 300), "file": null, "filament": null },
        "lastFileName": b.last_file,
        "lastFileCancelled": b.last_cancelled,
        "lastFileAborted": b.last_aborted
    })
}

/// Acts on one G-code line as RepRapFirmware does, for the codes that
/// matter here.
fn run_gcode(b: &mut Board, line: &str) {
    b.gcodes.push(line.to_owned());
    let word = line.split_whitespace().next().unwrap_or_default();
    match word {
        "M25" if b.status == "processing" => b.status = "paused".into(),
        "M24" if b.status == "paused" => b.status = "processing".into(),
        "M0" if b.status == "paused" => {
            b.last_file = b.file.take();
            b.last_cancelled = true;
            b.last_aborted = false;
            b.status = "idle".into();
        }
        "M32" if b.status == "idle" => {
            let path = line[3..].trim().trim_matches('"').to_owned();
            if b.files.contains_key(&path) {
                start(b, &path);
            }
        }
        _ => {}
    }
}

async fn handle(
    State(board): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    let mut b = board.lock().unwrap();
    let path = uri.path().to_owned();
    b.requests.push(path.clone());
    let ok = || axum::Json(json!({ "err": 0 })).into_response();
    if path == "/rr_connect" {
        b.connects += 1;
        let given = query.get("password").map(String::as_str).unwrap_or("");
        if b.password.as_deref().is_some_and(|p| p != given) {
            return axum::Json(json!({ "err": 1 })).into_response();
        }
        let key = b.next_session.to_string();
        b.next_session += 1;
        b.sessions.insert(key.clone());
        return axum::Json(json!({
            "err": 0, "sessionTimeout": 8000, "boardType": "duet3mb6hc102",
            "apiLevel": 1, "sessionKey": key.parse::<u32>().unwrap()
        }))
        .into_response();
    }
    let session = headers
        .get("x-session-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !b.sessions.contains(session) {
        return (StatusCode::UNAUTHORIZED, "Not authorized").into_response();
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/rr_disconnect") => {
            b.sessions.remove(session);
            ok()
        }
        ("GET", "/rr_model") => {
            let whole = model(&b);
            let key = query.get("key").map(String::as_str).unwrap_or("");
            let result = if key.is_empty() {
                whole
            } else {
                key.split('.').fold(whole, |v, k| v[k].clone())
            };
            let flags = query.get("flags").cloned().unwrap_or_default();
            axum::Json(json!({ "key": key, "flags": flags, "result": result })).into_response()
        }
        ("GET", "/rr_gcode") => {
            let gcode = query.get("gcode").cloned().unwrap_or_default();
            for line in gcode.lines() {
                run_gcode(&mut b, line.trim());
            }
            axum::Json(json!({ "buff": 255 })).into_response()
        }
        ("POST", "/rr_upload") => {
            let Some(name) = query.get("name").cloned() else {
                return axum::Json(json!({ "err": 1 })).into_response();
            };
            b.files.insert(name, body.to_vec());
            ok()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
