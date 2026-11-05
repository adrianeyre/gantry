//! Structured logs through `tracing`: human-readable by default, JSON for
//! containers and log shippers (`log_format = "json"` or
//! `GANTRY_LOG_FORMAT=json`).
use gantry_server::config::LogFormat;
use tracing_subscriber::EnvFilter;

/// Directives applied whatever `RUST_LOG` says. The MQTT client logs topic
/// names at debug level, and a Bambu printer's topics carry its serial number,
/// which Gantry treats as a credential.
const ALWAYS: &[&str] = &["rumqttc=warn"];

/// The filter: `RUST_LOG` (default `info`), then [`ALWAYS`].
pub fn filter(rust_log: Option<&str>) -> EnvFilter {
    let mut filter = rust_log
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new("info"));
    for directive in ALWAYS {
        filter = filter.add_directive(directive.parse().expect("a valid directive"));
    }
    filter
}

/// Installs the global subscriber. The level comes from `RUST_LOG`
/// (default `info`).
pub fn init(format: LogFormat) {
    let filter = filter(std::env::var("RUST_LOG").ok().as_deref());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Json => builder.json().flatten_event(true).init(),
        LogFormat::Text => builder.init(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mqtt_client_stays_quiet_even_at_trace() {
        let shown = filter(Some("trace,rumqttc=trace")).to_string();
        assert!(shown.contains("rumqttc=warn"), "{shown}");
        assert!(!shown.contains("rumqttc=trace"), "{shown}");
    }

    #[cfg(feature = "bambu")]
    #[test]
    fn covers_what_the_bambu_driver_asks_for() {
        for directive in gantry_driver_bambu::LOG_DIRECTIVES {
            assert!(ALWAYS.contains(directive), "{directive}");
        }
    }
}
