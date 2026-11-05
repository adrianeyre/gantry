//! A mock PrusaLink, written from Prusa's OpenAPI description of PrusaLink's
//! REST API v1 (`spec/openapi.yaml` in prusa3d/Prusa-Link-Web): an
//! in-process HTTP server on `127.0.0.1` that answers `/api/version`,
//! `/api/v1/status`, `/api/v1/job`, `/api/v1/storage`, the job commands and
//! file uploads, behind HTTP Digest authentication (or an `X-Api-Key`, as
//! older versions use). The test moves the printer along (a print's
//! progress, its end) and reads back the requests it was sent.
//!
//! It can be stopped and started again on the same port, closing every open
//! connection, so a test sees the Printer go away and come back.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use md5::{Digest as _, Md5};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub const REALM: &str = "Printer API";

/// How the mock wants clients to prove who they are.
#[derive(Debug, Clone)]
pub enum Auth {
    Digest { username: String, password: String },
    ApiKey(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    pub file: String,
    pub state: String,
    pub progress: f64,
    pub time_printing: u64,
    pub time_remaining: u64,
}

#[derive(Debug, Clone)]
pub struct Upload {
    pub path: String,
    pub data: Vec<u8>,
    pub print_after_upload: Option<String>,
    pub overwrite: Option<String>,
}

#[derive(Debug)]
pub struct Printer {
    pub auth: Auth,
    pub state: String,
    pub message: Option<String>,
    pub job: Option<Job>,
    pub next_id: u64,
    pub nonce: String,
    /// `METHOD /path` for every authenticated request, in order.
    pub requests: Vec<String>,
    /// Requests refused for their credentials.
    pub refused: usize,
    pub uploads: Vec<Upload>,
}

type Shared = Arc<Mutex<Printer>>;

pub struct MockPrusaLink {
    address: SocketAddr,
    printer: Shared,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl MockPrusaLink {
    pub async fn start(auth: Auth) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let printer = Arc::new(Mutex::new(Printer {
            auth,
            state: "IDLE".into(),
            message: None,
            job: None,
            next_id: 40,
            nonce: "8a3f0e9b5c1d2e7f".into(),
            requests: Vec::new(),
            refused: 0,
            uploads: Vec::new(),
        }));
        let task = tokio::spawn(serve(listener, printer.clone()));
        Self {
            address,
            printer,
            task: Mutex::new(Some(task)),
        }
    }

    pub fn host(&self) -> String {
        self.address.ip().to_string()
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Printer) -> T) -> T {
        f(&mut self.printer.lock().unwrap())
    }

    pub fn requests(&self) -> Vec<String> {
        self.with(|p| p.requests.clone())
    }

    pub fn uploads(&self) -> Vec<Upload> {
        self.with(|p| p.uploads.clone())
    }

    /// A print started at the printer's own screen.
    pub fn start_print(&self, file: &str) -> u64 {
        self.with(|p| {
            let id = p.next_id;
            p.next_id += 1;
            p.state = "PRINTING".into();
            p.job = Some(Job {
                id,
                file: file.into(),
                state: "PRINTING".into(),
                progress: 0.0,
                time_printing: 0,
                time_remaining: 900,
            });
            id
        })
    }

    pub fn progress(&self, progress: f64) {
        self.with(|p| {
            if let Some(job) = &mut p.job {
                job.progress = progress;
                job.time_printing = (progress * 10.0) as u64;
                job.time_remaining = ((100.0 - progress) * 10.0) as u64;
            }
        });
    }

    /// The print runs to its end.
    pub fn finish(&self) {
        self.with(|p| {
            p.state = "FINISHED".into();
            if let Some(job) = &mut p.job {
                job.state = "FINISHED".into();
                job.progress = 100.0;
                job.time_remaining = 0;
            }
        });
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
        *self.task.lock().unwrap() = Some(tokio::spawn(serve(listener, self.printer.clone())));
    }
}

impl Drop for MockPrusaLink {
    fn drop(&mut self) {
        self.go_down();
    }
}

/// Serves until aborted; aborting drops every connection with it.
async fn serve(listener: TcpListener, printer: Shared) {
    let app = Router::new().fallback(handle).with_state(printer);
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

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `key="value", key=value` pairs.
fn params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        rest = rest.trim_start_matches([',', ' ']);
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_ascii_lowercase();
        rest = &rest[eq + 1..];
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').unwrap_or(quoted.len());
            value = quoted[..end].to_owned();
            rest = quoted.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_owned();
            rest = &rest[end..];
        }
        out.push((key, value));
    }
    out
}

fn authorised(printer: &Printer, method: &Method, uri: &Uri, headers: &HeaderMap) -> bool {
    match &printer.auth {
        Auth::ApiKey(key) => headers
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == key),
        Auth::Digest { username, password } => {
            let Some(header) = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Digest "))
            else {
                return false;
            };
            let params = params(header);
            let get = |key: &str| {
                params
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
            };
            let target = uri.path_and_query().map_or(uri.path(), |p| p.as_str());
            if get("username") != username
                || get("realm") != REALM
                || get("nonce") != printer.nonce
                || get("uri") != target
                || get("qop") != "auth"
            {
                return false;
            }
            let ha1 = md5_hex(&format!("{username}:{REALM}:{password}"));
            let ha2 = md5_hex(&format!("{method}:{target}"));
            let expected = md5_hex(&format!(
                "{ha1}:{}:{}:{}:auth:{ha2}",
                printer.nonce,
                get("nc"),
                get("cnonce")
            ));
            get("response") == expected
        }
    }
}

