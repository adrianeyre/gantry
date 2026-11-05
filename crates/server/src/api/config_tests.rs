//! Config export and import (PRD §5.4).
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::store::audit::Filter;
use crate::store::users::Role;
use crate::test_support::Client;

const CODE: &str = "EXPORT-ACCESS-CODE-77";
const SERIAL: &str = "00MEXPORTSERIAL";

/// A Gantry (with the Bambu Driver) with a few Printers, overrides and a
/// setting.
async fn populated() -> Client {
    populated_into(with_bambu().await).await
}

fn bambu_drivers() -> gantry_core::DriverRegistry {
    let mut drivers = crate::test_support::drivers();
    drivers.register(gantry_driver_bambu::BambuDriver);
    drivers
}

/// A client whose Gantry also has the Bambu Driver.
async fn with_bambu() -> Client {
    let store = crate::store::Store::in_memory().await.unwrap();
    let key = crate::crypto::SecretKey::from_config("a secret key for tests only").unwrap();
    let log = std::sync::Arc::new(crate::store::jobs::SqlJobLog(store.clone()));
    let state = crate::AppState::new(
        store,
        key,
        false,
        gantry_core::Fleet::new(bambu_drivers(), log),
    );
    Client::with_state(state).await
}

async fn export(client: &Client, credentials: bool) -> String {
    let response = client
        .raw_get(&format!("/api/config/export?credentials={credentials}"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("gantry-config.toml")
    );
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// What matters about the Printers, without ids: name, driver, settings,
/// overrides and which credentials are set.
async fn printers(client: &Client) -> Vec<Value> {
    let (_, list) = client.get("/api/printers").await;
    let mut out = Vec::new();
    for p in list.as_array().unwrap() {
        let id = p["id"].as_str().unwrap();
        let overrides = client.state.store().capability_overrides(id).await.unwrap();
        out.push(json!({
            "name": p["name"], "driver": p["driver"], "settings": p["settings"],
            "secrets_set": p["secrets_set"], "overrides": overrides,
        }));
    }
    out
}

#[tokio::test]
async fn export_then_import_into_an_empty_gantry_reproduces_printers_and_settings() {
    let populated_source = populated().await;
    let file = export(&populated_source, true).await;
    assert!(file.contains("WARNING: this file holds Printer credentials"));

    let target = with_bambu().await;
    let (status, plan) = target
        .post("/api/config/import", json!({ "toml": file }))
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(printers(&target).await, printers(&populated_source).await);
    assert_eq!(
        target.state.store().settings().await.unwrap(),
        populated_source.state.store().settings().await.unwrap()
    );
    // The credentials came across and decrypt with this Gantry's key.
    let (_, list) = target.get("/api/printers").await;
    let x1 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Garage X1C")
        .unwrap();
    let secrets = target
        .state
        .store()
        .printer_secrets(target.state.key(), x1["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(secrets.get("access_code"), Some(CODE));
    // And the imported Printers are running.
    let id = x1["id"].as_str().unwrap();
    assert!(target.state.fleet().contains(id));

    // Importing the same file again changes nothing.
    let (_, plan) = target
        .post("/api/config/import/preview", json!({ "toml": file }))
        .await;
    assert_eq!(plan["changes"], json!([]), "{plan}");
    assert_eq!(plan["unchanged"].as_array().unwrap().len(), 2);
}

async fn populated_into(client: Client) -> Client {
    let sim = client
        .add_simulated_with_secret("Bench Sim", "SIM-CODE-1234")
        .await;
    client
        .put(
            &format!("/api/printers/{sim}"),
            json!({ "name": "Bench Sim", "settings": { "model": "X1C", "filament_slots": 2, "camera": false } }),
        )
        .await;
    client
        .state
        .fleet()
        .wait_for(&sim, Duration::from_secs(5), |v| {
            v.status.connection.is_connected()
        })
        .await
        .unwrap();
    client
        .put(
            &format!("/api/printers/{sim}/capabilities"),
            json!({ "overrides": { "homing": false, "camera": true } }),
        )
        .await;
    let (status, body) = client
        .post(
            "/api/printers",
            json!({ "name": "Garage X1C", "driver": "bambu",
                    "settings": { "host": "192.0.2.10", "model": "X1C" },
                    "secrets": { "serial": SERIAL, "access_code": CODE } }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    client
        .state
        .store()
        .set_setting("low_filament_percent", &json!(15))
        .await
        .unwrap();
    client
}

#[tokio::test]
async fn credentials_are_left_out_unless_asked_for() {
    let source = populated().await;
    let without = export(&source, false).await;
    for secret in [CODE, SERIAL, "SIM-CODE-1234"] {
        assert!(!without.contains(secret), "{secret} leaked:\n{without}");
    }
    assert!(!without.contains("[printers.credentials]"));
    let with = export(&source, true).await;
    for secret in [CODE, SERIAL, "SIM-CODE-1234"] {
        assert!(with.contains(secret), "{secret} missing:\n{with}");
    }

    // Imported without credentials, the Printers arrive and say what's missing.
    let target = with_bambu().await;
    let (_, plan) = target
        .post("/api/config/import/preview", json!({ "toml": without }))
        .await;
    assert!(
        plan["warnings"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("Garage X1C")
            && w.as_str().unwrap().contains("serial")),
        "{plan}"
    );
    let (status, _) = target
        .post("/api/config/import", json!({ "toml": without }))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(printers(&target).await.len(), 2);
}

#[tokio::test]
async fn an_import_previews_its_changes_and_applies_them_all_or_not_at_all() {
    let client = populated().await;
    let before = printers(&client).await;
    let file = r#"
        format = 1
        [settings]
        low_filament_percent = 20

        [[printers]]
        name = "Bench Sim"
        driver = "simulated"
        [printers.settings]
        model = "P1S"
        filament_slots = 2
        camera = false

        [[printers]]
        name = "Brand new"
        driver = "simulated"
    "#;
    let (status, plan) = client
        .post(
            "/api/config/import/preview",
            json!({ "toml": file, "prune": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let kinds: Vec<&str> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "update_printer",
            "add_printer",
            "remove_printer",
            "set_setting"
        ],
        "{plan}"
    );
    let update = &plan["changes"][0];
    assert!(
        update["changed"]
            .as_array()
            .unwrap()
            .contains(&json!("model"))
    );
    assert!(
        update["changed"]
            .as_array()
            .unwrap()
            .contains(&json!("capabilities"))
    );
    // The preview wrote nothing.
    assert_eq!(printers(&client).await, before);

    // One bad Printer refuses the whole import.
    let broken = format!("{file}\n[[printers]]\nname = \"Mystery\"\ndriver = \"no-such-driver\"\n");
    let (status, body) = client
        .post(
            "/api/config/import",
            json!({ "toml": broken, "prune": true }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["plan"]["errors"][0]
            .as_str()
            .unwrap()
            .contains("no-such-driver")
    );
    assert_eq!(printers(&client).await, before, "nothing applied");
    assert_eq!(
        client.state.store().settings().await.unwrap()["low_filament_percent"],
        json!(15)
    );

    // The good one applies.
    let (status, _) = client
        .post("/api/config/import", json!({ "toml": file, "prune": true }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let after = printers(&client).await;
    let names: Vec<&str> = after.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Bench Sim", "Brand new"]);
    assert_eq!(after[0]["settings"]["model"], "P1S");
    assert_eq!(after[0]["overrides"], json!({}));
    assert_eq!(
        client.state.store().settings().await.unwrap()["low_filament_percent"],
        json!(20)
    );
}

#[tokio::test]
async fn a_file_that_isnt_an_export_is_refused_with_a_reason() {
    let client = Client::new().await;
    for (text, says) in [
        ("this is not toml = = =", "isn't a Gantry export"),
        ("format = 99", "format 99"),
        (
            "format = 1\n[[printers]]\nname = \"A\"\ndriver = \"simulated\"\n[[printers]]\nname = \"a\"\ndriver = \"simulated\"\n",
            "more than once",
        ),
    ] {
        let (status, body) = client
            .post("/api/config/import", json!({ "toml": text }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
        assert!(
            body["plan"]["errors"][0].as_str().unwrap().contains(says),
            "{body}"
        );
    }
}

#[tokio::test]
async fn admin_only_and_audited() {
    let client = populated().await;
    let operator = client.as_role(Role::Operator).await;
    assert_eq!(
        operator.raw_get("/api/config/export").await.status(),
        StatusCode::FORBIDDEN
    );
    for path in ["/api/config/import/preview", "/api/config/import"] {
        let (status, _) = operator.post(path, json!({ "toml": "format = 1" })).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }

    let file = export(&client, true).await;
    let (status, _) = client
        .post("/api/config/import", json!({ "toml": file }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let entries = client
        .state
        .store()
        .audit_entries(&Filter::default())
        .await
        .unwrap();
    let imported = entries
        .iter()
        .find(|e| e.action == "config.imported")
        .unwrap();
    let exported = entries
        .iter()
        .find(|e| e.action == "config.exported")
        .unwrap();
    assert_eq!(exported.detail, Some(json!({ "credentials": true })));
    // Neither entry holds a credential, even though the file did.
    let text = format!("{:?}{:?}", imported.detail, exported.detail);
    assert!(!text.contains("SIM-CODE-1234"), "{text}");
}

#[tokio::test]
async fn tags_round_trip_through_export_and_import() {
    let source = Client::new().await;
    let x1 = source.add_simulated("X1", json!({})).await;
    source.add_simulated("P1", json!({})).await;
    let (_, garage) = source.post("/api/tags", json!({ "name": "Garage" })).await;
    let (_, pla) = source.post("/api/tags", json!({ "name": "PLA" })).await;
    // A Tag on no Printer still travels.
    source.post("/api/tags", json!({ "name": "Spare" })).await;
    source
        .put(
            &format!("/api/printers/{x1}/tags"),
            json!({ "tags": [garage["id"], pla["id"]] }),
        )
        .await;
    let (status, file) = source.get("/api/config/export").await;
    assert_eq!(status, StatusCode::OK);
    let file = file.as_str().unwrap().to_owned();
    assert!(file.contains("tags = ["), "{file}");

    let target = Client::new().await;
    let (_, plan) = target
        .post("/api/config/import/preview", json!({ "toml": file }))
        .await;
    let added: Vec<&Value> = plan["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "add_tag")
        .collect();
    assert_eq!(added.len(), 3, "{plan}");
    let (status, plan) = target
        .post("/api/config/import", json!({ "toml": file }))
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");

    let names = |tags: &Value| -> Vec<String> {
        tags.as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (_, source_tags) = source.get("/api/tags").await;
    let (_, target_tags) = target.get("/api/tags").await;
    assert_eq!(names(&target_tags), names(&source_tags));
    let (_, list) = target.get("/api/printers").await;
    let target_x1 = list
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "X1")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        target.state.store().printer_tags(&target_x1).await.unwrap(),
        ["Garage", "PLA"]
    );

    // The same file again changes nothing; one with a Tag moved changes it.
    let (_, plan) = target
        .post("/api/config/import/preview", json!({ "toml": file }))
        .await;
    assert_eq!(plan["changes"], json!([]), "{plan}");
    let moved = file.replace("\"PLA\"", "\"Spare\"");
    let (_, plan) = target
        .post("/api/config/import/preview", json!({ "toml": moved }))
        .await;
    assert!(plan["changes"].to_string().contains("tags"), "{plan}");
}
