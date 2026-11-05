//! Tags (PRD §7.6): Admins create Tags and attach them to Printers; everyone
//! can see them and filter the dashboard by them.
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Deserialize;

use super::ApiError;
use crate::AppState;
use crate::audit::{self, EntryExt};
use crate::store::audit::Action;
use crate::store::tags::{Tag, TagError};
use crate::store::users::User;

/// `GET /api/tags`: every Tag, with the Printers it is on.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Tag>>, ApiError> {
    Ok(Json(state.store().tags().await?))
}

#[derive(Debug, Deserialize)]
pub struct TagForm {
    name: String,
}

/// `POST /api/tags`.
pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(form): Json<TagForm>,
) -> Result<(StatusCode, Json<Tag>), ApiError> {
    let name = form.name.trim().to_owned();
    let result = async {
        if name.is_empty() {
            return Err(ApiError::field("name", "Required"));
        }
        if name.chars().count() > 32 {
            return Err(ApiError::field("name", "Must be 32 characters or fewer"));
        }
        state.store().create_tag(&name).await.map_err(|e| match e {
            TagError::Duplicate(_) => ApiError::field("name", &e.to_string()),
            TagError::Database(e) => e.into(),
        })
    }
    .await;
    audit::record(
        &state,
        audit::by(&user, Action::TagCreated)
            .target(name.clone())
            .result(&result),
    )
    .await;
    result.map(|tag| (StatusCode::CREATED, Json(tag)))
}

/// `DELETE /api/tags/{id}`: removes the Tag from every Printer.
pub async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let result = state
        .store()
        .delete_tag(id)
        .await
        .map_err(ApiError::from)
        .and_then(|name| name.ok_or_else(|| ApiError::NotFound("no such Tag".into())));
    let target = result
        .as_ref()
        .map_or_else(|_| id.to_string(), Clone::clone);
    audit::record(
        &state,
        audit::by(&user, Action::TagRemoved)
            .target(target)
            .result(&result),
    )
    .await;
    result.map(|_| StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PrinterTagsForm {
    /// Tag ids. The Printer gets exactly these.
    tags: Vec<i64>,
}

/// `PUT /api/printers/{id}/tags`: sets which Tags a Printer has.
pub async fn set_for_printer(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(printer): Path<String>,
    Json(form): Json<PrinterTagsForm>,
) -> Result<Json<Vec<String>>, ApiError> {
    let result = async {
        if state
            .store()
            .printer(state.key(), &printer)
            .await?
            .is_none()
        {
            return Err(ApiError::NotFound("no such Printer".into()));
        }
        let known = state.store().tags().await?;
        if let Some(unknown) = form
            .tags
            .iter()
            .find(|t| !known.iter().any(|k| k.id == **t))
        {
            return Err(ApiError::field(
                "tags",
                &format!("there is no Tag {unknown}"),
            ));
        }
        state.store().set_printer_tags(&printer, &form.tags).await?;
        Ok(state.store().printer_tags(&printer).await?)
    }
    .await;
    let mut entry = audit::by(&user, Action::PrinterTagged)
        .printer(&printer)
        .result(&result);
    if let Ok(names) = &result {
        entry = entry.detail(serde_json::json!({ "tags": names }));
    }
    audit::record(&state, entry).await;
    result.map(Json)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::store::users::Role;
    use crate::test_support::Client;

    #[tokio::test]
    async fn admins_create_attach_and_remove_tags() {
        let admin = Client::new().await;
        let printer = admin.add_simulated("X1", json!({})).await;
        let (status, garage) = admin.post("/api/tags", json!({ "name": "Garage" })).await;
        assert_eq!(status, StatusCode::CREATED, "{garage}");
        let (_, pla) = admin.post("/api/tags", json!({ "name": "PLA only" })).await;
        let (status, _) = admin.post("/api/tags", json!({ "name": "garage" })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "names are unique");

        let (status, names) = admin
            .put(
                &format!("/api/printers/{printer}/tags"),
                json!({ "tags": [garage["id"], pla["id"]] }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{names}");
        assert_eq!(names, json!(["Garage", "PLA only"]));
        let (_, tags) = admin.get("/api/tags").await;
        assert_eq!(tags[0]["printers"], json!([printer]));

        // Removing a Tag takes it off the Printer.
        let (status, _) = admin.delete(&format!("/api/tags/{}", pla["id"])).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            admin.state.store().printer_tags(&printer).await.unwrap(),
            ["Garage"]
        );
        let (status, _) = admin
            .put(
                &format!("/api/printers/{printer}/tags"),
                json!({ "tags": [999] }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn only_admins_change_tags_but_everyone_sees_them() {
        let admin = Client::new().await;
        let printer = admin.add_simulated("X1", json!({})).await;
        let (_, tag) = admin.post("/api/tags", json!({ "name": "Garage" })).await;
        for role in [Role::Viewer, Role::Operator] {
            let user = admin.as_role(role).await;
            let (status, _) = user.post("/api/tags", json!({ "name": "Mine" })).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{role:?}");
            let (status, _) = user
                .put(
                    &format!("/api/printers/{printer}/tags"),
                    json!({ "tags": [tag["id"]] }),
                )
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{role:?}");
            let (status, _) = user.delete(&format!("/api/tags/{}", tag["id"])).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{role:?}");
            let (status, tags) = user.get("/api/tags").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(tags.as_array().unwrap().len(), 1);
        }
        let (_, audit) = admin.get("/api/audit?action=tag.created").await;
        assert!(audit.to_string().contains("Garage"), "{audit}");
    }
}
