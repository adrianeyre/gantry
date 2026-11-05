//! Webhooks (PRD §7.7): the same events as Alerts, sent as signed JSON to
//! URLs an Admin registers, so other tools can act on them.
//!
//! Each delivery is one `POST` of a JSON [`Payload`], with these headers:
//!
//! - `Gantry-Event`: the event, such as `job_ended` (or `test`).
//! - `Gantry-Delivery`: an id for this event, the same on every retry, so
//!   a receiver can drop duplicates.
//! - `Gantry-Signature`: `t=<unix seconds>,v1=<hex>`, where the hex is
//!   HMAC-SHA256, keyed with the webhook's secret, of `<t>.<body>` (the
//!   timestamp, a full stop, then the exact bytes of the body). A receiver
//!   recomputes it, compares in constant time, and refuses a `t` more than
//!   a few minutes old. `docs/webhooks.md` shows how.
//!
//! A delivery succeeds on any 2xx. Otherwise it is tried again after each
//! wait in [`Backoff`] (by default 1 s, doubling to 64 s: 8 tries in all),
//! except when the receiver says the request itself is wrong (a 4xx other
//! than 408 and 429), which no retry would fix. Redirects are not followed.
//! Deliveries run in their own tasks and never hold up Alerts or the server.
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::AppState;
use crate::alerts::{Alert, EventKind, Level};
use crate::store::alerts::Rule;
use crate::store::webhooks::Webhook;

/// How long one try may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The waits between tries of a failed delivery.
#[derive(Debug, Clone)]
pub struct Backoff(pub Vec<Duration>);

impl Backoff {
    /// `tries - 1` waits, starting at `first` and doubling, each at most `max`.
    pub fn exponential(first: Duration, max: Duration, tries: u32) -> Self {
        Self(
            (0..tries.saturating_sub(1))
                .map(|n| first.saturating_mul(2u32.saturating_pow(n)).min(max))
                .collect(),
        )
    }
}

impl Default for Backoff {
    /// 1 s, 2 s, 4 s … 64 s: 8 tries in all (each wait at most 5 minutes).
    fn default() -> Self {
        Self::exponential(Duration::from_secs(1), Duration::from_secs(300), 8)
    }
}

/// The Printer an event is about.
#[derive(Debug, Clone, Serialize)]
pub struct PrinterRef {
    pub id: String,
    pub name: String,
}

/// What a webhook is sent.
#[derive(Debug, Clone, Serialize)]
pub struct Payload {
    /// The same as the `Gantry-Delivery` header.
    pub id: String,
    /// `job_ended`, `job_paused`, `fault_raised`, `connection_lost`,
    /// `low_filament`, or `test`.
    pub event: String,
    /// When Gantry saw it, RFC 3339.
    pub created_at: String,
    pub printer: Option<PrinterRef>,
    /// For `job_ended`: `succeeded`, `failed`, `cancelled` or `lost`.
    pub outcome: Option<gantry_core::JobOutcome>,
    /// The same words an Alert would use.
    pub title: String,
    pub message: String,
    pub level: Level,
}

impl Payload {
    pub fn from_alert(alert: &Alert) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            event: serde_json::to_value(alert.event.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            created_at: crate::store::rfc3339(crate::store::now()),
            printer: (!alert.event.printer_id.is_empty()).then(|| PrinterRef {
                id: alert.event.printer_id.clone(),
                name: alert.event.printer_name.clone(),
            }),
            outcome: alert.event.outcome,
            title: alert.title.clone(),
            message: alert.message.clone(),
            level: alert.level,
        }
    }

    /// A delivery an Admin asks for, to check their receiver.
    pub fn test(by: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            event: "test".into(),
            created_at: crate::store::rfc3339(crate::store::now()),
            printer: None,
            outcome: None,
            title: "Gantry: test webhook".into(),
            message: format!("This is a test delivery from Gantry, sent by {by}."),
            level: Level::Info,
        }
    }
}

/// The `Gantry-Signature` value for `body` sent at `timestamp`.
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("t={timestamp},v1={hex}")
}

/// Whether `header` is a valid signature of `body` with `secret`, made no
/// more than `tolerance` from `now` (unix seconds). This is what a receiver
/// does; Gantry uses it in its own tests.
pub fn verify(secret: &str, header: &str, body: &[u8], now: i64, tolerance: i64) -> bool {
    let mut timestamp = None;
    let mut given = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", t)) => timestamp = t.parse::<i64>().ok(),
            Some(("v1", v)) => given.push(v.to_owned()),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    if (now - timestamp).abs() > tolerance {
        return false;
    }
    let expected = signature(secret, timestamp, body);
    let expected = expected.rsplit_once("v1=").map_or("", |(_, v)| v);
    given.iter().any(|v| {
        v.len() == expected.len()
            && v.bytes()
                .zip(expected.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    })
}

/// How one try went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TryResult {
    Delivered(u16),
    /// Worth trying again: unreachable, a timeout, a 5xx, 408 or 429.
    Retry(String),
    /// The receiver refused the request itself (another 4xx).
    GiveUp(String),
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("Gantry-Webhook/{}", crate::VERSION))
        .build()
        .unwrap_or_default()
}

