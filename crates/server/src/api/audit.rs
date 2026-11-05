//! Browsing the audit log, for Admins (PRD §9.2).
use axum::Json;
use axum::extract::{Query, State};
use serde::Serialize;

use super::ApiError;
use crate::AppState;
use crate::store::audit::{Action, Filter, Record};

/// A page of the log, with what it can be filtered by.
#[derive(Debug, Serialize)]
pub struct AuditPage {
    pub entries: Vec<Record>,
    /// Every username in the log.
    pub users: Vec<String>,
    /// Every Printer in the log, as `{ id, name }`.
    pub printers: Vec<PrinterName>,
    /// Every action Gantry records.
    pub actions: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct PrinterName {
    pub id: String,
    pub name: String,
}

/// `GET /api/audit?user=&printer=&action=&before=&limit=`.
pub async fn list(
    State(state): State<AppState>,
    Query(filter): Query<Filter>,
) -> Result<Json<AuditPage>, ApiError> {
    let entries = state.store().audit_entries(&filter).await?;
    let (users, printers) = state.store().audit_facets().await?;
    Ok(Json(AuditPage {
        entries,
        users,
        printers: printers
            .into_iter()
            .map(|(id, name)| PrinterName { id, name })
            .collect(),
        actions: Action::ALL.iter().map(|a| a.as_str()).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::store::audit::Filter;
    use crate::store::users::Role;
    use crate::test_support::{Client, PASSWORD};

    async fn count(client: &Client) -> i64 {
        client.state.store().audit_count().await.unwrap()
    }

    async fn newest(client: &Client) -> crate::store::audit::Record {
        client
            .state
            .store()
            .audit_entries(&Filter::default())
            .await
            .unwrap()
            .remove(0)
    }

    /// Runs `act`, then checks it wrote exactly one entry, for `action`.
    async fn exactly_one<F, Fut>(
        client: &Client,
        action: &str,
        act: F,
    ) -> crate::store::audit::Record
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let before = count(client).await;
        act().await;
        assert_eq!(count(client).await, before + 1, "{action} writes one entry");
        let entry = newest(client).await;
        assert_eq!(entry.action, action);
        entry
    }

    async fn connected(client: &Client, id: &str) {
        client
            .state
            .fleet()
            .wait_for(id, Duration::from_secs(5), |v| {
                v.status.connection.is_connected()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn every_listed_action_writes_exactly_one_entry() {
        let client = Client::new().await;
        let anonymous = crate::test_support::anonymous(&client.state);

        // Logins, successful and failed.
        let entry = exactly_one(&client, "login.succeeded", || async {
            let _ = client.as_role(Role::Admin).await;
        })
        .await;
        assert_eq!(entry.username.as_deref(), Some("admin"));
        let entry = exactly_one(&client, "login.failed", || async {
            let (status, _) = anonymous
                .post(
                    "/api/login",
                    json!({ "username": "admin", "password": "wrong!" }),
                )
                .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        })
        .await;
        assert_eq!(entry.result, crate::store::audit::Outcome::Refused);

        // Admin changes to Printers.
        let mut id = String::new();
        let entry = exactly_one(&client, "printer.created", || async {
            id = client.add_simulated("Audited", json!({})).await;
        })
        .await;
        assert_eq!(entry.printer_name.as_deref(), Some("Audited"));
        assert_eq!(entry.username.as_deref(), Some("admin"));
        connected(&client, &id).await;
        exactly_one(&client, "printer.updated", || async {
            let (status, body) = client
                .put(
                    &format!("/api/printers/{id}"),
                    json!({ "name": "Audited", "settings": {} }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        })
        .await;
        connected(&client, &id).await;

        // The simulation panel, and Printer commands (done or refused).
        exactly_one(&client, "printer.simulation", || async {
            let (status, _) = client
                .post(
                    &format!("/api/printers/{id}/simulation"),
                    json!({ "action": "start_print", "duration_secs": 6000, "layers": 3 }),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        })
        .await;
        client
            .state
            .fleet()
            .wait_for(&id, Duration::from_secs(5), |v| v.job.is_some())
            .await
            .unwrap();
        let entry = exactly_one(&client, "printer.command", || async {
            let (status, _) = client
                .post(
                    &format!("/api/printers/{id}/commands"),
                    json!({ "command": "pause" }),
                )
                .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        })
        .await;
        assert_eq!(entry.detail, Some(json!({ "command": "pause" })));
        assert_eq!(entry.result, crate::store::audit::Outcome::Ok);
        let entry = exactly_one(&client, "printer.command", || async {
            let (status, _) = client
                .post(
                    &format!("/api/printers/{id}/commands"),
                    json!({ "command": "pause" }),
                )
                .await;
            assert_eq!(status, StatusCode::CONFLICT);
        })
        .await;
        assert_eq!(entry.result, crate::store::audit::Outcome::Refused);
        assert!(entry.message.unwrap().contains("already paused"));

        // Mark Clear, with the overrule recorded.
        client
            .post(
                &format!("/api/printers/{id}/commands"),
                json!({ "command": "cancel" }),
            )
            .await;
        client
            .state
            .fleet()
            .wait_for(&id, Duration::from_secs(5), |v| v.awaiting_clear.is_some())
            .await
            .unwrap();
        let entry = exactly_one(&client, "printer.mark_clear", || async {
            let (status, _) = client
                .post(
                    &format!("/api/printers/{id}/mark-clear"),
                    json!({ "overrule_to_failed": false }),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
        })
        .await;
        assert_eq!(entry.detail.unwrap()["overrule_to_failed"], false);

        // Users.
        let mut user_id = 0;
        let entry = exactly_one(&client, "user.created", || async {
            let (status, body) = client
                .post(
                    "/api/users",
                    json!({ "username": "sam", "password": "a long password", "role": "viewer" }),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED);
            user_id = body["id"].as_i64().unwrap();
        })
        .await;
        assert_eq!(entry.target.as_deref(), Some("sam"));
        let entry = exactly_one(&client, "user.updated", || async {
            client
                .put(
                    &format!("/api/users/{user_id}"),
                    json!({ "role": "operator", "password": "a new long password" }),
                )
                .await;
        })
        .await;
        assert_eq!(entry.detail.unwrap()["password_changed"], true);
        exactly_one(&client, "user.removed", || async {
            client.delete(&format!("/api/users/{user_id}")).await;
        })
        .await;

        // And removing the Printer, which keeps its name in the log.
        let entry = exactly_one(&client, "printer.removed", || async {
            client.delete(&format!("/api/printers/{id}")).await;
        })
        .await;
        assert_eq!(entry.printer_name.as_deref(), Some("Audited"));
    }

    #[tokio::test]
    async fn reads_write_nothing() {
        let client = Client::new().await;
        let id = client.add_simulated("Read only", json!({})).await;
        connected(&client, &id).await;
        let before = count(&client).await;
        for path in [
            "/api/session".to_owned(),
            "/api/drivers".to_owned(),
            "/api/printers".to_owned(),
            format!("/api/printers/{id}"),
            format!("/api/printers/{id}/jobs"),
            format!("/api/printers/{id}/simulation"),
            "/api/users".to_owned(),
            "/api/audit".to_owned(),
        ] {
            let (status, body) = client.get(&path).await;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
        }
        assert_eq!(count(&client).await, before);
    }

    #[tokio::test]
    async fn no_credential_or_password_appears_in_the_log() {
        let client = Client::new().await;
        let id = client
            .add_simulated_with_secret("Secretive", "ACCESS-CODE-9911")
            .await;
        client
            .put(
                &format!("/api/printers/{id}"),
                json!({ "name": "Secretive", "settings": { "access_code": "ACCESS-CODE-9911" },
                        "secrets": { "access_code": "ACCESS-CODE-7722" } }),
            )
            .await;
        let (_, user) = client
            .post(
                "/api/users",
                json!({ "username": "sam", "password": "PASSWORD-sam-123", "role": "viewer" }),
            )
            .await;
        client
            .put(
                &format!("/api/users/{}", user["id"]),
                json!({ "password": "PASSWORD-sam-456" }),
            )
            .await;
        let anonymous = crate::test_support::anonymous(&client.state);
        anonymous
            .post(
                "/api/login",
                json!({ "username": "sam", "password": "PASSWORD-wrong-789" }),
            )
            .await;

        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT COALESCE(username,'') || COALESCE(target,'') || COALESCE(detail,'') || \
             COALESCE(message,'') || COALESCE(printer_name,'') FROM audit_log",
        )
        .fetch_all(client.state.store().pool())
        .await
        .unwrap();
        let all: String = rows.into_iter().map(|r| r.0).collect();
        assert!(!all.is_empty());
        for secret in [
            "ACCESS-CODE",
            "PASSWORD-",
            PASSWORD,
            "a secret key for tests only",
        ] {
            assert!(!all.contains(secret), "{secret} leaked into: {all}");
        }
    }

    #[tokio::test]
    async fn admins_filter_by_user_printer_and_action() {
        let client = Client::new().await;
        let id = client.add_simulated("Filtered", json!({})).await;
        connected(&client, &id).await;
        let operator = client.as_role(Role::Operator).await;
        operator
            .post(
                &format!("/api/printers/{id}/commands"),
                json!({ "command": "pause" }),
            )
            .await;

        let (status, page) = client.get("/api/audit?user=operator").await;
        assert_eq!(status, StatusCode::OK);
        let actions: Vec<&str> = page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["printer.command", "login.succeeded"]);

        let (_, page) = client.get(&format!("/api/audit?printer={id}")).await;
        assert!(
            page["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["printer_id"] == id.as_str())
        );
        assert_eq!(page["entries"].as_array().unwrap().len(), 2);

        let (_, page) = client.get("/api/audit?action=printer.created").await;
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        assert_eq!(page["printers"][0]["name"], "Filtered");
        assert!(
            page["users"]
                .as_array()
                .unwrap()
                .contains(&Value::from("operator"))
        );
        assert!(
            page["actions"]
                .as_array()
                .unwrap()
                .contains(&Value::from("login.failed"))
        );

        // Only Admins.
        assert_eq!(operator.get("/api/audit").await.0, StatusCode::FORBIDDEN);
    }
}
