//! The meshes inside a `.3mf` (PRD §8.2, level 2), for the 3D viewer.
//!
//! A 3MF's `3D/3dmodel.model` lists objects, each a mesh (vertices and
//! triangles) or a set of components that point at other objects, possibly
//! in other files (`p:path`, as Bambu Studio and OrcaSlicer write them). The
//! `<build>` places objects with 4×3 transforms. This reads all of it into
//! one flat, transformed mesh per placed object, coloured by the filament its
//! slicer assigned it (`Metadata/model_settings.config`), and simplifies big
//! models so a browser stays responsive.
use std::collections::HashMap;
use std::io::{Cursor, Read};

use base64::Engine;
use serde::Serialize;

use super::xml;

/// More triangles than this, across the whole model, and it is simplified.
pub const MAX_TRIANGLES: usize = 200_000;

/// Refuse to read more than this many triangles at all.
const HARD_LIMIT: usize = 20_000_000;

/// The largest model file read, uncompressed.
const MAX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;

/// One placed object, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub name: Option<String>,
    /// The slicer's filament number for it, from 1.
    pub filament: Option<u32>,
    /// That filament's colour, `#RRGGBB`.
    pub colour: Option<String>,
    /// x, y, z for each vertex, in mm.
    pub positions: Vec<f32>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
}

impl Object {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Every placed object in a Print File.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub objects: Vec<Object>,
    /// How many triangles the file had, before any simplifying.
    pub original_triangles: usize,
}

impl Model {
    pub fn triangles(&self) -> usize {
        self.objects.iter().map(Object::triangles).sum()
    }

    pub fn simplified(&self) -> bool {
        self.triangles() < self.original_triangles
    }
}

/// Why a model couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeshError {
    #[error("this Print File has no 3D model in it")]
    NoModel,
    #[error("the 3D model can't be read: {0}")]
    Invalid(String),
    #[error("the 3D model is too big to show ({0} triangles)")]
    TooBig(usize),
}

/// A 4×3 affine transform, as 3MF writes it (row vectors: p' = p·M).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transform([f32; 12]);

impl Transform {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);

    fn parse(value: Option<String>) -> Self {
        let Some(value) = value else {
            return Self::IDENTITY;
        };
        let numbers: Vec<f32> = value
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        match <[f32; 12]>::try_from(numbers) {
            Ok(m) => Self(m),
            Err(_) => Self::IDENTITY,
        }
    }

    fn apply(&self, [x, y, z]: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            x * m[0] + y * m[3] + z * m[6] + m[9],
            x * m[1] + y * m[4] + z * m[7] + m[10],
            x * m[2] + y * m[5] + z * m[8] + m[11],
        ]
    }

    /// `self` first, then `outer`.
    fn then(&self, outer: &Self) -> Self {
        let a = &self.0;
        let b = &outer.0;
        let mut m = [0.0; 12];
        for row in 0..3 {
            for col in 0..3 {
                m[row * 3 + col] = (0..3).map(|k| a[row * 3 + k] * b[k * 3 + col]).sum();
            }
        }
        for col in 0..3 {
            m[9 + col] = (0..3).map(|k| a[9 + k] * b[k * 3 + col]).sum::<f32>() + b[9 + col];
        }
        Self(m)
    }
}

#[derive(Debug, Clone)]
enum Definition {
    Mesh {
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
        name: Option<String>,
    },
    Components {
        parts: Vec<(Option<String>, u32, Transform)>,
        name: Option<String>,
    },
}

/// One model file's objects, and its build items.
#[derive(Debug, Default)]
struct ModelFile {
    objects: HashMap<u32, Definition>,
    build: Vec<(u32, Transform)>,
}

