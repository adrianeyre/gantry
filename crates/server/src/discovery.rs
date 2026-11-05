//! LAN auto-discovery (PRD §7.6): Printers that announce themselves on the
//! network are offered to an Admin to add, with what was found filled in.
//! Credentials (an access code, an API key) are still entered by hand.
//!
//! - **Bambu Lab** printers announce themselves over SSDP: a `NOTIFY` with
//!   `NT: urn:bambulab-com:device:3dprinter:1`, sent to UDP port 2021 (and
//!   1990), naming the printer's address in `Location`, its serial in
//!   `USN` and its name in `DevName.bambu.com`. Gantry also sends an
//!   `M-SEARCH` for that type now and then, which they answer the same way.
//! - **Moonraker** and **OctoPrint** advertise `_moonraker._tcp` and
//!   `_octoprint._tcp` over mDNS (their zeroconf support, on by default).
//!
//! What each announcement means is worked out by pure functions
//! ([`from_ssdp`], [`from_mdns`]), tested with recorded announcements; the
//! sockets only feed them. A Printer already in the Fleet (same Driver and
//! address, or same serial) is not offered again. An Admin can turn
//! discovery off under Settings, which stops listening and forgets what was
//! found.
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::AppState;

/// How long a Printer stays offered after it was last heard.
pub const FORGET_AFTER: Duration = Duration::from_secs(15 * 60);

/// Bambu's SSDP device type.
pub const BAMBU_TYPE: &str = "urn:bambulab-com:device:3dprinter:1";

/// The mDNS service types, with the Driver each is for.
pub const MDNS_TYPES: [(&str, &str); 2] = [
    ("_moonraker._tcp.local.", "moonraker"),
    ("_octoprint._tcp.local.", "octoprint"),
];

/// A Printer found on the network, as offered to an Admin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    /// The Driver to add it with.
    pub driver: String,
    /// What it calls itself.
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub model: Option<String>,
    /// For a Bambu printer.
    pub serial: Option<String>,
    /// How it was found: `ssdp` or `mdns`.
    pub via: &'static str,
    /// The add-Printer form, filled in: settings by field key.
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// Credential fields that can be filled in too (a Bambu serial number,
    /// which Bambu's own announcement broadcasts). Never an access code or
    /// an API key.
    pub secrets: serde_json::Map<String, serde_json::Value>,
}

impl Found {
    /// What identifies it across announcements.
    fn key(&self) -> String {
        match &self.serial {
            Some(serial) => format!("{}:{serial}", self.driver),
            None => format!("{}:{}:{}", self.driver, self.host, self.port.unwrap_or(0)),
        }
    }
}

