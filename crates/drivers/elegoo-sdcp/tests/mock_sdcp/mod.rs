//! A mock SDCP v3 printer, in process, written from Elegoo's SDCP v3 spec:
//! the WebSocket at `/websocket` (requests, responses, status and
//! attributes pushes, `ping`/`pong`), the chunked upload at
//! `/uploadFile/upload`, and an MJPEG stream at `/video` that Cmd 386 names.
//! It keeps a small Centauri Carbon-like status the tests drive.
//!
//! Like the real printer (as far as the spec says), it answers status and
//! attributes requests (Cmd 0 and 1) from anyone, but ignores every other
//! request not addressed to its MainboardID, so the Driver must learn it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

pub const MAINBOARD: &str = "a1b2c3d4e5f60718";

/// A JPEG the mock's camera serves, over and over.
pub const JPEG: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0xFF, 0xD9];

/// A request the mock took.
#[derive(Debug, Clone)]
pub struct Received {
    pub cmd: u64,
    pub data: Value,
    pub mainboard: String,
}

/// One upload chunk, as the mock read its fields.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub md5: String,
    pub check: String,
    pub offset: usize,
    pub uuid: String,
    pub total: usize,
    pub filename: String,
    pub bytes: usize,
}

pub struct Printer {
    pub status: Value,
    pub attributes: Value,
    pub commands: Vec<Received>,
    pub chunks: Vec<Chunk>,
    /// Uploads in progress, by Uuid.
    partial: HashMap<String, Vec<u8>>,
    /// Files on the printer, with the MD5 they came with.
    pub files: HashMap<String, (Vec<u8>, String)>,
    tasks: u32,
}

struct Inner {
    printer: Mutex<Printer>,
    pushes: broadcast::Sender<String>,
    port: Mutex<u16>,
}

