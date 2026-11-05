//! What the desktop app remembers between runs: which mode it runs in, and
//! for a remote server, its address. A small JSON file in the app's config
//! folder. It never holds a password or the secret key.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file, in the app's config folder.
pub const FILE_NAME: &str = "desktop.json";

/// How the desktop app reaches Gantry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Mode {
    /// A client of a Gantry server running elsewhere, at this address.
    Remote { server: String },
    /// The core and server run inside the app, for one machine.
    Embedded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopConfig {
    /// `None` until the person has chosen.
    #[serde(default)]
    pub mode: Option<Mode>,
    /// The port the embedded server last listened on, tried first next time
    /// so the address (and so the login) stays the same.
    #[serde(default)]
    pub embedded_port: Option<u16>,
}

/// Why the config could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum DesktopConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("could not save {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl DesktopConfig {
    /// Reads the config from `dir`. No file yet means nothing chosen.
    pub fn load(dir: &Path) -> Result<Self, DesktopConfigError> {
        let path = dir.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(DesktopConfigError::Read { path, source }),
        };
        serde_json::from_str(&text).map_err(|source| DesktopConfigError::Parse { path, source })
    }

    /// Writes the config to `dir`, creating it, through a temporary file so
    /// a crash never leaves half a file.
    pub fn save(&self, dir: &Path) -> Result<(), DesktopConfigError> {
        let path = dir.join(FILE_NAME);
        let write = |source| DesktopConfigError::Write {
            path: path.clone(),
            source,
        };
        std::fs::create_dir_all(dir).map_err(write)?;
        let text = serde_json::to_string_pretty(self).expect("the config serialises");
        let temporary = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&temporary, text).map_err(write)?;
        std::fs::rename(&temporary, &path).map_err(write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_chosen_before_the_first_save() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            DesktopConfig::load(dir.path()).unwrap(),
            DesktopConfig::default()
        );
    }

    #[test]
    fn each_mode_survives_a_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("not/yet/there");
        for config in [
            DesktopConfig {
                mode: Some(Mode::Remote {
                    server: "http://pi:8080".into(),
                }),
                embedded_port: None,
            },
            DesktopConfig {
                mode: Some(Mode::Embedded),
                embedded_port: Some(51234),
            },
        ] {
            config.save(&nested).unwrap();
            assert_eq!(DesktopConfig::load(&nested).unwrap(), config);
        }
        assert!(!nested.join(format!("{FILE_NAME}.tmp")).exists());
    }

    #[test]
    fn the_file_format_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            r#"{ "mode": { "kind": "remote", "server": "https://gantry.example.com" } }"#,
        )
        .unwrap();
        assert_eq!(
            DesktopConfig::load(dir.path()).unwrap().mode,
            Some(Mode::Remote {
                server: "https://gantry.example.com".into()
            })
        );
    }

    #[test]
    fn a_damaged_file_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        let err = DesktopConfig::load(dir.path()).unwrap_err();
        assert!(matches!(err, DesktopConfigError::Parse { .. }));
        assert!(err.to_string().contains(FILE_NAME), "{err}");
    }
}