fn read_model_file(text: &str, budget: &mut usize) -> Result<ModelFile, MeshError> {
    let mut file = ModelFile::default();
    let mut current: Option<(u32, Definition)> = None;
    for tag in xml::tags(text) {
        match (tag.name, tag.end) {
            ("object", false) => {
                let id = tag
                    .number("id")
                    .ok_or_else(|| MeshError::Invalid("an object has no id".into()))?;
                let name = tag.attr("name");
                let definition = Definition::Mesh {
                    vertices: Vec::new(),
                    triangles: Vec::new(),
                    name,
                };
                if tag.empty {
                    file.objects.insert(id, definition);
                } else {
                    current = Some((id, definition));
                }
            }
            ("object", true) => {
                if let Some((id, definition)) = current.take() {
                    file.objects.insert(id, definition);
                }
            }
            ("components", false) => {
                if let Some((_, definition)) = current.as_mut()
                    && let Definition::Mesh { name, .. } = definition
                {
                    *definition = Definition::Components {
                        parts: Vec::new(),
                        name: name.take(),
                    };
                }
            }
            ("component", false) => {
                if let Some((_, Definition::Components { parts, .. })) = current.as_mut()
                    && let Some(object) = tag.number("objectid")
                {
                    parts.push((
                        tag.attr("path"),
                        object,
                        Transform::parse(tag.attr("transform")),
                    ));
                }
            }
            ("vertex", false) => {
                if let Some((_, Definition::Mesh { vertices, .. })) = current.as_mut() {
                    let coordinate = |axis| tag.number::<f32>(axis).filter(|v| v.is_finite());
                    match (coordinate("x"), coordinate("y"), coordinate("z")) {
                        (Some(x), Some(y), Some(z)) => vertices.push([x, y, z]),
                        _ => return Err(MeshError::Invalid("a vertex isn't three numbers".into())),
                    }
                }
            }
            ("triangle", false) => {
                if let Some((_, Definition::Mesh { triangles, .. })) = current.as_mut() {
                    match (tag.number("v1"), tag.number("v2"), tag.number("v3")) {
                        (Some(a), Some(b), Some(c)) => {
                            *budget = budget.checked_sub(1).ok_or(MeshError::TooBig(HARD_LIMIT))?;
                            triangles.push([a, b, c]);
                        }
                        _ => {
                            return Err(MeshError::Invalid(
                                "a triangle isn't three vertex numbers".into(),
                            ));
                        }
                    }
                }
            }
            ("item", false) => {
                if let Some(object) = tag.number("objectid")
                    && tag.attr("printable").as_deref() != Some("0")
                {
                    file.build
                        .push((object, Transform::parse(tag.attr("transform"))));
                }
            }
            _ => {}
        }
    }
    Ok(file)
}

/// Which filament each object uses, from `Metadata/model_settings.config`.
fn object_filaments(text: &str) -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    let mut object = None;
    let mut depth = 0usize;
    for tag in xml::tags(text) {
        match (tag.name, tag.end) {
            ("object", false) if !tag.empty => {
                object = tag.number("id");
                depth = 0;
            }
            ("object", true) => object = None,
            // Only the object's own extruder, not its parts'.
            ("part", false) if !tag.empty => depth += 1,
            ("part", true) => depth = depth.saturating_sub(1),
            ("metadata", false) if depth == 0 => {
                if let Some(id) = object
                    && tag.attr("key").as_deref() == Some("extruder")
                    && let Some(extruder) = tag.number::<u32>("value").filter(|e| *e > 0)
                {
                    out.insert(id, extruder);
                }
            }
            _ => {}
        }
    }
    out
}

