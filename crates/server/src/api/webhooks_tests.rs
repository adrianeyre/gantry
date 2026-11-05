//! Webhooks end to end (#71): Simulated Printers raise events, and a local
//! receiver checks each delivery's signature, sees failures retried with
//! growing waits, and that the secret never leaves Gantry after it is made.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

use crate::store::users::Role;
use crate::test_support::{Client, state_with};
use crate::webhooks::Backoff;

/// One delivery the receiver took in.
#[derive(Debug, Clone)]
struct Delivery {
    at: Instant,
    event: String,
    id: String,
    signature: String,
    body: Vec<u8>,
}

impl Delivery {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A local webhook receiver. Answers `fail_with` to the first `fail_first`
/// deliveries, then 204.
struct Receiver {
    url: String,
    deliveries: Arc<Mutex<Vec<Delivery>>>,
}

async fn receiver(fail_first: usize, fail_with: StatusCode) -> Receiver {
    let deliveries: Arc<Mutex<Vec<Delivery>>> = Arc::default();
    let failures = Arc::new(AtomicUsize::new(fail_first));
    let kept = deliveries.clone();
    let app = axum::Router::new().route(
        "/hooks/gantry",
        axum::routing::post(move |headers: HeaderMap, body: axum::body::Bytes| {
            let kept = kept.clone();
            let failures = failures.clone();
            async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                kept.lock().unwrap().push(Delivery {
                    at: Instant::now(),
                    event: header("gantry-event"),
                    id: header("gantry-delivery"),
                    signature: header("gantry-signature"),
                    body: body.to_vec(),
                });
                if failures
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
                {
                    fail_with
                } else {
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hooks/gantry", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    Receiver { url, deliveries }
}

impl Receiver {
    fn deliveries(&self) -> Vec<Delivery> {
        self.deliveries.lock().unwrap().clone()
    }

    async fn wait_for(&self, count: usize) -> Vec<Delivery> {
        for _ in 0..400 {
            let deliveries = self.deliveries();
            if deliveries.len() >= count {
                return deliveries;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("expected {count} deliveries, got {:?}", self.deliveries());
    }
}

async fn client(backoff: Backoff) -> Client {
    let state = state_with(crate::Options {
        webhook_backoff: Some(backoff),
        ..Default::default()
    })
    .await;
    Client::with_state(state).await
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

async fn webhook(client: &Client, body: Value) -> (i64, String) {
    let (status, hook) = client.post("/api/webhooks", body).await;
    assert_eq!(status, StatusCode::CREATED, "{hook}");
    let secret = hook["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("whsec_"));
    (hook["id"].as_i64().unwrap(), secret)
}

fn raise_fault(message: &str) -> Value {
    json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": message })
}

#[tokio::test]
async fn deliveries_are_signed_with_the_documented_scheme() {
    let client = client(Backoff(vec![])).await;
    let receiver = receiver(0, StatusCode::OK).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (_, secret) = webhook(
        &client,
        json!({ "name": "Home Assistant", "url": receiver.url, "events": ["fault_raised", "job_ended"] }),
    )
    .await;
    client
        .simulate(&printer, raise_fault("The door is open"))
        .await;
    let delivery = receiver.wait_for(1).await.remove(0);

    // The documented check: HMAC-SHA256 of "<t>.<body>" with the secret.
    assert!(crate::webhooks::verify(
        &secret,
        &delivery.signature,
        &delivery.body,
        now(),
        300
    ));
    assert!(!crate::webhooks::verify(
        "whsec_wrong",
        &delivery.signature,
        &delivery.body,
        now(),
        300
    ));
    // Recomputed independently of `verify`, as a receiver in another
    // language would.
    let (t, v1) = delivery.signature.split_once(",v1=").unwrap();
    let t: i64 = t.strip_prefix("t=").unwrap().parse().unwrap();
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{t}.").as_bytes());
    mac.update(&delivery.body);
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(v1, expected);
    assert!((now() - t).abs() < 60);

    let body = delivery.json();
    assert_eq!(delivery.event, "fault_raised");
    assert_eq!(body["event"], "fault_raised");
    assert_eq!(body["id"], delivery.id.as_str());
    assert_eq!(body["printer"]["id"], printer.as_str());
    assert_eq!(body["printer"]["name"], "X1");
    assert_eq!(body["title"], "X1: Fault");
    assert_eq!(body["message"], "The door is open");
    assert_eq!(body["level"], "bad");
    assert!(body["created_at"].is_string());

    // A Job's ending carries its Outcome.
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    client.until(&printer, |v| v.status.job.is_some()).await;
    let (status, _) = client
        .post(
            &format!("/api/printers/{printer}/commands"),
            json!({ "command": "cancel" }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let ended = receiver.wait_for(2).await.remove(1);
    assert_eq!(ended.event, "job_ended");
    assert_eq!(ended.json()["outcome"], "cancelled");
}

#[tokio::test]
async fn failed_deliveries_are_retried_with_backoff() {
    let backoff = Backoff::exponential(Duration::from_millis(100), Duration::from_secs(5), 4);
    let client = client(backoff).await;
    let receiver = receiver(3, StatusCode::SERVICE_UNAVAILABLE).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (_, secret) = webhook(
        &client,
        json!({ "name": "Flaky", "url": receiver.url, "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(&printer, raise_fault("The door is open"))
        .await;
    let deliveries = receiver.wait_for(4).await;
    // Three 503s, then the fourth try got through, and no fifth.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(receiver.deliveries().len(), 4);
    // The same event every time, each try signed afresh.
    assert!(deliveries.iter().all(|d| d.id == deliveries[0].id));
    assert!(deliveries.iter().all(|d| crate::webhooks::verify(
        &secret,
        &d.signature,
        &d.body,
        now(),
        300
    )));
    // Waits of 100, 200 and 400 ms: each gap at least its wait, and growing.
    let gaps: Vec<Duration> = deliveries
        .windows(2)
        .map(|w| w[1].at.duration_since(w[0].at))
        .collect();
    for (gap, wait) in gaps.iter().zip([100, 200, 400]) {
        assert!(*gap >= Duration::from_millis(wait), "{gaps:?}");
    }
    assert!(gaps[0] < gaps[1] && gaps[1] < gaps[2], "{gaps:?}");
}

#[tokio::test]
async fn a_receiver_that_refuses_the_request_is_not_retried_and_nothing_breaks() {
    let client = client(Backoff(vec![Duration::from_millis(20); 3])).await;
    let refusing = receiver(10, StatusCode::BAD_REQUEST).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    webhook(
        &client,
        json!({ "name": "Wrong", "url": refusing.url, "events": ["fault_raised"] }),
    )
    .await;
    // And one nobody is listening at, which is retried and then given up.
    webhook(
        &client,
        json!({ "name": "Nowhere", "url": "http://127.0.0.1:9/hook", "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(&printer, raise_fault("The door is open"))
        .await;
    refusing.wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(refusing.deliveries().len(), 1, "a 400 is not retried");
    let (status, _) = client.get("/api/printers").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn webhooks_follow_printers_tags_and_events_like_alert_rules() {
    let client = client(Backoff(vec![])).await;
    let receiver = receiver(0, StatusCode::OK).await;
    let x1 = client.connected_simulated("X1", json!({})).await;
    let p1 = client.connected_simulated("P1", json!({})).await;
    webhook(
        &client,
        json!({ "name": "X1 only", "url": receiver.url, "events": ["fault_raised"], "printers": [x1] }),
    )
    .await;
    client.simulate(&p1, raise_fault("P1's door")).await;
    client.simulate(&x1, raise_fault("X1's door")).await;
    let deliveries = receiver.wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(receiver.deliveries().len(), 1, "{deliveries:?}");
    assert_eq!(deliveries[0].json()["message"], "X1's door");

    // Disabled: nothing.
    let (_, hooks) = client.get("/api/webhooks").await;
    let id = hooks[0]["id"].as_i64().unwrap();
    let (status, _) = client
        .put(&format!("/api/webhooks/{id}"), json!({ "enabled": false }))
        .await;
    assert_eq!(status, StatusCode::OK);
    client
        .simulate(
            &x1,
            json!({ "action": "raise_fault", "code": "clog", "severity": "error", "message": "Clogged" }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(receiver.deliveries().len(), 1);
}

#[tokio::test]
async fn the_secret_is_shown_once_encrypted_at_rest_and_can_be_replaced() {
    let client = client(Backoff(vec![])).await;
    let receiver = receiver(0, StatusCode::OK).await;
    let (id, secret) = webhook(
        &client,
        json!({ "name": "Hook", "url": receiver.url, "events": ["job_ended"] }),
    )
    .await;
    let (_, listed) = client.get("/api/webhooks").await;
    assert!(!listed.to_string().contains(&secret), "{listed}");
    assert_eq!(listed[0]["url"], receiver.url.as_str());
    let raw: String = sqlx::query_scalar("SELECT secret FROM webhooks")
        .fetch_one(client.state.store().pool())
        .await
        .unwrap();
    assert!(raw.starts_with("v1:") && !raw.contains(&secret[6..]));

    // The test delivery is signed with it.
    let (_, result) = client
        .post(&format!("/api/webhooks/{id}/test"), json!({}))
        .await;
    assert_eq!(result["delivered"], true, "{result}");
    let test = receiver.wait_for(1).await.remove(0);
    assert_eq!(test.event, "test");
    assert!(crate::webhooks::verify(
        &secret,
        &test.signature,
        &test.body,
        now(),
        300
    ));

    // A new secret is shown once, and the old one stops working.
    let (_, changed) = client
        .put(
            &format!("/api/webhooks/{id}"),
            json!({ "new_secret": true }),
        )
        .await;
    let fresh = changed["secret"].as_str().unwrap();
    assert_ne!(fresh, secret);
    let (_, unchanged) = client
        .put(&format!("/api/webhooks/{id}"), json!({ "name": "Hook 2" }))
        .await;
    assert!(unchanged.get("secret").is_none());
    client
        .post(&format!("/api/webhooks/{id}/test"), json!({}))
        .await;
    let test = receiver.wait_for(2).await.remove(1);
    assert!(crate::webhooks::verify(
        fresh,
        &test.signature,
        &test.body,
        now(),
        300
    ));
    assert!(!crate::webhooks::verify(
        &secret,
        &test.signature,
        &test.body,
        now(),
        300
    ));

    let (_, audit) = client.get("/api/audit").await;
    let actions: Vec<&str> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    for action in ["webhook.created", "webhook.updated", "webhook.tested"] {
        assert!(actions.contains(&action), "{actions:?}");
    }
    assert!(!audit.to_string().contains(fresh));
}

#[tokio::test]
async fn only_admins_manage_webhooks_and_bad_input_is_refused() {
    let client = client(Backoff(vec![])).await;
    let operator = client.as_role(Role::Operator).await;
    let (status, _) = operator.get("/api/webhooks").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = operator
        .post(
            "/api/webhooks",
            json!({ "name": "x", "url": "http://localhost/x", "events": ["job_ended"] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for (body, field) in [
        (
            json!({ "name": "x", "url": "ftp://x", "events": ["job_ended"] }),
            "url",
        ),
        (
            json!({ "name": "x", "url": "http://x/y", "events": [] }),
            "events",
        ),
        (
            json!({ "name": "", "url": "http://x/y", "events": ["job_ended"] }),
            "name",
        ),
        (
            json!({ "name": "x", "url": "http://x/y", "events": ["job_ended"], "printers": ["nope"] }),
            "printers",
        ),
    ] {
        let (status, error) = client.post("/api/webhooks", body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {error}");
        assert_eq!(error["fields"][0]["field"], field, "{error}");
    }
}
