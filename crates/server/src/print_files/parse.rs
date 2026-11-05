//! Reading a Print File (PRD §8.1): its Plates, the model it was sliced for,
//! the filament each Plate needs, its estimated time and its thumbnails.
//!
//! - A sliced `.gcode.3mf` is a zip. Each printable Plate has
//!   `Metadata/plate_N.gcode` and usually `Metadata/plate_N.png`. Bambu Studio
//!   and OrcaSlicer add `Metadata/slice_info.config` (per Plate: estimate,
//!   filaments, model code) and `Metadata/project_settings.config` (JSON,
//!   with `printer_model`).
//! - A plain `.gcode` is one Plate. Slicers write settings as comments, in
//!   their own spellings (`; printer_model = MK4`, `;TIME:3723`, …), and many
//!   embed base64 thumbnails between `; thumbnail begin` and `; thumbnail end`.
//! - `.stl`, `.obj` and `.step` are models, not Print Files: they need
//!   slicing first.
use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::xml;

/// The two kinds of Print File.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[serde(rename = "gcode_3mf")]
    Gcode3mf,
    Gcode,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gcode3mf => "gcode_3mf",
            Self::Gcode => "gcode",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gcode_3mf" => Some(Self::Gcode3mf),
            "gcode" => Some(Self::Gcode),
            _ => None,
        }
    }
}

/// One filament a Plate uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlateFilament {
    /// The material, such as PLA.
    pub kind: String,
    /// `#RRGGBB`, when the slicer says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// How much of it the Plate uses, in grams, when the slicer says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grams: Option<f64>,
}

/// A Plate's thumbnail image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// One Plate, as read from the file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPlate {
    /// From 1.
    pub number: u32,
    pub name: Option<String>,
    pub estimated_secs: Option<u64>,
    pub layers: Option<u32>,
    pub filaments: Vec<PlateFilament>,
    pub thumbnail: Option<Thumbnail>,
}

/// What a Print File holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub kind: Kind,
    /// The model it was sliced for, as the file names it.
    pub printer_model: Option<String>,
    /// Which slicer made it, such as `PrusaSlicer 2.7.1`.
    pub slicer: Option<String>,
    pub plates: Vec<ParsedPlate>,
}

/// Why a file can't go in the library. The message is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("{0}")]
    NeedsSlicing(String),
    #[error("{0}")]
    NotAPrintFile(String),
    #[error("{0}")]
    Unreadable(String),
}

/// The kind a file name says it is, or why it can't be one.
pub fn kind_for(name: &str) -> Result<Kind, ParseError> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.ends_with(".3mf") {
        return Ok(Kind::Gcode3mf);
    }
    if lower.ends_with(".gcode") || lower.ends_with(".gco") || lower.ends_with(".g") {
        return Ok(Kind::Gcode);
    }
    let extension = lower.rsplit_once('.').map_or("", |(_, e)| e);
    if matches!(extension, "stl" | "obj" | "step" | "stp" | "amf" | "ply") {
        return Err(ParseError::NeedsSlicing(format!(
            "A .{extension} file is a 3D model, not a Print File: it needs slicing first. \
             Slice it in your slicer and upload the .gcode.3mf or .gcode it exports."
        )));
    }
    Err(ParseError::NotAPrintFile(
        "Gantry's library takes sliced .gcode.3mf and .gcode Print Files.".into(),
    ))
}

/// Reads a Print File.
pub fn parse(name: &str, bytes: &[u8]) -> Result<Parsed, ParseError> {
    if bytes.is_empty() {
        return Err(ParseError::Unreadable("The file is empty.".into()));
    }
    match kind_for(name)? {
        Kind::Gcode3mf => parse_3mf(bytes),
        Kind::Gcode => parse_gcode(bytes),
    }
}

type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

fn read_entry(zip: &mut Zip<'_>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let entry = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    entry.take(limit).read_to_end(&mut out).ok()?;
    Some(out)
}

/// The largest Plate G-code read for a toolpath, uncompressed.
pub const MAX_PLATE_GCODE: u64 = 1024 * 1024 * 1024;

