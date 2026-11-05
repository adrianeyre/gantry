//! Test support: small Print Files, generated in code (PRD §8.1).
//!
//! Each imitates what a real slicer writes, cut down to what Gantry reads:
//! a Bambu Studio style multi-Plate `.gcode.3mf` (with thumbnails, slice
//! info, project settings and meshes), an unsliced `.3mf` project, and plain
//! `.gcode` from PrusaSlicer and Cura with their own comment styles and
//! embedded thumbnails. Everything is deterministic, so the copies written to
//! `web/e2e/fixtures` for Playwright can be checked against these.
use std::io::{Cursor, Write};

use base64::Engine;
use zip::write::SimpleFileOptions;

/// One Plate of a generated `.gcode.3mf`.
#[derive(Debug, Clone)]
pub struct PlateSpec {
    pub name: &'static str,
    pub estimated_secs: u64,
    pub layers: u32,
    /// Material and `#RRGGBB` colour of each filament it uses.
    pub filaments: Vec<(&'static str, &'static str)>,
    pub thumbnail: bool,
}

/// Three Plates: one PLA, one PLA and PETG, one without a thumbnail.
pub fn three_plates() -> Vec<PlateSpec> {
    vec![
        PlateSpec {
            name: "Cubes",
            estimated_secs: 1200,
            layers: 50,
            filaments: vec![("PLA", "#FFFFFF")],
            thumbnail: true,
        },
        PlateSpec {
            name: "Two colours",
            estimated_secs: 2400,
            layers: 80,
            filaments: vec![("PLA", "#FFFFFF"), ("PETG", "#D33A2C")],
            thumbnail: true,
        },
        PlateSpec {
            name: "Brackets",
            estimated_secs: 600,
            layers: 20,
            filaments: vec![("PLA", "#1A1A1A")],
            thumbnail: false,
        },
    ]
}

/// What else goes in a generated `.gcode.3mf`.
#[derive(Debug, Clone)]
pub struct Options {
    /// `Metadata/project_settings.config`, which names the model.
    pub project_settings: bool,
    /// The model the file was sliced for, as Bambu Studio names it.
    pub printer_model: &'static str,
    /// Its model code in `slice_info.config`.
    pub model_code: &'static str,
    /// The meshes: a cube (filament 1) inline, and a pyramid (filament 2)
    /// in its own file, referenced as Bambu Studio does.
    pub meshes: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            project_settings: true,
            printer_model: "Bambu Lab X1 Carbon",
            model_code: "BL-P001",
            meshes: true,
        }
    }
}

