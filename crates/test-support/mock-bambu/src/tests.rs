use std::sync::Arc;
use std::time::Duration;

use rumqttc::{
    AsyncClient, ConnAck, ConnectReturnCode, ConnectionError, Event, EventLoop, MqttOptions,
    Packet, QoS, TlsConfiguration, Transport,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::json;

use super::*;

const SERIAL: &str = "TESTSERIAL00001";
const CODE: &str = "00000000";

/// Accepts any certificate, as a client of a self-signed printer must.
#[derive(Debug)]
struct AnyCertificate(rustls::crypto::CryptoProvider);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn client(mock: &MockBambu, password: &str) -> (AsyncClient, EventLoop) {
    let provider = rustls::crypto::ring::default_provider();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider.clone()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    let mut options = MqttOptions::new(
        "test-client",
        mock.address().ip().to_string(),
        mock.address().port(),
    );
    options
        .set_credentials(USERNAME, password)
        .set_keep_alive(Duration::from_secs(5))
        .set_transport(Transport::Tls(TlsConfiguration::Rustls(Arc::new(config))));
    AsyncClient::new(options, 16)
}

// rumqttc's error type is large; it is not ours to shrink.
#[allow(clippy::result_large_err)]
async fn connack(events: &mut EventLoop) -> Result<ConnAck, ConnectionError> {
    loop {
        if let Event::Incoming(Packet::ConnAck(ack)) = events.poll().await? {
            return Ok(ack);
        }
    }
}

async fn next_publish(events: &mut EventLoop) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Event::Incoming(Packet::Publish(p)) = events.poll().await.unwrap() {
                return serde_json::from_slice(&p.payload).unwrap();
            }
        }
    })
    .await
    .expect("a report")
}

#[tokio::test]
async fn reports_flow_and_requests_are_recorded() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    let (client, mut events) = client(&mock, CODE);
    assert_eq!(
        connack(&mut events).await.unwrap().code,
        ConnectReturnCode::Success
    );
    client
        .subscribe(format!("device/{SERIAL}/report"), QoS::AtMostOnce)
        .await
        .unwrap();
    // Wait for the subscription to land.
    loop {
        if let Event::Incoming(Packet::SubAck(_)) = events.poll().await.unwrap() {
            break;
        }
    }

    mock.report_fixture("printing");
    let report = next_publish(&mut events).await;
    assert_eq!(report["print"]["gcode_state"], "RUNNING");

    client
        .publish(
            format!("device/{SERIAL}/request"),
            QoS::AtMostOnce,
            false,
            json!({ "print": { "sequence_id": "1", "command": "pause" } }).to_string(),
        )
        .await
        .unwrap();
    let request = tokio::spawn(async move {
        loop {
            if events.poll().await.is_err() {
                break;
            }
        }
    });
    let pause = mock
        .wait_for_request(Duration::from_secs(3), |r| r["print"]["command"] == "pause")
        .await
        .expect("the pause was recorded");
    assert_eq!(pause["print"]["sequence_id"], "1");
    assert_eq!(mock.requests().len(), 1);
    request.abort();
}

#[tokio::test]
async fn pushall_returns_the_whole_merged_state() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    mock.report_fixture("printing");
    mock.report_fixture("printing_delta");
    let (client, mut events) = client(&mock, CODE);
    connack(&mut events).await.unwrap();
    client
        .subscribe(format!("device/{SERIAL}/report"), QoS::AtMostOnce)
        .await
        .unwrap();
    loop {
        if let Event::Incoming(Packet::SubAck(_)) = events.poll().await.unwrap() {
            break;
        }
    }
    client
        .publish(
            format!("device/{SERIAL}/request"),
            QoS::AtMostOnce,
            false,
            json!({ "pushing": { "sequence_id": "0", "command": "pushall" } }).to_string(),
        )
        .await
        .unwrap();
    let state = next_publish(&mut events).await;
    assert_eq!(
        state["print"]["gcode_state"], "RUNNING",
        "from the full report"
    );
    assert_eq!(state["print"]["mc_percent"], 44, "from the delta");
}

#[tokio::test]
async fn the_wrong_access_code_is_refused_as_a_printer_refuses_it() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    let (_client, mut events) = client(&mock, "99999999");
    match connack(&mut events).await {
        Err(ConnectionError::ConnectionRefused(code)) => {
            assert_eq!(code, ConnectReturnCode::NotAuthorized);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(mock.refused_logins(), 1);
    assert_eq!(mock.accepted_logins(), 0);
}

#[tokio::test]
async fn it_can_go_down_and_come_back_on_the_same_port() {
    let mock = MockBambu::start(SERIAL, CODE).await;
    let (_client, mut events) = client(&mock, CODE);
    connack(&mut events).await.unwrap();
    assert_eq!(mock.open_connections(), 1);

    mock.go_down();
    let dropped = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if events.poll().await.is_err() {
                return;
            }
        }
    })
    .await;
    assert!(dropped.is_ok(), "the client saw the connection drop");
    assert!(
        tokio::net::TcpStream::connect(mock.address())
            .await
            .is_err(),
        "nothing listens while down"
    );

    mock.come_back().await;
    let (_client, mut events) = client(&mock, CODE);
    connack(&mut events).await.unwrap();
    assert_eq!(mock.accepted_logins(), 2);
}

