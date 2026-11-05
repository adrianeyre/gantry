//! Where Alerts go (PRD §7.4). Each kind of channel says which settings it
//! needs, as Drivers do, so the UI builds its form from them, and turns an
//! Alert into one delivery: an HTTP request (ntfy, Discord, Telegram), an
//! email over SMTP, or a message to the running desktop apps. Every kind
//! plugs in here without touching the Alert Rules.
use std::time::Duration;

use async_trait::async_trait;
use gantry_core::{FieldKind, FieldSpec};
use serde::Serialize;

use super::{Alert, DesktopAlerts, Level};
use crate::store::alerts::StoredChannel;

/// How long one delivery may take.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Why an Alert didn't get through. Never holds a credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryError {
    #[error("the channel isn't set up: {0}")]
    Setup(String),
    #[error("could not reach it: {0}")]
    Unreachable(String),
    #[error("it answered {status}: {body}")]
    Refused { status: u16, body: String },
}

/// Somewhere an Alert can be sent.
#[async_trait]
pub trait Channel: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError>;
}

/// A kind of channel, for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelKind {
    pub id: &'static str,
    pub name: &'static str,
    pub fields: Vec<FieldSpec>,
}

/// Every kind of channel Gantry can send to.
pub fn kinds() -> Vec<ChannelKind> {
    vec![
        ChannelKind {
            id: "ntfy",
            name: "ntfy",
            fields: vec![
                FieldSpec::new("server", "Server", FieldKind::Text)
                    .help("The ntfy server, such as https://ntfy.sh or your own.")
                    .default_value("https://ntfy.sh".into())
                    .required(),
                FieldSpec::new("topic", "Topic", FieldKind::Text)
                    .help("The topic to publish to. On ntfy.sh, anyone who knows it can read it, so make it hard to guess.")
                    .required(),
                FieldSpec::new("token", "Access token", FieldKind::Secret)
                    .help("For a server or topic that needs one (tk_…). Optional."),
            ],
        },
        ChannelKind {
            id: "email",
            name: "Email",
            fields: vec![
                FieldSpec::new("host", "SMTP server", FieldKind::Text)
                    .help("Your mail provider's SMTP server, such as smtp.gmail.com.")
                    .required(),
                FieldSpec::new("port", "Port", FieldKind::Number)
                    .help("587 for STARTTLS, 465 for TLS, 25 for neither.")
                    .default_value(587.into()),
                FieldSpec::new("security", "Security", FieldKind::Text)
                    .help("starttls (the usual), tls, or none (only on a network you trust).")
                    .default_value("starttls".into()),
                FieldSpec::new("username", "Username", FieldKind::Secret)
                    .help("The SMTP login, often your email address. Optional."),
                FieldSpec::new("password", "Password", FieldKind::Secret)
                    .help("The SMTP password, or an app password. Optional."),
                FieldSpec::new("from", "From", FieldKind::Text)
                    .help("The address Alerts come from, such as gantry@example.com.")
                    .required(),
                FieldSpec::new("to", "To", FieldKind::Text)
                    .help("Who gets the Alerts. Separate several addresses with commas.")
                    .required(),
            ],
        },
        ChannelKind {
            id: "telegram",
            name: "Telegram",
            fields: vec![
                FieldSpec::new("bot_token", "Bot token", FieldKind::Secret)
                    .help("From @BotFather: /newbot, then copy the token (123456:ABC…).")
                    .required(),
                FieldSpec::new("chat_id", "Chat ID", FieldKind::Text)
                    .help("The chat, group or channel to post to: a number such as 123456789, or @channelname. Send your bot a message first.")
                    .required(),
                FieldSpec::new("api_server", "Bot API server", FieldKind::Text)
                    .help("Only change this for a self-hosted Bot API server.")
                    .default_value(TELEGRAM_API.into()),
            ],
        },
        ChannelKind {
            id: "desktop",
            name: "Desktop notifications",
            // Nothing to set: it goes to every Gantry desktop app that is
            // open and logged in.
            fields: vec![],
        },
        ChannelKind {
            id: "discord",
            name: "Discord",
            fields: vec![
                FieldSpec::new("webhook_url", "Webhook URL", FieldKind::Secret)
                    .help("Server Settings → Integrations → Webhooks → New Webhook → Copy Webhook URL.")
                    .required(),
            ],
        },
    ]
}

