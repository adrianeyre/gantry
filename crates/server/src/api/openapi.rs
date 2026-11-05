//! The REST API's OpenAPI 3.1 document (PRD §7.7), served at
//! `GET /api/openapi.json` to anyone, like the login page: it describes the
//! API and holds nothing about this Gantry.
//!
//! The document is built from [`OPERATIONS`], one line per route. A test
//! (`openapi_tests`) checks the table against the router both ways: every
//! documented operation is routed, and every route in the source is
//! documented, with the role [`authz`](crate::authz) enforces.
use axum::Json;
use axum::extract::State;
use axum::http::Method;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::store::users::Role;

/// Who may call an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No session or token needed.
    Open,
    /// A session or API token, with the role `authz` asks for.
    Gated,
}

/// One documented operation.
#[derive(Debug, Clone, Copy)]
pub struct Operation {
    pub method: &'static str,
    /// As routed under `/api`.
    pub path: &'static str,
    pub tag: &'static str,
    pub summary: &'static str,
    /// An example JSON body, for operations that take one.
    pub body: Option<&'static str>,
    pub access: Access,
}

const fn op(
    method: &'static str,
    path: &'static str,
    tag: &'static str,
    summary: &'static str,
) -> Operation {
    Operation {
        method,
        path,
        tag,
        summary,
        body: None,
        access: Access::Gated,
    }
}

const fn with(mut o: Operation, body: &'static str) -> Operation {
    o.body = Some(body);
    o
}

const fn open(mut o: Operation) -> Operation {
    o.access = Access::Open;
    o
}