/// The filament colours, by filament number from 1.
fn filament_colours(project: Option<&serde_json::Value>) -> Vec<String> {
    project
        .and_then(|p| p.get("filament_colour"))
        .and_then(serde_json::Value::as_array)
        .map(|colours| {
            colours
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|c| {
                    let hex = c.trim().trim_start_matches('#');
                    format!("#{}", hex.get(..6).unwrap_or(hex).to_ascii_uppercase())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads and flattens every placed object.
pub fn read(bytes: &[u8]) -> Result<Model, MeshError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| MeshError::Invalid("it isn't a valid 3MF (zip) file".into()))?;
    let mut read_text = |name: &str| -> Option<String> {
        let name = name.trim_start_matches('/');
        let entry = zip.by_name(name).ok()?;
        let mut text = String::new();
        entry.take(MAX_MODEL_BYTES).read_to_string(&mut text).ok()?;
        Some(text)
    };
    let main = read_text("3D/3dmodel.model").ok_or(MeshError::NoModel)?;
    let filaments = read_text("Metadata/model_settings.config")
        .map(|t| object_filaments(&t))
        .unwrap_or_default();
    let project: Option<serde_json::Value> =
        read_text("Metadata/project_settings.config").and_then(|t| serde_json::from_str(&t).ok());
    let colours = filament_colours(project.as_ref());

    let mut budget = HARD_LIMIT;
    let main = read_model_file(&main, &mut budget)?;
    let mut others: HashMap<String, ModelFile> = HashMap::new();
    let mut objects = Vec::new();
    for (id, transform) in &main.build {
        let mut object = Object {
            name: None,
            filament: filaments.get(id).copied(),
            colour: None,
            positions: Vec::new(),
            indices: Vec::new(),
        };
        object.colour = object
            .filament
            .and_then(|f| colours.get(f as usize - 1).cloned());
        let mut stack = vec![(None::<String>, *id, *transform, 0usize)];
        while let Some((path, id, transform, depth)) = stack.pop() {
            if depth > 16 {
                return Err(MeshError::Invalid("components nest too deeply".into()));
            }
            let file = match &path {
                None => &main,
                Some(path) => {
                    if !others.contains_key(path) {
                        let text = read_text(path)
                            .ok_or_else(|| MeshError::Invalid(format!("{path} is missing")))?;
                        others.insert(path.clone(), read_model_file(&text, &mut budget)?);
                    }
                    &others[path]
                }
            };
            match file.objects.get(&id) {
                Some(Definition::Mesh {
                    vertices,
                    triangles,
                    name,
                }) => {
                    if object.name.is_none() {
                        object.name.clone_from(name);
                    }
                    let base = (object.positions.len() / 3) as u32;
                    for vertex in vertices {
                        object.positions.extend(transform.apply(*vertex));
                    }
                    let count = vertices.len() as u32;
                    for [a, b, c] in triangles {
                        if *a >= count || *b >= count || *c >= count {
                            return Err(MeshError::Invalid(
                                "a triangle names a vertex that isn't there".into(),
                            ));
                        }
                        object.indices.extend([base + a, base + b, base + c]);
                    }
                }
                Some(Definition::Components { parts, name }) => {
                    if object.name.is_none() {
                        object.name.clone_from(name);
                    }
                    for (part_path, part_id, part_transform) in parts {
                        let part_path = part_path.clone().or_else(|| path.clone());
                        stack.push((
                            part_path,
                            *part_id,
                            part_transform.then(&transform),
                            depth + 1,
                        ));
                    }
                }
                None => {
                    return Err(MeshError::Invalid(format!("object {id} isn't defined")));
                }
            }
        }
        if !object.indices.is_empty() {
            objects.push(object);
        }
    }
    if objects.is_empty() {
        return Err(MeshError::NoModel);
    }
    let mut model = Model {
        original_triangles: objects.iter().map(Object::triangles).sum(),
        objects,
    };
    simplify(&mut model, MAX_TRIANGLES);
    Ok(model)
}

/// Simplifies by vertex clustering until the model has at most `limit`
/// triangles: vertices in the same grid cell merge into their average, and
/// triangles that collapse are dropped. Coarse, but fast and enough for a
/// preview; small models are left exactly as they are.
pub fn simplify(model: &mut Model, limit: usize) {
    if model.triangles() <= limit {
        return;
    }
    let (min, max) = bounds(model);
    let size = (0..3)
        .map(|i| max[i] - min[i])
        .fold(0.0f32, f32::max)
        .max(1e-3);
    // Start near the right resolution: triangles scale with cells².
    let mut cells = ((limit as f32 / 2.0).sqrt()).clamp(8.0, 4096.0);
    let originals = model.objects.clone();
    loop {
        let cell = size / cells;
        for (object, original) in model.objects.iter_mut().zip(&originals) {
            cluster(object, original, min, cell);
        }
        if model.triangles() <= limit || cells <= 8.0 {
            return;
        }
        cells *= 0.75;
    }
}

fn bounds(model: &Model) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for object in &model.objects {
        for p in object.positions.as_chunks::<3>().0.iter() {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
    }
    (min, max)
}

fn cluster(object: &mut Object, original: &Object, min: [f32; 3], cell: f32) {
    let mut cells: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut sums: Vec<([f64; 3], u32)> = Vec::new();
    let mut remap = Vec::with_capacity(original.positions.len() / 3);
    for p in original.positions.as_chunks::<3>().0.iter() {
        let key = (
            ((p[0] - min[0]) / cell) as i32,
            ((p[1] - min[1]) / cell) as i32,
            ((p[2] - min[2]) / cell) as i32,
        );
        let index = *cells.entry(key).or_insert_with(|| {
            sums.push(([0.0; 3], 0));
            (sums.len() - 1) as u32
        });
        let sum = &mut sums[index as usize];
        for (total, value) in sum.0.iter_mut().zip(p) {
            *total += f64::from(*value);
        }
        sum.1 += 1;
        remap.push(index);
    }
    object.positions = sums
        .iter()
        .flat_map(|(sum, n)| sum.map(|s| (s / f64::from(*n)) as f32))
        .collect();
    let mut seen = std::collections::HashSet::new();
    object.indices = original
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            [
                remap[t[0] as usize],
                remap[t[1] as usize],
                remap[t[2] as usize],
            ]
        })
        .filter(|[a, b, c]| a != b && b != c && a != c)
        .filter(|t| {
            let mut key = *t;
            key.sort_unstable();
            seen.insert(key)
        })
        .flatten()
        .collect();
}

/// The model as the viewer takes it: each object's positions (f32) and
/// indices (u32), little-endian and base64-encoded, so even a big model is
/// one compact JSON document.
#[derive(Debug, Serialize)]
pub struct ModelJson {
    pub objects: Vec<ObjectJson>,
    pub triangles: usize,
    pub original_triangles: usize,
    pub simplified: bool,
}

