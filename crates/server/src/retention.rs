//! Size-based cleanup (PRD §7.5). Job History is kept forever and is never
//! cleaned up; what is cleaned up is stored *files*: uploaded Print Files
//! now, and timelapses later.
//!
//! The rule is the same for every kind of file, so it lives here apart from
//! any of them: given everything stored of one kind, with its size, when it
//! was added and whether something still needs it, and a limit, remove the
//! oldest unneeded ones until the total fits. A timelapse store only has to
//! list its files as [`Stored`] and delete what [`to_remove`] names.
//!
//! For the Print File library, a file is needed while a queued Job prints
//! it, while a running Job is printing it, or just after it was uploaded.
use std::collections::HashSet;

use time::OffsetDateTime;

use crate::AppState;
use crate::audit;
use crate::store::audit::{Action, Entry};

/// One stored file, as cleanup sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub id: i64,
    pub bytes: u64,
    pub added_at: OffsetDateTime,
    /// Something still needs it, so it is never removed.
    pub protected: bool,
}

/// Which files to remove, oldest first, so the rest fit in `limit` bytes.
/// Protected files are never removed, even when the rest can't be made to
/// fit. A limit of 0 means no limit.
pub fn to_remove(stored: &[Stored], limit: u64) -> Vec<i64> {
    if limit == 0 {
        return Vec::new();
    }
    let mut total: u64 = stored.iter().map(|s| s.bytes).sum();
    let mut oldest: Vec<&Stored> = stored.iter().filter(|s| !s.protected).collect();
    oldest.sort_by_key(|s| (s.added_at, s.id));
    let mut out = Vec::new();
    for s in oldest {
        if total <= limit {
            break;
        }
        total = total.saturating_sub(s.bytes);
        out.push(s.id);
    }
    out
}

/// The Print File library's limit, in bytes, from the Admin's setting.
pub fn library_limit_bytes(megabytes: u64) -> u64 {
    megabytes.saturating_mul(1024 * 1024)
}

/// Removes the oldest Print Files no queued or running Job uses, until the
/// library fits its limit. `keep` is also never removed (a file just
/// uploaded). Each removal is audited as done by Gantry. Returns the ids
/// removed.
pub async fn clean_library(state: &AppState, keep: Option<i64>) -> Result<Vec<i64>, sqlx::Error> {
    let settings = crate::api::settings::current(state).await?;
    let limit = library_limit_bytes(settings.library_limit_mb);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut needed: HashSet<i64> = state
        .store()
        .queue()
        .await?
        .iter()
        .map(|q| q.print_file_id)
        .collect();
    needed.extend(state.store().print_files_in_use().await?);
    for view in state.fleet().snapshot() {
        needed.extend(view.job.as_ref().and_then(|j| j.print_file_id));
    }
    needed.extend(keep);
    let files = state.store().print_files().await?;
    let stored: Vec<Stored> = files
        .iter()
        .map(|f| Stored {
            id: f.id,
            bytes: u64::try_from(f.size_bytes).unwrap_or(0),
            added_at: f.uploaded_at,
            protected: needed.contains(&f.id),
        })
        .collect();
    let remove = to_remove(&stored, limit);
    for id in &remove {
        let name = files
            .iter()
            .find(|f| f.id == *id)
            .map_or_else(|| id.to_string(), |f| f.name.clone());
        state.store().delete_print_file(*id).await?;
        if let Err(e) = state.library().remove(*id).await {
            tracing::warn!(print_file = id, error = %e, "could not delete a Print File's folder");
        }
        tracing::info!(print_file = %name, "removed by size-based cleanup");
        let mut entry = Entry::new(Action::PrintFileRemoved);
        entry.target = Some(name);
        entry.detail = Some(serde_json::json!({
            "print_file_id": id,
            "retention": true,
            "limit_mb": settings.library_limit_mb,
        }));
        audit::record(state, entry).await;
    }
    Ok(remove)
}

/// How often the library is checked against its limit, besides after each
/// upload and each change of the limit.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Starts the task that checks the library every [`EVERY`]. Must be called
/// within a tokio runtime.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            clean_library_logged(&state, None).await;
        }
    });
}

