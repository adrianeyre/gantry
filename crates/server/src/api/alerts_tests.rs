//! Alert Rules end to end (#47, #48): Simulated Printers raise each event,
//! Rules pick which go where, and Alerts reach fake ntfy, Discord and
//! Telegram endpoints, a local SMTP server, and desktop apps' WebSockets, all
//! on this machine.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode, Uri};
use serde_json::{Value, json};

use crate::alerts::Retry;
use crate::store::users::Role;
use crate::test_support::{Client, state_with};

/// One request a fake endpoint received.
#[derive(Debug, Clone)]
struct Hit {
    path: String,
    authorization: Option<String>,
    body: Value,
}

/// A local stand-in for an ntfy server or a Discord webhook. It fails the
/// first `fail_first` requests with 500.
#[derive(Clone)]
struct Fake {
    url: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

async fn fake(fail_first: usize) -> Fake {
    let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
    let failures = Arc::new(AtomicUsize::new(fail_first));
    let recorded = hits.clone();
    let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: String| {
        let recorded = recorded.clone();
        let failures = failures.clone();
        async move {
            recorded.lock().unwrap().push(Hit {
                path: uri.path().to_owned(),
                authorization: headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned),
                body: serde_json::from_str(&body).unwrap_or(Value::Null),
            });
            if failures
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::OK
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    Fake { url, hits }
}

impl Fake {
    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// Waits for a request whose title (ntfy) or embed title (Discord)
    /// contains `text`.
    async fn wait_for(&self, text: &str) -> Hit {
        for _ in 0..200 {
            if let Some(hit) = self.hits().into_iter().find(|h| title(h).contains(text)) {
                return hit;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("no Alert titled `{text}` arrived: {:?}", self.hits());
    }

    /// Asserts nothing with `text` in its title arrives for a while.
    async fn never(&self, text: &str) {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let hits = self.hits();
        assert!(
            !hits.iter().any(|h| title(h).contains(text)),
            "unexpected `{text}`: {hits:?}"
        );
    }
}

fn title(hit: &Hit) -> String {
    hit.body["title"]
        .as_str()
        .or_else(|| hit.body["embeds"][0]["title"].as_str())
        .or_else(|| hit.body["text"].as_str())
        .unwrap_or_default()
        .to_owned()
}

async fn client() -> Client {
    let state = state_with(crate::Options {
        alert_retry: Some(Retry(vec![
            Duration::from_millis(50),
            Duration::from_millis(50),
        ])),
        ..Default::default()
    })
    .await;
    Client::with_state(state).await
}

async fn ntfy(client: &Client, fake: &Fake) -> i64 {
    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Phone", "kind": "ntfy",
                    "settings": { "server": fake.url, "topic": "gantry-test" },
                    "secrets": { "token": "tk_secret_token" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    channel["id"].as_i64().unwrap()
}

async fn rule(client: &Client, body: Value) -> Value {
    let (status, rule) = client.post("/api/alerts/rules", body).await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    rule
}

async fn command(client: &Client, printer: &str, command: &str) {
    let (status, body) = client
        .post(
            &format!("/api/printers/{printer}/commands"),
            json!({ "command": command }),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{command}: {body}");
}

#[tokio::test]
async fn each_event_sends_an_alert_when_a_rule_wants_it() {
    let client = client().await;
    let fake = fake(0).await;
    let printer = client
        .connected_simulated("X1", json!({ "filament_slots": 4 }))
        .await;
    let channel = ntfy(&client, &fake).await;
    rule(
        &client,
        json!({ "name": "Everything", "channel_id": channel,
                "events": ["job_ended", "job_paused", "fault_raised", "connection_lost", "low_filament"] }),
    )
    .await;

    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "door", "severity": "warning", "message": "The door is open" }),
        )
        .await;
    let hit = fake.wait_for("X1: Warning").await;
    assert_eq!(hit.path, "/");
    assert_eq!(hit.body["topic"], "gantry-test");
    assert_eq!(hit.body["message"], "The door is open");
    assert_eq!(hit.authorization.as_deref(), Some("Bearer tk_secret_token"));

    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "print_file": "cube.gcode", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    client.until(&printer, |v| v.status.job.is_some()).await;
    command(&client, &printer, "pause").await;
    let hit = fake.wait_for("X1: Job paused").await;
    assert!(hit.body["message"].as_str().unwrap().contains("cube.gcode"));

    command(&client, &printer, "cancel").await;
    fake.wait_for("X1: Job was cancelled").await;

    client
        .simulate(
            &printer,
            json!({ "action": "set_filament_slot", "slot": "1",
                    "filament": { "kind": "PETG", "colour": "#000000", "remaining_percent": 4 } }),
        )
        .await;
    let hit = fake.wait_for("X1: Low filament").await;
    assert!(
        hit.body["message"].as_str().unwrap().contains("4% left"),
        "{:?}",
        hit.body
    );

    client
        .simulate(&printer, json!({ "action": "drop_connection" }))
        .await;
    let hit = fake.wait_for("X1: Connection lost").await;
    assert!(
        hit.body["message"]
            .as_str()
            .unwrap()
            .contains("unreachable")
    );
    assert_eq!(
        fake.hits().len(),
        5,
        "one Alert per event: {:?}",
        fake.hits()
    );
}

#[tokio::test]
async fn no_rule_means_no_alert() {
    let client = client().await;
    let fake = fake(0).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let channel = ntfy(&client, &fake).await;
    // A Rule for Job endings only, and only failures.
    rule(
        &client,
        json!({ "name": "Failures", "channel_id": channel, "events": ["job_ended"], "outcomes": ["failed"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "clog", "severity": "error", "message": "Nozzle clogged" }),
        )
        .await;
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    client.until(&printer, |v| v.status.job.is_some()).await;
    command(&client, &printer, "cancel").await;
    fake.never("X1").await;

    // A failure is what it wants.
    let (status, _) = client
        .post(&format!("/api/printers/{printer}/mark-clear"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    client
        .simulate(
            &printer,
            json!({ "action": "start_print", "duration_secs": 600, "layers": 10 }),
        )
        .await;
    client.until(&printer, |v| v.status.job.is_some()).await;
    client
        .simulate(&printer, json!({ "action": "fail_print" }))
        .await;
    fake.wait_for("X1: Job failed").await;
    assert_eq!(fake.hits().len(), 1, "{:?}", fake.hits());
}

#[tokio::test]
async fn rules_target_printers_tags_or_the_whole_fleet() {
    let client = client().await;
    let fleet_fake = fake(0).await;
    let tag_fake = fake(0).await;
    let printer_fake = fake(0).await;
    let a = client.connected_simulated("Alpha", json!({})).await;
    let b = client.connected_simulated("Bravo", json!({})).await;
    let (_, garage) = client.post("/api/tags", json!({ "name": "Garage" })).await;
    client
        .put(
            &format!("/api/printers/{b}/tags"),
            json!({ "tags": [garage["id"]] }),
        )
        .await;
    let channel = |name: &str, fake: &Fake| json!({ "name": name, "kind": "ntfy", "settings": { "server": fake.url, "topic": name } });
    let mut ids = Vec::new();
    for (name, fake) in [
        ("fleet", &fleet_fake),
        ("tag", &tag_fake),
        ("printer", &printer_fake),
    ] {
        let (_, c) = client
            .post("/api/alerts/channels", channel(name, fake))
            .await;
        ids.push(c["id"].clone());
    }
    rule(
        &client,
        json!({ "name": "All", "channel_id": ids[0], "events": ["fault_raised"] }),
    )
    .await;
    rule(
        &client,
        json!({ "name": "Garage", "channel_id": ids[1], "events": ["fault_raised"], "tags": [garage["id"]] }),
    )
    .await;
    rule(
        &client,
        json!({ "name": "Alpha", "channel_id": ids[2], "events": ["fault_raised"], "printers": [a] }),
    )
    .await;

    let fault = |code: &str| json!({ "action": "raise_fault", "code": code, "severity": "error", "message": format!("{code} fault") });
    client.simulate(&a, fault("a1")).await;
    client.simulate(&b, fault("b1")).await;
    fleet_fake.wait_for("Alpha: Fault").await;
    fleet_fake.wait_for("Bravo: Fault").await;
    tag_fake.wait_for("Bravo: Fault").await;
    printer_fake.wait_for("Alpha: Fault").await;
    tag_fake.never("Alpha").await;
    printer_fake.never("Bravo").await;
}

#[tokio::test]
async fn failed_deliveries_are_retried_and_never_take_the_server_down() {
    let client = client().await;
    // Fails twice, then works: the third try gets through.
    let flaky = fake(2).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let channel = ntfy(&client, &flaky).await;
    rule(
        &client,
        json!({ "name": "Faults", "channel_id": channel, "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "x", "severity": "error", "message": "x" }),
        )
        .await;
    for _ in 0..100 {
        if flaky.hits().len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(flaky.hits().len(), 3, "{:?}", flaky.hits());

    // A channel nobody answers: tried, given up on, and the server carries on.
    let (_, dead) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Nowhere", "kind": "ntfy",
                    "settings": { "server": "http://127.0.0.1:9", "topic": "t" } }),
        )
        .await;
    rule(
        &client,
        json!({ "name": "Dead", "channel_id": dead["id"], "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "y", "severity": "error", "message": "y" }),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (status, _) = client.get("/api/printers").await;
    assert_eq!(status, StatusCode::OK);
    // The test send says what went wrong.
    let (status, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", dead["id"]),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["sent"], false);
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("could not reach"),
        "{result}"
    );
}

#[tokio::test]
async fn discord_webhooks_get_an_embed_and_the_url_stays_secret() {
    let client = client().await;
    let fake = fake(0).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let webhook = format!("{}/api/webhooks/123/very-secret-token", fake.url);
    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Workshop", "kind": "discord", "secrets": { "webhook_url": webhook } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    assert_eq!(channel["secrets_set"], json!(["webhook_url"]));
    // The same Rule logic, a different channel.
    rule(
        &client,
        json!({ "name": "Faults", "channel_id": channel["id"], "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": "The door is open" }),
        )
        .await;
    let hit = fake.wait_for("X1: Fault").await;
    assert_eq!(hit.path, "/api/webhooks/123/very-secret-token");
    assert_eq!(hit.body["username"], "Gantry");
    assert_eq!(hit.body["embeds"][0]["description"], "The door is open");
    assert_eq!(hit.body["embeds"][0]["color"], 0xC62828);

    // Never sent back, and encrypted at rest.
    let (_, channels) = client.get("/api/alerts/channels").await;
    assert!(
        !channels.to_string().contains("very-secret-token"),
        "{channels}"
    );
    let raw: String = sqlx::query_scalar("SELECT secrets FROM alert_channels")
        .fetch_one(client.state.store().pool())
        .await
        .unwrap();
    assert!(!raw.contains("very-secret-token"));

    // Editing without the URL keeps it.
    let (status, _) = client
        .put(
            &format!("/api/alerts/channels/{}", channel["id"]),
            json!({ "name": "Workshop 2" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", channel["id"]),
            json!({}),
        )
        .await;
    assert_eq!(result["sent"], true, "{result}");
}

#[tokio::test]
async fn only_admins_manage_alerts_and_bad_input_is_refused() {
    let client = client().await;
    let fake = fake(0).await;
    let channel = ntfy(&client, &fake).await;
    let operator = client.as_role(Role::Operator).await;
    for path in ["/api/alerts/rules", "/api/alerts/channels"] {
        let (status, _) = operator.get(path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
    let (status, _) = operator
        .post(
            "/api/alerts/rules",
            json!({ "name": "x", "channel_id": channel, "events": ["fault_raised"] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    for bad in [
        json!({ "name": "x", "channel_id": channel, "events": [] }),
        json!({ "name": "x", "channel_id": 999, "events": ["fault_raised"] }),
        json!({ "name": "x", "channel_id": channel, "events": ["fault_raised"], "printers": ["nope"] }),
        json!({ "name": "", "channel_id": channel, "events": ["fault_raised"] }),
    ] {
        let (status, body) = client.post("/api/alerts/rules", bad.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
    }
    let (status, _) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Bad", "kind": "ntfy", "settings": { "server": "ftp://x", "topic": "t" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "No topic", "kind": "ntfy", "settings": { "server": fake.url } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Removing a channel removes its Rules.
    rule(
        &client,
        json!({ "name": "Faults", "channel_id": channel, "events": ["fault_raised"] }),
    )
    .await;
    let (status, _) = client
        .delete(&format!("/api/alerts/channels/{channel}"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, rules) = client.get("/api/alerts/rules").await;
    assert_eq!(rules, json!([]));
    let (_, audit) = client.get("/api/audit?action=alert_rule.created").await;
    assert!(audit.to_string().contains("Faults"), "{audit}");
}

/// One email a fake SMTP server took in.
#[derive(Debug, Clone, Default)]
struct Mail {
    /// The decoded `AUTH PLAIN` credentials, if the client logged in.
    login: Option<(String, String)>,
    from: String,
    to: Vec<String>,
    data: String,
}

/// A local SMTP server that speaks just enough of RFC 5321 (and AUTH PLAIN
/// from RFC 4954) to take mail, and keeps every message.
async fn fake_smtp() -> (u16, Arc<Mutex<Vec<Mail>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mails: Arc<Mutex<Vec<Mail>>> = Arc::default();
    let kept = mails.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let kept = kept.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let mut mail = Mail::default();
                write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n"
                    } else if let Some(credentials) = line.strip_prefix("AUTH PLAIN ") {
                        use base64::Engine;
                        let raw = base64::engine::general_purpose::STANDARD
                            .decode(credentials.trim())
                            .unwrap();
                        let parts: Vec<String> = raw
                            .split(|b| *b == 0)
                            .map(|p| String::from_utf8_lossy(p).into_owned())
                            .collect();
                        mail.login = Some((parts[1].clone(), parts[2].clone()));
                        b"235 2.7.0 ok\r\n"
                    } else if upper.starts_with("MAIL FROM:") {
                        mail.from = line[10..].trim().to_owned();
                        b"250 ok\r\n"
                    } else if upper.starts_with("RCPT TO:") {
                        mail.to.push(line[8..].trim().to_owned());
                        b"250 ok\r\n"
                    } else if upper == "DATA" {
                        write.write_all(b"354 go on\r\n").await.unwrap();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if line == "." {
                                break;
                            }
                            mail.data.push_str(&line);
                            mail.data.push('\n');
                        }
                        kept.lock().unwrap().push(std::mem::take(&mut mail));
                        b"250 queued\r\n"
                    } else if upper == "QUIT" {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        b"250 ok\r\n"
                    };
                    if write.write_all(reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, mails)
}

async fn wait_for_mail(mails: &Arc<Mutex<Vec<Mail>>>, subject: &str) -> Mail {
    for _ in 0..200 {
        if let Some(mail) = mails
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.data.contains(&format!("Subject: {subject}")))
        {
            return mail.clone();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "no email with the subject `{subject}`: {:?}",
        mails.lock().unwrap()
    );
}

#[tokio::test]
async fn email_alerts_reach_a_local_smtp_server_and_the_login_is_encrypted() {
    let client = client().await;
    let (port, mails) = fake_smtp().await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Inbox", "kind": "email",
                    "settings": { "host": "127.0.0.1", "port": port, "security": "none",
                                  "from": "Gantry <gantry@example.com>",
                                  "to": "me@example.com, you@example.com" },
                    "secrets": { "username": "smtp-user", "password": "smtp-very-secret" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    assert_eq!(channel["secrets_set"], json!(["password", "username"]));
    rule(
        &client,
        json!({ "name": "Faults", "channel_id": channel["id"], "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": "The door is open" }),
        )
        .await;
    let mail = wait_for_mail(&mails, "X1: Fault").await;
    assert_eq!(
        mail.login,
        Some(("smtp-user".to_owned(), "smtp-very-secret".to_owned()))
    );
    assert_eq!(mail.from, "<gantry@example.com>");
    assert_eq!(mail.to, ["<me@example.com>", "<you@example.com>"]);
    assert!(mail.data.contains("The door is open"), "{}", mail.data);
    assert!(mail.data.contains("From: Gantry <gantry@example.com>"));

    // The credentials are never sent back, and are encrypted at rest.
    let (_, channels) = client.get("/api/alerts/channels").await;
    let listed = channels.to_string();
    assert!(!listed.contains("smtp-very-secret") && !listed.contains("smtp-user"));
    let (settings, secrets): (String, String) =
        sqlx::query_as("SELECT settings, secrets FROM alert_channels")
            .fetch_one(client.state.store().pool())
            .await
            .unwrap();
    for raw in [&settings, &secrets] {
        assert!(!raw.contains("smtp-very-secret") && !raw.contains("smtp-user"));
    }
    assert!(secrets.starts_with("v1:"), "{secrets}");

    // The test button goes the same way.
    let (_, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", channel["id"]),
            json!({}),
        )
        .await;
    assert_eq!(result["sent"], true, "{result}");
    wait_for_mail(&mails, "Gantry: test Alert").await;
}

#[tokio::test]
async fn email_settings_are_checked() {
    let client = client().await;
    let cases = [
        (
            json!({ "host": "smtp.example.com", "from": "nope", "to": "me@example.com" }),
            "from",
        ),
        (
            json!({ "host": "smtp.example.com", "from": "g@example.com", "to": "me@example.com, ?" }),
            "to",
        ),
        (
            json!({ "host": "smtp.example.com", "from": "g@example.com", "to": "me@example.com", "security": "maybe" }),
            "security",
        ),
        (
            json!({ "host": "smtp://x", "from": "g@example.com", "to": "me@example.com" }),
            "host",
        ),
        (
            json!({ "host": "smtp.example.com", "from": "g@example.com", "to": "me@example.com", "port": 70000 }),
            "port",
        ),
        (
            json!({ "from": "g@example.com", "to": "me@example.com" }),
            "host",
        ),
    ];
    for (settings, field) in cases {
        let (status, body) = client
            .post(
                "/api/alerts/channels",
                json!({ "name": "Bad", "kind": "email", "settings": settings }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{settings}: {body}"
        );
        assert!(
            body["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["field"] == field),
            "{settings}: {body}"
        );
    }
    // Only a username, without the password, is refused when sending.
    let (port, _) = fake_smtp().await;
    let (_, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Half", "kind": "email",
                    "settings": { "host": "127.0.0.1", "port": port, "security": "none",
                                  "from": "g@example.com", "to": "me@example.com" },
                    "secrets": { "username": "only-me" } }),
        )
        .await;
    let (_, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", channel["id"]),
            json!({}),
        )
        .await;
    assert_eq!(result["sent"], false, "{result}");
    assert!(
        result["error"].as_str().unwrap().contains("both"),
        "{result}"
    );
}

#[tokio::test]
async fn telegram_alerts_reach_a_fake_bot_api_and_the_token_stays_secret() {
    let client = client().await;
    let fake = fake(1).await;
    let printer = client.connected_simulated("X1", json!({})).await;
    let token = "123456:ABC-very-secret-bot-token";
    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Telegram", "kind": "telegram",
                    "settings": { "chat_id": "-1001234567890", "api_server": fake.url },
                    "secrets": { "bot_token": token } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    rule(
        &client,
        json!({ "name": "Faults", "channel_id": channel["id"], "events": ["fault_raised"] }),
    )
    .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": "The door is open" }),
        )
        .await;
    let hit = fake.wait_for("X1: Fault").await;
    assert_eq!(hit.path, format!("/bot{token}/sendMessage"));
    assert_eq!(hit.body["chat_id"], "-1001234567890");
    assert_eq!(hit.body["text"], "🚨 X1: Fault\nThe door is open");
    assert!(hit.body.get("parse_mode").is_none(), "plain text");
    // The first try failed (500), and the retry got through.
    for _ in 0..200 {
        if fake.hits().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(fake.hits().len(), 2, "{:?}", fake.hits());
    assert_eq!(fake.hits()[1].body, hit.body);

    // The token is never sent back, and is encrypted at rest.
    let (_, channels) = client.get("/api/alerts/channels").await;
    assert!(!channels.to_string().contains(token), "{channels}");
    let raw: String = sqlx::query_scalar("SELECT secrets FROM alert_channels")
        .fetch_one(client.state.store().pool())
        .await
        .unwrap();
    assert!(!raw.contains(token) && !raw.contains("very-secret"));

    // Telegram's own server is the default.
    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Default server", "kind": "telegram",
                    "settings": { "chat_id": "1" }, "secrets": { "bot_token": "1:x" } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let (status, body) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "Bad server", "kind": "telegram",
                    "settings": { "chat_id": "1", "api_server": "ftp://x" },
                    "secrets": { "bot_token": "1:x" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

/// Logs in over real HTTP to a server at `address`, returning the cookie.
async fn http_login(address: std::net::SocketAddr) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body =
        json!({ "username": "admin", "password": crate::test_support::PASSWORD }).to_string();
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

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn socket(address: std::net::SocketAddr, cookie: &str, query: &str) -> Socket {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{address}/api/ws{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

/// The next `desktop_alert` within `wait`, skipping Fleet updates.
async fn next_desktop_alert(socket: &mut Socket, wait: Duration) -> Option<Value> {
    use futures::StreamExt;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .ok()??;
        if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = message {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["type"] == "desktop_alert" {
                return Some(value);
            }
        }
    }
}

#[tokio::test]
async fn desktop_alerts_go_only_to_desktop_apps_with_the_same_rule_logic() {
    let client = client().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = crate::app(client.state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let printer = client.connected_simulated("X1", json!({})).await;
    let other = client.connected_simulated("P1", json!({})).await;

    let (status, channel) = client
        .post(
            "/api/alerts/channels",
            json!({ "name": "This computer", "kind": "desktop" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");

    // With no desktop app open, the test says so.
    let (_, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", channel["id"]),
            json!({}),
        )
        .await;
    assert_eq!(result["sent"], false, "{result}");
    assert!(
        result["error"].as_str().unwrap().contains("desktop app"),
        "{result}"
    );

    let cookie = http_login(address).await;
    let mut desktop = socket(address, &cookie, "?client=desktop").await;
    let mut browser = socket(address, &cookie, "").await;

    // An ordinary Rule, aimed at one Printer.
    rule(
        &client,
        json!({ "name": "X1 Faults", "channel_id": channel["id"], "events": ["fault_raised"],
                "printers": [printer] }),
    )
    .await;
    client
        .simulate(
            &other,
            json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": "Not this one" }),
        )
        .await;
    client
        .simulate(
            &printer,
            json!({ "action": "raise_fault", "code": "door", "severity": "error", "message": "The door is open" }),
        )
        .await;
    let message = next_desktop_alert(&mut desktop, Duration::from_secs(5))
        .await
        .expect("the desktop app is sent the Alert");
    let alert = &message["alert"];
    assert_eq!(alert["title"], "X1: Fault");
    assert_eq!(alert["message"], "The door is open");
    assert_eq!(alert["level"], "bad");
    assert_eq!(alert["event"]["kind"], "fault_raised");
    assert_eq!(alert["event"]["printer_id"], printer.as_str());
    // P1's Fault matched no Rule, so it was never sent.
    assert!(
        next_desktop_alert(&mut desktop, Duration::from_millis(600))
            .await
            .is_none()
    );
    // A browser's socket gets the Fleet, never desktop Alerts.
    assert!(
        next_desktop_alert(&mut browser, Duration::from_millis(300))
            .await
            .is_none()
    );

    let (_, result) = client
        .post(
            &format!("/api/alerts/channels/{}/test", channel["id"]),
            json!({}),
        )
        .await;
    assert_eq!(result["sent"], true, "{result}");
    let message = next_desktop_alert(&mut desktop, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(message["alert"]["title"], "Gantry: test Alert");
}

#[tokio::test]
async fn the_new_channel_kinds_are_offered() {
    let client = client().await;
    let (_, kinds) = client.get("/api/alerts/channel-kinds").await;
    let ids: Vec<&str> = kinds
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["ntfy", "email", "telegram", "desktop", "discord"]);
}