/// A Bambu Studio style `.gcode.3mf` with these Plates.
pub fn bambu_3mf(plates: &[PlateSpec]) -> Vec<u8> {
    bambu_3mf_with(plates, Options::default())
}

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// [`bambu_3mf`] with other [`Options`].
pub fn bambu_3mf_with(plates: &[PlateSpec], options: Options) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut add = |name: &str, bytes: &[u8]| {
        zip.start_file(name, self::options()).unwrap();
        zip.write_all(bytes).unwrap();
    };
    add("[Content_Types].xml", CONTENT_TYPES.as_bytes());
    add("_rels/.rels", RELS.as_bytes());
    if options.meshes {
        add("3D/3dmodel.model", MAIN_MODEL.as_bytes());
        add("3D/Objects/object_2.model", &pyramid_model());
    } else {
        add("3D/3dmodel.model", EMPTY_MODEL.as_bytes());
    }
    let mut slice_info = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n  <header>\n    \
         <header_item key=\"X-BBL-Client-Type\" value=\"slicer\"/>\n    \
         <header_item key=\"X-BBL-Client-Version\" value=\"01.09.00.70\"/>\n  </header>\n",
    );
    let mut model_settings = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n  <object id=\"1\">\n    \
         <metadata key=\"name\" value=\"Cube\"/>\n    <metadata key=\"extruder\" value=\"1\"/>\n  \
         </object>\n  <object id=\"3\">\n    <metadata key=\"name\" value=\"Pyramid\"/>\n    \
         <metadata key=\"extruder\" value=\"2\"/>\n  </object>\n",
    );
    for (i, plate) in plates.iter().enumerate() {
        let n = i + 1;
        let gcode = format!(
            "; HEADER_BLOCK_START\n; BambuStudio 01.09.00.70\n; model printing time: 1m; \
             total estimated time: {}s\n; total layer number: {}\n; HEADER_BLOCK_END\n\n\
             ; printer_model = {}\nG28\nG1 X10 Y10 Z0.2 F3000\nG1 X20 Y10 E1\n",
            plate.estimated_secs, plate.layers, options.printer_model
        );
        add(&format!("Metadata/plate_{n}.gcode"), gcode.as_bytes());
        add(
            &format!("Metadata/plate_{n}.gcode.md5"),
            b"00000000000000000000000000000000",
        );
        if plate.thumbnail {
            add(
                &format!("Metadata/plate_{n}.png"),
                &png(32, 32, [0x2E, 0x7D, 0x32]),
            );
        }
        slice_info.push_str(&format!(
            "  <plate>\n    <metadata key=\"index\" value=\"{n}\"/>\n    \
             <metadata key=\"printer_model_id\" value=\"{}\"/>\n    \
             <metadata key=\"prediction\" value=\"{}\"/>\n    \
             <metadata key=\"weight\" value=\"12.50\"/>\n",
            options.model_code, plate.estimated_secs
        ));
        for (id, (kind, colour)) in plate.filaments.iter().enumerate() {
            slice_info.push_str(&format!(
                "    <filament id=\"{}\" type=\"{kind}\" color=\"{colour}\" used_m=\"3.9\" \
                 used_g=\"11.8\"/>\n",
                id + 1
            ));
        }
        slice_info.push_str("  </plate>\n");
        model_settings.push_str(&format!(
            "  <plate>\n    <metadata key=\"plater_id\" value=\"{n}\"/>\n    \
             <metadata key=\"plater_name\" value=\"{}\"/>\n  </plate>\n",
            plate.name
        ));
    }
    slice_info.push_str("</config>\n");
    model_settings.push_str("</config>\n");
    add("Metadata/slice_info.config", slice_info.as_bytes());
    add("Metadata/model_settings.config", model_settings.as_bytes());
    if options.project_settings {
        let settings = serde_json::json!({
            "printer_model": options.printer_model,
            "printer_settings_id": format!("{} 0.4 nozzle", options.printer_model),
            "filament_type": ["PLA", "PETG"],
            "filament_colour": ["#FFFFFF", "#D33A2C"],
        });
        add(
            "Metadata/project_settings.config",
            serde_json::to_string_pretty(&settings).unwrap().as_bytes(),
        );
    }
    zip.finish().unwrap().into_inner()
}

