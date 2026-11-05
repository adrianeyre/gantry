//! The Print File library's rows: each file and its Plates.
use serde::Serialize;
use time::OffsetDateTime;

use super::{Store, now, parse_time, rfc3339};
use crate::print_files::parse::{Kind, Parsed, PlateFilament};

/// Where a Print File's model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Read from the file.
    File,
    /// Set by hand, for a plain `.gcode`.
    Manual,
}

/// One Plate in the library.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plate {
    pub number: u32,
    pub name: Option<String>,
    pub estimated_secs: Option<u64>,
    pub layers: Option<u32>,
    pub filaments: Vec<PlateFilament>,
    pub has_thumbnail: bool,
}

/// One Print File in the library.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrintFile {
    pub id: i64,
    pub name: String,
    pub kind: Kind,
    /// The model it was sliced for, or `None` while unknown.
    pub printer_model: Option<String>,
    pub model_source: Option<ModelSource>,
    pub slicer: Option<String>,
    pub size_bytes: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub uploaded_at: OffsetDateTime,
    pub uploaded_by: Option<String>,
    pub plates: Vec<Plate>,
}

impl PrintFile {
    /// Every filament type any Plate needs, without repeats, in order.
    pub fn filament_kinds(&self) -> Vec<String> {
        let mut kinds: Vec<String> = Vec::new();
        for f in self.plates.iter().flat_map(|p| &p.filaments) {
            if !kinds.iter().any(|k| k.eq_ignore_ascii_case(&f.kind)) {
                kinds.push(f.kind.clone());
            }
        }
        kinds
    }

    pub fn plate(&self, number: u32) -> Option<&Plate> {
        self.plates.iter().find(|p| p.number == number)
    }
}

type FileRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<String>,
);

type PlateRow = (
    i64,
    i64,
    Option<String>,
    Option<i64>,
    Option<i64>,
    String,
    bool,
);

const FILE_COLUMNS: &str = "id, name, kind, printer_model, model_source, slicer, size_bytes, \
                            uploaded_at, uploaded_by";

impl Store {
    /// Records a new Print File and its Plates. Returns its id.
    pub async fn insert_print_file(
        &self,
        name: &str,
        parsed: &Parsed,
        size: usize,
        sha256: &str,
        uploaded_by: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let done = sqlx::query(
            "INSERT INTO print_files (name, kind, printer_model, model_source, slicer, size_bytes, \
             sha256, uploaded_at, uploaded_by) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(parsed.kind.as_str())
        .bind(&parsed.printer_model)
        .bind(parsed.printer_model.as_ref().map(|_| "file"))
        .bind(&parsed.slicer)
        .bind(size as i64)
        .bind(sha256)
        .bind(rfc3339(now()))
        .bind(uploaded_by)
        .execute(&mut *tx)
        .await?;
        let id = done.last_insert_rowid();
        for plate in &parsed.plates {
            sqlx::query(
                "INSERT INTO plates (print_file_id, number, name, estimated_secs, layers, \
                 filaments, has_thumbnail) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(i64::from(plate.number))
            .bind(&plate.name)
            .bind(plate.estimated_secs.map(|s| s as i64))
            .bind(plate.layers.map(i64::from))
            .bind(serde_json::to_string(&plate.filaments).unwrap_or_else(|_| "[]".into()))
            .bind(plate.thumbnail.is_some())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    async fn plates_by_file(&self) -> Result<Vec<PlateRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT print_file_id, number, name, estimated_secs, layers, filaments, has_thumbnail \
             FROM plates ORDER BY print_file_id, number",
        )
        .fetch_all(self.pool())
        .await
    }

    fn print_file_from(row: FileRow, plates: &[PlateRow]) -> PrintFile {
        let (id, name, kind, printer_model, model_source, slicer, size_bytes, at, by) = row;
        PrintFile {
            id,
            name,
            kind: Kind::parse(&kind).unwrap_or(Kind::Gcode),
            printer_model,
            model_source: match model_source.as_deref() {
                Some("file") => Some(ModelSource::File),
                Some("manual") => Some(ModelSource::Manual),
                _ => None,
            },
            slicer,
            size_bytes,
            uploaded_at: parse_time(&at).unwrap_or_else(now),
            uploaded_by: by,
            plates: plates
                .iter()
                .filter(|p| p.0 == id)
                .map(|(_, number, name, secs, layers, filaments, thumb)| Plate {
                    number: u32::try_from(*number).unwrap_or(1),
                    name: name.clone(),
                    estimated_secs: secs.and_then(|s| u64::try_from(s).ok()),
                    layers: layers.and_then(|l| u32::try_from(l).ok()),
                    filaments: serde_json::from_str(filaments).unwrap_or_default(),
                    has_thumbnail: *thumb,
                })
                .collect(),
        }
    }

    /// The whole library, newest first.
    pub async fn print_files(&self) -> Result<Vec<PrintFile>, sqlx::Error> {
        let rows: Vec<FileRow> = sqlx::query_as(&format!(
            "SELECT {FILE_COLUMNS} FROM print_files ORDER BY uploaded_at DESC, id DESC"
        ))
        .fetch_all(self.pool())
        .await?;
        let plates = self.plates_by_file().await?;
        Ok(rows
            .into_iter()
            .map(|row| Self::print_file_from(row, &plates))
            .collect())
    }

    pub async fn print_file(&self, id: i64) -> Result<Option<PrintFile>, sqlx::Error> {
        let row: Option<FileRow> = sqlx::query_as(&format!(
            "SELECT {FILE_COLUMNS} FROM print_files WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else { return Ok(None) };
        let plates: Vec<PlateRow> = sqlx::query_as(
            "SELECT print_file_id, number, name, estimated_secs, layers, filaments, has_thumbnail \
             FROM plates WHERE print_file_id = ? ORDER BY number",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        Ok(Some(Self::print_file_from(row, &plates)))
    }

    /// Sets (or clears) the model a Print File was sliced for, by hand.
    pub async fn set_print_file_model(
        &self,
        id: i64,
        model: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let done =
            sqlx::query("UPDATE print_files SET printer_model = ?, model_source = ? WHERE id = ?")
                .bind(model)
                .bind(model.map(|_| "manual"))
                .bind(id)
                .execute(self.pool())
                .await?;
        Ok(done.rows_affected() > 0)
    }

    /// The Print Files that Jobs still running are printing.
    pub async fn print_files_in_use(&self) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT DISTINCT print_file_id FROM jobs              WHERE ended_at IS NULL AND print_file_id IS NOT NULL",
        )
        .fetch_all(self.pool())
        .await
    }

    pub async fn delete_print_file(&self, id: i64) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM print_files WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
