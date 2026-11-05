//! Tags (PRD §7.6): labels an Admin attaches to Printers, for filtering.
use serde::Serialize;

use super::{Store, now, rfc3339};

/// A Tag and the Printers it is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// Printer ids.
    pub printers: Vec<String>,
}

/// Why a Tag couldn't be created.
#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("a Tag called `{0}` already exists")]
    Duplicate(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl Store {
    /// Every Tag, by name, with its Printers.
    pub async fn tags(&self) -> Result<Vec<Tag>, sqlx::Error> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, name FROM tags ORDER BY name COLLATE NOCASE")
                .fetch_all(self.pool())
                .await?;
        let links: Vec<(i64, String)> =
            sqlx::query_as("SELECT tag_id, printer_id FROM printer_tags ORDER BY printer_id")
                .fetch_all(self.pool())
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name)| Tag {
                printers: links
                    .iter()
                    .filter(|(tag, _)| *tag == id)
                    .map(|(_, p)| p.clone())
                    .collect(),
                id,
                name,
            })
            .collect())
    }

    pub async fn create_tag(&self, name: &str) -> Result<Tag, TagError> {
        let done = sqlx::query("INSERT INTO tags (name, created_at) VALUES (?, ?)")
            .bind(name)
            .bind(rfc3339(now()))
            .execute(self.pool())
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    TagError::Duplicate(name.to_owned())
                }
                _ => TagError::Database(e),
            })?;
        Ok(Tag {
            id: done.last_insert_rowid(),
            name: name.to_owned(),
            printers: Vec::new(),
        })
    }

    /// Removes a Tag from every Printer and deletes it. Returns its name.
    pub async fn delete_tag(&self, id: i64) -> Result<Option<String>, sqlx::Error> {
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM tags WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        if name.is_some() {
            sqlx::query("DELETE FROM tags WHERE id = ?")
                .bind(id)
                .execute(self.pool())
                .await?;
        }
        Ok(name)
    }

    /// A Printer's Tags, by name.
    pub async fn printer_tags(&self, printer: &str) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT t.name FROM tags t JOIN printer_tags pt ON pt.tag_id = t.id \
             WHERE pt.printer_id = ? ORDER BY t.name COLLATE NOCASE",
        )
        .bind(printer)
        .fetch_all(self.pool())
        .await
    }

    /// A Printer's Tags, by id.
    pub async fn printer_tag_ids(&self, printer: &str) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT tag_id FROM printer_tags WHERE printer_id = ?")
            .bind(printer)
            .fetch_all(self.pool())
            .await
    }

    /// Replaces a Printer's Tags with `tags` (ids). Unknown ids are an error.
    pub async fn set_printer_tags(&self, printer: &str, tags: &[i64]) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query("DELETE FROM printer_tags WHERE printer_id = ?")
            .bind(printer)
            .execute(&mut *tx)
            .await?;
        for tag in tags {
            sqlx::query("INSERT OR IGNORE INTO printer_tags (printer_id, tag_id) VALUES (?, ?)")
                .bind(printer)
                .bind(tag)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }
}