fn refuse(printer: &Printer) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({ "title": "Unauthorized", "message": "Unauthorized" })),
    )
        .into_response();
    if matches!(printer.auth, Auth::Digest { .. }) {
        response.headers_mut().insert(
            "www-authenticate",
            format!(
                "Digest realm=\"{REALM}\", nonce=\"{}\", qop=\"auth\", algorithm=MD5",
                printer.nonce
            )
            .parse()
            .unwrap(),
        );
    }
    response
}

fn conflict(message: &str) -> Response {
    (
        StatusCode::CONFLICT,
        axum::Json(json!({ "title": "Conflict", "message": message })),
    )
        .into_response()
}

fn job_json(job: &Job) -> Value {
    json!({
        "id": job.id,
        "state": job.state,
        "progress": job.progress,
        "time_remaining": job.time_remaining,
        "time_printing": job.time_printing,
        "file": {
            "name": job.file.to_ascii_uppercase().chars().take(8).collect::<String>() + ".GCO",
            "display_name": job.file,
            "path": "/usb",
            "size": 1234,
            "m_timestamp": 1_700_000_000
        }
    })
}

async fn handle(
    State(printer): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut p = printer.lock().unwrap();
    if !authorised(&p, &method, &uri, &headers) {
        p.refused += 1;
        return refuse(&p);
    }
    let path = uri.path().to_owned();
    p.requests.push(format!("{method} {path}"));
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), segments.as_slice()) {
        ("GET", ["api", "version"]) => axum::Json(json!({
            "api": "2.0.0", "server": "2.1.2", "nozzle_diameter": 0.4,
            "text": "PrusaLink", "hostname": "mock-mk4"
        }))
        .into_response(),
        ("GET", ["api", "v1", "storage"]) => axum::Json(json!({
            "storage_list": [
                { "path": "/usb", "name": "usb", "type": "USB", "read_only": false, "available": true }
            ]
        }))
        .into_response(),
        ("GET", ["api", "v1", "status"]) => {
            let mut status = json!({
                "storage": { "path": "/usb/", "name": "usb", "read_only": false },
                "printer": {
                    "state": p.state,
                    "temp_nozzle": 214.8, "target_nozzle": 215.0,
                    "temp_bed": 60.1, "target_bed": 60.0,
                    "axis_z": 0.2, "flow": 100, "speed": 100,
                    "fan_hotend": 5000, "fan_print": 3000
                }
            });
            if let Some(message) = &p.message {
                status["printer"]["status_printer"] = json!({ "ok": false, "message": message });
            }
            if let Some(job) = &p.job {
                status["job"] = json!({
                    "id": job.id,
                    "progress": job.progress,
                    "time_remaining": job.time_remaining,
                    "time_printing": job.time_printing
                });
            }
            axum::Json(status).into_response()
        }
        ("GET", ["api", "v1", "job"]) => match &p.job {
            Some(job) => axum::Json(job_json(job)).into_response(),
            None => StatusCode::NO_CONTENT.into_response(),
        },
        ("PUT", ["api", "v1", "job", id, action]) => {
            let Some(job) = p.job.as_mut().filter(|j| j.id.to_string() == *id) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            match (*action, job.state.as_str()) {
                ("pause", "PRINTING") => {
                    job.state = "PAUSED".into();
                    p.state = "PAUSED".into();
                    StatusCode::NO_CONTENT.into_response()
                }
                ("resume", "PAUSED") => {
                    job.state = "PRINTING".into();
                    p.state = "PRINTING".into();
                    StatusCode::NO_CONTENT.into_response()
                }
                ("pause" | "resume", _) => conflict("Not in a state to do that"),
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }
        ("DELETE", ["api", "v1", "job", id]) => {
            let Some(job) = p.job.as_mut().filter(|j| j.id.to_string() == *id) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            job.state = "STOPPED".into();
            p.state = "STOPPED".into();
            StatusCode::NO_CONTENT.into_response()
        }
        ("PUT", ["api", "v1", "files", _storage, name @ ..]) => {
            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let print_after = header("print-after-upload");
            p.uploads.push(Upload {
                path: path.clone(),
                data: body.to_vec(),
                print_after_upload: print_after.clone(),
                overwrite: header("overwrite-file"),
            });
            if print_after.as_deref() == Some("?1") {
                if matches!(p.state.as_str(), "PRINTING" | "PAUSED" | "BUSY") {
                    return conflict("Printer is busy");
                }
                let id = p.next_id;
                p.next_id += 1;
                p.state = "PRINTING".into();
                let file = name.join("/").replace("%20", " ");
                p.job = Some(Job {
                    id,
                    file,
                    state: "PRINTING".into(),
                    progress: 0.0,
                    time_printing: 0,
                    time_remaining: 600,
                });
            }
            StatusCode::CREATED.into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