pub struct MockSdcp {
    inner: Arc<Inner>,
    pub port: u16,
    server: Mutex<Option<(JoinHandle<()>, watch::Sender<bool>)>>,
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn idle_status() -> Value {
    json!({
        "CurrentStatus": [0],
        "PreviousStatus": 0,
        "TempOfNozzle": 24.6, "TempTargetNozzle": 0,
        "TempOfHotbed": 23.9, "TempTargetHotbed": 0,
        "TempOfBox": 25.1, "TempTargetBox": 0,
        "CurrentFanSpeed": { "ModelFan": 0, "AuxiliaryFan": 0, "BoxFan": 0 },
        "LightStatus": { "SecondLight": false },
        "CurrentCoord": "0.00,0.00,0.00",
        "PrintInfo": {
            "Status": 0, "CurrentLayer": 0, "TotalLayer": 0,
            "CurrentTicks": 0, "TotalTicks": 0,
            "Filename": "", "ErrorNumber": 0, "TaskId": "", "PrintSpeedPct": 100
        }
    })
}

fn status_message(status: &Value) -> String {
    json!({
        "Status": status,
        "MainboardID": MAINBOARD,
        "TimeStamp": now(),
        "Topic": format!("sdcp/status/{MAINBOARD}"),
    })
    .to_string()
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl MockSdcp {
    pub async fn start() -> Self {
        let inner = Arc::new(Inner {
            printer: Mutex::new(Printer {
                status: idle_status(),
                attributes: json!({
                    "Name": "Centauri Carbon", "MachineName": "Centauri Carbon",
                    "BrandName": "ELEGOO", "ProtocolVersion": "V3.0.0",
                    "FirmwareVersion": "V1.1.25", "MainboardID": MAINBOARD,
                    "Capabilities": ["FILE_TRANSFER", "PRINT_CONTROL", "VIDEO_STREAM"]
                }),
                commands: Vec::new(),
                chunks: Vec::new(),
                partial: HashMap::new(),
                files: HashMap::new(),
                tasks: 0,
            }),
            pushes: broadcast::channel(64).0,
            port: Mutex::new(0),
        });
        let mock = Self {
            inner,
            port: 0,
            server: Mutex::new(None),
        };
        let port = mock.listen(0).await;
        Self { port, ..mock }
    }

    async fn listen(&self, port: u16) -> u16 {
        // The port may take a moment to be free again after a stop.
        let mut tries = 0;
        let listener = loop {
            match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(l) => break l,
                Err(e) if tries < 50 => {
                    tries += 1;
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(e) => panic!("the mock couldn't listen on {port}: {e}"),
            }
        };
        let port = listener.local_addr().unwrap().port();
        *self.inner.port.lock().unwrap() = port;
        let (stop, stopped) = watch::channel(false);
        let app = Router::new()
            .route("/websocket", get(websocket))
            .route("/uploadFile/upload", post(upload))
            .route("/video", get(video))
            .with_state((self.inner.clone(), stopped));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        *self.server.lock().unwrap() = Some((task, stop));
        port
    }

    /// Stops answering: closes every connection and the listener.
    pub async fn stop(&self) {
        if let Some((task, stop)) = self.server.lock().unwrap().take() {
            let _ = stop.send(true);
            task.abort();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    /// Listens again on the same port, as a restarted printer would.
    pub async fn restart(&self) {
        self.listen(self.port).await;
    }

    pub fn printer(&self) -> std::sync::MutexGuard<'_, Printer> {
        self.inner.printer.lock().unwrap()
    }

    pub fn commands(&self) -> Vec<Received> {
        self.printer().commands.clone()
    }

    /// The commands addressed to this printer, by number.
    pub fn cmds(&self) -> Vec<u64> {
        self.commands().iter().map(|c| c.cmd).collect()
    }

    /// Changes the status, and pushes it to every client.
    pub fn update(&self, change: impl FnOnce(&mut Value)) {
        let message = {
            let mut printer = self.printer();
            change(&mut printer.status);
            status_message(&printer.status)
        };
        let _ = self.inner.pushes.send(message);
    }

    /// A print started from the printer's screen.
    pub fn print_from_screen(&self, file: &str) -> String {
        let task = {
            let mut printer = self.printer();
            start_print(&mut printer, file)
        };
        let message = status_message(&self.printer().status);
        let _ = self.inner.pushes.send(message);
        task
    }

    /// Moves the running print on to `layer` of 10, printing.
    pub fn print_layer(&self, layer: u64) {
        self.update(|s| {
            let info = &mut s["PrintInfo"];
            info["Status"] = json!(13);
            info["CurrentLayer"] = json!(layer);
            info["CurrentTicks"] = json!(layer * 60_000);
            s["TempTargetNozzle"] = json!(220);
            s["TempOfNozzle"] = json!(219.8);
        });
    }

    /// The running print finishes.
    pub fn complete(&self) {
        self.update(|s| {
            s["CurrentStatus"] = json!([0]);
            let info = &mut s["PrintInfo"];
            info["Status"] = json!(9);
            info["CurrentLayer"] = json!(10);
            info["CurrentTicks"] = json!(600_000);
        });
    }
}

fn start_print(printer: &mut Printer, file: &str) -> String {
    printer.tasks += 1;
    let task = format!("mock-task-{}", printer.tasks);
    let status = &mut printer.status;
    status["CurrentStatus"] = json!([1]);
    status["PrintInfo"] = json!({
        "Status": 16, "CurrentLayer": 0, "TotalLayer": 10,
        "CurrentTicks": 0, "TotalTicks": 600_000,
        "Filename": file, "ErrorNumber": 0, "TaskId": task, "PrintSpeedPct": 100
    });
    task
}

type AppState = (Arc<Inner>, watch::Receiver<bool>);

async fn websocket(
    State((inner, stopped)): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| client(socket, inner, stopped))
}

async fn client(mut socket: WebSocket, inner: Arc<Inner>, mut stopped: watch::Receiver<bool>) {
    let mut pushes = inner.pushes.subscribe();
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(Ok(message)) = message else { return };
                let Message::Text(text) = message else { continue };
                if text.as_str() == "ping" {
                    if socket.send(Message::Text("pong".into())).await.is_err() {
                        return;
                    }
                    continue;
                }
                let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                for reply in answer(&inner, &request) {
                    if socket.send(Message::Text(reply.into())).await.is_err() {
                        return;
                    }
                }
            }
            push = pushes.recv() => {
                let Ok(push) = push else { continue };
                if socket.send(Message::Text(push.into())).await.is_err() {
                    return;
                }
            }
            _ = stopped.changed() => return,
        }
    }
}