/// The kind with this id.
pub fn kind(id: &str) -> Option<ChannelKind> {
    kinds().into_iter().find(|k| k.id == id)
}

fn client() -> Result<reqwest::Client, DeliveryError> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(format!("Gantry/{}", crate::VERSION))
        .build()
        .map_err(|e| DeliveryError::Setup(e.to_string()))
}

/// Telegram's own Bot API server.
pub const TELEGRAM_API: &str = "https://api.telegram.org";

/// A channel ready to send, from what is stored. `desktop` is where the
/// desktop channel hands its Alerts to the running desktop apps.
pub fn build(
    channel: &StoredChannel,
    desktop: &DesktopAlerts,
) -> Result<Box<dyn Channel>, DeliveryError> {
    let text = |key: &str| {
        channel
            .settings
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    match channel.kind.as_str() {
        "ntfy" => Ok(Box::new(Ntfy {
            client: client()?,
            server: text("server").unwrap_or_else(|| "https://ntfy.sh".into()),
            topic: text("topic").ok_or_else(|| DeliveryError::Setup("no topic".into()))?,
            token: channel.secrets.get("token").map(str::to_owned),
        })),
        "discord" => Ok(Box::new(Discord {
            client: client()?,
            webhook: channel
                .secrets
                .get("webhook_url")
                .filter(|u| !u.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| DeliveryError::Setup("no webhook URL".into()))?,
        })),
        "email" => Ok(Box::new(Email::from_stored(channel)?)),
        "telegram" => Ok(Box::new(Telegram {
            client: client()?,
            server: text("api_server").unwrap_or_else(|| TELEGRAM_API.into()),
            token: channel
                .secrets
                .get("bot_token")
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| DeliveryError::Setup("no bot token".into()))?,
            chat_id: text("chat_id").ok_or_else(|| DeliveryError::Setup("no chat ID".into()))?,
        })),
        "desktop" => Ok(Box::new(Desktop(desktop.clone()))),
        other => Err(DeliveryError::Setup(format!("no channel kind `{other}`"))),
    }
}

/// Checks a URL is http(s), so a channel can't be pointed at anything else.
pub fn check_url(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "Must be a URL".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Must be an http:// or https:// URL".into());
    }
    Ok(())
}

