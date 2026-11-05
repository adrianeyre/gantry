//! A probe of a real Bambu printer, for validating the Driver once one is
//! owned (#27, #28). `gantry bambu-probe` runs it.
//!
//! It connects the way the Driver does, asks for the whole state and the
//! firmware versions, records every payload for a while, and (optionally)
//! reads the camera. Payloads are saved with the serial number and access
//! code replaced, so they can become fixtures after a check by eye. It sends
//! no command that changes anything on the printer.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gantry_core::{CameraSource, PrinterStatus};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use serde_json::Value;

use crate::model::{self, CameraKind, Model};
use crate::report::Report;

/// What the serial number becomes in saved payloads.
pub const SERIAL_PLACEHOLDER: &str = "SERIAL-REDACTED";
/// What the access code becomes in saved payloads.
pub const CODE_PLACEHOLDER: &str = "ACCESS-CODE-REDACTED";

/// What to probe, and how.
#[derive(Clone)]
pub struct ProbeOptions {
    pub host: String,
    pub port: u16,
    pub serial: String,
    pub access_code: String,
    /// The model, when the serial number doesn't say.
    pub model: Option<String>,
    /// How long to record payloads.
    pub listen: Duration,
    /// Also read the camera, for this long.
    pub camera: Option<Duration>,
    pub camera_port: Option<u16>,
    /// Where to save payloads and the first camera frame.
    pub out: Option<PathBuf>,
}

impl std::fmt::Debug for ProbeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeOptions")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("model", &self.model)
            .field("listen", &self.listen)
            .field("camera", &self.camera)
            .finish_non_exhaustive()
    }
}

/// How the camera check went.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraCheck {
    pub kind: Option<&'static str>,
    pub frames: usize,
    pub seconds: f64,
    pub first_frame_bytes: Option<usize>,
    pub error: Option<String>,
}

/// What the probe found.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub model: Option<Model>,
    /// `(module, software version)` from `get_version`.
    pub firmware: Vec<(String, String)>,
    /// Status reports received.
    pub reports: usize,
    /// Answers and other messages received.
    pub other_messages: usize,
    /// How long the first status report took to arrive after connecting.
    pub first_status_after: Option<Duration>,
    /// The normalised status after merging every report.
    pub status: Option<PrinterStatus>,
    pub camera: Option<CameraCheck>,
    pub saved: Vec<PathBuf>,
}

/// Why the probe couldn't run.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("could not save to {path}: {source}")]
    Save {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Replaces every occurrence of each secret in every string in `value`.
pub fn redact(value: &mut Value, serial: &str, access_code: &str) {
    match value {
        Value::String(s) => {
            if !serial.is_empty() && s.contains(serial) {
                *s = s.replace(serial, SERIAL_PLACEHOLDER);
            }
            if !access_code.is_empty() && s.contains(access_code) {
                *s = s.replace(access_code, CODE_PLACEHOLDER);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| redact(v, serial, access_code)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|v| redact(v, serial, access_code)),
        _ => {}
    }
}

fn save(path: &Path, bytes: &[u8]) -> Result<(), ProbeError> {
    std::fs::write(path, bytes).map_err(|source| ProbeError::Save {
        path: path.to_owned(),
        source,
    })
}

/// Runs the probe. Changes nothing on the printer.
pub async fn probe(options: &ProbeOptions) -> Result<ProbeReport, ProbeError> {
    if let Some(out) = &options.out {
        std::fs::create_dir_all(out).map_err(|source| ProbeError::Save {
            path: out.clone(),
            source,
        })?;
    }
    let model = model::detect(options.model.as_deref(), &options.serial);
    let mut mqtt = MqttOptions::new(
        format!("gantry-probe-{}", std::process::id()),
        options.host.clone(),
        options.port,
    );
    mqtt.set_credentials("bblp", options.access_code.clone())
        .set_keep_alive(Duration::from_secs(30))
        .set_max_packet_size(1 << 20, 64 << 10)
        .set_transport(Transport::Tls(TlsConfiguration::Rustls(
            crate::tls::client_config(),
        )));
    let (client, mut eventloop) = AsyncClient::new(mqtt, 32);
    eventloop.network_options.set_connection_timeout(10);
    let report_topic = format!("device/{}/report", options.serial);
    let request_topic = format!("device/{}/request", options.serial);

    // Connect first, so a refusal is reported as one.
    let connected = Instant::now();
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => break,
            Ok(_) => {}
            Err(e) => {
                let (_, detail) = crate::classify(&e);
                return Err(ProbeError::Connect(detail));
            }
        }
        if connected.elapsed() > Duration::from_secs(20) {
            return Err(ProbeError::Connect("timed out".into()));
        }
    }
    let connected = Instant::now();
    let _ = client.try_subscribe(report_topic.as_str(), QoS::AtMostOnce);
    for request in [
        serde_json::json!({ "pushing": { "sequence_id": "0", "command": "pushall" } }),
        serde_json::json!({ "info": { "sequence_id": "1", "command": "get_version" } }),
    ] {
        let _ = client.try_publish(
            request_topic.as_str(),
            QoS::AtMostOnce,
            false,
            request.to_string(),
        );
    }

    let mut report = Report::default();
    let mut firmware = Vec::new();
    let mut reports = 0;
    let mut other_messages = 0;
    let mut first_status_after = None;
    let mut saved = Vec::new();
    let deadline = tokio::time::Instant::now() + options.listen;
    loop {
        let event = tokio::time::timeout_at(deadline, eventloop.poll()).await;
        let Ok(event) = event else { break };
        let Ok(Event::Incoming(Packet::Publish(publish))) = event else {
            if let Err(e) = event {
                tracing::warn!(error = %e, "the probe's connection dropped");
                break;
            }
            continue;
        };
        let Ok(mut payload) = serde_json::from_slice::<Value>(&publish.payload) else {
            continue;
        };
        if report.apply(&payload) {
            reports += 1;
            first_status_after.get_or_insert_with(|| connected.elapsed());
        } else {
            other_messages += 1;
        }
        if payload["info"]["command"] == "get_version"
            && let Some(modules) = payload["info"]["module"].as_array()
        {
            firmware = modules
                .iter()
                .filter_map(|m| {
                    Some((
                        m["name"].as_str()?.to_owned(),
                        m["sw_ver"].as_str()?.to_owned(),
                    ))
                })
                .collect();
        }
        if let Some(out) = &options.out {
            redact(&mut payload, &options.serial, &options.access_code);
            let path = out.join(format!("payload-{:04}.json", saved.len() + 1));
            let text = serde_json::to_string_pretty(&payload).expect("JSON serialises");
            save(&path, text.as_bytes())?;
            saved.push(path);
        }
    }
    let _ = client.try_disconnect();

    let camera = match options.camera {
        Some(watch) => Some(check_camera(options, model, watch, &mut saved).await?),
        None => None,
    };

    Ok(ProbeReport {
        model,
        firmware,
        reports,
        other_messages,
        first_status_after,
        status: report.received().then(|| report.status()),
        camera,
        saved,
    })
}