/// A `.3mf` project that hasn't been sliced: meshes, no Plate G-code.
pub fn unsliced_3mf() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
        ("_rels/.rels", RELS.as_bytes().to_vec()),
        ("3D/3dmodel.model", MAIN_MODEL.as_bytes().to_vec()),
        ("3D/Objects/object_2.model", pyramid_model()),
    ] {
        zip.start_file(name, options()).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A `.gcode.3mf` with one Plate whose mesh is a grid of `n` × `n` small
/// cubes: `12 n²` triangles, for testing that big models are simplified.
pub fn big_mesh_3mf(n: usize) -> Vec<u8> {
    let mut vertices = String::new();
    let mut triangles = String::new();
    let mut count = 0;
    for i in 0..n {
        for j in 0..n {
            let (x, y) = (i as f32 * 2.0, j as f32 * 2.0);
            let base = count;
            for (dx, dy, dz) in CUBE_CORNERS {
                vertices.push_str(&format!(
                    "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                    x + dx,
                    y + dy,
                    dz
                ));
            }
            count += 8;
            for (a, b, c) in CUBE_TRIANGLES {
                triangles.push_str(&format!(
                    "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                    base + a,
                    base + b,
                    base + c
                ));
            }
        }
    }
    let model = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
         xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n<resources>\n\
         <object id=\"1\" type=\"model\"><mesh><vertices>{vertices}</vertices>\
         <triangles>{triangles}</triangles></mesh></object>\n</resources>\n\
         <build><item objectid=\"1\"/></build>\n</model>\n"
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in [
        ("3D/3dmodel.model", model.into_bytes()),
        (
            "Metadata/plate_1.gcode",
            b"; printer_model = Bambu Lab X1 Carbon\nG28\nG1 X1\n".to_vec(),
        ),
    ] {
        zip.start_file(name, options()).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A PrusaSlicer `.gcode`: thumbnails at the start (a small and a bigger
/// one, `thumbnail_PNG` style), settings in a block at the end.
pub fn prusaslicer_gcode() -> Vec<u8> {
    let mut out =
        String::from("; generated by PrusaSlicer 2.7.1 on 2026-01-01 at 12:00:00 UTC\n\n");
    out.push_str(&thumbnail_block(
        "thumbnail_PNG",
        8,
        8,
        &png(8, 8, [0xFF, 0x80, 0x00]),
    ));
    out.push_str(&thumbnail_block(
        "thumbnail_PNG",
        16,
        16,
        &png(16, 16, [0xFF, 0x80, 0x00]),
    ));
    out.push_str(
        "\nM73 P0 R62\nG28 ; home\nG1 Z0.2 F720\nG1 X10 Y10 E2 F1500\n; LAYER_CHANGE\n\
         G1 X20 Y20 E3\n\n; filament used [mm] = 1234.56\n; filament used [g] = 37.21\n\
         ; estimated printing time (normal mode) = 1h 2m 3s\n\n\
         ; prusaslicer_config = begin\n; filament_colour = #FF8000\n; filament_type = PETG\n\
         ; printer_model = MK4\n; printer_settings_id = Original Prusa MK4 0.4 nozzle\n\
         ; prusaslicer_config = end\n",
    );
    out.into_bytes()
}

/// A Cura `.gcode`: `;KEY:value` settings at the start and a `thumbnail`
/// block (as the Cura thumbnail plugin writes).
pub fn cura_gcode() -> Vec<u8> {
    let mut out = String::from(
        ";FLAVOR:Marlin\n;TIME:3723\n;Filament used: 1.2m\n;Layer height: 0.2\n\
         ;TARGET_MACHINE.NAME:Creality Ender-3\n;Generated with Cura_SteamEngine 5.6.0\n\
         ;LAYER_COUNT:42\n",
    );
    out.push_str(&thumbnail_block(
        "thumbnail",
        12,
        12,
        &png(12, 12, [0x22, 0x55, 0xDD]),
    ));
    out.push_str("G28 ;Home\nG1 Z15.0 F6000\n;LAYER:0\nG1 X10 Y10 E1\n");
    out.into_bytes()
}

fn thumbnail_block(kind: &str, w: u32, h: u32, png: &[u8]) -> String {
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let mut block = format!(";\n; {kind} begin {w}x{h} {}\n", data.len());
    for chunk in data.as_bytes().chunks(78) {
        block.push_str("; ");
        block.push_str(std::str::from_utf8(chunk).unwrap());
        block.push('\n');
    }
    block.push_str(&format!("; {kind} end\n;\n"));
    block
}

/// A solid-colour PNG.
pub fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    // Each row: filter type 0, then the pixels.
    let row: Vec<u8> = std::iter::once(0)
        .chain(rgb.repeat(width as usize))
        .collect();
    let raw = row.repeat(height as usize);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw).unwrap();
    let data = encoder.finish().unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(body);
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    };
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(b"IDAT", &data);
    chunk(b"IEND", &[]);
    out
}

/// A PNG's width and height.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(16..24)?;
    Some((
        u32::from_be_bytes(header[..4].try_into().ok()?),
        u32::from_be_bytes(header[4..].try_into().ok()?),
    ))
}

/// The files written to `web/e2e/fixtures` for the Playwright tests, by name.
pub fn e2e_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("three-plates.gcode.3mf", bambu_3mf(&three_plates())),
        ("unsliced.3mf", unsliced_3mf()),
        ("benchy-prusaslicer.gcode", prusaslicer_gcode()),
        ("part.stl", b"solid part\nendsolid part\n".to_vec()),
    ]
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types \
    xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
    <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
    <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\
    <Default Extension=\"png\" ContentType=\"image/png\"/>\
    <Default Extension=\"gcode\" ContentType=\"text/x.gcode\"/></Types>\n";

const RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships \
    xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
    <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel-1\" \
    Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>\n";

const CUBE_CORNERS: [(f32, f32, f32); 8] = [
    (0.0, 0.0, 0.0),
    (1.0, 0.0, 0.0),
    (1.0, 1.0, 0.0),
    (0.0, 1.0, 0.0),
    (0.0, 0.0, 1.0),
    (1.0, 0.0, 1.0),
    (1.0, 1.0, 1.0),
    (0.0, 1.0, 1.0),
];

const CUBE_TRIANGLES: [(usize, usize, usize); 12] = [
    (0, 2, 1),
    (0, 3, 2),
    (4, 5, 6),
    (4, 6, 7),
    (0, 1, 5),
    (0, 5, 4),
    (1, 2, 6),
    (1, 6, 5),
    (2, 3, 7),
    (2, 7, 6),
    (3, 0, 4),
    (3, 4, 7),
];

