//! `GET /api/discovery` (PRD §7.6): Printers found on the LAN that aren't in
//! the Fleet yet, for an Admin to add.
use axum::Json;
use axum::extract::State;
use serde::Serialize;

use super::ApiError;
use crate::AppState;
use crate::discovery::{Found, already_added, known};

#[derive(Debug, Serialize)]
pub struct Discovery {
    /// Discovery is on (an Admin can turn it off under Settings).
    pub enabled: bool,
    pub printers: Vec<Found>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Discovery>, ApiError> {
    let enabled = crate::api::settings::current(&state)
        .await?
        .discovery_enabled;
    if !enabled {
        return Ok(Json(Discovery {
            enabled,
            printers: Vec::new(),
        }));
    }
    let known = known(&state).await?;
    let drivers: Vec<&str> = state
        .fleet()
        .registry()
        .list()
        .iter()
        .map(|d| d.id)
        .collect();
    let printers = state
        .discovered()
        .list()
        .into_iter()
        // Only what this Gantry can add, and isn't in the Fleet already.
        .filter(|f| drivers.contains(&f.driver.as_str()) && !already_added(f, &known))
        .collect();
    Ok(Json(Discovery { enabled, printers }))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::discovery::{from_mdns, from_ssdp};
    use crate::store::printers::PrinterInput;
    use crate::store::users::Role;
    use crate::test_support::Client;

    const NOTIFY: &str = "NOTIFY * HTTP/1.1\r\nLocation: 192.168.1.50\r\n\
        NT: urn:bambulab-com:device:3dprinter:1\r\nUSN: 01P00A123456789\r\n\
        DevModel.bambu.com: C12\r\nDevName.bambu.com: Workshop P1S\r\n\r\n";

    fn octoprint(host: [u8; 4], port: u16) -> crate::discovery::Found {
        from_mdns(
            "_octoprint._tcp.local.",
            "Ender._octoprint._tcp.local.",
            &[host.into()],
            port,
            &Default::default(),
        )
        .unwrap()
    }

    async fn add(
        client: &Client,
        id: &str,
        driver: &str,
        settings: serde_json::Value,
        serial: Option<&str>,
    ) {
        let secrets = serial
            .map(|s| [("serial".to_owned(), s.to_owned())].into())
            .unwrap_or_default();
        client
            .state
            .store()
            .insert_printer(
                client.state.key(),
                id,
                &PrinterInput {
                    name: id.into(),
                    driver: driver.into(),
                    settings: settings.as_object().unwrap().clone(),
                    secrets: gantry_core::Secrets::new(secrets),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn found_printers_are_offered_filled_in_until_they_are_added() {
        let client = crate::test_support::Client::with_state(
            crate::test_support::state_with_drivers(&["bambu", "octoprint"]).await,
        )
        .await;
        client
            .state
            .discovered()
            .record(from_ssdp(NOTIFY, None).unwrap());
        client
            .state
            .discovered()
            .record(octoprint([192, 168, 1, 60], 5000));
        client
            .state
            .discovered()
            .record(octoprint([192, 168, 1, 61], 80));

        let (status, found) = client.get("/api/discovery").await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(found["enabled"], true);
        let printers = found["printers"].as_array().unwrap();
        assert_eq!(printers.len(), 3, "{found}");
        let bambu = printers.iter().find(|p| p["driver"] == "bambu").unwrap();
        assert_eq!(bambu["name"], "Workshop P1S");
        assert_eq!(
            bambu["settings"],
            json!({ "host": "192.168.1.50", "model": "P1S" })
        );
        assert_eq!(bambu["secrets"], json!({ "serial": "01P00A123456789" }));
        assert!(!found.to_string().contains("access_code"));

        // Added (the Bambu one by its serial, though at another address):
        // no longer offered.
        add(
            &client,
            "b",
            "bambu",
            json!({ "host": "192.168.1.99" }),
            Some("01P00A123456789"),
        )
        .await;
        add(
            &client,
            "o",
            "octoprint",
            json!({ "host": "192.168.1.60", "port": 5000 }),
            None,
        )
        .await;
        let (_, found) = client.get("/api/discovery").await;
        let printers = found["printers"].as_array().unwrap();
        assert_eq!(printers.len(), 1, "{found}");
        assert_eq!(printers[0]["host"], "192.168.1.61");
    }

    #[tokio::test]
    async fn a_driver_that_isnt_compiled_in_isnt_offered() {
        let client = Client::new().await;
        client
            .state
            .discovered()
            .record(from_ssdp(NOTIFY, None).unwrap());
        let (_, found) = client.get("/api/discovery").await;
        assert_eq!(found["printers"], json!([]));
    }

    #[tokio::test]
    async fn discovery_can_be_turned_off_and_only_admins_see_it() {
        let client = crate::test_support::Client::with_state(
            crate::test_support::state_with_drivers(&["bambu"]).await,
        )
        .await;
        client
            .state
            .discovered()
            .record(from_ssdp(NOTIFY, None).unwrap());
        let operator = client.as_role(Role::Operator).await;
        let (status, _) = operator.get("/api/discovery").await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, settings) = client
            .put("/api/settings", json!({ "discovery_enabled": false }))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(settings["discovery_enabled"], false);
        let (_, found) = client.get("/api/discovery").await;
        assert_eq!(found, json!({ "enabled": false, "printers": [] }));
        // What was found is forgotten, not just hidden.
        assert!(client.state.discovered().list().is_empty());

        client
            .put("/api/settings", json!({ "discovery_enabled": true }))
            .await;
        let (_, found) = client.get("/api/discovery").await;
        assert_eq!(found["enabled"], true);
    }
}