async fn check_camera(
    options: &ProbeOptions,
    model: Option<Model>,
    watch: Duration,
    saved: &mut Vec<PathBuf>,
) -> Result<CameraCheck, ProbeError> {
    let Some(model) = model else {
        return Ok(CameraCheck {
            kind: None,
            frames: 0,
            seconds: 0.0,
            first_frame_bytes: None,
            error: Some("the model is unknown: pass --model to choose the camera".into()),
        });
    };
    let (kind, source): (&'static str, std::sync::Arc<dyn CameraSource>) = match model.camera() {
        CameraKind::Jpeg => (
            "P1/A1 (TLS, JPEG, port 6000)",
            crate::camera::p1(
                &options.host,
                options.camera_port.unwrap_or(crate::camera::P1_CAMERA_PORT),
                &options.access_code,
            ),
        ),
        CameraKind::Rtsps => (
            "X1 (RTSPS, H.264, port 322)",
            crate::camera::x1(
                &options.host,
                options.camera_port.unwrap_or(crate::camera::X1_CAMERA_PORT),
                &options.access_code,
            ),
        ),
    };
    let started = Instant::now();
    let mut stream = source.open();
    let mut frames = 0;
    let mut first = None;
    let mut error = None;
    let deadline = tokio::time::Instant::now() + watch;
    while let Ok(next) = tokio::time::timeout_at(deadline, stream.next()).await {
        match next {
            Some(Ok(frame)) => {
                frames += 1;
                if first.is_none() {
                    first = Some(frame.len());
                    if let Some(out) = &options.out {
                        let path = out.join("camera-first-frame.jpg");
                        save(&path, &frame)?;
                        saved.push(path);
                    }
                }
            }
            Some(Err(e)) => {
                error = Some(e.to_string());
                break;
            }
            None => break,
        }
    }
    // Dropping the stream closes the camera connection.
    drop(stream);
    Ok(CameraCheck {
        kind: Some(kind),
        frames,
        seconds: started.elapsed().as_secs_f64(),
        first_frame_bytes: first,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_every_string_holding_a_secret() {
        let mut value = serde_json::json!({
            "info": { "module": [{ "sn": "01P00A123456789" }, { "sn": "other" }] },
            "url": "rtsps://bblp:87654321@10.0.0.2/streaming",
            "n": 1
        });
        redact(&mut value, "01P00A123456789", "87654321");
        let text = value.to_string();
        assert!(!text.contains("01P00A123456789") && !text.contains("87654321"));
        assert!(text.contains(SERIAL_PLACEHOLDER) && text.contains(CODE_PLACEHOLDER));
        assert!(text.contains("other"));
    }
}