/// The main model: a 10 mm cube (object 1) inline, and object 3, whose mesh
/// is a pyramid in `3D/Objects/object_2.model`, placed 20 mm along X.
const MAIN_MODEL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
    xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\" \
    xmlns:p=\"http://schemas.microsoft.com/3dmanufacturing/production/2015/06\" \
    requiredextensions=\"p\">\n <resources>\n  <object id=\"1\" type=\"model\" name=\"Cube\">\n   \
    <mesh>\n    <vertices>\n     <vertex x=\"0\" y=\"0\" z=\"0\"/>\n     <vertex x=\"10\" y=\"0\" z=\"0\"/>\n     \
    <vertex x=\"10\" y=\"10\" z=\"0\"/>\n     <vertex x=\"0\" y=\"10\" z=\"0\"/>\n     \
    <vertex x=\"0\" y=\"0\" z=\"10\"/>\n     <vertex x=\"10\" y=\"0\" z=\"10\"/>\n     \
    <vertex x=\"10\" y=\"10\" z=\"10\"/>\n     <vertex x=\"0\" y=\"10\" z=\"10\"/>\n    </vertices>\n    \
    <triangles>\n     <triangle v1=\"0\" v2=\"2\" v3=\"1\"/>\n     <triangle v1=\"0\" v2=\"3\" v3=\"2\"/>\n     \
    <triangle v1=\"4\" v2=\"5\" v3=\"6\"/>\n     <triangle v1=\"4\" v2=\"6\" v3=\"7\"/>\n     \
    <triangle v1=\"0\" v2=\"1\" v3=\"5\"/>\n     <triangle v1=\"0\" v2=\"5\" v3=\"4\"/>\n     \
    <triangle v1=\"1\" v2=\"2\" v3=\"6\"/>\n     <triangle v1=\"1\" v2=\"6\" v3=\"5\"/>\n     \
    <triangle v1=\"2\" v2=\"3\" v3=\"7\"/>\n     <triangle v1=\"2\" v2=\"7\" v3=\"6\"/>\n     \
    <triangle v1=\"3\" v2=\"0\" v3=\"4\"/>\n     <triangle v1=\"3\" v2=\"4\" v3=\"7\"/>\n    </triangles>\n   \
    </mesh>\n  </object>\n  <object id=\"3\" type=\"model\" name=\"Pyramid\">\n   <components>\n    \
    <component p:path=\"/3D/Objects/object_2.model\" objectid=\"2\" \
    transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/>\n   </components>\n  </object>\n </resources>\n <build>\n  \
    <item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\" printable=\"1\"/>\n  \
    <item objectid=\"3\" transform=\"1 0 0 0 1 0 0 0 1 20 0 0\" printable=\"1\"/>\n </build>\n</model>\n";

const EMPTY_MODEL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
    xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"><resources/><build/></model>\n";

/// A square pyramid, 10 mm across and 8 mm high, as object 2.
fn pyramid_model() -> Vec<u8> {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" \
     xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n <resources>\n  \
     <object id=\"2\" type=\"model\">\n   <mesh>\n    <vertices>\n     \
     <vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"10\" y=\"0\" z=\"0\"/>\
     <vertex x=\"10\" y=\"10\" z=\"0\"/><vertex x=\"0\" y=\"10\" z=\"0\"/>\
     <vertex x=\"5\" y=\"5\" z=\"8\"/>\n    </vertices>\n    <triangles>\n     \
     <triangle v1=\"0\" v2=\"2\" v3=\"1\"/><triangle v1=\"0\" v2=\"3\" v3=\"2\"/>\
     <triangle v1=\"0\" v2=\"1\" v3=\"4\"/><triangle v1=\"1\" v2=\"2\" v3=\"4\"/>\
     <triangle v1=\"2\" v2=\"3\" v3=\"4\"/><triangle v1=\"3\" v2=\"0\" v3=\"4\"/>\n    \
     </triangles>\n   </mesh>\n  </object>\n </resources>\n <build/>\n</model>\n"
        .as_bytes()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_files_are_deterministic() {
        assert_eq!(bambu_3mf(&three_plates()), bambu_3mf(&three_plates()));
        assert_eq!(png_size(&png(3, 5, [0, 0, 0])), Some((3, 5)));
    }

    /// The copies Playwright uploads must match what these functions make.
    /// Regenerate them with
    /// `cargo run -p gantry-print-file-fixtures --bin write-e2e-fixtures`.
    #[test]
    fn the_e2e_fixtures_are_up_to_date() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../web/e2e/fixtures");
        for (name, bytes) in e2e_files() {
            let on_disk = std::fs::read(dir.join(name))
                .unwrap_or_else(|_| panic!("web/e2e/fixtures/{name} is missing: regenerate it"));
            assert!(
                on_disk == bytes,
                "web/e2e/fixtures/{name} is stale: regenerate it"
            );
        }
    }
}