/// One Plate's G-code: a plain `.gcode` is its own, a `.gcode.3mf` holds
/// each Plate's as `Metadata/plate_N.gcode`. `None` when there is none.
pub fn plate_gcode(kind: Kind, bytes: &[u8], plate: u32) -> Option<Vec<u8>> {
    match kind {
        Kind::Gcode => (plate == 1).then(|| bytes.to_vec()),
        Kind::Gcode3mf => {
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
            read_entry(
                &mut zip,
                &format!("Metadata/plate_{plate}.gcode"),
                MAX_PLATE_GCODE,
            )
        }
    }
}

/// The Plate number in `Metadata/plate_N.gcode`.
fn plate_gcode_number(entry: &str) -> Option<u32> {
    entry
        .strip_prefix("Metadata/plate_")?
        .strip_suffix(".gcode")?
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

#[derive(Default)]
struct SliceInfoPlate {
    estimated_secs: Option<u64>,
    model_code: Option<String>,
    filaments: Vec<PlateFilament>,
}

/// `Metadata/slice_info.config`, by Plate number.
fn slice_info(text: &str) -> BTreeMap<u32, SliceInfoPlate> {
    let mut plates = BTreeMap::new();
    let mut current: Option<(Option<u32>, SliceInfoPlate)> = None;
    for tag in xml::tags(text) {
        match (tag.name, tag.end) {
            ("plate", false) => current = Some((None, SliceInfoPlate::default())),
            ("plate", true) => {
                if let Some((Some(index), plate)) = current.take() {
                    plates.insert(index, plate);
                }
            }
            ("metadata", false) => {
                if let Some((index, plate)) = current.as_mut() {
                    let value = tag.attr("value").unwrap_or_default();
                    match tag.attr("key").as_deref() {
                        Some("index") => *index = value.trim().parse().ok(),
                        Some("prediction") => {
                            plate.estimated_secs = value
                                .trim()
                                .parse::<f64>()
                                .ok()
                                .filter(|s| *s > 0.0)
                                .map(|s| s.round() as u64);
                        }
                        Some("printer_model_id") if !value.trim().is_empty() => {
                            plate.model_code = Some(value.trim().to_owned());
                        }
                        _ => {}
                    }
                }
            }
            ("filament", false) => {
                if let Some((_, plate)) = current.as_mut()
                    && let Some(kind) = tag.attr("type").filter(|t| !t.trim().is_empty())
                {
                    plate.filaments.push(PlateFilament {
                        kind: kind.trim().to_owned(),
                        colour: tag.attr("color").and_then(|c| colour(&c)),
                        grams: tag.attr("used_g").and_then(|g| grams(&g)),
                    });
                }
            }
            _ => {}
        }
    }
    plates
}

/// Plate names from `Metadata/model_settings.config`.
fn plate_names(text: &str) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    let mut depth_in_plate = false;
    let mut id = None;
    let mut name = None;
    for tag in xml::tags(text) {
        match (tag.name, tag.end) {
            ("plate", false) => {
                depth_in_plate = true;
                id = None;
                name = None;
            }
            ("plate", true) => {
                if let (Some(id), Some(name)) = (id.take(), name.take()) {
                    names.insert(id, name);
                }
                depth_in_plate = false;
            }
            ("metadata", false) if depth_in_plate => {
                let value = tag.attr("value").unwrap_or_default();
                match tag.attr("key").as_deref() {
                    Some("plater_id") => id = value.trim().parse().ok(),
                    Some("plater_name") if !value.trim().is_empty() => {
                        name = Some(value.trim().to_owned());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    names
}

/// `#RRGGBB` from a slicer's colour, which may carry alpha (`#RRGGBBAA`).
fn colour(value: &str) -> Option<String> {
    let hex = value.trim().trim_start_matches('#');
    (hex.len() >= 6 && hex[..6].chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex[..6].to_ascii_uppercase()))
}

fn parse_3mf(bytes: &[u8]) -> Result<Parsed, ParseError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| {
        ParseError::Unreadable("This .3mf can't be opened: it isn't a valid 3MF (zip) file.".into())
    })?;
    let mut numbers: Vec<u32> = zip.file_names().filter_map(plate_gcode_number).collect();
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        return Err(ParseError::NeedsSlicing(
            "This .3mf has no sliced Plates, so it is a project, not a Print File. Slice it, \
             export it as a .gcode.3mf (\"Export plate sliced file\" or \"Export all sliced \
             files\"), and upload that."
                .into(),
        ));
    }
    const SMALL: u64 = 16 * 1024 * 1024;
    let info = read_entry(&mut zip, "Metadata/slice_info.config", SMALL)
        .map(|b| slice_info(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let names = read_entry(&mut zip, "Metadata/model_settings.config", SMALL)
        .map(|b| plate_names(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
    let project: Option<serde_json::Value> =
        read_entry(&mut zip, "Metadata/project_settings.config", SMALL)
            .and_then(|b| serde_json::from_slice(&b).ok());
    let project_text = |key: &str| {
        project
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let project_list = |key: &str| -> Vec<String> {
        project
            .as_ref()
            .and_then(|p| p.get(key))
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut printer_model = project_text("printer_model");
    let mut slicer = None;
    let mut plates = Vec::new();
    for number in numbers {
        let header = read_entry(
            &mut zip,
            &format!("Metadata/plate_{number}.gcode"),
            256 * 1024,
        )
        .map(|b| GcodeHeader::read(&String::from_utf8_lossy(&b)))
        .unwrap_or_default();
        let from_info = info.get(&number);
        if printer_model.is_none() {
            printer_model = header
                .printer_model
                .clone()
                .or_else(|| from_info.and_then(|i| i.model_code.clone()));
        }
        if slicer.is_none() {
            slicer.clone_from(&header.slicer);
        }
        let mut filaments = from_info.map(|i| i.filaments.clone()).unwrap_or_default();
        if filaments.is_empty() {
            filaments = header.filaments();
        }
        let thumbnail = read_entry(&mut zip, &format!("Metadata/plate_{number}.png"), SMALL)
            .filter(|b| b.starts_with(PNG_MAGIC))
            .map(|bytes| Thumbnail {
                bytes,
                content_type: "image/png",
            });
        plates.push(ParsedPlate {
            number,
            name: names.get(&number).cloned(),
            estimated_secs: from_info
                .and_then(|i| i.estimated_secs)
                .or(header.estimated_secs),
            layers: header.layers,
            filaments,
            thumbnail,
        });
    }
    // A project's filament list, when no Plate said which it uses.
    if plates.iter().all(|p| p.filaments.is_empty()) {
        let kinds = project_list("filament_type");
        let colours = project_list("filament_colour");
        let all: Vec<PlateFilament> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| PlateFilament {
                kind: kind.clone(),
                colour: colours.get(i).and_then(|c| colour(c)),
                grams: None,
            })
            .collect();
        for plate in &mut plates {
            plate.filaments.clone_from(&all);
        }
    }
    Ok(Parsed {
        kind: Kind::Gcode3mf,
        printer_model,
        slicer,
        plates,
    })
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const JPEG_MAGIC: &[u8] = b"\xff\xd8\xff";

/// What a G-code file's comments say.
#[derive(Debug, Default, Clone, PartialEq)]
struct GcodeHeader {
    printer_model: Option<String>,
    slicer: Option<String>,
    estimated_secs: Option<u64>,
    layers: Option<u32>,
    filament_kinds: Vec<String>,
    filament_colours: Vec<String>,
    /// Grams of each filament, in the slicer's order.
    filament_grams: Vec<f64>,
    /// The largest embedded thumbnail, with its pixel area.
    thumbnail: Option<(u64, Thumbnail)>,
    has_moves: bool,
}

impl GcodeHeader {
    fn read(text: &str) -> Self {
        let mut header = Self::default();
        header.scan(text);
        header
    }

    fn filaments(&self) -> Vec<PlateFilament> {
        self.filament_kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| PlateFilament {
                kind: kind.clone(),
                colour: self.filament_colours.get(i).and_then(|c| colour(c)),
                grams: self.filament_grams.get(i).copied(),
            })
            .collect()
    }

    fn scan(&mut self, text: &str) {
        let mut lines = text.lines();
        while let Some(raw) = lines.next() {
            let line = raw.trim();
            if line.starts_with("G0") || line.starts_with("G1") || line.starts_with("G28") {
                self.has_moves = true;
                continue;
            }
            let Some(comment) = line.strip_prefix(';') else {
                continue;
            };
            let comment = comment.trim();
            if let Some(format) = thumbnail_begin(comment) {
                let (width, height) = thumbnail_size(comment);
                let mut data = String::new();
                for raw in lines.by_ref() {
                    let body = raw.trim().trim_start_matches(';').trim();
                    if body.starts_with("thumbnail") && body.contains(" end") {
                        break;
                    }
                    data.push_str(body);
                }
                if let Some(thumbnail) = decode_thumbnail(format, &data) {
                    let area = width * height;
                    if self.thumbnail.as_ref().is_none_or(|(a, _)| area > *a) {
                        self.thumbnail = Some((area, thumbnail));
                    }
                }
                continue;
            }
            self.setting(comment);
        }
    }

    /// One `; key = value`, `;KEY:value` or `; key: value` comment.
    fn setting(&mut self, comment: &str) {
        let lower = comment.to_ascii_lowercase();
        let value = |key: &str| -> Option<String> {
            let rest = lower.strip_prefix(key)?;
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
            let at = comment.len() - rest.len();
            Some(comment[at..].trim().to_owned()).filter(|v| !v.is_empty())
        };
        if self.slicer.is_none() {
            if let Some(rest) = lower.strip_prefix("generated by ") {
                let at = comment.len() - rest.len();
                let name = comment[at..]
                    .split(" on ")
                    .next()
                    .unwrap_or_default()
                    .trim();
                self.slicer = Some(name.to_owned()).filter(|n| !n.is_empty());
            } else if let Some(rest) = lower.strip_prefix("generated with ") {
                let at = comment.len() - rest.len();
                self.slicer = Some(comment[at..].trim().to_owned());
            } else if lower.starts_with("bambustudio ") || lower.starts_with("orcaslicer ") {
                self.slicer = Some(comment.trim().to_owned());
            }
        }
        if self.printer_model.is_none()
            && let Some(model) = value("printer_model").or_else(|| value("target_machine.name"))
        {
            self.printer_model = Some(model);
        }
        if self.estimated_secs.is_none() {
            if let Some(secs) = value("time").and_then(|v| v.parse::<f64>().ok()) {
                self.estimated_secs = Some(secs.round() as u64);
            } else if let Some(v) = value("estimated printing time (normal mode)") {
                self.estimated_secs = duration(&v);
            } else if let Some(at) = lower.find("total estimated time") {
                let rest = &comment[at + "total estimated time".len()..];
                let rest = rest.trim_start_matches([':', '=', ' ']);
                self.estimated_secs = duration(rest.split(';').next().unwrap_or(rest));
            }
        }
        if self.layers.is_none()
            && let Some(layers) = value("total layer number")
                .or_else(|| value("layer_count"))
                .or_else(|| value("total layers count"))
                .and_then(|v| v.parse().ok())
        {
            self.layers = Some(layers);
        }
        if self.filament_kinds.is_empty()
            && let Some(kinds) = value("filament_type")
        {
            self.filament_kinds = split_list(&kinds);
        }
        if self.filament_colours.is_empty()
            && let Some(colours) = value("filament_colour")
        {
            self.filament_colours = split_list(&colours);
        }
        // PrusaSlicer, OrcaSlicer and Bambu Studio: `; filament used [g] =
        // 12.3, 4.5`, one per filament. Cura only gives a length.
        if self.filament_grams.is_empty()
            && let Some(list) = value("filament used [g]")
        {
            self.filament_grams = split_list(&list).iter().filter_map(|g| grams(g)).collect();
        }
    }
}

/// A weight in grams, when it is one and more than nothing.
fn grams(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|g| g.is_finite() && *g > 0.0)
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split([';', ','])
        .map(|v| v.trim().trim_matches('"').to_owned())
        .filter(|v| !v.is_empty())
        .collect()
}

/// `PNG`, `JPG` or `QOI` for a thumbnail block's first line.
fn thumbnail_begin(comment: &str) -> Option<&'static str> {
    let lower = comment.to_ascii_lowercase();
    if !lower.contains(" begin") {
        return None;
    }
    match lower.split_whitespace().next()? {
        "thumbnail" | "thumbnail_png" => Some("PNG"),
        "thumbnail_jpg" => Some("JPG"),
        "thumbnail_qoi" => Some("QOI"),
        _ => None,
    }
}

/// `300x300` in `thumbnail begin 300x300 12345`.
fn thumbnail_size(comment: &str) -> (u64, u64) {
    comment
        .split_whitespace()
        .find_map(|w| {
            let (a, b) = w.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((1, 1))
}

fn decode_thumbnail(format: &str, data: &str) -> Option<Thumbnail> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .ok()?;
    match format {
        "PNG" if bytes.starts_with(PNG_MAGIC) => Some(Thumbnail {
            bytes,
            content_type: "image/png",
        }),
        "JPG" if bytes.starts_with(JPEG_MAGIC) => Some(Thumbnail {
            bytes,
            content_type: "image/jpeg",
        }),
        // QOI isn't something a browser can show.
        _ => None,
    }
}

/// `1d 2h 3m 4s`, `33m 52s` or `2h 3m`, in seconds.
fn duration(value: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut any = false;
    for part in value.split_whitespace() {
        let (number, unit) = part.split_at(part.find(|c: char| !c.is_ascii_digit())?);
        let n: u64 = number.parse().ok()?;
        total += n * match unit {
            "d" => 86_400,
            "h" => 3_600,
            "m" => 60,
            "s" => 1,
            _ => return None,
        };
        any = true;
    }
    any.then_some(total)
}

/// How much of each end of a plain `.gcode` is searched for settings.
/// PrusaSlicer writes its settings at the end, most others at the start.
const ENDS: usize = 2 * 1024 * 1024;

fn parse_gcode(bytes: &[u8]) -> Result<Parsed, ParseError> {
    let mut header = GcodeHeader::default();
    if bytes.len() <= 2 * ENDS {
        header.scan(&String::from_utf8_lossy(bytes));
    } else {
        header.scan(&String::from_utf8_lossy(&bytes[..ENDS]));
        let tail = &bytes[bytes.len() - ENDS..];
        // Start at a line, not mid-way through one.
        let from = tail.iter().position(|b| *b == b'\n').map_or(0, |i| i + 1);
        header.scan(&String::from_utf8_lossy(&tail[from..]));
        // The moves are in the middle of a big file.
        header.has_moves = true;
    }
    if !header.has_moves {
        return Err(ParseError::Unreadable(
            "This doesn't look like G-code: it has no moves (G0 or G1).".into(),
        ));
    }
    let filaments = header.filaments();
    Ok(Parsed {
        kind: Kind::Gcode,
        printer_model: header.printer_model,
        slicer: header.slicer,
        plates: vec![ParsedPlate {
            number: 1,
            name: None,
            estimated_secs: header.estimated_secs,
            layers: header.layers,
            filaments,
            thumbnail: header.thumbnail.map(|(_, t)| t),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gantry_print_file_fixtures as fixtures;

    #[test]
    fn a_multi_plate_gcode_3mf_has_one_plate_per_sliced_plate() {
        let file = fixtures::bambu_3mf(&fixtures::three_plates());
        let parsed = parse("tray.gcode.3mf", &file).unwrap();
        assert_eq!(parsed.kind, Kind::Gcode3mf);
        assert_eq!(parsed.plates.len(), 3);
        assert_eq!(parsed.printer_model.as_deref(), Some("Bambu Lab X1 Carbon"));
        assert_eq!(parsed.slicer.as_deref(), Some("BambuStudio 01.09.00.70"));
        let first = &parsed.plates[0];
        assert_eq!(first.number, 1);
        assert_eq!(first.name.as_deref(), Some("Cubes"));
        assert_eq!(first.estimated_secs, Some(1200));
        assert_eq!(first.layers, Some(50));
        assert_eq!(
            first.filaments,
            [PlateFilament {
                kind: "PLA".into(),
                colour: Some("#FFFFFF".into()),
                grams: Some(11.8),
            }]
        );
        assert_eq!(first.thumbnail.as_ref().unwrap().content_type, "image/png");
        assert_eq!(parsed.plates[1].filaments.len(), 2, "PLA and PETG");
        assert!(parsed.plates[2].thumbnail.is_none(), "plate 3 has none");
    }

    #[test]
    fn the_model_falls_back_to_the_slice_info_code() {
        let mut plates = fixtures::three_plates();
        plates.truncate(1);
        let file = fixtures::bambu_3mf_with(
            &plates,
            fixtures::Options {
                project_settings: false,
                ..Default::default()
            },
        );
        let parsed = parse("cube.3mf", &file).unwrap();
        assert_eq!(parsed.printer_model.as_deref(), Some("Bambu Lab X1 Carbon"));
        assert_eq!(parsed.plates.len(), 1);
    }

    #[test]
    fn an_unsliced_3mf_needs_slicing() {
        let err = parse("model.3mf", &fixtures::unsliced_3mf()).unwrap_err();
        assert!(matches!(err, ParseError::NeedsSlicing(_)), "{err}");
        assert!(err.to_string().contains("Slice it"), "{err}");
    }

    #[test]
    fn models_are_refused_with_a_clear_message() {
        for name in ["part.stl", "part.STL", "part.obj", "part.step", "part.stp"] {
            let err = parse(name, b"solid x").unwrap_err();
            assert!(matches!(err, ParseError::NeedsSlicing(_)), "{name}: {err}");
            assert!(err.to_string().contains("needs slicing first"), "{err}");
        }
        assert!(matches!(
            parse("notes.txt", b"hello").unwrap_err(),
            ParseError::NotAPrintFile(_)
        ));
        assert!(matches!(
            parse("broken.gcode.3mf", b"not a zip").unwrap_err(),
            ParseError::Unreadable(_)
        ));
    }

    #[test]
    fn a_prusaslicer_gcode_is_one_plate_with_its_settings_and_thumbnail() {
        let parsed = parse("benchy.gcode", &fixtures::prusaslicer_gcode()).unwrap();
        assert_eq!(parsed.kind, Kind::Gcode);
        assert_eq!(parsed.plates.len(), 1);
        assert_eq!(parsed.slicer.as_deref(), Some("PrusaSlicer 2.7.1"));
        assert_eq!(parsed.printer_model.as_deref(), Some("MK4"));
        let plate = &parsed.plates[0];
        assert_eq!(plate.estimated_secs, Some(3600 + 2 * 60 + 3));
        assert_eq!(
            plate.filaments,
            [PlateFilament {
                kind: "PETG".into(),
                colour: Some("#FF8000".into()),
                grams: Some(37.21),
            }]
        );
        // The bigger of its two thumbnails.
        let thumbnail = plate.thumbnail.as_ref().unwrap();
        assert_eq!(thumbnail.content_type, "image/png");
        assert_eq!(fixtures::png_size(&thumbnail.bytes), Some((16, 16)));
    }

    #[test]
    fn a_cura_gcode_is_read_too() {
        let parsed = parse("cura.gcode", &fixtures::cura_gcode()).unwrap();
        assert_eq!(parsed.slicer.as_deref(), Some("Cura_SteamEngine 5.6.0"));
        assert_eq!(parsed.printer_model.as_deref(), Some("Creality Ender-3"));
        assert_eq!(parsed.plates[0].estimated_secs, Some(3723));
        assert_eq!(parsed.plates[0].layers, Some(42));
        assert!(parsed.plates[0].thumbnail.is_some());
    }

    #[test]
    fn a_gcode_without_a_model_or_thumbnail_is_still_a_print_file() {
        let parsed = parse("bare.gcode", b"G28\nG1 X10 Y10\n").unwrap();
        assert_eq!(parsed.printer_model, None);
        assert_eq!(parsed.plates[0].thumbnail, None);
        assert!(parse("empty.gcode", b"; nothing\n").is_err());
        assert!(parse("empty.gcode", b"").is_err());
    }

    #[test]
    fn durations_parse() {
        assert_eq!(duration("1d 2h 3m 4s"), Some(93_784));
        assert_eq!(duration("33m 52s"), Some(2032));
        assert_eq!(duration("soon"), None);
        assert_eq!(duration(""), None);
    }
}
