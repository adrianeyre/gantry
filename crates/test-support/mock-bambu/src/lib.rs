//! A mock of a Bambu Lab printer's local MQTT broker, for tests (PRD §5.6).
//!
//! `cargo test` starts it in-process, so it needs no Docker. It is an embedded
//! `rumqttd` broker behind a TLS front with a freshly generated self-signed
//! certificate, as a real printer has. The front also checks the login the way
//! a printer does: username `bblp`, password the access code, and a refusal is
//! a CONNACK with return code 5 ("not authorised"). `rumqttd` itself would
//! just close the connection.
//!
//! Tests push `report` payloads (the hand-written [`fixtures`]) to
//! `device/<serial>/report` and read back what was published to
//! `device/<serial>/request`. A `pushall` request is answered with the
//! printer's whole current state, as a real printer does. [`MockBambu::go_down`]
//! and [`MockBambu::come_back`] imitate the printer dropping off the network.
//!
//! [`ftps::MockFtps`] is the printer's FTPS server, for Print File uploads.
//!
//! Never put a real serial number or access code in a test.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rumqttd::local::LinkTx;
use rumqttd::{Broker, Config, ConnectionSettings, Notification, RouterConfig, ServerSettings};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub mod camera;
pub mod fixtures;
pub mod ftps;
pub mod rtsp;

/// The username every Bambu printer expects.
pub const USERNAME: &str = "bblp";

/// A CONNACK refusing the login: MQTT 3.1.1 return code 5, "not authorised".
const CONNACK_NOT_AUTHORISED: [u8; 4] = [0x20, 0x02, 0x00, 0x05];

/// How the mock printer answers commands (`print` and `system` requests,
/// other than `pushall`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Answers {
    /// Each command is answered `result: success`, echoing its sequence id.
    #[default]
    Accept,
    /// Each command is refused the way firmware with Bambu's Authorization
    /// Control is reported to refuse a local command it won't take (see the
    /// `command_rejected` fixture and its README entry).
    BlockControl,
    /// No answer at all, as older firmware might.
    Silent,
}

struct Shared {
    serial: String,
    access_code: String,
    link: Mutex<LinkTx>,
    requests: Mutex<Vec<Value>>,
    request_arrived: Notify,
    answers: Mutex<Answers>,
    /// The printer's whole state: every report so far, merged.
    state: Mutex<Value>,
    /// Bumped to cut every open connection.
    cut: watch::Sender<u64>,
    open: AtomicUsize,
    accepted: AtomicUsize,
    refused_logins: AtomicUsize,
    tls: tokio_rustls::TlsAcceptor,
    broker: SocketAddr,
}

/// A running mock printer broker.
pub struct MockBambu {
    shared: Arc<Shared>,
    address: SocketAddr,
    front: Mutex<Option<JoinHandle<()>>>,
}