/// Every operation under `/api`.
pub const OPERATIONS: &[Operation] = &[
    // Sessions.
    open(with(
        op(
            "POST",
            "/login",
            "Session",
            "Log in with a password (and a TOTP code when it is on); sets the session cookie",
        ),
        r#"{"username":"admin","password":"…","code":"123456"}"#,
    )),
    open(op(
        "POST",
        "/login/passkey/start",
        "Session",
        "Start logging in with a passkey: options for navigator.credentials.get()",
    )),
    open(with(
        op(
            "POST",
            "/login/passkey/finish",
            "Session",
            "Finish logging in with a passkey; sets the session cookie",
        ),
        r#"{"credential":{"id":"…","response":{"clientDataJSON":"…","authenticatorData":"…","signature":"…"}}}"#,
    )),
    op(
        "GET",
        "/session",
        "Session",
        "Who is calling: the user (and the API token, if one is used)",
    ),
    op("POST", "/logout", "Session", "End the session"),
    op(
        "GET",
        "/ws",
        "Session",
        "WebSocket of live Fleet updates (a snapshot, then each change); ?client=desktop also gets desktop Alerts",
    ),
    open(op("GET", "/openapi.json", "Session", "This document")),
    // The Fleet.
    op(
        "GET",
        "/drivers",
        "Printers",
        "The Drivers compiled in, with the settings each needs",
    ),
    op(
        "GET",
        "/printers",
        "Printers",
        "Every Printer with its live status",
    ),
    with(
        op("POST", "/printers", "Printers", "Add a Printer"),
        r#"{"name":"X1","driver":"simulated","settings":{},"secrets":{}}"#,
    ),
    op(
        "GET",
        "/printers/{id}",
        "Printers",
        "One Printer with its live status",
    ),
    op(
        "PUT",
        "/printers/{id}",
        "Printers",
        "Change a Printer's name or settings",
    ),
    op("DELETE", "/printers/{id}", "Printers", "Remove a Printer"),
    op(
        "GET",
        "/printers/{id}/jobs",
        "Jobs",
        "A Printer's recent Jobs",
    ),
    op(
        "GET",
        "/printers/{id}/history",
        "Jobs",
        "A Printer's History and its summary (?from=&to=)",
    ),
    op(
        "GET",
        "/analytics/utilisation",
        "Jobs",
        "Utilisation and uptime (?printer=&from=&to=)",
    ),
    op(
        "PUT",
        "/printers/{id}/capabilities",
        "Printers",
        "Override Capabilities the Driver can't detect",
    ),
    with(
        op(
            "PUT",
            "/printers/{id}/automation",
            "Printers",
            "Turn Auto-start or Auto-Clear on or off",
        ),
        r#"{"auto_start":true,"auto_clear":false}"#,
    ),
    op(
        "GET",
        "/printers/{id}/timelapse",
        "Printers",
        "A Printer's timelapse setting",
    ),
    with(
        op(
            "PUT",
            "/printers/{id}/timelapse",
            "Printers",
            "Turn timelapse capture off, or on per layer or on a timer",
        ),
        r#"{"mode":"interval","interval_secs":30}"#,
    ),
    with(
        op(
            "POST",
            "/printers/{id}/commands",
            "Control",
            "Pause, resume, Cancel or a fine control",
        ),
        r#"{"command":"pause"}"#,
    ),
    with(
        op(
            "POST",
            "/printers/{id}/mark-clear",
            "Control",
            "Mark Clear: the bed is empty (optionally overrule Succeeded to Failed)",
        ),
        r#"{"overrule_to_failed":false}"#,
    ),
    op(
        "GET",
        "/printers/{id}/camera",
        "Control",
        "The camera as MJPEG (?quality=)",
    ),
    with(
        op(
            "POST",
            "/printers/{id}/start",
            "Control",
            "Send a Plate of a Print File to this Printer and start it",
        ),
        r#"{"print_file_id":1,"plate":1}"#,
    ),
    with(
        op("PUT", "/printers/{id}/tags", "Tags", "Set a Printer's Tags"),
        r#"{"tags":[1,2]}"#,
    ),
    with(
        op(
            "POST",
            "/bulk/commands",
            "Control",
            "One command to several Printers, with each one's result",
        ),
        r#"{"command":"pause","printers":["…"]}"#,
    ),
    op(
        "GET",
        "/printers/{id}/simulation",
        "Simulation",
        "A Simulated Printer's simulation state",
    ),
    with(
        op(
            "POST",
            "/printers/{id}/simulation",
            "Simulation",
            "Drive a Simulated Printer: start a print, raise a Fault, drop the Connection…",
        ),
        r#"{"action":"start_print","print_file":"cube.gcode","duration_secs":600,"layers":10}"#,
    ),
    op(
        "GET",
        "/discovery",
        "Printers",
        "Printers found on the LAN that aren't added yet",
    ),
    // Print Files and the Queue.
    op(
        "GET",
        "/print-files",
        "Print Files",
        "The Print File library",
    ),
    op(
        "POST",
        "/print-files",
        "Print Files",
        "Upload a Print File (the raw .gcode.3mf or .gcode as the body, ?name=)",
    ),
    op(
        "GET",
        "/print-files/{id}",
        "Print Files",
        "One Print File with its Plates",
    ),
    op(
        "PUT",
        "/print-files/{id}",
        "Print Files",
        "Change a Print File's details, such as the model it was sliced for",
    ),
    op(
        "DELETE",
        "/print-files/{id}",
        "Print Files",
        "Remove a Print File",
    ),
    op(
        "GET",
        "/print-files/{id}/plates/{plate}/thumbnail",
        "Print Files",
        "A Plate's thumbnail (PNG)",
    ),
    op(
        "GET",
        "/print-files/{id}/model",
        "Print Files",
        "The 3D model (binary meshes)",
    ),
    op(
        "GET",
        "/print-files/{id}/plates/{plate}/toolpath",
        "Print Files",
        "A Plate's toolpath (binary layers)",
    ),
    op(
        "GET",
        "/queue",
        "Queue",
        "The Queue, with what each Job could run on",
    ),
    with(
        op(
            "POST",
            "/queue",
            "Queue",
            "Queue a Print File's Plates, for one Printer or any Compatible Printer",
        ),
        r#"{"print_file_id":1,"plates":[1],"printer_id":null}"#,
    ),
    with(
        op("PUT", "/queue/order", "Queue", "Reorder the Queue"),
        r#"{"ids":[3,1,2]}"#,
    ),
    op("DELETE", "/queue/{id}", "Queue", "Remove a queued Job"),
    with(
        op(
            "POST",
            "/queue/{id}/start",
            "Queue",
            "Start a queued Job now",
        ),
        r#"{"printer_id":"…"}"#,
    ),
    op(
        "GET",
        "/jobs/{id}/timelapse",
        "Jobs",
        "A Job's timelapse: its frames",
    ),
    op(
        "GET",
        "/jobs/{id}/timelapse/frames/{frame}",
        "Jobs",
        "One frame of a Job's timelapse (JPEG)",
    ),
    op(
        "DELETE",
        "/jobs/{id}/timelapse",
        "Jobs",
        "Remove a Job's timelapse (the Job stays in History)",
    ),
    // Tags, filament.
    op("GET", "/tags", "Tags", "Every Tag"),
    with(
        op("POST", "/tags", "Tags", "Make a Tag"),
        r#"{"name":"Workshop"}"#,
    ),
    op("DELETE", "/tags/{id}", "Tags", "Remove a Tag"),
    op("GET", "/spools", "Filament", "The filament inventory"),
    op("POST", "/spools", "Filament", "Add a spool"),
    op("PUT", "/spools/{id}", "Filament", "Change a spool"),
    op("DELETE", "/spools/{id}", "Filament", "Remove a spool"),
    // Alerts and webhooks.
    op(
        "GET",
        "/alerts/channel-kinds",
        "Alerts",
        "The kinds of Alert channel, with their settings",
    ),
    op(
        "GET",
        "/alerts/channels",
        "Alerts",
        "Alert channels (never their credentials)",
    ),
    with(
        op("POST", "/alerts/channels", "Alerts", "Add an Alert channel"),
        r#"{"name":"Phone","kind":"ntfy","settings":{"topic":"…"},"secrets":{}}"#,
    ),
    op(
        "PUT",
        "/alerts/channels/{id}",
        "Alerts",
        "Change an Alert channel",
    ),
    op(
        "DELETE",
        "/alerts/channels/{id}",
        "Alerts",
        "Remove an Alert channel and its Rules",
    ),
    op(
        "POST",
        "/alerts/channels/{id}/test",
        "Alerts",
        "Send a test Alert",
    ),
    op("GET", "/alerts/rules", "Alerts", "Alert Rules"),
    with(
        op("POST", "/alerts/rules", "Alerts", "Add an Alert Rule"),
        r#"{"name":"Faults","events":["fault_raised"],"channel_id":1}"#,
    ),
    op(
        "PUT",
        "/alerts/rules/{id}",
        "Alerts",
        "Change an Alert Rule",
    ),
    op(
        "DELETE",
        "/alerts/rules/{id}",
        "Alerts",
        "Remove an Alert Rule",
    ),
    op(
        "GET",
        "/webhooks",
        "Webhooks",
        "Webhooks (never their secrets)",
    ),
    with(
        op(
            "POST",
            "/webhooks",
            "Webhooks",
            "Add a webhook; the answer holds its signing secret, once",
        ),
        r#"{"name":"Home Assistant","url":"https://…","events":["job_ended"]}"#,
    ),
    op(
        "PUT",
        "/webhooks/{id}",
        "Webhooks",
        "Change a webhook ({\"new_secret\":true} makes a new secret)",
    ),
    op("DELETE", "/webhooks/{id}", "Webhooks", "Remove a webhook"),
    op(
        "POST",
        "/webhooks/{id}/test",
        "Webhooks",
        "Send a signed test delivery, once",
    ),
    // Administration.
    op("GET", "/settings", "Settings", "Gantry-wide settings"),
    op("PUT", "/settings", "Settings", "Change settings"),
    op(
        "GET",
        "/audit",
        "Settings",
        "The audit log (?user=&printer=&action=&before=&limit=)",
    ),
    op(
        "GET",
        "/config/export",
        "Settings",
        "Export Printers, Tags and Alerts as TOML (?credentials=true to include them)",
    ),
    op(
        "POST",
        "/config/import/preview",
        "Settings",
        "What an import would change",
    ),
    op("POST", "/config/import", "Settings", "Import a config"),
    op("GET", "/users", "Users", "Every account"),
    with(
        op("POST", "/users", "Users", "Make an account"),
        r#"{"username":"sam","password":"…","role":"operator"}"#,
    ),
    op(
        "PUT",
        "/users/{id}",
        "Users",
        "Change an account's role or password",
    ),
    op("DELETE", "/users/{id}", "Users", "Remove an account"),
    op("POST", "/users/{id}/unlock", "Users", "End a lockout"),
    op(
        "DELETE",
        "/users/{id}/totp",
        "Users",
        "Turn off a user's two-factor authentication",
    ),
    op("GET", "/users/{id}/passkeys", "Users", "A user's passkeys"),
    op(
        "DELETE",
        "/users/{id}/passkeys/{passkey}",
        "Users",
        "Remove one of a user's passkeys",
    ),
    // The caller's own account.
    op(
        "GET",
        "/account",
        "Account",
        "The caller's two-factor authentication and passkeys",
    ),
    with(
        op(
            "POST",
            "/account/totp/setup",
            "Account",
            "Start setting up TOTP: a secret and QR code (asks for the password)",
        ),
        r#"{"password":"…"}"#,
    ),
    with(
        op(
            "POST",
            "/account/totp/enable",
            "Account",
            "Turn TOTP on with a code; answers with recovery codes, once",
        ),
        r#"{"code":"123456"}"#,
    ),
    with(
        op(
            "POST",
            "/account/totp/disable",
            "Account",
            "Turn TOTP off (the password and a code)",
        ),
        r#"{"password":"…","code":"123456"}"#,
    ),
    with(
        op(
            "POST",
            "/account/totp/recovery-codes",
            "Account",
            "Replace the recovery codes, given a code",
        ),
        r#"{"code":"123456"}"#,
    ),
    op(
        "POST",
        "/account/passkeys/register/start",
        "Account",
        "Start adding a passkey: options for navigator.credentials.create()",
    ),
    with(
        op(
            "POST",
            "/account/passkeys/register/finish",
            "Account",
            "Finish adding a passkey",
        ),
        r#"{"name":"Phone","credential":{"id":"…","response":{"clientDataJSON":"…","attestationObject":"…"}}}"#,
    ),
    op(
        "DELETE",
        "/account/passkeys/{id}",
        "Account",
        "Remove one of the caller's passkeys",
    ),
    op(
        "GET",
        "/tokens",
        "Account",
        "The caller's API tokens (?all=true for an Admin: everyone's)",
    ),
    with(
        op(
            "POST",
            "/tokens",
            "Account",
            "Make an API token; the answer holds the token, once",
        ),
        r#"{"name":"Home Assistant","expires_in_days":365}"#,
    ),
    op("DELETE", "/tokens/{id}", "Account", "Revoke an API token"),
];

fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("operations use standard methods")
}

/// The least role an operation needs, as `authz` enforces it.
pub fn role(operation: &Operation) -> Option<Role> {
    (operation.access == Access::Gated)
        .then(|| crate::authz::required_role(&method(operation.method), operation.path))
}

/// The whole document.
pub fn document(server: Option<&str>) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for operation in OPERATIONS {
        let params: Vec<Value> = operation
            .path
            .split('/')
            .filter_map(|part| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')))
            .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
            .collect();
        let mut entry = json!({
            "tags": [operation.tag],
            "summary": operation.summary,
            "operationId": format!(
                "{}{}",
                operation.method.to_ascii_lowercase(),
                operation.path.replace(['{', '}'], "").replace(['/', '-', '.'], "_")
            ),
            "parameters": params,
            "responses": {
                "2XX": { "description": "Done. JSON, unless the summary says otherwise" },
                "401": { "$ref": "#/components/responses/Unauthorized" },
                "403": { "$ref": "#/components/responses/Forbidden" },
                "422": { "$ref": "#/components/responses/Invalid" },
            },
        });
        match role(operation) {
            Some(role) => {
                entry["x-gantry-role"] = json!(role.as_str());
                entry["description"] =
                    json!(format!("Needs the {} role (or higher).", role.label()));
            }
            None => {
                entry["security"] = json!([]);
                entry["description"] = json!("Open: needs no session or token.");
            }
        }
        if let Some(body) = operation.body {
            let example: Value = serde_json::from_str(body).expect("example bodies are JSON");
            entry["requestBody"] = json!({
                "required": true,
                "content": { "application/json": { "schema": { "type": "object" }, "example": example } },
            });
        }
        paths
            .entry(format!("/api{}", operation.path))
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("a path item is an object")
            .insert(operation.method.to_ascii_lowercase(), entry);
    }
    let error = json!({
        "description": "An error",
        "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } },
    });
    let tags: Vec<Value> = [
        "Session",
        "Printers",
        "Control",
        "Jobs",
        "Print Files",
        "Queue",
        "Tags",
        "Filament",
        "Alerts",
        "Webhooks",
        "Settings",
        "Users",
        "Account",
        "Simulation",
    ]
    .iter()
    .map(|name| json!({ "name": name }))
    .collect();
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Gantry",
            "version": crate::VERSION,
            "description": "Read the Fleet's state and trigger prints from other tools. \
                Authenticate with an API token (made under Account → API tokens) as \
                `Authorization: Bearer gantry_…`. A token acts as its owner, with the owner's \
                role, and everything it changes is in the audit log with the token's name. \
                See docs/rest-api.md.",
            "license": { "name": "ISC" },
        },
        "servers": [{ "url": server.unwrap_or("/") }],
        "security": [{ "apiToken": [] }, { "session": [] }],
        "tags": tags,
        "paths": paths,
        "components": {
            "securitySchemes": {
                "apiToken": { "type": "http", "scheme": "bearer", "description": "An API token, gantry_…" },
                "session": { "type": "apiKey", "in": "cookie", "name": crate::auth::COOKIE },
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": { "type": "string" },
                        "fields": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "field": { "type": "string" }, "message": { "type": "string" } },
                            },
                        },
                    },
                },
            },
            "responses": {
                "Unauthorized": error.clone(),
                "Forbidden": error.clone(),
                "Invalid": error,
            },
        },
    })
}

/// `GET /api/openapi.json`.
pub async fn serve(State(state): State<AppState>) -> Json<Value> {
    Json(document(state.public_url()))
}
