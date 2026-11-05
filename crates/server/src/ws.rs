//! The WebSocket the UI keeps open for live updates (PRD §7.1).
//!
//! On connecting, a browser gets a full snapshot of the Fleet, then one
//! message per change. A browser that falls behind gets a fresh snapshot, so
//! it never shows a stale card. The upgrade sits behind the same session gate
//! as the rest of the API, and the socket closes when the session ends.
//!
//! The desktop app opens it as `/api/ws?client=desktop`, and only those
//! sockets are also sent the Alerts that Alert Rules route to the desktop
//! channel, which the app shows as native notifications.
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use gantry_core::{FleetUpdate, PrinterView};
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

/// How often an open socket re-checks that its session is still live.
const SESSION_CHECK: Duration = Duration::from_secs(15);

/// A message to the browser.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage<'a> {
    /// Every Printer, replacing whatever the browser had.
    Snapshot { printers: &'a [Arc<PrinterView>] },
    /// One Printer changed (or was added).
    Printer { printer: &'a PrinterView },
    /// A Printer was removed.
    Removed { id: &'a str },
    /// An Alert for the desktop channel. Only desktop apps get these.
    DesktopAlert { alert: &'a crate::alerts::Alert },
}

/// What the socket was opened as.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Params {
    #[serde(default)]
    client: Option<String>,
}

/// `GET /api/ws`.
pub async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<Params>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = crate::auth::session_token(&headers).unwrap_or_default();
    let desktop = params.client.as_deref() == Some("desktop");
    ws.on_upgrade(move |socket| feed(state, token, desktop, socket))
}

async fn send(socket: &mut WebSocket, message: &ServerMessage<'_>) -> bool {
    let text = serde_json::to_string(message).expect("messages serialise");
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn feed(state: AppState, token: String, desktop: bool, mut socket: WebSocket) {
    // Subscribe before the snapshot, so nothing between the two is missed.
    let mut updates = state.fleet().subscribe();
    // A browser never subscribes, so desktop Alerts reach only the apps.
    let mut alerts = desktop.then(|| state.desktop_alerts().subscribe());
    let snapshot = state.fleet().snapshot();
    if !send(
        &mut socket,
        &ServerMessage::Snapshot {
            printers: &snapshot,
        },
    )
    .await
    {
        return;
    }
    let mut session_check = tokio::time::interval(SESSION_CHECK);
    session_check.tick().await;

    loop {
        tokio::select! {
            update = updates.recv() => {
                let sent = match update {
                    Ok(FleetUpdate::Printer(view)) => {
                        send(&mut socket, &ServerMessage::Printer { printer: &view }).await
                    }
                    Ok(FleetUpdate::Removed(id)) => {
                        send(&mut socket, &ServerMessage::Removed { id: &id }).await
                    }
                    Err(RecvError::Lagged(_)) => {
                        let snapshot = state.fleet().snapshot();
                        send(&mut socket, &ServerMessage::Snapshot { printers: &snapshot }).await
                    }
                    Err(RecvError::Closed) => false,
                };
                if !sent {
                    break;
                }
            }
            alert = async {
                match alerts.as_mut() {
                    Some(alerts) => alerts.recv().await,
                    None => std::future::pending().await,
                }
            } => match alert {
                Ok(alert) => {
                    if !send(&mut socket, &ServerMessage::DesktopAlert { alert: &alert }).await {
                        break;
                    }
                }
                // Missed some: carry on with the next.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => alerts = None,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            () = state.stopping() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            _ = session_check.tick() => {
                if !matches!(state.store().session_user(&token).await, Ok(Some(_))) {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{SinkExt, StreamExt};
    use gantry_driver_simulated::{SimulatedPrinter, StartPrint};
    use serde_json::{Value, json};
    use tokio_tungstenite::tungstenite;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    use crate::test_support::{Client, PASSWORD};

    struct Server {
        client: Client,
        address: std::net::SocketAddr,
        cookie: String,
    }

    async fn server() -> Server {
        let client = Client::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = crate::app(client.state.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest_login(address).await;
        Server {
            client,
            address,
            cookie: response,
        }
    }

    /// Logs in over real HTTP and returns the cookie pair.
    async fn reqwest_login(address: std::net::SocketAddr) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = json!({ "username": "admin", "password": PASSWORD }).to_string();
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!(
                    "POST /api/login HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let line = response
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("set-cookie:"))
            .unwrap();
        line.split_once(':')
            .unwrap()
            .1
            .trim()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    async fn open(server: &Server, cookie: Option<&str>) -> Result<Socket, tungstenite::Error> {
        let mut request = format!("ws://{}/api/ws", server.address)
            .into_client_request()
            .unwrap();
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert("cookie", cookie.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request)
            .await
            .map(|(socket, _)| socket)
    }

    async fn next(socket: &mut Socket) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("a message within 2 s")
                .unwrap()
                .unwrap();
            if let tungstenite::Message::Text(text) = message {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }

    /// The next message about `id` that satisfies `pred`.
    async fn until(socket: &mut Socket, pred: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let message = next(socket).await;
                if pred(&message) {
                    return message;
                }
            }
        })
        .await
        .expect("the expected message")
    }

    fn sim(server: &Server, id: &str) -> SimulatedPrinter {
        server
            .client
            .state
            .fleet()
            .handle(id)
            .unwrap()
            .as_any()
            .downcast_ref::<SimulatedPrinter>()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn the_websocket_needs_a_session() {
        let server = server().await;
        let refused = open(&server, None).await.unwrap_err();
        assert!(
            matches!(&refused, tungstenite::Error::Http(r) if r.status() == 401),
            "{refused:?}"
        );
        assert!(open(&server, Some("gantry_session=forged")).await.is_err());
    }

    #[tokio::test]
    async fn a_browser_gets_a_snapshot_then_deltas() {
        let server = server().await;
        let id = server.client.add_simulated("Live", json!({})).await;
        server
            .client
            .state
            .fleet()
            .wait_for(&id, Duration::from_secs(2), |v| {
                v.status.connection.is_connected()
            })
            .await
            .unwrap();

        let mut socket = open(&server, Some(&server.cookie)).await.unwrap();
        let snapshot = next(&mut socket).await;
        assert_eq!(snapshot["type"], "snapshot");
        assert_eq!(snapshot["printers"][0]["name"], "Live");

        // A change reaches the browser within a second.
        let started = std::time::Instant::now();
        sim(&server, &id)
            .start_print(StartPrint {
                plate: None,
                print_file: Some("cube.gcode".into()),
                duration_secs: 600.0,
                layers: 30,
            })
            .unwrap();
        let delta = until(&mut socket, |m| {
            m["type"] == "printer" && m["printer"]["status"]["job"]["print_file"] == "cube.gcode"
        })
        .await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(delta["printer"]["id"], id.as_str());

        // Reconnecting gets a full snapshot again, then deltas.
        socket.close(None).await.unwrap();
        let mut socket = open(&server, Some(&server.cookie)).await.unwrap();
        let snapshot = next(&mut socket).await;
        assert_eq!(snapshot["type"], "snapshot");
        assert_eq!(
            snapshot["printers"][0]["status"]["job"]["print_file"],
            "cube.gcode"
        );
        sim(&server, &id).set_speed(5.0).unwrap();
        let delta = until(&mut socket, |m| m["type"] == "printer").await;
        assert_eq!(delta["printer"]["id"], id.as_str());

        // Removing the Printer is a message too.
        server.client.delete(&format!("/api/printers/{id}")).await;
        let removed = until(&mut socket, |m| m["type"] == "removed").await;
        assert_eq!(removed["id"], id.as_str());
        let _ = socket.send(tungstenite::Message::Close(None)).await;
    }
}
