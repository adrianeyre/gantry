//! The audit log (PRD §9.2): every state-changing action, never a read.
use serde::{Deserialize, Serialize};

use super::{Store, now, rfc3339};

/// What was done. Stored as its `as_str` name, so names never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    #[serde(rename = "login.succeeded")]
    LoginSucceeded,
    #[serde(rename = "login.failed")]
    LoginFailed,
    #[serde(rename = "account.locked")]
    AccountLocked,
    #[serde(rename = "account.unlocked")]
    AccountUnlocked,
    /// Pause, resume, Cancel or a fine control, alone or as part of a bulk action.
    #[serde(rename = "printer.command")]
    PrinterCommand,
    #[serde(rename = "printer.mark_clear")]
    MarkClear,
    #[serde(rename = "printer.created")]
    PrinterCreated,
    #[serde(rename = "printer.updated")]
    PrinterUpdated,
    #[serde(rename = "printer.removed")]
    PrinterRemoved,
    #[serde(rename = "printer.capabilities")]
    CapabilitiesOverridden,
    #[serde(rename = "printer.simulation")]
    Simulation,
    /// Auto-start or Auto-Clear turned on or off.
    #[serde(rename = "printer.automation")]
    PrinterAutomation,
    #[serde(rename = "user.created")]
    UserCreated,
    #[serde(rename = "user.updated")]
    UserUpdated,
    #[serde(rename = "user.removed")]
    UserRemoved,
    #[serde(rename = "config.exported")]
    ConfigExported,
    #[serde(rename = "config.imported")]
    ConfigImported,
    #[serde(rename = "print_file.uploaded")]
    PrintFileUploaded,
    #[serde(rename = "print_file.updated")]
    PrintFileUpdated,
    #[serde(rename = "print_file.removed")]
    PrintFileRemoved,
    #[serde(rename = "job.started")]
    JobStarted,
    #[serde(rename = "queue.added")]
    QueueAdded,
    #[serde(rename = "queue.reordered")]
    QueueReordered,
    #[serde(rename = "queue.removed")]
    QueueRemoved,
    #[serde(rename = "tag.created")]
    TagCreated,
    #[serde(rename = "tag.removed")]
    TagRemoved,
    #[serde(rename = "printer.tags")]
    PrinterTagged,
    #[serde(rename = "settings.updated")]
    SettingsUpdated,
    #[serde(rename = "alert_channel.created")]
    AlertChannelCreated,
    #[serde(rename = "alert_channel.updated")]
    AlertChannelUpdated,
    #[serde(rename = "alert_channel.removed")]
    AlertChannelRemoved,
    #[serde(rename = "alert_channel.tested")]
    AlertChannelTested,
    #[serde(rename = "alert_rule.created")]
    AlertRuleCreated,
    #[serde(rename = "alert_rule.updated")]
    AlertRuleUpdated,
    #[serde(rename = "alert_rule.removed")]
    AlertRuleRemoved,
    #[serde(rename = "spool.created")]
    SpoolCreated,
    #[serde(rename = "spool.updated")]
    SpoolUpdated,
    #[serde(rename = "spool.removed")]
    SpoolRemoved,
    #[serde(rename = "api_token.created")]
    ApiTokenCreated,
    #[serde(rename = "api_token.revoked")]
    ApiTokenRevoked,
    #[serde(rename = "webhook.created")]
    WebhookCreated,
    #[serde(rename = "webhook.updated")]
    WebhookUpdated,
    #[serde(rename = "webhook.removed")]
    WebhookRemoved,
    #[serde(rename = "webhook.tested")]
    WebhookTested,
    #[serde(rename = "totp.enabled")]
    TotpEnabled,
    #[serde(rename = "totp.disabled")]
    TotpDisabled,
    #[serde(rename = "totp.recovery_code_used")]
    RecoveryCodeUsed,
    #[serde(rename = "passkey.added")]
    PasskeyAdded,
    #[serde(rename = "passkey.removed")]
    PasskeyRemoved,
    #[serde(rename = "printer.timelapse")]
    PrinterTimelapse,
    #[serde(rename = "timelapse.removed")]
    TimelapseRemoved,
}

