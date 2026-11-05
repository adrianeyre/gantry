//! Which Bambu model a Printer is. It decides the camera (P1 and A1 send JPEG
//! frames on port 6000, X1 sends H.264 over RTSPS on port 322, PRD §10.3).
//!
//! The model is read from the serial number's first three characters, as the
//! community integrations do (ha-bambulab's printer-type table), or set by
//! hand in the Printer's settings when the serial isn't recognised. The
//! prefixes are community-documented and unverified against a real printer
//! (#27).

/// A Bambu printer family and model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    X1,
    X1C,
    X1E,
    P1P,
    P1S,
    A1,
    A1Mini,
}

/// Which kind of camera a model has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraKind {
    /// TLS on port 6000 with JPEG frames: P1 and A1.
    Jpeg,
    /// RTSPS on port 322 with H.264: X1.
    Rtsps,
}

impl Model {
    pub const ALL: [Model; 7] = [
        Model::X1,
        Model::X1C,
        Model::X1E,
        Model::P1P,
        Model::P1S,
        Model::A1,
        Model::A1Mini,
    ];

    /// How the UI and the settings name it.
    pub fn name(self) -> &'static str {
        match self {
            Self::X1 => "X1",
            Self::X1C => "X1C",
            Self::X1E => "X1E",
            Self::P1P => "P1P",
            Self::P1S => "P1S",
            Self::A1 => "A1",
            Self::A1Mini => "A1 mini",
        }
    }

    /// A model named in the settings, ignoring case and spaces.
    pub fn parse(value: &str) -> Option<Self> {
        let wanted: String = value
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| {
            m.name()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase()
                == wanted
        })
    }

    /// The model a serial number belongs to, when its prefix is known.
    pub fn from_serial(serial: &str) -> Option<Self> {
        match serial.trim().get(..3)?.to_ascii_uppercase().as_str() {
            "00W" => Some(Self::X1),
            "00M" => Some(Self::X1C),
            "03W" => Some(Self::X1E),
            "01S" => Some(Self::P1P),
            "01P" => Some(Self::P1S),
            "039" => Some(Self::A1),
            "030" => Some(Self::A1Mini),
            _ => None,
        }
    }

    pub fn camera(self) -> CameraKind {
        match self {
            Self::X1 | Self::X1C | Self::X1E => CameraKind::Rtsps,
            Self::P1P | Self::P1S | Self::A1 | Self::A1Mini => CameraKind::Jpeg,
        }
    }
}

/// The model from the settings if given, otherwise from the serial number.
pub fn detect(setting: Option<&str>, serial: &str) -> Option<Model> {
    setting
        .and_then(Model::parse)
        .or_else(|| Model::from_serial(serial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_model_from_the_serial_prefix() {
        assert_eq!(Model::from_serial("00M09A000000000"), Some(Model::X1C));
        assert_eq!(Model::from_serial("01P00A000000000"), Some(Model::P1S));
        assert_eq!(Model::from_serial("030000000000000"), Some(Model::A1Mini));
        assert_eq!(Model::from_serial("ZZZ"), None);
        assert_eq!(Model::from_serial("0"), None);
    }

    #[test]
    fn a_setting_beats_the_serial() {
        assert_eq!(detect(Some("a1 MINI"), "00M000"), Some(Model::A1Mini));
        assert_eq!(detect(Some(""), "00M000"), Some(Model::X1C));
        assert_eq!(detect(None, "nothing known"), None);
    }

    #[test]
    fn x1_has_the_rtsps_camera_and_the_rest_jpeg() {
        assert_eq!(Model::X1C.camera(), CameraKind::Rtsps);
        assert_eq!(Model::P1S.camera(), CameraKind::Jpeg);
        assert_eq!(Model::A1.camera(), CameraKind::Jpeg);
    }
}