/// Cleans the library now, logging rather than failing.
pub async fn clean_library_logged(state: &AppState, keep: Option<i64>) {
    if let Err(e) = clean_library(state, keep).await {
        tracing::warn!(error = %e, "size-based cleanup of the library failed");
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;
    use time::macros::datetime;

    use super::*;

    fn stored(id: i64, mb: u64, day: i64, protected: bool) -> Stored {
        Stored {
            id,
            bytes: mb * 1024 * 1024,
            added_at: datetime!(2026-11-01 00:00 UTC) + Duration::days(day),
            protected,
        }
    }

    #[test]
    fn the_oldest_go_first_until_the_rest_fit() {
        let files = [
            stored(3, 10, 3, false),
            stored(1, 10, 1, false),
            stored(2, 10, 2, false),
        ];
        assert_eq!(to_remove(&files, library_limit_bytes(25)), [1]);
        assert_eq!(to_remove(&files, library_limit_bytes(15)), [1, 2]);
        assert!(to_remove(&files, library_limit_bytes(30)).is_empty());
    }

    #[test]
    fn a_needed_file_is_never_removed_even_if_the_rest_cant_fit() {
        let files = [
            stored(1, 50, 1, true),
            stored(2, 10, 2, false),
            stored(3, 10, 3, true),
        ];
        assert_eq!(to_remove(&files, library_limit_bytes(5)), [2]);
    }

    #[test]
    fn no_limit_removes_nothing() {
        let files = [stored(1, 500, 1, false)];
        assert!(to_remove(&files, 0).is_empty());
    }
}

#[cfg(test)]
mod api_tests {
    use axum::http::StatusCode;
    use gantry_core::PrinterState;
    use gantry_print_file_fixtures as fixtures;
    use serde_json::{Value, json};

    use crate::store::users::Role;
    use crate::test_support::Client;

    /// A plain `.gcode` for an MK4, padded to about 0.6 MB.
    fn big_gcode() -> Vec<u8> {
        let mut bytes = fixtures::prusaslicer_gcode();
        bytes.extend(
            "; padding to make the file big enough to count\n"
                .repeat(13_000)
                .into_bytes(),
        );
        bytes
    }

    async fn upload(client: &Client, name: &str) -> Value {
        let (status, file) = client
            .upload(&format!("/api/print-files?name={name}"), &big_gcode())
            .await;
        assert_eq!(status, StatusCode::CREATED, "{file}");
        file
    }

    async fn names(client: &Client) -> Vec<String> {
        let (_, files) = client.get("/api/print-files").await;
        let mut names: Vec<String> = files
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn cleanup_keeps_what_queued_and_running_jobs_use_and_never_touches_history() {
        let client = Client::new().await;
        let (_, settings) = client.get("/api/settings").await;
        assert_eq!(settings["library_limit_mb"], 0, "no limit by default");
        // Only an Admin sets the limit.
        let operator = client.as_role(Role::Operator).await;
        let (status, _) = operator
            .put("/api/settings", json!({ "library_limit_mb": 1 }))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, body) = client
            .put("/api/settings", json!({ "library_limit_mb": 1 }))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let printer = client
            .connected_simulated("MK4", json!({ "model": "MK4" }))
            .await;
        client
            .put(
                &format!("/api/printers/{printer}/automation"),
                json!({ "auto_start": false }),
            )
            .await;
        client
            .simulate(&printer, json!({ "action": "set_speed", "speed": 1000 }))
            .await;

        // A is queued, so it stays however full the library gets.
        let a = upload(&client, "a.gcode").await;
        client
            .post(
                "/api/queue",
                json!({ "printer_id": printer, "print_file_id": a["id"] }),
            )
            .await;
        // B: 1.2 MB in all, over the limit, but A is queued and B is new.
        let b = upload(&client, "b.gcode").await;
        assert_eq!(names(&client).await, ["a.gcode", "b.gcode"]);

        // B prints; while it runs, it stays too.
        let (status, job) = client
            .post(
                &format!("/api/printers/{printer}/start"),
                json!({ "print_file_id": b["id"] }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{job}");
        upload(&client, "c.gcode").await;
        assert_eq!(names(&client).await, ["a.gcode", "b.gcode", "c.gcode"]);

        // Once B has ended nothing needs it, and it is the oldest unneeded file.
        client
            .until(&printer, |v| {
                v.status.state == Some(PrinterState::AwaitingClear)
            })
            .await;
        upload(&client, "d.gcode").await;
        assert_eq!(
            names(&client).await,
            ["a.gcode", "d.gcode"],
            "B and then C went"
        );

        // B's Job is still in History, with its file's name.
        let (_, history) = client
            .get(&format!("/api/printers/{printer}/history"))
            .await;
        let jobs = history["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["print_file"], "b.gcode");
        assert!(jobs[0]["print_file_id"].is_null());
        assert_eq!(jobs[0]["outcome"], "succeeded");
        // And the cleanup is audited as Gantry's doing.
        let (_, audit) = client.get("/api/audit?action=print_file.removed").await;
        let entries = audit["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{audit}");
        assert!(
            entries
                .iter()
                .all(|e| e["username"].is_null() && e["detail"]["retention"] == true)
        );
    }
}