#[derive(Debug, Serialize)]
pub struct ObjectJson {
    pub name: Option<String>,
    pub filament: Option<u32>,
    pub colour: Option<String>,
    pub positions: String,
    pub indices: String,
}

impl From<&Model> for ModelJson {
    fn from(model: &Model) -> Self {
        let b64 = base64::engine::general_purpose::STANDARD;
        Self {
            objects: model
                .objects
                .iter()
                .map(|o| ObjectJson {
                    name: o.name.clone(),
                    filament: o.filament,
                    colour: o.colour.clone(),
                    positions: b64.encode(
                        o.positions
                            .iter()
                            .flat_map(|v| v.to_le_bytes())
                            .collect::<Vec<u8>>(),
                    ),
                    indices: b64.encode(
                        o.indices
                            .iter()
                            .flat_map(|v| v.to_le_bytes())
                            .collect::<Vec<u8>>(),
                    ),
                })
                .collect(),
            triangles: model.triangles(),
            original_triangles: model.original_triangles,
            simplified: model.simplified(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gantry_print_file_fixtures as fixtures;

    #[test]
    fn reads_inline_and_referenced_meshes_with_their_filaments() {
        let model = read(&fixtures::bambu_3mf(&fixtures::three_plates())).unwrap();
        assert_eq!(model.objects.len(), 2);
        let cube = &model.objects[0];
        assert_eq!(cube.name.as_deref(), Some("Cube"));
        assert_eq!(cube.triangles(), 12);
        assert_eq!(cube.positions.len(), 8 * 3);
        assert_eq!(cube.filament, Some(1));
        assert_eq!(cube.colour.as_deref(), Some("#FFFFFF"));
        let pyramid = &model.objects[1];
        assert_eq!(pyramid.name.as_deref(), Some("Pyramid"));
        assert_eq!(pyramid.triangles(), 6);
        assert_eq!(pyramid.filament, Some(2));
        assert_eq!(pyramid.colour.as_deref(), Some("#D33A2C"));
        // Placed 20 mm along X by its build item: it spans x = 20 to 30.
        let apex_x = pyramid
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p[0])
            .fold(f32::MIN, f32::max);
        assert_eq!(apex_x, 30.0);
        assert!(
            pyramid
                .positions
                .as_chunks::<3>()
                .0
                .contains(&[25.0, 5.0, 8.0])
        );
        assert!(!model.simplified());
    }

    #[test]
    fn an_unsliced_project_still_has_its_meshes() {
        let model = read(&fixtures::unsliced_3mf()).unwrap();
        assert_eq!(model.triangles(), 18);
        assert_eq!(model.objects[0].colour, None, "no filament settings");
    }

    #[test]
    fn a_file_without_meshes_says_so() {
        let file = fixtures::bambu_3mf_with(
            &fixtures::three_plates(),
            fixtures::Options {
                meshes: false,
                ..Default::default()
            },
        );
        assert_eq!(read(&file).unwrap_err(), MeshError::NoModel);
        assert!(read(b"not a zip").is_err());
    }

    #[test]
    fn transforms_compose_in_order() {
        let scale = Transform([2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0]);
        let shift = Transform([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 10.0, 0.0, 0.0]);
        assert_eq!(scale.then(&shift).apply([1.0, 1.0, 1.0]), [12.0, 2.0, 2.0]);
        assert_eq!(shift.then(&scale).apply([1.0, 1.0, 1.0]), [22.0, 2.0, 2.0]);
    }

    #[test]
    fn big_models_are_simplified_to_the_limit() {
        // 60 × 60 cubes: 43,200 triangles.
        let mut model = read(&fixtures::big_mesh_3mf(60)).unwrap();
        assert_eq!(model.original_triangles, 43_200);
        assert!(!model.simplified(), "under the default limit");
        simplify(&mut model, 10_000);
        assert!(model.triangles() <= 10_000, "{}", model.triangles());
        assert!(
            model.triangles() > 500,
            "still a shape: {}",
            model.triangles()
        );
        assert!(model.simplified());
        // Every index still points at a vertex.
        let vertices = model.objects[0].positions.len() as u32 / 3;
        assert!(model.objects[0].indices.iter().all(|i| *i < vertices));
    }

    #[test]
    fn the_json_carries_little_endian_arrays() {
        let model = read(&fixtures::unsliced_3mf()).unwrap();
        let json = ModelJson::from(&model);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&json.objects[0].positions)
            .unwrap();
        assert_eq!(bytes.len(), model.objects[0].positions.len() * 4);
        assert_eq!(json.triangles, 18);
    }
}