/// One try at delivering `body`.
pub async fn send_once(hook: &Webhook, payload: &Payload, body: &[u8]) -> TryResult {
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
    let response = client()
        .post(&hook.url)
        .header("content-type", "application/json")
        .header("gantry-event", &payload.event)
        .header("gantry-delivery", &payload.id)
        .header("gantry-signature", signature(&hook.secret, timestamp, body))
        .body(body.to_vec())
        .send()
        .await;
    match response {
        Err(e) => TryResult::Retry(if e.is_timeout() {
            "timed out".into()
        } else {
            "could not connect".into()
        }),
        Ok(r) if r.status().is_success() => TryResult::Delivered(r.status().as_u16()),
        Ok(r) => {
            let status = r.status().as_u16();
            if matches!(status, 408 | 429) || status >= 500 {
                TryResult::Retry(format!("answered {status}"))
            } else {
                TryResult::GiveUp(format!("answered {status}"))
            }
        }
    }
}

/// Delivers `payload`, trying again after each wait in `backoff`. Returns
/// the last try's result.
pub async fn deliver(hook: &Webhook, payload: &Payload, backoff: &Backoff) -> TryResult {
    let body = serde_json::to_vec(payload).expect("payloads serialise");
    let mut waits = backoff.0.iter();
    let mut attempt = 1;
    loop {
        let result = send_once(hook, payload, &body).await;
        match &result {
            TryResult::Delivered(status) => {
                tracing::info!(webhook = %hook.name, event = %payload.event, attempt, status, "webhook delivered");
                return result;
            }
            TryResult::GiveUp(why) => {
                tracing::error!(webhook = %hook.name, event = %payload.event, attempt, error = %why, "webhook refused; not trying again");
                return result;
            }
            TryResult::Retry(why) => match waits.next() {
                Some(wait) => {
                    tracing::warn!(webhook = %hook.name, event = %payload.event, attempt, error = %why, wait_ms = wait.as_millis() as u64, "webhook not delivered; trying again");
                    tokio::time::sleep(*wait).await;
                    attempt += 1;
                }
                None => {
                    tracing::error!(webhook = %hook.name, event = %payload.event, attempts = attempt, error = %why, "webhook not delivered; giving up");
                    return result;
                }
            },
        }
    }
}

/// Whether `hook` wants this Alert's event: the same test as an Alert Rule.
pub fn wants(hook: &Webhook, alert: &Alert, printer_tags: &[i64]) -> bool {
    let rule = Rule {
        id: hook.id,
        name: hook.name.clone(),
        enabled: hook.enabled,
        events: hook.events.clone(),
        outcomes: hook.outcomes.clone(),
        printers: hook.printers.clone(),
        tags: hook.tags.clone(),
        channel_id: 0,
    };
    crate::alerts::matches(&rule, &alert.event, printer_tags)
}

/// Sends `alert`'s event to every webhook that wants it, each in its own task.
pub async fn dispatch(state: &AppState, alert: &Alert) {
    let hooks = match state.store().webhooks(state.key()).await {
        Ok(hooks) => hooks,
        Err(e) => {
            tracing::error!(error = %e, "could not read the webhooks");
            return;
        }
    };
    if hooks.is_empty() {
        return;
    }
    let tags = state
        .store()
        .printer_tag_ids(&alert.event.printer_id)
        .await
        .unwrap_or_default();
    let payload = Payload::from_alert(alert);
    for hook in hooks.into_iter().filter(|h| wants(h, alert, &tags)) {
        let payload = payload.clone();
        let backoff = state.webhook_backoff().clone();
        tokio::spawn(async move {
            deliver(&hook, &payload, &backoff).await;
        });
    }
}

/// The events a webhook can be sent, for the UI.
pub fn events() -> &'static [EventKind] {
    &EventKind::ALL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_hmac_sha256_of_the_timestamp_and_body() {
        // Worked by hand with:
        //   printf '1700000000.{"a":1}' | openssl dgst -sha256 -hmac whsec_test
        assert_eq!(
            signature("whsec_test", 1_700_000_000, br#"{"a":1}"#),
            "t=1700000000,v1=38877139021993b830af32feea6e18a8da83eb2f6e49ee50bd9e4cf4ca4d3789"
        );
        let header = signature("whsec_test", 1_700_000_000, br#"{"a":1}"#);
        assert!(verify(
            "whsec_test",
            &header,
            br#"{"a":1}"#,
            1_700_000_060,
            300
        ));
        assert!(!verify(
            "whsec_other",
            &header,
            br#"{"a":1}"#,
            1_700_000_060,
            300
        ));
        assert!(!verify(
            "whsec_test",
            &header,
            br#"{"a":2}"#,
            1_700_000_060,
            300
        ));
        // Too old: a replayed delivery.
        assert!(!verify(
            "whsec_test",
            &header,
            br#"{"a":1}"#,
            1_700_001_000,
            300
        ));
        assert!(!verify("whsec_test", "v1=abc", b"", 0, 300));
    }

    #[test]
    fn backoff_doubles_up_to_its_limit() {
        let backoff = Backoff::exponential(Duration::from_secs(1), Duration::from_secs(5), 6);
        assert_eq!(backoff.0, [1, 2, 4, 5, 5].map(Duration::from_secs).to_vec());
        assert_eq!(Backoff::default().0.len(), 7);
        assert_eq!(Backoff::default().0[6], Duration::from_secs(64));
    }
}