async fn answer(response: Result<reqwest::Response, reqwest::Error>) -> Result<(), DeliveryError> {
    // The error's text can hold the URL, and a Discord webhook URL is a
    // credential, so only the kind of failure is kept.
    let response = response.map_err(|e| {
        DeliveryError::Unreachable(if e.is_timeout() {
            "timed out".into()
        } else if e.is_connect() {
            "the connection failed".into()
        } else {
            "the request failed".into()
        })
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body: String = response
        .text()
        .await
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Err(DeliveryError::Refused {
        status: status.as_u16(),
        body,
    })
}

/// [ntfy](https://ntfy.sh): a JSON message published to a topic.
struct Ntfy {
    client: reqwest::Client,
    server: String,
    topic: String,
    token: Option<String>,
}

#[async_trait]
impl Channel for Ntfy {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError> {
        let (priority, tag) = match alert.level {
            Level::Good => (3, "white_check_mark"),
            Level::Info => (3, "information_source"),
            Level::Warning => (4, "warning"),
            Level::Bad => (5, "rotating_light"),
        };
        let body = serde_json::json!({
            "topic": self.topic,
            "title": alert.title,
            "message": alert.message,
            "priority": priority,
            "tags": [tag, "gantry"],
        });
        let mut request = self
            .client
            .post(self.server.trim_end_matches('/'))
            .json(&body);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        answer(request.send().await).await
    }
}

/// A Discord webhook: one embed per Alert.
struct Discord {
    client: reqwest::Client,
    webhook: String,
}

#[async_trait]
impl Channel for Discord {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError> {
        let colour = match alert.level {
            Level::Good => 0x2E7D32,
            Level::Info => 0x1E88E5,
            Level::Warning => 0xF9A825,
            Level::Bad => 0xC62828,
        };
        let body = serde_json::json!({
            "username": "Gantry",
            "embeds": [{
                "title": alert.title,
                "description": alert.message,
                "color": colour,
                "timestamp": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            }],
            // Never ping anyone from an Alert's text.
            "allowed_mentions": { "parse": [] },
        });
        answer(self.client.post(&self.webhook).json(&body).send().await).await
    }
}

/// Telegram: a message from a bot (`sendMessage` on the Bot API).
struct Telegram {
    client: reqwest::Client,
    server: String,
    /// A credential: the request URL holds it, so neither is ever logged.
    token: String,
    chat_id: String,
}

#[async_trait]
impl Channel for Telegram {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError> {
        let mark = match alert.level {
            Level::Good => "✅",
            Level::Info => "ℹ️",
            Level::Warning => "⚠️",
            Level::Bad => "🚨",
        };
        let body = serde_json::json!({
            "chat_id": self.chat_id,
            // Plain text, so nothing in a Print File's name is read as markup.
            "text": format!("{mark} {}\n{}", alert.title, alert.message),
            "disable_web_page_preview": true,
        });
        let url = format!(
            "{}/bot{}/sendMessage",
            self.server.trim_end_matches('/'),
            self.token
        );
        answer(self.client.post(url).json(&body).send().await).await
    }
}

/// How an email channel's connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Plain, then upgraded with STARTTLS, which is required, never optional.
    StartTls,
    /// TLS from the first byte (port 465).
    Tls,
    /// Neither: only for a relay on a network you trust.
    None,
}

impl Security {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "starttls" => Some(Self::StartTls),
            "tls" | "ssl" => Some(Self::Tls),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// Checks an email channel's settings beyond their kinds: the security
/// word, the port, the server's name and each address.
pub fn check_email(
    settings: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(&'static str, String)> {
    let mut errors = Vec::new();
    let text = |key: &str| settings.get(key).and_then(serde_json::Value::as_str);
    if let Some(security) = text("security")
        && Security::parse(security).is_none()
    {
        errors.push(("security", "Must be starttls, tls or none".to_owned()));
    }
    if let Some(port) = settings.get("port").and_then(serde_json::Value::as_f64)
        && !(1.0..=65535.0).contains(&port)
    {
        errors.push(("port", "Must be between 1 and 65535".to_owned()));
    }
    if let Some(host) = text("host")
        && (host.contains('/') || host.contains(' ') || host.contains(':'))
    {
        errors.push((
            "host",
            "Just the server's name, such as smtp.example.com".to_owned(),
        ));
    }
    if let Some(from) = text("from")
        && from.trim().parse::<lettre::message::Mailbox>().is_err()
    {
        errors.push(("from", "Must be an email address".to_owned()));
    }
    if let Some(to) = text("to")
        && recipients(to).is_err()
    {
        errors.push((
            "to",
            "Must be one or more email addresses, separated by commas".to_owned(),
        ));
    }
    errors
}

fn recipients(to: &str) -> Result<Vec<lettre::message::Mailbox>, ()> {
    let list: Vec<lettre::message::Mailbox> = to
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(|a| a.parse().map_err(|_| ()))
        .collect::<Result<_, _>>()?;
    if list.is_empty() {
        return Err(());
    }
    Ok(list)
}

/// Email: one plain-text message per Alert, over SMTP.
struct Email {
    host: String,
    port: u16,
    security: Security,
    /// Both credentials, or neither. Never logged.
    login: Option<(String, String)>,
    from: lettre::message::Mailbox,
    to: Vec<lettre::message::Mailbox>,
}

impl Email {
    fn from_stored(channel: &StoredChannel) -> Result<Self, DeliveryError> {
        let text = |key: &str| {
            channel
                .settings
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
        };
        let security = Security::parse(text("security").unwrap_or("starttls"))
            .ok_or_else(|| DeliveryError::Setup("an unknown security setting".into()))?;
        let port = channel
            .settings
            .get("port")
            .and_then(serde_json::Value::as_f64)
            .map_or(
                match security {
                    Security::StartTls => 587,
                    Security::Tls => 465,
                    Security::None => 25,
                },
                |p| p.clamp(1.0, 65535.0) as u16,
            );
        let secret = |key: &str| channel.secrets.get(key).filter(|v| !v.is_empty());
        let login = match (secret("username"), secret("password")) {
            (Some(user), Some(password)) => Some((user.to_owned(), password.to_owned())),
            (None, None) => None,
            _ => {
                return Err(DeliveryError::Setup(
                    "give both the SMTP username and password, or neither".into(),
                ));
            }
        };
        Ok(Self {
            host: text("host")
                .ok_or_else(|| DeliveryError::Setup("no SMTP server".into()))?
                .to_owned(),
            port,
            security,
            login,
            from: text("from")
                .and_then(|f| f.parse().ok())
                .ok_or_else(|| DeliveryError::Setup("no From address".into()))?,
            to: recipients(text("to").unwrap_or_default())
                .map_err(|()| DeliveryError::Setup("no To address".into()))?,
        })
    }

    fn transport(
        &self,
    ) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, DeliveryError> {
        use lettre::transport::smtp::client::{Tls, TlsParameters};
        type Transport = lettre::AsyncSmtpTransport<lettre::Tokio1Executor>;
        let tls = || {
            TlsParameters::new(self.host.clone())
                .map_err(|e| DeliveryError::Setup(format!("TLS: {e}")))
        };
        let builder = Transport::builder_dangerous(&self.host)
            .port(self.port)
            .timeout(Some(TIMEOUT))
            .tls(match self.security {
                Security::StartTls => Tls::Required(tls()?),
                Security::Tls => Tls::Wrapper(tls()?),
                Security::None => Tls::None,
            });
        let builder = match &self.login {
            Some((user, password)) => {
                builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                    user.clone(),
                    password.clone(),
                ))
            }
            None => builder,
        };
        Ok(builder.build())
    }
}

#[async_trait]
impl Channel for Email {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError> {
        use lettre::AsyncTransport;
        let mut message = lettre::Message::builder()
            .from(self.from.clone())
            .subject(&alert.title)
            .header(lettre::message::header::ContentType::TEXT_PLAIN);
        for to in &self.to {
            message = message.to(to.clone());
        }
        let message = message
            .body(format!(
                "{}\n\n-- \nSent by Gantry, because an Alert Rule sends this event here.\n",
                alert.message
            ))
            .map_err(|e| DeliveryError::Setup(e.to_string()))?;
        // lettre's errors never hold the password. A refusal carries the
        // server's own reply.
        self.transport()?
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| {
                let text: String = e.to_string().chars().take(200).collect();
                match e.status() {
                    Some(code) => DeliveryError::Refused {
                        status: code.to_string().parse().unwrap_or(0),
                        body: text,
                    },
                    None => DeliveryError::Unreachable(text),
                }
            })
    }
}

/// Desktop notifications: the Alert goes to every Gantry desktop app that is
/// open and logged in, over its WebSocket, and the app shows it natively.
struct Desktop(DesktopAlerts);

#[async_trait]
impl Channel for Desktop {
    async fn send(&self, alert: &Alert) -> Result<(), DeliveryError> {
        self.0.send(alert.clone()).map(|_| ()).map_err(|_| {
            DeliveryError::Unreachable("no Gantry desktop app is open and logged in".into())
        })
    }
}