impl Action {
    pub const ALL: [Action; 51] = [
        Action::LoginSucceeded,
        Action::LoginFailed,
        Action::AccountLocked,
        Action::AccountUnlocked,
        Action::PrinterCommand,
        Action::MarkClear,
        Action::PrinterCreated,
        Action::PrinterUpdated,
        Action::PrinterRemoved,
        Action::CapabilitiesOverridden,
        Action::Simulation,
        Action::PrinterAutomation,
        Action::UserCreated,
        Action::UserUpdated,
        Action::UserRemoved,
        Action::ConfigExported,
        Action::ConfigImported,
        Action::PrintFileUploaded,
        Action::PrintFileUpdated,
        Action::PrintFileRemoved,
        Action::JobStarted,
        Action::QueueAdded,
        Action::QueueReordered,
        Action::QueueRemoved,
        Action::TagCreated,
        Action::TagRemoved,
        Action::PrinterTagged,
        Action::SettingsUpdated,
        Action::AlertChannelCreated,
        Action::AlertChannelUpdated,
        Action::AlertChannelRemoved,
        Action::AlertChannelTested,
        Action::AlertRuleCreated,
        Action::AlertRuleUpdated,
        Action::AlertRuleRemoved,
        Action::SpoolCreated,
        Action::SpoolUpdated,
        Action::SpoolRemoved,
        Action::ApiTokenCreated,
        Action::ApiTokenRevoked,
        Action::WebhookCreated,
        Action::WebhookUpdated,
        Action::WebhookRemoved,
        Action::WebhookTested,
        Action::TotpEnabled,
        Action::TotpDisabled,
        Action::RecoveryCodeUsed,
        Action::PasskeyAdded,
        Action::PasskeyRemoved,
        Action::PrinterTimelapse,
        Action::TimelapseRemoved,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login.succeeded",
            Self::LoginFailed => "login.failed",
            Self::AccountLocked => "account.locked",
            Self::AccountUnlocked => "account.unlocked",
            Self::PrinterCommand => "printer.command",
            Self::MarkClear => "printer.mark_clear",
            Self::PrinterCreated => "printer.created",
            Self::PrinterUpdated => "printer.updated",
            Self::PrinterRemoved => "printer.removed",
            Self::CapabilitiesOverridden => "printer.capabilities",
            Self::Simulation => "printer.simulation",
            Self::PrinterAutomation => "printer.automation",
            Self::UserCreated => "user.created",
            Self::UserUpdated => "user.updated",
            Self::UserRemoved => "user.removed",
            Self::ConfigExported => "config.exported",
            Self::ConfigImported => "config.imported",
            Self::PrintFileUploaded => "print_file.uploaded",
            Self::PrintFileUpdated => "print_file.updated",
            Self::PrintFileRemoved => "print_file.removed",
            Self::JobStarted => "job.started",
            Self::QueueAdded => "queue.added",
            Self::QueueReordered => "queue.reordered",
            Self::QueueRemoved => "queue.removed",
            Self::TagCreated => "tag.created",
            Self::TagRemoved => "tag.removed",
            Self::PrinterTagged => "printer.tags",
            Self::SettingsUpdated => "settings.updated",
            Self::AlertChannelCreated => "alert_channel.created",
            Self::AlertChannelUpdated => "alert_channel.updated",
            Self::AlertChannelRemoved => "alert_channel.removed",
            Self::AlertChannelTested => "alert_channel.tested",
            Self::AlertRuleCreated => "alert_rule.created",
            Self::AlertRuleUpdated => "alert_rule.updated",
            Self::AlertRuleRemoved => "alert_rule.removed",
            Self::SpoolCreated => "spool.created",
            Self::SpoolUpdated => "spool.updated",
            Self::SpoolRemoved => "spool.removed",
            Self::ApiTokenCreated => "api_token.created",
            Self::ApiTokenRevoked => "api_token.revoked",
            Self::WebhookCreated => "webhook.created",
            Self::WebhookUpdated => "webhook.updated",
            Self::WebhookRemoved => "webhook.removed",
            Self::WebhookTested => "webhook.tested",
            Self::TotpEnabled => "totp.enabled",
            Self::TotpDisabled => "totp.disabled",
            Self::RecoveryCodeUsed => "totp.recovery_code_used",
            Self::PasskeyAdded => "passkey.added",
            Self::PasskeyRemoved => "passkey.removed",
            Self::PrinterTimelapse => "printer.timelapse",
            Self::TimelapseRemoved => "timelapse.removed",
        }
    }
}

/// How an action turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    /// Gantry or the Printer said no: a role, a state, a disconnection.
    Refused,
    /// It was tried and went wrong: a timeout, a database error.
    Failed,
    /// One part of a bulk action that didn't apply, such as pausing a
    /// Printer that isn't printing.
    Skipped,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "ok" => Self::Ok,
            "refused" => Self::Refused,
            "skipped" => Self::Skipped,
            _ => Self::Failed,
        }
    }
}

/// One entry to write. Build it with [`Entry::new`] and the setters.
#[derive(Debug, Clone)]
pub struct Entry {
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub action: Action,
    pub printer_id: Option<String>,
    pub printer_name: Option<String>,
    pub target: Option<String>,
    /// Never a credential or password.
    pub detail: Option<serde_json::Value>,
    pub outcome: Outcome,
    pub message: Option<String>,
    /// The API token the action was made with, by name, if it was.
    pub api_token: Option<String>,
}

impl Entry {
    pub fn new(action: Action) -> Self {
        Self {
            user_id: None,
            username: None,
            action,
            printer_id: None,
            printer_name: None,
            target: None,
            detail: None,
            outcome: Outcome::Ok,
            message: None,
            api_token: None,
        }
    }
}

/// One entry, as read back.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub id: i64,
    pub at: String,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub action: String,
    pub printer_id: Option<String>,
    pub printer_name: Option<String>,
    pub target: Option<String>,
    pub detail: Option<serde_json::Value>,
    pub result: Outcome,
    pub message: Option<String>,
    /// The API token used, by name, when the action came through the API
    /// with one.
    pub api_token: Option<String>,
}

