//! The OpenAPI document matches the routes (#70): every documented
//! operation is routed, every route is documented, and each one's access is
//! what the server enforces.
use std::collections::BTreeSet;

use axum::http::StatusCode;

use super::openapi::{Access, OPERATIONS, document};
use crate::test_support::{Client, anonymous};

/// Every `(METHOD, path)` routed under `/api`, read from the router's source:
/// `api::routes()` and the routes `app()` adds to the API router.
fn routed() -> BTreeSet<(String, String)> {
    let api = include_str!("mod.rs");
    let lib = include_str!("../lib.rs");
    // In lib.rs, only the API router: from `let api = Router::new()` to the
    // `/api` nest.
    let start = lib.find("let api = Router::new()").expect("the API router");
    let end = lib[start..].find(".nest(\"/api\"").expect("the /api nest") + start;
    let mut out = BTreeSet::new();
    for source in [
        &api[..api.find("pub enum ApiError").unwrap()],
        &lib[start..end],
    ] {
        let mut rest = source;
        while let Some(at) = rest.find(".route(") {
            let body_start = at + ".route(".len();
            let mut depth = 1;
            let mut end = body_start;
            for (i, c) in rest[body_start..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            let body = &rest[body_start..end];
            let path = body
                .trim_start()
                .strip_prefix('"')
                .and_then(|b| b.split('"').next())
                .expect("a route's path is a string literal");
            for method in ["get", "post", "put", "delete", "patch"] {
                let call = format!("{method}(");
                let mut search = body;
                while let Some(i) = search.find(&call) {
                    let before = search[..i].chars().last();
                    if before.is_none_or(|c| !(c.is_alphanumeric() || c == '_')) {
                        out.insert((method.to_ascii_uppercase(), path.to_owned()));
                    }
                    search = &search[i + call.len()..];
                }
            }
            rest = &rest[end..];
        }
    }
    out
}

fn documented() -> BTreeSet<(String, String)> {
    OPERATIONS
        .iter()
        .map(|o| (o.method.to_owned(), o.path.to_owned()))
        .collect()
}

#[test]
fn every_route_is_documented_and_nothing_else_is() {
    let routed = routed();
    let documented = documented();
    assert!(routed.len() > 80, "the source scan found {}", routed.len());
    let undocumented: Vec<_> = routed.difference(&documented).collect();
    let unrouted: Vec<_> = documented.difference(&routed).collect();
    assert!(
        undocumented.is_empty(),
        "routed but not documented: {undocumented:?}"
    );
    assert!(
        unrouted.is_empty(),
        "documented but not routed: {unrouted:?}"
    );
    assert_eq!(
        documented.len(),
        OPERATIONS.len(),
        "an operation is listed twice"
    );
}

/// A path with each `{param}` filled in.
fn concrete(path: &str) -> String {
    path.split('/')
        .map(|part| if part.starts_with('{') { "1" } else { part })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn every_documented_operation_reaches_a_handler_with_the_documented_access() {
    let client = Client::new().await;
    let visitor = anonymous(&client.state);
    for operation in OPERATIONS {
        let path = format!("/api{}", concrete(operation.path));
        let (status, body) = visitor.send(operation.method, &path, None).await;
        // The router's own answers: no such path, or not that method.
        assert_ne!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{} {path}",
            operation.method
        );
        assert!(
            !(status == StatusCode::NOT_FOUND && body["error"] == "not found"),
            "{} {path} isn't routed",
            operation.method
        );
        match operation.access {
            Access::Gated => assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{} {path} must need a session",
                operation.method
            ),
            Access::Open => assert_ne!(
                status,
                StatusCode::UNAUTHORIZED,
                "{} {path} is documented as open",
                operation.method
            ),
        }
    }
}

#[test]
fn documented_roles_are_the_enforced_ones() {
    let doc = document(None);
    for operation in OPERATIONS {
        let entry =
            &doc["paths"][format!("/api{}", operation.path)][operation.method.to_ascii_lowercase()];
        assert_eq!(entry["summary"], operation.summary);
        match super::openapi::role(operation) {
            Some(role) => {
                assert_eq!(entry["x-gantry-role"], role.as_str(), "{}", operation.path);
                let enforced = crate::authz::required_role(
                    &axum::http::Method::from_bytes(operation.method.as_bytes()).unwrap(),
                    operation.path,
                );
                assert_eq!(role, enforced);
            }
            None => assert_eq!(entry["security"], serde_json::json!([])),
        }
    }
    // Spot checks against the policy table.
    let role = |method: &str, path: &str| {
        doc["paths"][format!("/api{path}")][method]["x-gantry-role"].clone()
    };
    assert_eq!(role("get", "/printers"), "viewer");
    assert_eq!(role("post", "/printers/{id}/commands"), "operator");
    assert_eq!(role("post", "/printers"), "admin");
    assert_eq!(role("post", "/tokens"), "viewer");
    assert_eq!(role("get", "/webhooks"), "admin");
}

#[tokio::test]
async fn the_document_is_served_to_anyone_as_openapi_3_1() {
    let client = Client::new().await;
    let (status, doc) = anonymous(&client.state).get("/api/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["info"]["title"], "Gantry");
    assert_eq!(
        doc["components"]["securitySchemes"]["apiToken"]["scheme"],
        "bearer"
    );
    let operations: usize = doc["paths"]
        .as_object()
        .unwrap()
        .values()
        .map(|item| item.as_object().unwrap().len())
        .sum();
    assert_eq!(operations, OPERATIONS.len());
    // Every {param} in a path is declared.
    for (path, item) in doc["paths"].as_object().unwrap() {
        let params = path.matches('{').count();
        for operation in item.as_object().unwrap().values() {
            assert_eq!(
                operation["parameters"].as_array().unwrap().len(),
                params,
                "{path}"
            );
        }
    }
    // The examples are what the handlers take: queue a Plate as documented.
    let entry = &doc["paths"]["/api/queue/order"]["put"];
    assert!(entry["requestBody"]["content"]["application/json"]["example"]["ids"].is_array());
}