/// The messages the mock sends back for one request.
fn answer(inner: &Inner, request: &Value) -> Vec<String> {
    let data = &request["Data"];
    let cmd = data["Cmd"].as_u64().unwrap_or(u64::MAX);
    let mainboard = data["MainboardID"].as_str().unwrap_or_default().to_owned();
    let request_id = data["RequestID"].as_str().unwrap_or_default().to_owned();
    let addressed = mainboard == MAINBOARD
        && request["Topic"].as_str() == Some(&format!("sdcp/request/{MAINBOARD}"));
    if !addressed && !(mainboard.is_empty() && matches!(cmd, 0 | 1)) {
        return Vec::new();
    }
    let port = *inner.port.lock().unwrap();
    let mut printer = inner.printer.lock().unwrap();
    printer.commands.push(Received {
        cmd,
        data: data["Data"].clone(),
        mainboard: mainboard.clone(),
    });
    let mut after = None;
    let reply = match cmd {
        0 => {
            after = Some(status_message(&printer.status));
            json!({ "Ack": 0 })
        }
        1 => {
            after = Some(
                json!({
                    "Attributes": printer.attributes,
                    "MainboardID": MAINBOARD,
                    "TimeStamp": now(),
                    "Topic": format!("sdcp/attributes/{MAINBOARD}"),
                })
                .to_string(),
            );
            json!({ "Ack": 0 })
        }
        128 => {
            let file = data["Data"]["Filename"].as_str().unwrap_or_default();
            let busy = printer.status["CurrentStatus"] != json!([0]);
            if busy {
                json!({ "Ack": 1 })
            } else if printer.files.contains_key(file) {
                start_print(&mut printer, file);
                after = Some(status_message(&printer.status));
                json!({ "Ack": 0 })
            } else {
                json!({ "Ack": 2 })
            }
        }
        129..=131 => {
            let s = &mut printer.status;
            match cmd {
                129 => s["PrintInfo"]["Status"] = json!(6),
                131 => s["PrintInfo"]["Status"] = json!(13),
                _ => {
                    s["PrintInfo"]["Status"] = json!(8);
                    s["CurrentStatus"] = json!([0]);
                }
            }
            after = Some(status_message(s));
            json!({ "Ack": 0 })
        }
        386 => json!({ "Ack": 0, "VideoUrl": format!("http://127.0.0.1:{port}/video") }),
        403 => {
            let control = data["Data"].clone();
            let s = &mut printer.status;
            for (field, status) in [
                ("TempTargetNozzle", "TempTargetNozzle"),
                ("TempTargetHotbed", "TempTargetHotbed"),
            ] {
                if let Some(t) = control.get(field) {
                    s[status] = t.clone();
                }
            }
            if let Some(Value::Object(fans)) = control.get("TargetFanSpeed") {
                for (fan, speed) in fans {
                    s["CurrentFanSpeed"][fan] = speed.clone();
                }
            }
            if let Some(light) = control.get("LightStatus") {
                s["LightStatus"] = light.clone();
            }
            if let Some(pct) = control.get("PrintSpeedPct") {
                s["PrintInfo"]["PrintSpeedPct"] = pct.clone();
            }
            after = Some(status_message(s));
            json!({ "Ack": 0 })
        }
        _ => json!({ "Ack": 0 }),
    };
    let response = json!({
        "Id": request["Id"],
        "Data": {
            "Cmd": cmd,
            "Data": reply,
            "RequestID": request_id,
            "MainboardID": MAINBOARD,
            "TimeStamp": now(),
        },
        "Topic": format!("sdcp/response/{MAINBOARD}"),
    });
    let mut out = vec![response.to_string()];
    out.extend(after);
    out
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// A `multipart/form-data` body's parts: name, filename and bytes.
fn parts(content_type: &str, body: &[u8]) -> Vec<(String, Option<String>, Vec<u8>)> {
    let Some(boundary) = content_type.split("boundary=").nth(1) else {
        return Vec::new();
    };
    let delimiter = format!("--{}", boundary.trim_matches('"')).into_bytes();
    let mut parts = Vec::new();
    let Some(mut at) = find(body, &delimiter, 0).map(|p| p + delimiter.len()) else {
        return parts;
    };
    while !body[at..].starts_with(b"--") {
        let start = at + 2;
        let Some(next) = find(body, &delimiter, start) else {
            break;
        };
        let part = &body[start..next - 2];
        let Some(end) = find(part, b"\r\n\r\n", 0) else {
            break;
        };
        let headers = String::from_utf8_lossy(&part[..end]).into_owned();
        let quoted = |key: &str| {
            let from = headers.find(key)? + key.len();
            let len = headers[from..].find('"')?;
            Some(headers[from..from + len].to_owned())
        };
        if let Some(name) = quoted("; name=\"") {
            parts.push((name, quoted("filename=\""), part[end + 4..].to_vec()));
        }
        at = next + delimiter.len();
    }
    parts
}

fn upload_failed(code: i64) -> Response {
    axum::Json(json!({
        "code": "111111",
        "messages": [{ "field": "common_field", "message": code }],
        "data": null,
        "success": false,
    }))
    .into_response()
}

async fn upload(State((inner, _)): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let mut fields = HashMap::new();
    let mut file = None;
    for (name, filename, data) in parts(content_type, &body) {
        if name == "File" {
            file = Some((filename.unwrap_or_default(), data));
        } else {
            fields.insert(name, String::from_utf8_lossy(&data).into_owned());
        }
    }
    let field = |k: &str| fields.get(k).cloned().unwrap_or_default();
    let Some((filename, data)) = file else {
        return upload_failed(-4);
    };
    let chunk = Chunk {
        md5: field("S-File-MD5"),
        check: field("Check"),
        offset: field("Offset").parse().unwrap_or(usize::MAX),
        uuid: field("Uuid"),
        total: field("TotalSize").parse().unwrap_or(0),
        filename: filename.clone(),
        bytes: data.len(),
    };
    let mut printer = inner.printer.lock().unwrap();
    printer.chunks.push(chunk.clone());
    let received = printer.partial.entry(chunk.uuid.clone()).or_default();
    if received.len() != chunk.offset {
        return upload_failed(-2);
    }
    received.extend_from_slice(&data);
    if received.len() >= chunk.total {
        let whole = printer.partial.remove(&chunk.uuid).unwrap_or_default();
        if chunk.check == "1" && md5_hex(&whole) != chunk.md5 {
            return upload_failed(-4);
        }
        printer.files.insert(filename, (whole, chunk.md5));
    }
    axum::Json(json!({ "code": "000000", "messages": null, "data": {}, "success": true }))
        .into_response()
}

async fn video() -> Response {
    let frames = futures::stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut part = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
        part.extend_from_slice(&JPEG);
        part.extend_from_slice(b"\r\n");
        Some((Ok::<_, Infallible>(Bytes::from(part)), ()))
    });
    Response::builder()
        .header("content-type", "multipart/x-mixed-replace;boundary=frame")
        .body(Body::from_stream(frames))
        .unwrap()
}