/// Reads one SSDP message (a `NOTIFY` or an `M-SEARCH` answer). `None`
/// unless it is a Bambu printer announcing itself.
pub fn from_ssdp(message: &str, sender: Option<SocketAddr>) -> Option<Found> {
    let mut lines = message.lines();
    let first = lines.next()?.trim();
    if !(first.starts_with("NOTIFY") || first.starts_with("HTTP/1.1 200")) {
        return None;
    }
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let kind = headers.get("nt").or_else(|| headers.get("st"))?;
    if kind != BAMBU_TYPE {
        return None;
    }
    // Bambu puts a bare address in Location, not a URL.
    let host = headers
        .get("location")
        .map(|l| {
            l.trim_start_matches("http://")
                .split(['/', ':'])
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .filter(|h| !h.is_empty())
        .or_else(|| sender.map(|s| s.ip().to_string()))?;
    let serial = headers
        .get("usn")
        .map(|u| {
            u.trim_start_matches("uuid:")
                .split("::")
                .next()
                .unwrap_or(u)
                .to_owned()
        })
        .filter(|s| !s.is_empty());
    let model = headers
        .get("devmodel.bambu.com")
        .map(|code| bambu_model(code).unwrap_or(code).to_owned());
    let name = headers
        .get("devname.bambu.com")
        .cloned()
        .or_else(|| serial.clone())
        .unwrap_or_else(|| host.clone());
    let mut settings = serde_json::Map::new();
    settings.insert("host".into(), host.clone().into());
    if let Some(model) = model.as_ref().filter(|m| bambu_model_name(m)) {
        settings.insert("model".into(), model.clone().into());
    }
    let mut secrets = serde_json::Map::new();
    if let Some(serial) = &serial {
        secrets.insert("serial".into(), serial.clone().into());
    }
    Some(Found {
        driver: "bambu".into(),
        name,
        host,
        port: None,
        model,
        serial,
        via: "ssdp",
        settings,
        secrets,
    })
}

/// Bambu's model codes in SSDP, as the community documents them, to the
/// names the Bambu Driver takes.
fn bambu_model(code: &str) -> Option<&'static str> {
    Some(match code.trim() {
        "BL-P001" => "X1C",
        "BL-P002" => "X1",
        "C13" => "X1E",
        "C11" => "P1P",
        "C12" => "P1S",
        "N2S" => "A1",
        "N1" => "A1 mini",
        _ => return None,
    })
}

fn bambu_model_name(name: &str) -> bool {
    ["X1C", "X1", "X1E", "P1P", "P1S", "A1", "A1 mini"].contains(&name)
}

/// Reads one resolved mDNS service. `None` for a type Gantry doesn't know,
/// or one without an IPv4 address.
pub fn from_mdns(
    service_type: &str,
    instance: &str,
    addresses: &[Ipv4Addr],
    port: u16,
    txt: &HashMap<String, String>,
) -> Option<Found> {
    let driver = MDNS_TYPES
        .iter()
        .find(|(t, _)| t.trim_end_matches('.') == service_type.trim_end_matches('.'))?
        .1;
    // Prefer a private address, which is what a LAN printer has.
    let host = addresses
        .iter()
        .find(|a| a.is_private())
        .or_else(|| {
            addresses
                .iter()
                .find(|a| !a.is_loopback() && !a.is_link_local())
        })?
        .to_string();
    let name = instance
        .strip_suffix(service_type)
        .unwrap_or(instance)
        .trim_end_matches('.')
        .to_owned();
    // OctoPrint names the printer's model in its TXT record when set.
    let model = txt.get("model").filter(|m| !m.trim().is_empty()).map(|m| {
        match txt.get("vendor").filter(|v| !v.trim().is_empty()) {
            Some(vendor) if !m.starts_with(vendor.as_str()) => format!("{vendor} {m}"),
            _ => m.clone(),
        }
    });
    let mut settings = serde_json::Map::new();
    settings.insert("host".into(), host.clone().into());
    settings.insert("port".into(), port.into());
    if let Some(model) = &model {
        settings.insert("model".into(), model.clone().into());
    }
    Some(Found {
        driver: driver.into(),
        name: if name.is_empty() { host.clone() } else { name },
        host,
        port: Some(port),
        model,
        serial: None,
        via: "mdns",
        settings,
        secrets: serde_json::Map::new(),
    })
}

/// What has been found, and when each was last heard.
#[derive(Default)]
pub struct Discovered(Mutex<HashMap<String, (Found, Instant)>>);

impl Discovered {
    pub fn record(&self, found: Found) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.insert(found.key(), (found, Instant::now()));
    }

    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Everything heard from recently, by name.
    pub fn list(&self) -> Vec<Found> {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|_, (_, at)| at.elapsed() < FORGET_AFTER);
        let mut out: Vec<Found> = map.values().map(|(f, _)| f.clone()).collect();
        out.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.host.cmp(&b.host))
        });
        out
    }
}

/// A Printer already in the Fleet, as discovery compares it.
#[derive(Debug, Clone, Default)]
pub struct Known {
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub serial: Option<String>,
}