/// Which entries to read. Every filter is optional.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filter {
    /// The user's name, exactly (ignoring case).
    #[serde(default)]
    pub user: Option<String>,
    /// The Printer's id.
    #[serde(default)]
    pub printer: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    /// Only entries older than this id, for paging.
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

type Row = (
    i64,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

impl Store {
    /// Writes one entry.
    pub async fn audit(&self, entry: &Entry) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO audit_log (at, user_id, username, action, printer_id, printer_name, \
             target, detail, result, message, api_token) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(rfc3339(now()))
        .bind(entry.user_id)
        .bind(&entry.username)
        .bind(entry.action.as_str())
        .bind(&entry.printer_id)
        .bind(&entry.printer_name)
        .bind(&entry.target)
        .bind(entry.detail.as_ref().map(serde_json::Value::to_string))
        .bind(entry.outcome.as_str())
        .bind(&entry.message)
        .bind(&entry.api_token)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Entries matching `filter`, newest first (at most 500 at a time).
    pub async fn audit_entries(&self, filter: &Filter) -> Result<Vec<Record>, sqlx::Error> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, at, user_id, username, action, printer_id, printer_name, target, detail, \
             result, message, api_token FROM audit_log \
             WHERE (?1 IS NULL OR username = ?1 COLLATE NOCASE) \
               AND (?2 IS NULL OR printer_id = ?2) \
               AND (?3 IS NULL OR action = ?3) \
               AND (?4 IS NULL OR id < ?4) \
             ORDER BY id DESC LIMIT ?5",
        )
        .bind(filter.user.as_deref().filter(|s| !s.is_empty()))
        .bind(filter.printer.as_deref().filter(|s| !s.is_empty()))
        .bind(filter.action.as_deref().filter(|s| !s.is_empty()))
        .bind(filter.before)
        .bind(filter.limit.unwrap_or(100).clamp(1, 500))
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    at,
                    user_id,
                    username,
                    action,
                    printer_id,
                    printer_name,
                    target,
                    detail,
                    result,
                    message,
                    api_token,
                )| Record {
                    id,
                    at,
                    user_id,
                    username,
                    action,
                    printer_id,
                    printer_name,
                    target,
                    detail: detail.and_then(|d| serde_json::from_str(&d).ok()),
                    result: Outcome::parse(&result),
                    message,
                    api_token,
                },
            )
            .collect())
    }

    /// How many entries there are, for tests and the UI.
    pub async fn audit_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(self.pool())
            .await
    }

    /// The names that appear in the log, and the Printers, for filtering.
    pub async fn audit_facets(&self) -> Result<(Vec<String>, Vec<(String, String)>), sqlx::Error> {
        let users: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT username FROM audit_log WHERE username IS NOT NULL \
             ORDER BY username COLLATE NOCASE",
        )
        .fetch_all(self.pool())
        .await?;
        let printers: Vec<(String, String)> = sqlx::query_as(
            "SELECT printer_id, MAX(printer_name) FROM audit_log WHERE printer_id IS NOT NULL \
             GROUP BY printer_id ORDER BY MAX(printer_name) COLLATE NOCASE",
        )
        .fetch_all(self.pool())
        .await?;
        Ok((users, printers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_and_filters_entries() {
        let store = Store::in_memory().await.unwrap();
        let mut entry = Entry::new(Action::PrinterCommand);
        entry.username = Some("adrian".into());
        entry.printer_id = Some("p1".into());
        entry.printer_name = Some("Bench".into());
        entry.detail = Some(serde_json::json!({ "command": "pause" }));
        store.audit(&entry).await.unwrap();
        let mut other = Entry::new(Action::LoginFailed);
        other.username = Some("sam".into());
        other.outcome = Outcome::Refused;
        store.audit(&other).await.unwrap();

        let all = store.audit_entries(&Filter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].action, "login.failed", "newest first");

        let by_user = Filter {
            user: Some("ADRIAN".into()),
            ..Filter::default()
        };
        let found = store.audit_entries(&by_user).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].detail,
            Some(serde_json::json!({ "command": "pause" }))
        );

        let by_printer = Filter {
            printer: Some("p1".into()),
            ..Filter::default()
        };
        assert_eq!(store.audit_entries(&by_printer).await.unwrap().len(), 1);
        let by_action = Filter {
            action: Some("login.failed".into()),
            ..Filter::default()
        };
        let found = store.audit_entries(&by_action).await.unwrap();
        assert_eq!(found[0].result, Outcome::Refused);

        let (users, printers) = store.audit_facets().await.unwrap();
        assert_eq!(users, ["adrian", "sam"]);
        assert_eq!(printers, [("p1".to_owned(), "Bench".to_owned())]);
    }

    #[test]
    fn action_names_match_their_serde_names() {
        for action in Action::ALL {
            assert_eq!(
                serde_json::to_value(action).unwrap(),
                serde_json::Value::String(action.as_str().into())
            );
        }
    }
}
