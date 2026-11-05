//! The Print File library (PRD §7.3, §8): sliced files Operators upload,
//! kept in the data directory, with their Plates read into SQLite.
//!
//! Each Print File has a folder, `print-files/<id>/`, holding the file under
//! its own name and one thumbnail per Plate that has one (`plate-N.png` or
//! `plate-N.jpg`).
use std::path::{Path, PathBuf};

pub mod mesh;
pub mod models;
pub mod parse;
pub mod toolpath;
pub mod xml;

/// The longest file name kept, in characters.
const MAX_NAME: usize = 160;

/// A name safe to store and to send to a Printer: the last path component,
/// with control characters and the characters Windows and FTP servers
/// refuse replaced by `_`.
pub fn safe_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.chars().count() <= MAX_NAME {
        return Some(cleaned);
    }
    // Keep the end, where the extension is.
    let skip = cleaned.chars().count() - MAX_NAME;
    Some(cleaned.chars().skip(skip).collect())
}

/// Where the library's files live.
#[derive(Debug, Clone)]
pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("print-files"),
        }
    }

    fn folder(&self, id: i64) -> PathBuf {
        self.dir.join(id.to_string())
    }

    /// The stored file.
    pub fn file(&self, id: i64, name: &str) -> PathBuf {
        self.folder(id).join(name)
    }

    /// Writes a new Print File and its Plates' thumbnails.
    pub async fn save(
        &self,
        id: i64,
        name: &str,
        bytes: &[u8],
        thumbnails: &[(u32, &parse::Thumbnail)],
    ) -> std::io::Result<()> {
        let folder = self.folder(id);
        tokio::fs::create_dir_all(&folder).await?;
        tokio::fs::write(folder.join(name), bytes).await?;
        for (plate, thumbnail) in thumbnails {
            let extension = if thumbnail.content_type == "image/jpeg" {
                "jpg"
            } else {
                "png"
            };
            tokio::fs::write(
                folder.join(format!("plate-{plate}.{extension}")),
                &thumbnail.bytes,
            )
            .await?;
        }
        Ok(())
    }

    /// A Plate's thumbnail and its content type, if it has one.
    pub async fn thumbnail(&self, id: i64, plate: u32) -> Option<(Vec<u8>, &'static str)> {
        let folder = self.folder(id);
        for (extension, content_type) in [("png", "image/png"), ("jpg", "image/jpeg")] {
            if let Ok(bytes) =
                tokio::fs::read(folder.join(format!("plate-{plate}.{extension}"))).await
            {
                return Some((bytes, content_type));
            }
        }
        None
    }

    /// Reads a stored file.
    pub async fn read(&self, id: i64, name: &str) -> std::io::Result<Vec<u8>> {
        tokio::fs::read(self.file(id, name)).await
    }

    /// Removes a Print File's folder. Missing already is fine.
    pub async fn remove(&self, id: i64) -> std::io::Result<()> {
        match tokio::fs::remove_dir_all(self.folder(id)).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// A cache file for something derived from the Print File, such as its
    /// meshes.
    pub fn cache(&self, id: i64, name: &str) -> PathBuf {
        self.folder(id).join(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(
            safe_name("cube.gcode.3mf").as_deref(),
            Some("cube.gcode.3mf")
        );
        assert_eq!(
            safe_name("C:\\Users\\me\\Desktop\\cube.gcode").as_deref(),
            Some("cube.gcode")
        );
        assert_eq!(safe_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(safe_name("a:b?.gcode").as_deref(), Some("a_b_.gcode"));
        assert_eq!(safe_name("..").as_deref(), None);
        assert_eq!(safe_name("   ").as_deref(), None);
        let long = format!("{}.gcode.3mf", "x".repeat(300));
        let safe = safe_name(&long).unwrap();
        assert_eq!(safe.chars().count(), MAX_NAME);
        assert!(safe.ends_with(".gcode.3mf"));
    }
}