/// Whether `found` is a Printer already in the Fleet: the same Driver and
/// address (and port, when both say), or the same serial.
pub fn already_added(found: &Found, known: &[Known]) -> bool {
    known.iter().any(|k| {
        if k.driver != found.driver {
            return false;
        }
        if let (Some(a), Some(b)) = (&k.serial, &found.serial)
            && a.eq_ignore_ascii_case(b)
        {
            return true;
        }
        k.host
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case(&found.host))
            && match (k.port, found.port) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    })
}

/// The Printers in the Fleet, for [`already_added`].
pub async fn known(state: &AppState) -> Result<Vec<Known>, sqlx::Error> {
    let mut out = Vec::new();
    for printer in state.store().printers(state.key()).await? {
        let text = |key: &str| {
            printer
                .settings
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(|s| s.trim().to_owned())
        };
        let serial = if printer.secrets_set.iter().any(|s| s == "serial") {
            state
                .store()
                .printer_secrets(state.key(), &printer.id)
                .await?
                .get("serial")
                .map(str::to_owned)
        } else {
            text("serial")
        };
        out.push(Known {
            port: printer
                .settings
                .get("port")
                .and_then(serde_json::Value::as_u64)
                .and_then(|p| u16::try_from(p).ok()),
            host: text("host"),
            serial,
            driver: printer.driver,
        });
    }
    Ok(out)
}

/// Turns the listeners on and off with the Admin's setting. Runs until the
/// process ends. Only started for a real server, never in tests.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut running: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        loop {
            let on = crate::api::settings::current(&state)
                .await
                .map(|s| s.discovery_enabled)
                .unwrap_or(false);
            if on && running.is_empty() {
                tracing::info!("looking for Printers on the LAN (SSDP and mDNS)");
                running = listen(&state);
            } else if !on && !running.is_empty() {
                tracing::info!("stopped looking for Printers on the LAN");
                for task in running.drain(..) {
                    task.abort();
                }
                state.discovered().clear();
            }
            tokio::select! {
                () = state.discovery_changed() => {}
                () = tokio::time::sleep(Duration::from_secs(30)) => {}
                () = state.stopping() => return,
            }
        }
    });
}

fn listen(state: &AppState) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = Vec::new();
    for port in [2021, 1990] {
        let state = state.clone();
        tasks.push(tokio::spawn(async move {
            if let Err(e) = ssdp(&state, port).await {
                tracing::warn!(port, error = %e, "SSDP discovery isn't listening");
            }
        }));
    }
    let state = state.clone();
    tasks.push(tokio::spawn(async move {
        if let Err(e) = mdns(&state).await {
            tracing::warn!(error = %e, "mDNS discovery isn't running");
        }
    }));
    tasks
}

async fn ssdp(state: &AppState, port: u16) -> anyhow::Result<()> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    let multicast = Ipv4Addr::new(239, 255, 255, 250);
    if let Err(e) = socket.join_multicast_v4(&multicast, &Ipv4Addr::UNSPECIFIED) {
        tracing::debug!(error = %e, "could not join the SSDP multicast group; broadcasts still arrive");
    }
    socket.set_broadcast(true)?;
    let socket = tokio::net::UdpSocket::from_std(socket.into())?;
    let search = format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1990\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: {BAMBU_TYPE}\r\n\r\n"
    );
    let mut ask = tokio::time::interval(Duration::from_secs(60));
    let mut buffer = vec![0u8; 4096];
    loop {
        tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                let (len, from) = received?;
                if let Ok(text) = std::str::from_utf8(&buffer[..len])
                    && let Some(found) = from_ssdp(text, Some(from))
                {
                    state.discovered().record(found);
                }
            }
            _ = ask.tick(), if port == 1990 => {
                let _ = socket.send_to(search.as_bytes(), (multicast, 1990)).await;
            }
        }
    }
}