impl MockBambu {
    /// Starts a broker on a random local port for a printer with this serial
    /// number and access code.
    pub async fn start(serial: &str, access_code: &str) -> Self {
        let broker_address = free_port();
        let (link, mut link_rx) = start_broker(broker_address);
        wait_for_port(broker_address).await;

        let (cut, _) = watch::channel(0);
        let shared = Arc::new(Shared {
            serial: serial.to_owned(),
            access_code: access_code.to_owned(),
            link: Mutex::new(link),
            requests: Mutex::new(Vec::new()),
            request_arrived: Notify::new(),
            answers: Mutex::new(Answers::default()),
            state: Mutex::new(serde_json::json!({ "print": {} })),
            cut,
            open: AtomicUsize::new(0),
            accepted: AtomicUsize::new(0),
            refused_logins: AtomicUsize::new(0),
            tls: tls_acceptor(serial),
            broker: broker_address,
        });

        // Record every request, and answer pushall.
        let recorder = shared.clone();
        tokio::spawn(async move {
            while let Ok(notification) = link_rx.next().await {
                let Some(Notification::Forward(forward)) = notification else {
                    continue;
                };
                let topic = String::from_utf8_lossy(&forward.publish.topic).into_owned();
                if topic != format!("device/{}/request", recorder.serial) {
                    continue;
                }
                let Ok(payload) = serde_json::from_slice::<Value>(&forward.publish.payload) else {
                    continue;
                };
                let pushall = payload["pushing"]["command"] == "pushall";
                let answer = answer(
                    &payload,
                    *recorder.answers.lock().expect("lock"),
                    &recorder.serial,
                );
                recorder.requests.lock().expect("lock").push(payload);
                recorder.request_arrived.notify_waiters();
                if pushall {
                    let state = recorder.state.lock().expect("lock").clone();
                    recorder.publish(&state);
                }
                if let Some(answer) = answer {
                    recorder.publish(&answer);
                }
            }
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let mock = Self {
            shared,
            address,
            front: Mutex::new(None),
        };
        mock.serve(listener);
        mock
    }

    fn serve(&self, listener: TcpListener) {
        let shared = self.shared.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(connection(shared.clone(), stream));
            }
        });
        *self.front.lock().expect("lock") = Some(task);
    }

    /// Where the printer listens (instead of port 8883).
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn serial(&self) -> &str {
        &self.shared.serial
    }

    /// Publishes a report, as the printer would. Also merged into the state
    /// a `pushall` returns.
    pub fn report(&self, payload: &Value) {
        merge(&mut self.shared.state.lock().expect("lock"), payload);
        self.shared.publish(payload);
    }

    /// Publishes one of the [`fixtures`].
    pub fn report_fixture(&self, name: &str) {
        self.report(&fixtures::load(name));
    }

    /// Changes how commands are answered from now on.
    pub fn set_answers(&self, answers: Answers) {
        *self.shared.answers.lock().expect("lock") = answers;
    }

    /// Everything published to the request topic so far.
    pub fn requests(&self) -> Vec<Value> {
        self.shared.requests.lock().expect("lock").clone()
    }

    /// Waits for a request satisfying `pred`, already recorded or yet to come.
    pub async fn wait_for_request(
        &self,
        timeout: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Option<Value> {
        tokio::time::timeout(timeout, async {
            loop {
                let arrived = self.shared.request_arrived.notified();
                if let Some(found) = self.requests().into_iter().find(|r| pred(r)) {
                    return found;
                }
                arrived.await;
            }
        })
        .await
        .ok()
    }

    /// How many logged-in connections are open now.
    pub fn open_connections(&self) -> usize {
        self.shared.open.load(Ordering::SeqCst)
    }

    /// How many logins have succeeded since the start.
    pub fn accepted_logins(&self) -> usize {
        self.shared.accepted.load(Ordering::SeqCst)
    }

    /// How many logins were refused for the wrong username or access code.
    pub fn refused_logins(&self) -> usize {
        self.shared.refused_logins.load(Ordering::SeqCst)
    }

    /// The printer drops off the network: every connection is cut and
    /// nothing listens at its address until [`come_back`](Self::come_back).
    pub fn go_down(&self) {
        if let Some(task) = self.front.lock().expect("lock").take() {
            task.abort();
        }
        self.shared.cut.send_modify(|n| *n += 1);
    }

    /// The printer is back at the same address.
    pub async fn come_back(&self) {
        let mut tries = 0;
        let listener = loop {
            match TcpListener::bind(self.address).await {
                Ok(listener) => break listener,
                Err(e) if tries < 50 => {
                    tries += 1;
                    tracing::debug!(error = %e, "rebinding the mock broker");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("could not rebind the mock broker: {e}"),
            }
        };
        self.serve(listener);
    }
}

impl Drop for MockBambu {
    fn drop(&mut self) {
        self.go_down();
    }
}

impl Shared {
    fn publish(&self, payload: &Value) {
        let topic = format!("device/{}/report", self.serial);
        let bytes = serde_json::to_vec(payload).expect("JSON serialises");
        if let Err(e) = self.link.lock().expect("lock").publish(topic, bytes) {
            tracing::warn!(error = ?e, "the mock broker could not publish");
        }
    }
}

/// The firmware version the mock printer reports to `get_version`.
pub const FIRMWARE: &str = "01.02.03.04";

/// The printer's answer to a command, if it gives one.
fn answer(request: &Value, answers: Answers, serial: &str) -> Option<Value> {
    if request["info"]["command"] == "get_version" {
        // As community clients document it: one entry per module, each with
        // its own serial (which is the printer's for the main board).
        return Some(serde_json::json!({ "info": {
            "command": "get_version",
            "sequence_id": request["info"]["sequence_id"].clone(),
            "result": "success",
            "module": [
                { "name": "ota", "sw_ver": FIRMWARE, "hw_ver": "OTA", "sn": serial },
                { "name": "mc", "sw_ver": "00.00.30.00", "hw_ver": "MC07", "sn": serial },
            ],
        } }));
    }
    let (section, body) = ["print", "system"]
        .into_iter()
        .find_map(|s| request.get(s).map(|b| (s, b)))?;
    let command = body.get("command")?.clone();
    let sequence_id = body.get("sequence_id").cloned().unwrap_or(Value::Null);
    let reply = match answers {
        Answers::Silent => return None,
        Answers::Accept => serde_json::json!({
            "command": command, "sequence_id": sequence_id, "result": "success", "reason": ""
        }),
        Answers::BlockControl => {
            let mut rejected = fixtures::load("command_rejected")["print"].clone();
            rejected["command"] = command;
            rejected["sequence_id"] = sequence_id;
            rejected
        }
    };
    Some(serde_json::json!({ section: reply }))
}

/// Folds a partial report into the state, as the printer's own state changes.
fn merge(state: &mut Value, update: &Value) {
    match (state, update) {
        (Value::Object(state), Value::Object(update)) => {
            for (key, value) in update {
                match state.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value);
                    }
                    _ => {
                        state.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (state, update) => *state = update.clone(),
    }
}

/// One client: TLS, then the login check, then everything relayed to the
/// broker until either side closes or the printer "goes down".
async fn connection(shared: Arc<Shared>, stream: TcpStream) {
    let mut cut = shared.cut.subscribe();
    let Ok(Ok(mut tls)) =
        tokio::time::timeout(Duration::from_secs(5), shared.tls.accept(stream)).await
    else {
        return;
    };
    let Ok(Ok(connect)) = tokio::time::timeout(Duration::from_secs(5), read_packet(&mut tls)).await
    else {
        return;
    };
    let Some((username, password)) = login(&connect) else {
        return;
    };
    if username != USERNAME || password != shared.access_code.as_bytes() {
        shared.refused_logins.fetch_add(1, Ordering::SeqCst);
        let _ = tls.write_all(&CONNACK_NOT_AUTHORISED).await;
        let _ = tls.flush().await;
        let _ = tls.shutdown().await;
        return;
    }
    let Ok(mut broker) = TcpStream::connect(shared.broker).await else {
        return;
    };
    if broker.write_all(&connect).await.is_err() {
        return;
    }
    shared.accepted.fetch_add(1, Ordering::SeqCst);
    shared.open.fetch_add(1, Ordering::SeqCst);
    tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut tls, &mut broker) => {}
        _ = cut.changed() => {}
    }
    shared.open.fetch_sub(1, Ordering::SeqCst);
}

