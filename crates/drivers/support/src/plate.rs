//! One Plate's G-code from a Print File. A plain `.gcode` is one Plate; a
//! sliced `.gcode.3mf` is a zip holding `Metadata/plate_N.gcode` for each.
//! Klipper, OctoPrint and PrusaLink print plain G-code, so a Driver for them
//! sends just the Plate's G-code.
use std::io::{Cursor, Read};

use gantry_core::{CommandError, PrintUpload};

/// The largest Plate G-code read out of a `.gcode.3mf`.
const MAX_GCODE: u64 = 1024 * 1024 * 1024;

/// The file to send for `upload`: its name on the printer, and its G-code.
/// A `.gcode.3mf` becomes `<name>-plate<N>.gcode`.
pub fn gcode_for(upload: &PrintUpload) -> Result<(String, Vec<u8>), CommandError> {
    let lower = upload.file_name.to_ascii_lowercase();
    if !lower.ends_with(".3mf") {
        return Ok((upload.file_name.clone(), upload.data.to_vec()));
    }
    let plate = upload.plate.unwrap_or(1);
    let mut zip = zip::ZipArchive::new(Cursor::new(&upload.data[..]))
        .map_err(|_| CommandError::invalid("the Print File isn't a readable .3mf"))?;
    let mut entry = zip
        .by_name(&format!("Metadata/plate_{plate}.gcode"))
        .map_err(|_| {
            CommandError::invalid(format!("the Print File has no G-code for Plate {plate}"))
        })?;
    let mut gcode = Vec::new();
    entry
        .by_ref()
        .take(MAX_GCODE)
        .read_to_end(&mut gcode)
        .map_err(|_| CommandError::invalid("the Plate's G-code couldn't be read"))?;
    let stem = upload
        .file_name
        .trim_end_matches(".3mf")
        .trim_end_matches(".3MF")
        .trim_end_matches(".gcode")
        .trim_end_matches(".GCODE");
    Ok((format!("{stem}-plate{plate}.gcode"), gcode))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn upload(name: &str, data: Vec<u8>, plate: Option<u32>) -> PrintUpload {
        PrintUpload {
            file_name: name.into(),
            plate,
            data: data.into(),
            estimated_secs: None,
            layers: None,
        }
    }

    fn three_mf() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            let options = zip::write::SimpleFileOptions::default();
            for n in [1, 2] {
                zip.start_file(format!("Metadata/plate_{n}.gcode"), options)
                    .unwrap();
                zip.write_all(format!("; plate {n}\nG28\n").as_bytes())
                    .unwrap();
            }
            zip.finish().unwrap();
        }
        out
    }

    #[test]
    fn a_plain_gcode_goes_as_it_is() {
        let (name, gcode) = gcode_for(&upload("cube.gcode", b"G28\n".to_vec(), None)).unwrap();
        assert_eq!(name, "cube.gcode");
        assert_eq!(gcode, b"G28\n");
    }

    #[test]
    fn a_gcode_3mf_sends_only_its_plate() {
        let (name, gcode) = gcode_for(&upload("tray.gcode.3mf", three_mf(), Some(2))).unwrap();
        assert_eq!(name, "tray-plate2.gcode");
        assert_eq!(gcode, b"; plate 2\nG28\n");
        assert!(gcode_for(&upload("tray.gcode.3mf", three_mf(), Some(3))).is_err());
        assert!(gcode_for(&upload("bad.3mf", b"not a zip".to_vec(), Some(1))).is_err());
    }
}