async fn mdns(state: &AppState) -> anyhow::Result<()> {
    let daemon = mdns_sd::ServiceDaemon::new()?;
    let mut receivers = Vec::new();
    for (service, _) in MDNS_TYPES {
        receivers.push(daemon.browse(service)?);
    }
    let result = async {
        let mut events =
            futures::stream::select_all(receivers.into_iter().map(|r| Box::pin(r.into_stream())));
        use futures::StreamExt;
        while let Some(event) = events.next().await {
            if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                let txt: HashMap<String, String> = info
                    .get_properties()
                    .iter()
                    .map(|p| (p.key().to_owned(), p.val_str().to_owned()))
                    .collect();
                let addresses: Vec<Ipv4Addr> =
                    info.get_addresses_v4().into_iter().copied().collect();
                if let Some(found) = from_mdns(
                    info.get_type(),
                    info.get_fullname(),
                    &addresses,
                    info.get_port(),
                    &txt,
                ) {
                    state.discovered().record(found);
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let _ = daemon.shutdown();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A NOTIFY as Bambu printers send it (as the community has recorded
    /// it, with the serial made up).
    const BAMBU_NOTIFY: &str = "NOTIFY * HTTP/1.1\r\n\
        HOST: 239.255.255.250:1900\r\n\
        Server: UPnP/1.0\r\n\
        Location: 192.168.1.50\r\n\
        NT: urn:bambulab-com:device:3dprinter:1\r\n\
        NTS: ssdp:alive\r\n\
        USN: 01P00A123456789\r\n\
        Cache-Control: max-age=1800\r\n\
        DevModel.bambu.com: C12\r\n\
        DevName.bambu.com: Workshop P1S\r\n\
        DevSignal.bambu.com: -45\r\n\
        DevConnect.bambu.com: lan\r\n\
        DevBind.bambu.com: free\r\n\
        Devseclink.bambu.com: secure\r\n\r\n";

    #[test]
    fn a_bambu_notify_is_a_printer_with_its_address_serial_and_model() {
        let found = from_ssdp(BAMBU_NOTIFY, None).unwrap();
        assert_eq!(found.driver, "bambu");
        assert_eq!(found.name, "Workshop P1S");
        assert_eq!(found.host, "192.168.1.50");
        assert_eq!(found.serial.as_deref(), Some("01P00A123456789"));
        assert_eq!(found.model.as_deref(), Some("P1S"));
        assert_eq!(found.settings["host"], "192.168.1.50");
        assert_eq!(found.settings["model"], "P1S");
        assert_eq!(found.secrets["serial"], "01P00A123456789");
        assert!(!found.secrets.contains_key("access_code"));
    }

    #[test]
    fn an_m_search_answer_counts_and_other_ssdp_doesnt() {
        let answer = BAMBU_NOTIFY
            .replace("NOTIFY * HTTP/1.1", "HTTP/1.1 200 OK")
            .replace("NT:", "ST:")
            .replace("Location: 192.168.1.50\r\n", "")
            .replace("DevModel.bambu.com: C12", "DevModel.bambu.com: Z99");
        let from: SocketAddr = "192.168.1.77:2021".parse().unwrap();
        let found = from_ssdp(&answer, Some(from)).unwrap();
        assert_eq!(found.host, "192.168.1.77", "the sender, without Location");
        assert_eq!(found.model.as_deref(), Some("Z99"));
        assert!(
            !found.settings.contains_key("model"),
            "an unknown code isn't filled in"
        );

        let router = "NOTIFY * HTTP/1.1\r\nNT: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nLocation: http://192.168.1.1:5000/desc.xml\r\n\r\n";
        assert!(from_ssdp(router, None).is_none());
        let search = "M-SEARCH * HTTP/1.1\r\nST: urn:bambulab-com:device:3dprinter:1\r\n\r\n";
        assert!(
            from_ssdp(search, None).is_none(),
            "another searcher, not a printer"
        );
    }

    fn txt(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn octoprint_and_moonraker_mdns_services_are_printers() {
        let octoprint = from_mdns(
            "_octoprint._tcp.local.",
            "OctoPi Ender._octoprint._tcp.local.",
            &[
                Ipv4Addr::new(169, 254, 3, 4),
                Ipv4Addr::new(192, 168, 1, 60),
            ],
            80,
            &txt(&[
                ("path", "/"),
                ("version", "1.11.2"),
                ("api", "0.1"),
                ("model", "Ender-3 V2"),
                ("vendor", "Creality"),
            ]),
        )
        .unwrap();
        assert_eq!(octoprint.driver, "octoprint");
        assert_eq!(octoprint.name, "OctoPi Ender");
        assert_eq!(octoprint.host, "192.168.1.60");
        assert_eq!(octoprint.port, Some(80));
        assert_eq!(octoprint.model.as_deref(), Some("Creality Ender-3 V2"));
        assert_eq!(octoprint.settings["port"], 80);
        assert!(
            octoprint.secrets.is_empty(),
            "the API key is entered by hand"
        );

        let moonraker = from_mdns(
            "_moonraker._tcp.local.",
            "voron._moonraker._tcp.local.",
            &[Ipv4Addr::new(10, 0, 0, 9)],
            7125,
            &txt(&[]),
        )
        .unwrap();
        assert_eq!(moonraker.driver, "moonraker");
        assert_eq!(moonraker.name, "voron");
        assert_eq!(moonraker.settings["host"], "10.0.0.9");
        assert!(moonraker.model.is_none());

        assert!(
            from_mdns(
                "_http._tcp.local.",
                "x._http._tcp.local.",
                &[Ipv4Addr::new(10, 0, 0, 1)],
                80,
                &txt(&[])
            )
            .is_none()
        );
        assert!(from_mdns("_moonraker._tcp.local.", "x", &[], 7125, &txt(&[])).is_none());
    }

    #[test]
    fn printers_already_added_are_recognised_by_address_or_serial() {
        let bambu = from_ssdp(BAMBU_NOTIFY, None).unwrap();
        let known = |driver: &str, host: Option<&str>, port, serial: Option<&str>| Known {
            driver: driver.into(),
            host: host.map(Into::into),
            port,
            serial: serial.map(Into::into),
        };
        assert!(already_added(
            &bambu,
            &[known("bambu", Some("192.168.1.50"), None, None)]
        ));
        // It moved address, but it is the same printer.
        assert!(already_added(
            &bambu,
            &[known(
                "bambu",
                Some("192.168.1.99"),
                None,
                Some("01p00a123456789")
            )]
        ));
        assert!(!already_added(
            &bambu,
            &[known("bambu", Some("192.168.1.99"), None, Some("other"))]
        ));
        assert!(!already_added(
            &bambu,
            &[known("octoprint", Some("192.168.1.50"), None, None)]
        ));

        let octoprint = from_mdns(
            "_octoprint._tcp.local.",
            "a._octoprint._tcp.local.",
            &[Ipv4Addr::new(192, 168, 1, 60)],
            5000,
            &HashMap::new(),
        )
        .unwrap();
        assert!(already_added(
            &octoprint,
            &[known("octoprint", Some("192.168.1.60"), Some(5000), None)]
        ));
        // Another OctoPrint on the same host, on another port.
        assert!(!already_added(
            &octoprint,
            &[known("octoprint", Some("192.168.1.60"), Some(80), None)]
        ));
        assert!(already_added(
            &octoprint,
            &[known("octoprint", Some("192.168.1.60"), None, None)]
        ));
    }

    #[test]
    fn the_same_printer_heard_twice_is_one_entry() {
        let discovered = Discovered::default();
        discovered.record(from_ssdp(BAMBU_NOTIFY, None).unwrap());
        discovered.record(
            from_ssdp(&BAMBU_NOTIFY.replace("192.168.1.50", "192.168.1.51"), None).unwrap(),
        );
        let list = discovered.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].host, "192.168.1.51", "the newest address");
        discovered.clear();
        assert!(discovered.list().is_empty());
    }
}