/// Reads one whole MQTT packet: fixed header, length, and body.
async fn read_packet<S: AsyncReadExt + Unpin>(stream: &mut S) -> std::io::Result<Vec<u8>> {
    let mut packet = vec![stream.read_u8().await?];
    let mut length = 0usize;
    for shift in [0, 7, 14, 21] {
        let byte = stream.read_u8().await?;
        packet.push(byte);
        length |= usize::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    let start = packet.len();
    packet.resize(start + length, 0);
    stream.read_exact(&mut packet[start..]).await?;
    Ok(packet)
}

/// The username and password from an MQTT 3.1.1 CONNECT packet.
fn login(packet: &[u8]) -> Option<(String, Vec<u8>)> {
    if packet.first()? >> 4 != 1 {
        return None;
    }
    let mut at = 1;
    while packet.get(at)? & 0x80 != 0 {
        at += 1;
    }
    at += 1;
    let field = |at: &mut usize| -> Option<Vec<u8>> {
        let len = usize::from(u16::from_be_bytes([
            *packet.get(*at)?,
            *packet.get(*at + 1)?,
        ]));
        let value = packet.get(*at + 2..*at + 2 + len)?.to_vec();
        *at += 2 + len;
        Some(value)
    };
    let _protocol = field(&mut at)?;
    let _level = packet.get(at)?;
    let flags = *packet.get(at + 1)?;
    at += 4; // level, flags, keep alive
    let _client_id = field(&mut at)?;
    if flags & 0x04 != 0 {
        field(&mut at)?; // will topic
        field(&mut at)?; // will message
    }
    let username = if flags & 0x80 != 0 {
        String::from_utf8(field(&mut at)?).ok()?
    } else {
        String::new()
    };
    let password = if flags & 0x40 != 0 {
        field(&mut at)?
    } else {
        Vec::new()
    };
    Some((username, password))
}

fn tls_acceptor(serial: &str) -> tokio_rustls::TlsAcceptor {
    // Like a real printer: a self-signed certificate naming its serial.
    let generated = rcgen::generate_simple_self_signed(vec![serial.to_owned(), "localhost".into()])
        .expect("a self-signed certificate");
    let cert = CertificateDer::from(generated.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        generated.signing_key.serialize_der(),
    ));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .expect("a TLS server config");
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

fn free_port() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("address")
}

async fn wait_for_port(address: SocketAddr) {
    for _ in 0..200 {
        if TcpStream::connect(address).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the embedded broker did not start");
}

/// Starts `rumqttd` on its own threads (it has no shutdown, so they live as
/// long as the test process) and links to it from inside.
fn start_broker(listen: SocketAddr) -> (LinkTx, rumqttd::local::LinkRx) {
    let server = ServerSettings {
        name: "mock-bambu".into(),
        listen,
        tls: None,
        next_connection_delay_ms: 1,
        connections: ConnectionSettings {
            connection_timeout_ms: 10_000,
            max_payload_size: 1 << 20,
            max_inflight_count: 100,
            auth: None,
            external_auth: None,
            dynamic_filters: true,
        },
    };
    let config = Config {
        id: 0,
        router: RouterConfig {
            max_connections: 64,
            max_outgoing_packet_count: 200,
            max_segment_size: 10 << 20,
            max_segment_count: 10,
            ..RouterConfig::default()
        },
        v4: Some(HashMap::from([("v4".to_owned(), server)])),
        ..Config::default()
    };
    let mut broker = Broker::new(config);
    let (mut tx, rx) = broker.link("gantry-mock-bambu").expect("a local link");
    tx.subscribe("device/+/request").expect("subscribe");
    std::thread::Builder::new()
        .name("mock-bambu-broker".into())
        .spawn(move || {
            if let Err(e) = broker.start() {
                tracing::error!(error = ?e, "the mock broker stopped");
            }
        })
        .expect("a broker thread");
    (tx, rx)
}

#[cfg(test)]
mod tests;
