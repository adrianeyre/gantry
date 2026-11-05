//! The Print File library and sending a Plate to a Printer, through the API,
//! with Simulated Printers (#39, #40, #41, #44).
use axum::http::StatusCode;
use gantry_core::PrinterState;
use gantry_print_file_fixtures as fixtures;
use http_body_util::BodyExt;
use serde_json::{Value, json};

use crate::store::users::Role;
use crate::test_support::Client;

async fn upload(client: &Client, name: &str, bytes: &[u8]) -> Value {
    let (status, body) = client
        .upload(&format!("/api/print-files?name={name}"), bytes)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

fn three_plates() -> Vec<u8> {
    fixtures::bambu_3mf(&fixtures::three_plates())
}

#[tokio::test]
async fn a_multi_plate_gcode_3mf_shows_each_plate() {
    let client = Client::new().await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    assert_eq!(file["name"], "tray.gcode.3mf");
    assert_eq!(file["kind"], "gcode_3mf");
    assert_eq!(file["printer_model"], "Bambu Lab X1 Carbon");
    assert_eq!(file["model_source"], "file");
    assert_eq!(file["plates"].as_array().unwrap().len(), 3);
    assert_eq!(file["plates"][0]["estimated_secs"], 1200);
    assert_eq!(file["plates"][1]["filaments"][1]["kind"], "PETG");
    assert_eq!(file["filament_kinds"], json!(["PLA", "PETG"]));
    assert_eq!(file["uploaded_by"], "admin");

    let (status, list) = client.get("/api/print-files").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn plate_thumbnails_are_served_and_missing_ones_are_404() {
    let client = Client::new().await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let id = file["id"].as_i64().unwrap();
    assert_eq!(file["plates"][0]["has_thumbnail"], true);
    assert_eq!(file["plates"][2]["has_thumbnail"], false);

    let response = client
        .raw_get(&format!("/api/print-files/{id}/plates/1/thumbnail"))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/png");
    let png = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(fixtures::png_size(&png), Some((32, 32)));
    let response = client
        .raw_get(&format!("/api/print-files/{id}/plates/3/thumbnail"))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Thumbnails from G-code headers, from two slicers.
    for (name, bytes) in [
        ("benchy.gcode", fixtures::prusaslicer_gcode()),
        ("cura.gcode", fixtures::cura_gcode()),
    ] {
        let file = upload(&client, name, &bytes).await;
        let id = file["id"].as_i64().unwrap();
        let response = client
            .raw_get(&format!("/api/print-files/{id}/plates/1/thumbnail"))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{name}");
    }
}

#[tokio::test]
async fn a_plain_gcode_is_one_plate_whose_model_can_be_set_by_hand() {
    let client = Client::new().await;
    let file = upload(&client, "bare.gcode", b"G28\nG1 X10 Y10 E1\n").await;
    let id = file["id"].as_i64().unwrap();
    assert_eq!(file["kind"], "gcode");
    assert_eq!(file["plates"].as_array().unwrap().len(), 1);
    assert_eq!(file["printer_model"], Value::Null);

    let (status, file) = client
        .put(
            &format!("/api/print-files/{id}"),
            json!({ "printer_model": "Ender-3" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["printer_model"], "Ender-3");
    assert_eq!(file["model_source"], "manual");

    // A .gcode.3mf names its own model.
    let tray = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let (status, _) = client
        .put(
            &format!("/api/print-files/{}", tray["id"]),
            json!({ "printer_model": "Ender-3" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn unsliced_files_are_refused_with_a_clear_message() {
    let client = Client::new().await;
    for (name, bytes) in [
        ("part.stl", b"solid part\nendsolid part\n".to_vec()),
        ("part.obj", b"v 0 0 0\n".to_vec()),
        ("part.step", b"ISO-10303-21;\n".to_vec()),
        ("project.3mf", fixtures::unsliced_3mf()),
    ] {
        let (status, body) = client
            .upload(&format!("/api/print-files?name={name}"), &bytes)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}: {body}");
        let message = body["fields"][0]["message"].as_str().unwrap();
        assert!(
            message.contains("slicing first") || message.contains("Slice it"),
            "{name}: {message}"
        );
    }
    let (_, list) = client.get("/api/print-files").await;
    assert!(list.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn operators_manage_the_library_and_viewers_only_look() {
    let admin = Client::new().await;
    let viewer = admin.as_role(Role::Viewer).await;
    let operator = admin.as_role(Role::Operator).await;
    let (status, _) = viewer
        .upload("/api/print-files?name=a.gcode", b"G1 X1\n")
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let file = upload(&operator, "a.gcode", b"G1 X1\n").await;
    let id = file["id"].as_i64().unwrap();
    let (status, _) = viewer.get(&format!("/api/print-files/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = viewer.delete(&format!("/api/print-files/{id}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = operator.delete(&format!("/api/print-files/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = viewer.get(&format!("/api/print-files/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, audit) = admin.get("/api/audit?action=print_file.uploaded").await;
    assert!(audit.to_string().contains("a.gcode"), "{audit}");
}

#[tokio::test]
async fn the_3d_model_is_served_for_a_gcode_3mf() {
    let client = Client::new().await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let (status, model) = client
        .get(&format!("/api/print-files/{}/model", file["id"]))
        .await;
    assert_eq!(status, StatusCode::OK, "{model}");
    assert_eq!(model["objects"].as_array().unwrap().len(), 2);
    assert_eq!(model["objects"][1]["colour"], "#D33A2C");
    assert_eq!(model["triangles"], 18);
    assert_eq!(model["simplified"], false);
    // Served again from the cache, the same.
    let (_, again) = client
        .get(&format!("/api/print-files/{}/model", file["id"]))
        .await;
    assert_eq!(again, model);

    let gcode = upload(&client, "a.gcode", b"G1 X1\n").await;
    let (status, _) = client
        .get(&format!("/api/print-files/{}/model", gcode["id"]))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn starting_a_plate_creates_one_job_linked_to_its_print_file_and_plate() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let operator = client.as_role(Role::Operator).await;
    let (status, job) = operator
        .post(
            &format!("/api/printers/{printer}/start"),
            json!({ "print_file_id": file["id"], "plate": 2 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{job}");
    assert_eq!(job["print_file_id"], file["id"]);
    assert_eq!(job["plate"], 2);
    assert_eq!(job["external"], false);
    assert_eq!(job["print_file"], "tray.gcode.3mf");

    // The Simulated Printer prints it, for the Plate's estimate.
    let view = client.until(&printer, |v| v.status.job.is_some()).await;
    let active = view.status.job.unwrap();
    assert_eq!(active.plate, Some(2));
    assert_eq!(active.remaining_secs.map(|s| s > 2000), Some(true));
    assert_eq!(view.job.as_ref().unwrap().id, job["id"].as_i64().unwrap());

    let (_, jobs) = client.get(&format!("/api/printers/{printer}/jobs")).await;
    assert_eq!(jobs.as_array().unwrap().len(), 1, "{jobs}");

    // A Viewer can't start one.
    let viewer = client.as_role(Role::Viewer).await;
    let (status, _) = viewer
        .post(
            &format!("/api/printers/{printer}/start"),
            json!({ "print_file_id": file["id"], "plate": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sending_to_a_printer_of_the_wrong_model_is_refused() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("P1", json!({ "model": "P1S" }))
        .await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let (status, body) = client
        .post(
            &format!("/api/printers/{printer}/start"),
            json!({ "print_file_id": file["id"], "plate": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("sliced for Bambu Lab X1 Carbon"),
        "{body}"
    );
    let (_, jobs) = client.get(&format!("/api/printers/{printer}/jobs")).await;
    assert!(jobs.as_array().unwrap().is_empty());

    // A plain .gcode without a model can't be checked, so it isn't sent.
    let gcode = upload(&client, "a.gcode", b"G1 X1\n").await;
    let (status, body) = client
        .post(
            &format!("/api/printers/{printer}/start"),
            json!({ "print_file_id": gcode["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("set the model"),
        "{body}"
    );
    // Once its model is set, it goes.
    client
        .put(
            &format!("/api/print-files/{}", gcode["id"]),
            json!({ "printer_model": "P1S" }),
        )
        .await;
    let (status, body) = client
        .post(
            &format!("/api/printers/{printer}/start"),
            json!({ "print_file_id": gcode["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["plate"], 1);
}

#[tokio::test]
async fn a_printer_awaiting_clear_refuses_a_new_job() {
    let client = Client::new().await;
    let printer = client
        .connected_simulated("X1", json!({ "model": "X1C" }))
        .await;
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
        .await;
    let file = upload(&client, "tray.gcode.3mf", &three_plates()).await;
    let path = format!("/api/printers/{printer}/start");
    let body = |plate: u32| json!({ "print_file_id": file["id"], "plate": plate });

    let (status, _) = client.post(&path, body(3)).await;
    assert_eq!(status, StatusCode::CREATED);
    client
        .until(&printer, |v| {
            v.status.state == Some(PrinterState::AwaitingClear)
        })
        .await;
    let (status, refused) = client.post(&path, body(1)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Awaiting Clear"),
        "{refused}"
    );
    let (_, jobs) = client.get(&format!("/api/printers/{printer}/jobs")).await;
    assert_eq!(jobs.as_array().unwrap().len(), 1);

    // After Mark Clear it starts; while that one prints, another can't.
    let (status, _) = client
        .post(&format!("/api/printers/{printer}/mark-clear"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    client
        .simulate(&printer, json!({ "action": "set_speed", "speed": 1 }))
        .await;
    let (status, _) = client.post(&path, body(1)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = client.post(&path, body(2)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_plates_toolpath_is_streamed_in_layers_and_cached() {
    let client = Client::new().await;
    // A plain .gcode: its only Plate.
    let gcode = crate::print_files::toolpath::synthetic_gcode(5, 3, 4);
    let (status, file) = client
        .upload("/api/print-files?name=cube.gcode", gcode.as_bytes())
        .await;
    assert_eq!(status, StatusCode::CREATED, "{file}");
    let path = format!("/api/print-files/{}/plates/1/toolpath", file["id"]);
    let response = client.raw_get(&path).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..4], b"GTP1");
    assert_eq!(
        u32::from_le_bytes(body[4..8].try_into().unwrap()),
        5,
        "5 layers"
    );
    // Again, from the cache: the same bytes.
    let again = client
        .raw_get(&path)
        .await
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(body, again);
    let (status, _) = client
        .get(&format!(
            "/api/print-files/{}/plates/2/toolpath",
            file["id"]
        ))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A .gcode.3mf: each Plate's own G-code.
    let (_, tray) = client
        .upload(
            "/api/print-files?name=tray.gcode.3mf",
            &fixtures::bambu_3mf(&fixtures::three_plates()),
        )
        .await;
    let response = client
        .raw_get(&format!(
            "/api/print-files/{}/plates/3/toolpath",
            tray["id"]
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(u32::from_le_bytes(body[4..8].try_into().unwrap()), 1);
    // A Viewer may look.
    let viewer = client.as_role(crate::store::users::Role::Viewer).await;
    assert_eq!(viewer.raw_get(&path).await.status(), StatusCode::OK);
}