#[test]
fn every_fixture_parses_and_is_documented() {
    let readme = include_str!("../fixtures/README.md");
    for (name, _) in fixtures::ALL {
        let value = fixtures::load(name);
        assert!(value["print"].is_object(), "{name} has a print object");
        assert!(
            readme.contains(&format!("`{name}.json`")),
            "{name}.json has a row in fixtures/README.md"
        );
    }
}

#[test]
fn no_fixture_contains_a_real_looking_secret() {
    for (name, text) in fixtures::ALL {
        for word in ["access_code", "password", "dev_id", "sn"] {
            assert!(
                !text.contains(&format!("\"{word}\"")),
                "{name} carries a `{word}` field"
            );
        }
    }
}

#[test]
fn reads_the_login_from_a_connect_packet() {
    // A CONNECT for client "c", user "bblp", password "pw", keep alive 5.
    let mut packet = vec![0x10, 0];
    let body: Vec<u8> = [
        &[0, 4][..],
        b"MQTT",
        &[4, 0xC2, 0, 5],
        &[0, 1],
        b"c",
        &[0, 4],
        b"bblp",
        &[0, 2],
        b"pw",
    ]
    .concat();
    packet[1] = body.len() as u8;
    packet.extend(body);
    assert_eq!(login(&packet), Some(("bblp".into(), b"pw".to_vec())));
    assert_eq!(login(&[0x20, 2, 0, 0]), None);
}

type FtpControl = tokio::io::BufReader<tokio_rustls::client::TlsStream<TcpStream>>;

/// A raw FTPS control connection to the mock, one command at a time.
async fn ftps_control(mock: &ftps::MockFtps) -> FtpControl {
    let provider = rustls::crypto::ring::default_provider();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider.clone()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    let tcp = TcpStream::connect(mock.address()).await.unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .unwrap();
    let mut control = tokio::io::BufReader::new(tls);
    assert!(ftp_line(&mut control).await.starts_with("220"));
    control
}

async fn ftp_line(control: &mut FtpControl) -> String {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    control.read_line(&mut line).await.unwrap();
    line.trim_end().to_owned()
}

async fn ftp(control: &mut FtpControl, command: &str) -> String {
    control
        .get_mut()
        .write_all(format!("{command}\r\n").as_bytes())
        .await
        .unwrap();
    ftp_line(control).await
}

#[tokio::test]
async fn the_ftps_mock_checks_the_login() {
    let mock = ftps::MockFtps::start(CODE).await;
    let mut control = ftps_control(&mock).await;
    assert!(ftp(&mut control, "TYPE I").await.starts_with("530"));
    assert!(ftp(&mut control, "USER bblp").await.starts_with("331"));
    assert!(ftp(&mut control, "PASS 11111111").await.starts_with("530"));
    assert!(ftp(&mut control, "USER bblp").await.starts_with("331"));
    let pass = format!("PASS {CODE}");
    assert!(ftp(&mut control, &pass).await.starts_with("230"));
    assert!(ftp(&mut control, "TYPE I").await.starts_with("200"));
    assert!(ftp(&mut control, "MKD x").await.starts_with("502"));
    assert!(ftp(&mut control, "EPSV").await.starts_with("229 "));
    assert!(ftp(&mut control, "QUIT").await.starts_with("221"));
    let login = |accepted| ftps::Login {
        user: "bblp".into(),
        accepted,
    };
    assert_eq!(mock.logins(), [login(false), login(true)]);

    mock.refuse_login();
    let mut control = ftps_control(&mock).await;
    ftp(&mut control, "USER bblp").await;
    assert!(ftp(&mut control, &pass).await.starts_with("530"));
    assert!(mock.files().is_empty());
}

#[tokio::test]
async fn the_ftps_mock_refuses_stor_when_told_to() {
    let mock = ftps::MockFtps::start(CODE).await;
    mock.fail_stor(552, "Disk full.");
    let mut control = ftps_control(&mock).await;
    ftp(&mut control, "USER bblp").await;
    ftp(&mut control, &format!("PASS {CODE}")).await;
    ftp(&mut control, "PROT P").await;
    assert!(ftp(&mut control, "PASV").await.starts_with("227 "));
    assert_eq!(ftp(&mut control, "STOR cube.3mf").await, "552 Disk full.");
    assert!(mock.files().is_empty());
}
