//! Toolpath preview (PRD §8.2, level 3): a Plate's G-code read in Rust and
//! turned into a simplified set of extrusion paths, layer by layer, which
//! the UI draws in 3D with a layer slider, coloured by feature or filament.
//!
//! - Moves are followed with absolute or relative positioning (`G90`/`G91`),
//!   absolute or relative extrusion (`M82`/`M83`), `G92` resets, and arcs
//!   (`G2`/`G3`, flattened into short segments). A move that extrudes is part
//!   of a path; anything else ends the path.
//! - A layer begins whenever extrusion happens at a new height.
//! - Features come from the slicers' comments: `;TYPE:` (PrusaSlicer,
//!   OrcaSlicer, SuperSlicer, Cura) and `; FEATURE:` (Bambu Studio). The
//!   filament is the active tool (`T0`, `T1`, …).
//! - Paths are simplified (Ramer-Douglas-Peucker), more coarsely the bigger
//!   the file, so even tens of megabytes of G-code come to a few megabytes
//!   that a browser draws without freezing.
//!
//! The result is sent as a compact binary, layer by layer, so the UI can
//! draw the first layers while the rest are still arriving. See
//! [`Toolpath::encode`] for the format.
use serde::Serialize;

/// What a path prints, as the UI colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Feature {
    /// Perimeters, inner and outer, and overhangs.
    Wall = 0,
    /// Sparse and solid infill, top and bottom skins, bridges, ironing.
    Infill = 1,
    /// Support and support interfaces.
    Support = 2,
    /// Skirt, brim, prime tower, and anything a slicer doesn't name.
    Other = 3,
}

impl Feature {
    /// The feature a slicer's comment names.
    pub fn from_comment(name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        if name.contains("support") || name.contains("raft") {
            Self::Support
        } else if name.contains("perimeter")
            || name.contains("wall")
            || name.contains("overhang")
            || name.contains("outer")
            || name.contains("inner")
        {
            Self::Wall
        } else if name.contains("infill")
            || name.contains("fill")
            || name.contains("solid")
            || name.contains("skin")
            || name.contains("top")
            || name.contains("bottom")
            || name.contains("bridge")
            || name.contains("ironing")
        {
            Self::Infill
        } else {
            Self::Other
        }
    }
}

/// One continuous extrusion of one feature with one filament.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub feature: Feature,
    pub tool: u8,
    /// `[x, y]` in mm, at the layer's height.
    pub points: Vec<[f32; 2]>,
}

/// One layer: everything extruded at one height.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The height it was printed at, in mm.
    pub z: f32,
    pub paths: Vec<Path>,
}

/// A Plate's toolpath.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Toolpath {
    pub layers: Vec<Layer>,
    /// `[min x, min y, max x, max y]` of everything extruded.
    pub bounds: [f32; 4],
    /// How many filaments (tools) it uses.
    pub tools: u8,
}

/// The most points a toolpath is simplified down to.
pub const POINT_BUDGET: usize = 600_000;

/// How far, in mm, an arc's flattened segments may stray from it.
const ARC_TOLERANCE: f64 = 0.05;

/// A height change smaller than this is the same layer.
const LAYER_EPSILON: f64 = 0.005;

struct Machine {
    absolute: bool,
    absolute_e: bool,
    x: f64,
    y: f64,
    z: f64,
    e: f64,
    feature: Feature,
    tool: u8,
}

/// The number after `letter` in a G-code word list, such as `X12.5`.
fn word(fields: &[&str], letter: u8) -> Option<f64> {
    fields.iter().find_map(|f| {
        let bytes = f.as_bytes();
        (bytes.first()?.to_ascii_uppercase() == letter)
            .then(|| f[1..].parse::<f64>().ok())
            .flatten()
            .filter(|v| v.is_finite())
    })
}

struct Builder {
    layers: Vec<Layer>,
    layer_z: Option<f64>,
    path: Option<Path>,
    max_tool: u8,
}

impl Builder {
    fn end_path(&mut self) {
        if let Some(path) = self.path.take()
            && path.points.len() >= 2
            && let Some(layer) = self.layers.last_mut()
        {
            layer.paths.push(path);
        }
    }

    /// An extruding move from `from` to `to` at height `z`.
    fn extrude(&mut self, m: &Machine, from: [f64; 2], to: [f64; 2], z: f64) {
        let new_layer = self.layer_z.is_none_or(|lz| (z - lz).abs() > LAYER_EPSILON);
        if new_layer {
            self.end_path();
            self.layer_z = Some(z);
            self.layers.push(Layer {
                z: z as f32,
                paths: Vec::new(),
            });
        }
        let same = self
            .path
            .as_ref()
            .is_some_and(|p| p.feature == m.feature && p.tool == m.tool);
        if !same {
            self.end_path();
            self.path = Some(Path {
                feature: m.feature,
                tool: m.tool,
                points: vec![[from[0] as f32, from[1] as f32]],
            });
        }
        self.max_tool = self.max_tool.max(m.tool);
        if let Some(path) = self.path.as_mut() {
            path.points.push([to[0] as f32, to[1] as f32]);
        }
    }
}

/// Points along an arc from `from` to `to` about `centre`, clockwise for
/// `G2`, ending exactly at `to`.
fn arc(from: [f64; 2], to: [f64; 2], centre: [f64; 2], clockwise: bool) -> Vec<[f64; 2]> {
    let r = ((from[0] - centre[0]).powi(2) + (from[1] - centre[1]).powi(2)).sqrt();
    if r < 1e-6 {
        return vec![to];
    }
    let a0 = (from[1] - centre[1]).atan2(from[0] - centre[0]);
    let a1 = (to[1] - centre[1]).atan2(to[0] - centre[0]);
    let mut sweep = a1 - a0;
    if clockwise && sweep >= 0.0 {
        sweep -= std::f64::consts::TAU;
    } else if !clockwise && sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }
    // A segment's chord strays r(1 - cos(θ/2)) from the arc.
    let step = 2.0 * (1.0 - ARC_TOLERANCE / r).clamp(-1.0, 1.0).acos();
    let n = ((sweep.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 360);
    (1..=n)
        .map(|i| {
            if i == n {
                to
            } else {
                let a = a0 + sweep * (i as f64) / (n as f64);
                [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
            }
        })
        .collect()
}

/// Reads G-code into its unsimplified toolpath.
pub fn parse(gcode: &[u8]) -> Toolpath {
    let text = String::from_utf8_lossy(gcode);
    let mut m = Machine {
        absolute: true,
        absolute_e: true,
        x: 0.0,
        y: 0.0,
        z: 0.0,
        e: 0.0,
        feature: Feature::Other,
        tool: 0,
    };
    let mut b = Builder {
        layers: Vec::new(),
        layer_z: None,
        path: None,
        max_tool: 0,
    };
    for raw in text.lines() {
        let (code, comment) = match raw.split_once(';') {
            Some((code, comment)) => (code.trim(), Some(comment.trim())),
            None => (raw.trim(), None),
        };
        if code.is_empty() {
            if let Some(c) = comment {
                let lower = c.to_ascii_lowercase();
                let named = lower
                    .strip_prefix("type:")
                    .or_else(|| lower.strip_prefix("feature:"));
                if let Some(name) = named {
                    m.feature = Feature::from_comment(name);
                    b.end_path();
                }
            }
            continue;
        }
        let fields: Vec<&str> = code.split_ascii_whitespace().collect();
        let Some(command) = fields.first().map(|c| c.to_ascii_uppercase()) else {
            continue;
        };
        match command.as_str() {
            "G0" | "G1" | "G2" | "G3" => {
                let pos = |value: Option<f64>, current: f64| match value {
                    Some(v) if m.absolute => v,
                    Some(v) => current + v,
                    None => current,
                };
                let (x, y, z) = (
                    pos(word(&fields, b'X'), m.x),
                    pos(word(&fields, b'Y'), m.y),
                    pos(word(&fields, b'Z'), m.z),
                );
                let extruded = match word(&fields, b'E') {
                    Some(e) if m.absolute_e => {
                        let d = e - m.e;
                        m.e = e;
                        d
                    }
                    Some(e) => e,
                    None => 0.0,
                };
                let moved = (x - m.x).abs() > 1e-6 || (y - m.y).abs() > 1e-6;
                if extruded > 1e-9 && moved {
                    let from = [m.x, m.y];
                    let to = [x, y];
                    if command == "G2" || command == "G3" {
                        let centre = [
                            m.x + word(&fields, b'I').unwrap_or(0.0),
                            m.y + word(&fields, b'J').unwrap_or(0.0),
                        ];
                        let mut last = from;
                        for p in arc(from, to, centre, command == "G2") {
                            b.extrude(&m, last, p, z);
                            last = p;
                        }
                    } else {
                        b.extrude(&m, from, to, z);
                    }
                } else if moved || (z - m.z).abs() > 1e-9 {
                    // A travel (or a retraction with a move) ends the path.
                    b.end_path();
                }
                m.x = x;
                m.y = y;
                m.z = z;
            }
            "G90" => m.absolute = true,
            "G91" => m.absolute = false,
            "M82" => m.absolute_e = true,
            "M83" => m.absolute_e = false,
            "G92" => {
                if let Some(v) = word(&fields, b'X') {
                    m.x = v;
                }
                if let Some(v) = word(&fields, b'Y') {
                    m.y = v;
                }
                if let Some(v) = word(&fields, b'Z') {
                    m.z = v;
                }
                if let Some(v) = word(&fields, b'E') {
                    m.e = v;
                }
                if fields.len() == 1 {
                    m.e = 0.0;
                }
            }
            t if t.starts_with('T') && t.len() > 1 => {
                if let Ok(tool) = t[1..].parse::<u8>() {
                    b.end_path();
                    m.tool = tool;
                }
            }
            _ => {}
        }
    }
    b.end_path();
    let mut layers = b.layers;
    layers.retain(|l| !l.paths.is_empty());
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for p in layers.iter().flat_map(|l| &l.paths).flat_map(|p| &p.points) {
        bounds[0] = bounds[0].min(p[0]);
        bounds[1] = bounds[1].min(p[1]);
        bounds[2] = bounds[2].max(p[0]);
        bounds[3] = bounds[3].max(p[1]);
    }
    if layers.is_empty() {
        bounds = [0.0; 4];
    }
    Toolpath {
        layers,
        bounds,
        tools: b.max_tool.saturating_add(1),
    }
}

fn distance_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Ramer-Douglas-Peucker: the fewest points that stay within `epsilon` mm.
pub fn simplify(points: &[[f32; 2]], epsilon: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        let mut far = (0.0f32, 0usize);
        for i in start + 1..end {
            let d = distance_to_segment(points[i], points[start], points[end]);
            if d > far.0 {
                far = (d, i);
            }
        }
        if far.0 > epsilon {
            keep[far.1] = true;
            stack.push((start, far.1));
            stack.push((far.1, end));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

impl Toolpath {
    pub fn points(&self) -> usize {
        self.layers
            .iter()
            .flat_map(|l| &l.paths)
            .map(|p| p.points.len())
            .sum()
    }

    /// Simplified until it has at most `budget` points, as coarsely as it
    /// needs and no more: 0.01 mm first, then coarser by halves.
    pub fn simplified(&self, budget: usize) -> Self {
        let mut epsilon = 0.01f32;
        loop {
            let out = Self {
                layers: self
                    .layers
                    .iter()
                    .map(|l| Layer {
                        z: l.z,
                        paths: l
                            .paths
                            .iter()
                            .map(|p| Path {
                                feature: p.feature,
                                tool: p.tool,
                                points: simplify(&p.points, epsilon),
                            })
                            .collect(),
                    })
                    .collect(),
                bounds: self.bounds,
                tools: self.tools,
            };
            if out.points() <= budget || epsilon > 2.0 {
                return out;
            }
            epsilon *= 2.0;
        }
    }

    /// The binary the UI reads, little-endian throughout:
    ///
    /// ```text
    /// "GTP1"                         magic
    /// u32 layers, u8 tools, 3 × u8 0
    /// f32 × 4 bounds                 min x, min y, max x, max y
    /// each layer:
    ///   f32 z, u32 paths
    ///   each path: u8 feature, u8 tool, u16 0, u32 points, points × (f32 x, f32 y)
    /// ```
    ///
    /// Each layer is complete in itself, so a reader can draw one as soon as
    /// its bytes have arrived.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.points() * 8);
        out.extend_from_slice(b"GTP1");
        out.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        out.extend_from_slice(&[self.tools, 0, 0, 0]);
        for b in self.bounds {
            out.extend_from_slice(&b.to_le_bytes());
        }
        for layer in &self.layers {
            out.extend_from_slice(&layer.z.to_le_bytes());
            out.extend_from_slice(&(layer.paths.len() as u32).to_le_bytes());
            for path in &layer.paths {
                out.extend_from_slice(&[path.feature as u8, path.tool, 0, 0]);
                out.extend_from_slice(&(path.points.len() as u32).to_le_bytes());
                for p in &path.points {
                    out.extend_from_slice(&p[0].to_le_bytes());
                    out.extend_from_slice(&p[1].to_le_bytes());
                }
            }
        }
        out
    }
}

/// G-code for tests and fixtures: `layers` layers of a square wall with
/// zig-zag infill, `lines` infill lines a layer, each line `steps` short
/// moves, with feature comments as PrusaSlicer writes them. Big enough
/// settings make files of tens of megabytes.
pub fn synthetic_gcode(layers: u32, lines: u32, steps: u32) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("; generated by Gantry's tests\nG90\nM83\nG28\n");
    let size = 100.0f64;
    for layer in 0..layers {
        let z = 0.2 * f64::from(layer + 1);
        let _ = writeln!(out, ";LAYER_CHANGE\n;Z:{z:.2}\nG1 Z{z:.2} F600");
        let _ = writeln!(out, ";TYPE:External perimeter\nG1 X10 Y10 F9000");
        for (x, y) in [(110.0, 10.0), (110.0, 110.0), (10.0, 110.0), (10.0, 10.0)] {
            let _ = writeln!(out, "G1 X{x:.3} Y{y:.3} E4.0");
        }
        let _ = writeln!(out, ";TYPE:Internal infill");
        for line in 0..lines {
            let y = 12.0 + (size - 4.0) * f64::from(line) / f64::from(lines.max(1));
            let _ = writeln!(out, "G1 X12 Y{y:.3} F9000");
            for step in 1..=steps {
                // A slight wobble, so simplification has work to do.
                let x = 12.0 + (size - 4.0) * f64::from(step) / f64::from(steps);
                let wobble = if step % 2 == 0 { 0.02 } else { -0.02 };
                let _ = writeln!(out, "G1 X{x:.3} Y{:.3} E{:.4}", y + wobble, 0.04);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrusions_become_paths_on_layers_and_travel_ends_them() {
        let gcode = b"G90\nM83\nG1 Z0.2\n;TYPE:Perimeter\nG1 X0 Y0\nG1 X10 Y0 E1\nG1 X10 Y10 E1\n\
                      G0 X20 Y20\nG1 X30 Y20 E1\n;TYPE:Internal infill\nG1 X30 Y30 E1\n\
                      G1 Z0.4\nG1 X0 Y30 E1\n";
        let t = parse(gcode);
        assert_eq!(t.layers.len(), 2);
        assert_eq!(t.layers[0].z, 0.2);
        let first = &t.layers[0].paths;
        assert_eq!(first.len(), 3, "{first:?}");
        assert_eq!(first[0].feature, Feature::Wall);
        assert_eq!(first[0].points, [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        assert_eq!(
            first[1].points,
            [[20.0, 20.0], [30.0, 20.0]],
            "after the travel"
        );
        assert_eq!(first[2].feature, Feature::Infill);
        assert_eq!(t.layers[1].z, 0.4);
        assert_eq!(t.bounds, [0.0, 0.0, 30.0, 30.0]);
        assert_eq!(t.tools, 1);
    }

    #[test]
    fn absolute_extrusion_retractions_and_resets() {
        let gcode = b"M82\nG92 E0\nG1 Z0.3\nG1 X0 Y0\nG1 X5 Y0 E1\nG1 E0.2\nG0 X6 Y0\nG1 E1\n\
                      G1 X9 Y0 E1.5\nG92 E0\nG1 X12 Y0 E0.5\n";
        let t = parse(gcode);
        let paths = &t.layers[0].paths;
        // The retraction (E falls) and re-prime move nothing; the reset keeps
        // the next move extruding.
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert_eq!(paths[1].points, [[6.0, 0.0], [9.0, 0.0], [12.0, 0.0]]);
    }

    #[test]
    fn relative_moves_tools_and_features_from_each_slicer() {
        let gcode = b"G91\nM83\nG1 Z0.2\nT1\n; FEATURE: Outer wall\nG1 X5 E1\n; FEATURE: Support\n\
                      G1 Y5 E1\n;TYPE:SKIRT\nG1 X-5 E1\n";
        let t = parse(gcode);
        let paths = &t.layers[0].paths;
        assert_eq!(
            paths.iter().map(|p| p.feature).collect::<Vec<_>>(),
            [Feature::Wall, Feature::Support, Feature::Other]
        );
        assert!(paths.iter().all(|p| p.tool == 1));
        assert_eq!(paths[1].points, [[5.0, 0.0], [5.0, 5.0]]);
        assert_eq!(t.tools, 2);
        assert_eq!(Feature::from_comment("WALL-INNER"), Feature::Wall);
        assert_eq!(Feature::from_comment("Solid infill"), Feature::Infill);
        assert_eq!(Feature::from_comment("Top surface"), Feature::Infill);
        assert_eq!(Feature::from_comment("Support interface"), Feature::Support);
        assert_eq!(Feature::from_comment("Prime tower"), Feature::Other);
    }

    #[test]
    fn arcs_are_flattened_and_end_where_they_should() {
        // A half circle of radius 10 about (10, 0), clockwise from (0, 0).
        let t = parse(b"G90\nM83\nG1 Z0.2\nG1 X0 Y0\nG2 X20 Y0 I10 J0 E5\n");
        let points = &t.layers[0].paths[0].points;
        assert!(points.len() > 10, "{}", points.len());
        assert_eq!(*points.last().unwrap(), [20.0, 0.0]);
        for p in points {
            let r = ((p[0] - 10.0).powi(2) + p[1].powi(2)).sqrt();
            assert!((r - 10.0).abs() < 0.01, "{p:?}");
            assert!(
                p[1] >= -1e-3,
                "clockwise from (0,0) goes over the top: {p:?}"
            );
        }
    }

    #[test]
    fn simplification_keeps_corners_and_drops_the_rest() {
        let line: Vec<[f32; 2]> = (0..=100).map(|i| [i as f32, 0.0]).collect();
        assert_eq!(simplify(&line, 0.01), [[0.0, 0.0], [100.0, 0.0]]);
        let corner = [
            [0.0, 0.0],
            [5.0, 0.0],
            [10.0, 0.0],
            [10.0, 5.0],
            [10.0, 10.0],
        ];
        assert_eq!(
            simplify(&corner, 0.01),
            [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]
        );
    }

    #[test]
    fn a_big_file_comes_down_to_the_budget_and_encodes_compactly() {
        let gcode = synthetic_gcode(40, 40, 200);
        let t = parse(gcode.as_bytes());
        assert_eq!(t.layers.len(), 40);
        let raw = t.points();
        let small = t.simplified(raw / 10);
        assert!(small.points() <= raw / 10, "{} of {raw}", small.points());
        assert_eq!(small.layers.len(), 40);
        let bytes = small.encode();
        assert_eq!(&bytes[..4], b"GTP1");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 40);
        assert_eq!(
            bytes.len(),
            28 + 40 * 8
                + small.layers.iter().map(|l| l.paths.len()).sum::<usize>() * 8
                + small.points() * 8
        );
    }

    #[test]
    fn no_extrusion_is_no_layers() {
        let t = parse(b"G28\nG1 X10 Y10\n; just moving\n");
        assert!(t.layers.is_empty());
        assert_eq!(t.bounds, [0.0; 4]);
        assert_eq!(t.encode().len(), 28);
    }
}
