//! The command layer as tools, for the assistant and for MCP clients.
//!
//! Every change an agent makes is a proposal: recipe lines (command-layer verbs with
//! every reference by name) plus a summary. Proposing runs the lines on a scratch copy
//! of the part first, so a line whose reference does not resolve, or a result that does
//! not rebuild, is rejected before anyone sees a preview. Applying runs the same
//! commands the ribbon runs, as one undo step, and records who asked for it in the
//! command log.
//!
//! The assistant can only propose. It may apply only when the user turned on auto-apply
//! for the session; otherwise the user applies from the proposal card. An MCP client is
//! a tool the user connected on purpose, so its verb tools and `apply` run directly.

use crate::commands::{self, Agents, CommandSpec};
use crate::document::EdgeRef;
use crate::document::{
    ChamferParams, Command, CutParams, Document, ExtrudeParams, FeatureId, FeatureKind, FeatureStatus, FilletParams,
    PlaneKind, Preview, Selection, SketchPlane, VertexKey, fmt_mm,
};
use crate::sketch::{self, ConstraintKind, EntityKind, P2, PointRef, Sketch};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{any_of, array, boolean, constant, non_empty_array, number, object, string};
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_api::{FaceId, Plane, SurfaceKind};

/// A world point in a recipe line, in millimetres.
pub type Point = [f64; 3];

/// A closed shape in a sketch line. Points are world coordinates on the sketch plane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// A rectangle: `width` along the plane's u direction, `height` along v.
    Rectangle {
        /// Centre.
        center: Point,
        /// Along u, mm.
        width: f64,
        /// Along v, mm.
        height: f64,
    },
    /// A circle.
    Circle {
        /// Centre.
        center: Point,
        /// Diameter, mm.
        diameter: f64,
    },
    /// A straight slot: two arc centres and the slot's width.
    Slot {
        /// First arc centre.
        start: Point,
        /// Second arc centre.
        end: Point,
        /// Width across the slot, mm.
        width: f64,
    },
}

/// One recipe line: a command-layer verb with every reference by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Line {
    /// A sketch of closed shapes on a reference plane or a planar face.
    Sketch {
        /// `top`, `front`, `right`, or a planar face's name.
        plane: String,
        /// The shapes.
        shapes: Vec<Shape>,
    },
    /// Adds material: extrudes a sketch's profiles and joins them to the part.
    Extrude {
        /// The sketch's name, or `previous` for the sketch an earlier line made.
        sketch: String,
        /// Depth, mm.
        depth: f64,
        /// Extrude against the sketch normal.
        flip: bool,
    },
    /// Removes material: cuts a sketch's profiles into the part.
    Cut {
        /// The sketch's name, or `previous`.
        sketch: String,
        /// Cut through the whole part; `depth` is then ignored.
        through_all: bool,
        /// Depth, mm.
        depth: f64,
        /// Cut along the sketch normal instead of into the face.
        flip: bool,
    },
    /// Rounds edges.
    Fillet {
        /// Edge names.
        edges: Vec<String>,
        /// Radius, mm.
        radius: f64,
    },
    /// Bevels edges.
    Chamfer {
        /// Edge names.
        edges: Vec<String>,
        /// Distance, mm.
        distance: f64,
    },
    /// Changes an extrude's depth or direction.
    EditExtrude {
        /// The feature's name.
        feature: String,
        /// New depth, mm.
        depth: f64,
        /// New direction.
        flip: bool,
    },
    /// Changes a cut's end condition.
    EditCut {
        /// The feature's name.
        feature: String,
        /// Through the whole part.
        through_all: bool,
        /// Depth, mm.
        depth: f64,
        /// Direction.
        flip: bool,
    },
    /// Changes a fillet's radius.
    EditFillet {
        /// The feature's name.
        feature: String,
        /// New radius, mm.
        radius: f64,
    },
    /// Changes a chamfer's distance.
    EditChamfer {
        /// The feature's name.
        feature: String,
        /// New distance, mm.
        distance: f64,
    },
    /// Suppresses or restores a feature.
    Suppress {
        /// The feature's name.
        feature: String,
        /// `true` to suppress.
        suppressed: bool,
    },
    /// Removes a feature, and the sketch it consumes.
    DeleteFeature {
        /// The feature's name.
        feature: String,
    },
}

fn point(what: &str) -> Value {
    array(number("mm"), &format!("{what}: [x, y, z] in world coordinates, mm, on the sketch plane"))
}

fn shape_schema() -> Value {
    any_of(vec![
        object(&[
            ("shape", constant("rectangle")),
            ("center", point("Centre")),
            ("width", number("Size along the plane's u direction, mm, greater than 0")),
            ("height", number("Size along the plane's v direction, mm, greater than 0")),
        ]),
        object(&[
            ("shape", constant("circle")),
            ("center", point("Centre")),
            ("diameter", number("Diameter, mm, greater than 0")),
        ]),
        object(&[
            ("shape", constant("slot")),
            ("start", point("First arc centre")),
            ("end", point("Second arc centre")),
            (
                "width",
                number("Width across the slot, mm; the slot's overall length is the centre distance plus the width"),
            ),
        ]),
    ])
}

fn feature_param() -> (&'static str, Value) {
    ("feature", string("The feature's tree name, such as Fillet1"))
}

fn sketch_param() -> (&'static str, Value) {
    (
        "sketch",
        string("The sketch's tree name, or `previous` for the sketch the nearest earlier line of this proposal made"),
    )
}

fn edges_param() -> (&'static str, Value) {
    ("edges", non_empty_array(string("An edge name, such as Boss-Extrude2 · Edge 5"), "The edges"))
}

/// The `sketch` verb's parameters.
pub fn sketch_params() -> Vec<(&'static str, Value)> {
    vec![
        (
            "plane",
            string(
                "`top` (XY, u=X, v=Y), `front` (XZ, normal -Y, u=X, v=Z), `right` (YZ, u=Y, v=Z), or a planar \
                 face's name, such as Boss-Extrude1 · End cap (its u and v are listed by `feature`)",
            ),
        ),
        ("shapes", non_empty_array(shape_schema(), "The closed shapes")),
    ]
}

/// The `extrude` verb's parameters.
pub fn extrude_params() -> Vec<(&'static str, Value)> {
    vec![
        sketch_param(),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true extrudes against the sketch normal")),
    ]
}

/// The `cut` verb's parameters.
pub fn cut_params() -> Vec<(&'static str, Value)> {
    vec![
        sketch_param(),
        ("through_all", boolean("true cuts through the whole part; depth is then ignored")),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true cuts along the sketch normal instead of into the face")),
    ]
}

/// The `fillet` verb's parameters.
pub fn fillet_params() -> Vec<(&'static str, Value)> {
    vec![edges_param(), ("radius", number("Radius, mm, greater than 0"))]
}

/// The `chamfer` verb's parameters.
pub fn chamfer_params() -> Vec<(&'static str, Value)> {
    vec![edges_param(), ("distance", number("Distance, mm, greater than 0"))]
}

/// The `edit_extrude` verb's parameters.
pub fn edit_extrude_params() -> Vec<(&'static str, Value)> {
    vec![
        feature_param(),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true extrudes against the sketch normal")),
    ]
}

/// The `edit_cut` verb's parameters.
pub fn edit_cut_params() -> Vec<(&'static str, Value)> {
    vec![
        feature_param(),
        ("through_all", boolean("true cuts through the whole part")),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true cuts along the sketch normal")),
    ]
}

/// The `edit_fillet` verb's parameters.
pub fn edit_fillet_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("radius", number("Radius, mm, greater than 0"))]
}

/// The `edit_chamfer` verb's parameters.
pub fn edit_chamfer_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("distance", number("Distance, mm, greater than 0"))]
}

/// The `suppress` verb's parameters.
pub fn suppress_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("suppressed", boolean("true suppresses, false restores"))]
}

/// The `delete_feature` verb's parameters.
pub fn delete_feature_params() -> Vec<(&'static str, Value)> {
    vec![feature_param()]
}

/// The `feature` tool's parameters.
pub fn feature_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("The feature's tree name, such as Boss-Extrude1"))]
}

/// The `measure` tool's parameters.
pub fn measure_params() -> Vec<(&'static str, Value)> {
    vec![("a", string("First entity name")), ("b", string("Second entity name"))]
}

/// The `propose` tool's parameters.
pub fn propose_params() -> Vec<(&'static str, Value)> {
    vec![
        ("summary", string("One line for the card's title, such as `Four Ø9 holes on 90 × 44`")),
        (
            "commands",
            non_empty_array(
                any_of(commands::verbs().map(line_schema).collect()),
                "Recipe lines, run in order. A sketch line followed by an extrude or cut line naming `previous` \
                 makes one feature from new geometry.",
            ),
        ),
    ]
}

/// The `apply` tool's parameters.
pub fn apply_params() -> Vec<(&'static str, Value)> {
    vec![("proposal_id", string("The id propose returned, such as P1"))]
}

/// The schema of one recipe line of a verb: its parameters tagged with `op`.
pub fn line_schema(verb: &CommandSpec) -> Value {
    let mut params = vec![("op", constant(verb.id))];
    params.extend((verb.params)());
    object(&params)
}

fn positive(what: &str, v: f64) -> Result<f64, String> {
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(format!("{what} must be greater than 0, got {v}")) }
}

fn feature_names(doc: &Document) -> String {
    let names: Vec<&str> = doc.features.iter().map(|f| f.name.as_str()).collect();
    if names.is_empty() { "nothing".into() } else { names.join(", ") }
}

fn feature_by_name(doc: &Document, name: &str) -> Result<FeatureId, String> {
    doc.features
        .iter()
        .find(|f| f.name == name.trim())
        .map(|f| f.id)
        .ok_or_else(|| format!("no feature named `{name}` (the tree has: {})", feature_names(doc)))
}

/// Splits `Boss-Extrude1 · End cap` into the feature and the label.
fn split_name(name: &str) -> Option<(&str, &str)> {
    let (f, l) = ["·", "/", ":"].iter().find_map(|sep| name.split_once(sep))?;
    Some((f.trim(), l.trim()))
}

fn face_by_name(doc: &Document, name: &str) -> Option<FaceId> {
    let (feature, label) = split_name(name)?;
    let id = doc.features.iter().find(|f| f.name == feature)?.id;
    doc.faces.iter().find(|f| f.feature == id && f.label.eq_ignore_ascii_case(label)).map(|f| f.id)
}

fn edge_by_name(doc: &Document, name: &str) -> Option<soliddraft_kernel_api::EdgeId> {
    let (feature, label) = split_name(name)?;
    let id = doc.features.iter().find(|f| f.name == feature)?.id;
    doc.edges.iter().find(|e| e.feature == id && e.label.eq_ignore_ascii_case(label)).map(|e| e.id)
}

fn name_of_face(doc: &Document, face: FaceId) -> Option<String> {
    let nf = doc.named_face(face)?;
    Some(format!("{} · {}", doc.feature(nf.feature).map(|f| f.name.as_str()).unwrap_or("Body"), nf.label))
}

fn name_of_edge(doc: &Document, edge: soliddraft_kernel_api::EdgeId) -> Option<String> {
    let ne = doc.named_edge(edge)?;
    Some(format!("{} · {}", doc.feature(ne.feature).map(|f| f.name.as_str()).unwrap_or("Body"), ne.label))
}

fn edge_ref(doc: &Document, name: &str) -> Result<EdgeRef, String> {
    let edge = edge_by_name(doc, name)
        .ok_or_else(|| format!("no edge named `{name}`; `feature` lists each feature's edges by name"))?;
    doc.edge_ref(edge).ok_or_else(|| format!("`{name}` cannot be referenced"))
}

fn sketch_plane(doc: &Document, plane: &str) -> Result<SketchPlane, String> {
    let p = plane.trim().to_ascii_lowercase();
    let reference = match p.trim_end_matches(" plane") {
        "top" => Some(PlaneKind::Top),
        "front" => Some(PlaneKind::Front),
        "right" => Some(PlaneKind::Right),
        _ => None,
    };
    if let Some(plane) = reference {
        return Ok(SketchPlane::Reference { plane });
    }
    let face = face_by_name(doc, plane).ok_or_else(|| {
        format!("`{plane}` is neither top, front, right nor a face name; `feature` lists each feature's faces")
    })?;
    let face = doc.face_ref(face).ok_or_else(|| format!("`{plane}` is not planar, so it cannot carry a sketch"))?;
    Ok(SketchPlane::Face { face })
}

/// Projects a world point into the plane's (u, v) frame. Points more than half a
/// millimetre off the plane are refused: they mean the plane or the point is wrong.
fn to_plane(plane: &Plane, p: Point, what: &str) -> Result<P2, String> {
    let w = soliddraft_kernel_api::Vec3::new(p[0], p[1], p[2]);
    if !w.is_finite() {
        return Err(format!("{what} is not a finite point"));
    }
    let d = w - plane.origin;
    let off = d.dot(plane.normal);
    if off.abs() > 0.5 {
        return Err(format!(
            "{what} [{}, {}, {}] is {} mm off the sketch plane; give points on the plane",
            fmt_mm(p[0]),
            fmt_mm(p[1]),
            fmt_mm(p[2]),
            fmt_mm(off)
        ));
    }
    Ok(P2::new(d.dot(plane.x_dir), d.dot(plane.y_dir())))
}

/// Draws a shape into a sketch with the relations and dimensions the sketcher would give it.
fn draw(sketch: &mut Sketch, plane: &Plane, shape: &Shape, n: usize) -> Result<(), String> {
    let what = |part: &str| format!("shape {n} {part}");
    let coincident = |s: &mut Sketch, a: PointRef, b: PointRef| {
        s.constrain(ConstraintKind::Coincident { a, b });
    };
    match *shape {
        Shape::Rectangle { center, width, height } => {
            let c = to_plane(plane, center, &what("centre"))?;
            sketch.rectangle(c, width, height).map_err(|e| format!("{}: {e}", what("rectangle")))?;
        }
        Shape::Circle { center, diameter } => {
            let c = to_plane(plane, center, &what("centre"))?;
            sketch.circle_at(c, diameter).map_err(|e| format!("{}: {e}", what("circle")))?;
        }
        Shape::Slot { start, end, width } => {
            let c0 = to_plane(plane, start, &what("start"))?;
            let c1 = to_plane(plane, end, &what("end"))?;
            let r = positive("width", width)? / 2.0;
            let d = c1 - c0;
            if d.len() < 1e-6 {
                return Err(format!("{}: the two centres are the same point", what("slot")));
            }
            if d.v.abs() < 1e-9 {
                // Along u: the sketch crate's slot, fully defined.
                sketch.slot(c0.mid(c1), d.len() + width, width).map_err(|e| format!("{}: {e}", what("slot")))?;
                return Ok(());
            }
            let n = d.normalized().perp();
            let top = sketch.add(EntityKind::Line { a: c0 + n.scale(r), b: c1 + n.scale(r) }, false);
            let right = sketch.add(EntityKind::Arc { center: c1, start: c1 - n.scale(r), end: c1 + n.scale(r) }, false);
            let bottom = sketch.add(EntityKind::Line { a: c1 - n.scale(r), b: c0 - n.scale(r) }, false);
            let left = sketch.add(EntityKind::Arc { center: c0, start: c0 + n.scale(r), end: c0 - n.scale(r) }, false);
            let axis = sketch.add(EntityKind::Line { a: c0, b: c1 }, true);
            for (a, b) in [
                (PointRef::new(top, 1), PointRef::new(right, 2)),
                (PointRef::new(right, 1), PointRef::new(bottom, 0)),
                (PointRef::new(bottom, 1), PointRef::new(left, 2)),
                (PointRef::new(left, 1), PointRef::new(top, 0)),
                (PointRef::new(axis, 0), PointRef::new(left, 0)),
                (PointRef::new(axis, 1), PointRef::new(right, 0)),
            ] {
                coincident(sketch, a, b);
            }
            for (a, b) in [(top, right), (bottom, right), (top, left), (bottom, left)] {
                sketch.constrain(ConstraintKind::Tangent { a, b });
            }
            sketch.constrain(ConstraintKind::Equal { a: left, b: right });
            sketch.constrain(ConstraintKind::Radius {
                entity: right,
                value: width,
                diameter: true,
                label: c1 + n.scale(r + 6.0),
            });
            sketch.constrain(ConstraintKind::Distance {
                a: PointRef::new(axis, 0),
                b: PointRef::new(axis, 1),
                kind: sketch::DistanceKind::Aligned,
                value: d.len(),
                label: c0.mid(c1) - n.scale(r + 8.0),
            });
        }
    }
    Ok(())
}

/// What a run of lines has made so far.
#[derive(Default)]
struct Run {
    last_sketch: Option<FeatureId>,
    made: Vec<FeatureId>,
}

impl Run {
    fn sketch(&self, doc: &Document, name: &str) -> Result<FeatureId, String> {
        let id = if name.trim().eq_ignore_ascii_case("previous") {
            self.last_sketch
                .ok_or("`previous` names the sketch an earlier line of this proposal made, and there is none")?
        } else {
            feature_by_name(doc, name)?
        };
        match doc.feature(id) {
            Some(f) if f.is_sketch() => Ok(id),
            Some(f) => Err(format!("{} is not a sketch", f.name)),
            None => Err("the sketch no longer exists".into()),
        }
    }

    fn apply(&mut self, doc: &mut Document, cmd: Command) -> Result<Option<FeatureId>, String> {
        let made = doc.apply(cmd)?;
        if let Some(id) = made {
            self.made.push(id);
        }
        Ok(made)
    }
}

impl Line {
    /// Runs the line on `doc` through the command layer.
    fn run(&self, doc: &mut Document, run: &mut Run) -> Result<(), String> {
        match self {
            Line::Sketch { plane, shapes } => {
                if shapes.is_empty() {
                    return Err("a sketch needs at least one shape".into());
                }
                let at = sketch_plane(doc, plane)?;
                let id = run.apply(doc, Command::AddSketch { plane: at })?.ok_or("the sketch was not created")?;
                let plane = doc.feature(id).and_then(|f| f.plane).ok_or("the sketch plane did not resolve")?;
                let mut s = Sketch::new();
                for (i, shape) in shapes.iter().enumerate() {
                    draw(&mut s, &plane, shape, i + 1)?;
                }
                sketch::profile::profiles(&s, plane).map_err(|e| format!("the shapes do not make profiles: {e}"))?;
                run.apply(doc, Command::EditSketch { id, sketch: s })?;
                run.last_sketch = Some(id);
            }
            Line::Extrude { sketch, depth, flip } => {
                let params = ExtrudeParams {
                    sketch: Some(run.sketch(doc, sketch)?),
                    depth: positive("depth", *depth)?,
                    flip: *flip,
                    ..Default::default()
                };
                run.apply(doc, Command::AddExtrude { params, via_assistant: true })?;
            }
            Line::Cut { sketch, through_all, depth, flip } => {
                let params = CutParams {
                    sketch: run.sketch(doc, sketch)?,
                    depth: if *through_all { depth.max(1.0) } else { positive("depth", *depth)? },
                    through_all: *through_all,
                    flip: *flip,
                };
                run.apply(doc, Command::AddCut { params })?;
            }
            Line::Fillet { edges, radius } => {
                let edges = edges.iter().map(|e| edge_ref(doc, e)).collect::<Result<Vec<_>, _>>()?;
                let params = FilletParams { edges, radius: positive("radius", *radius)? };
                run.apply(doc, Command::AddFillet { params })?;
            }
            Line::Chamfer { edges, distance } => {
                let edges = edges.iter().map(|e| edge_ref(doc, e)).collect::<Result<Vec<_>, _>>()?;
                let params = ChamferParams { edges, distance: positive("distance", *distance)? };
                run.apply(doc, Command::AddChamfer { params })?;
            }
            Line::EditExtrude { feature, depth, flip } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Extrude(p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not an extrude"));
                };
                let params = ExtrudeParams { depth: positive("depth", *depth)?, flip: *flip, ..p };
                run.apply(doc, Command::EditExtrude { id, params })?;
            }
            Line::EditCut { feature, through_all, depth, flip } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Cut(p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a cut"));
                };
                let depth = if *through_all { depth.max(1.0) } else { positive("depth", *depth)? };
                let params = CutParams { through_all: *through_all, depth, flip: *flip, ..p };
                run.apply(doc, Command::EditCut { id, params })?;
            }
            Line::EditFillet { feature, radius } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Fillet(p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a fillet"));
                };
                let params = FilletParams { radius: positive("radius", *radius)?, ..p };
                run.apply(doc, Command::EditFillet { id, params })?;
            }
            Line::EditChamfer { feature, distance } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Chamfer(p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a chamfer"));
                };
                let params = ChamferParams { distance: positive("distance", *distance)?, ..p };
                run.apply(doc, Command::EditChamfer { id, params })?;
            }
            Line::Suppress { feature, suppressed } => {
                let id = feature_by_name(doc, feature)?;
                run.apply(doc, Command::Suppress { id, suppressed: *suppressed })?;
            }
            Line::DeleteFeature { feature } => {
                let id = feature_by_name(doc, feature)?;
                run.apply(doc, Command::DeleteFeature(id))?;
            }
        }
        Ok(())
    }

    /// The line's `op`: the id of its verb in the command registry.
    pub fn op(&self) -> &'static str {
        match self {
            Line::Sketch { .. } => "sketch",
            Line::Extrude { .. } => "extrude",
            Line::Cut { .. } => "cut",
            Line::Fillet { .. } => "fillet",
            Line::Chamfer { .. } => "chamfer",
            Line::EditExtrude { .. } => "edit_extrude",
            Line::EditCut { .. } => "edit_cut",
            Line::EditFillet { .. } => "edit_fillet",
            Line::EditChamfer { .. } => "edit_chamfer",
            Line::Suppress { .. } => "suppress",
            Line::DeleteFeature { .. } => "delete_feature",
        }
    }

    /// The line as people read it, for the card and the command log.
    pub fn describe(&self) -> (String, String) {
        let pt = |p: &Point| format!("({}, {}, {})", fmt_mm(p[0]), fmt_mm(p[1]), fmt_mm(p[2]));
        let end = |through: bool, depth: f64, flip: bool| {
            let mut s = if through { "through all".to_owned() } else { format!("{} mm", fmt_mm(depth)) };
            if flip {
                s.push_str(", flipped");
            }
            s
        };
        match self {
            Line::Sketch { plane, shapes } => {
                let list: Vec<String> = shapes
                    .iter()
                    .map(|s| match s {
                        Shape::Rectangle { center, width, height } => {
                            format!("rectangle {} × {} at {}", fmt_mm(*width), fmt_mm(*height), pt(center))
                        }
                        Shape::Circle { center, diameter } => format!("Ø{} at {}", fmt_mm(*diameter), pt(center)),
                        Shape::Slot { start, end, width } => {
                            let (a, b) = (glam::DVec3::from(*start), glam::DVec3::from(*end));
                            format!(
                                "slot {} × {} from {} to {}",
                                fmt_mm(a.distance(b) + width),
                                fmt_mm(*width),
                                pt(start),
                                pt(end)
                            )
                        }
                    })
                    .collect();
                (format!("Sketch on {plane}"), list.join("; "))
            }
            Line::Extrude { sketch, depth, flip } => {
                ("Extrude".into(), format!("{sketch}, {}", end(false, *depth, *flip)))
            }
            Line::Cut { sketch, through_all, depth, flip } => {
                ("Cut".into(), format!("{sketch}, {}", end(*through_all, *depth, *flip)))
            }
            Line::Fillet { edges, radius } => {
                ("Fillet".into(), format!("R{} on {}", fmt_mm(*radius), edges.join(", ")))
            }
            Line::Chamfer { edges, distance } => {
                ("Chamfer".into(), format!("{} mm on {}", fmt_mm(*distance), edges.join(", ")))
            }
            Line::EditExtrude { feature, depth, flip } => (format!("Edit {feature}"), end(false, *depth, *flip)),
            Line::EditCut { feature, through_all, depth, flip } => {
                (format!("Edit {feature}"), end(*through_all, *depth, *flip))
            }
            Line::EditFillet { feature, radius } => (format!("Edit {feature}"), format!("R{}", fmt_mm(*radius))),
            Line::EditChamfer { feature, distance } => (format!("Edit {feature}"), format!("{} mm", fmt_mm(*distance))),
            Line::Suppress { feature, suppressed } => {
                (if *suppressed { "Suppress" } else { "Unsuppress" }.into(), feature.clone())
            }
            Line::DeleteFeature { feature } => ("Delete".into(), feature.clone()),
        }
    }
}

/// Runs lines on `doc` in order. Each line resolves against the part as the earlier
/// lines left it.
fn run_lines(doc: &mut Document, lines: &[Line]) -> Result<Vec<FeatureId>, String> {
    let mut run = Run::default();
    for (i, line) in lines.iter().enumerate() {
        line.run(doc, &mut run).map_err(|e| format!("line {} ({}): {e}", i + 1, line.describe().0))?;
    }
    Ok(run.made)
}

/// Runs a proposal's lines, marking what they add as the assistant's.
fn run_marked(doc: &mut Document, lines: &[Line]) -> Result<Vec<FeatureId>, String> {
    doc.set_assistant_mark(true);
    let made = run_lines(doc, lines);
    doc.set_assistant_mark(false);
    made
}

/// Volume and area of the body the part shows.
fn body_mass(doc: &Document) -> Option<soliddraft_kernel_api::MassProperties> {
    doc.kernel().mass_properties(doc.display_body()?).ok()
}

/// An independent copy of the part on the same kernel, rebuilt from its features.
fn scratch_copy(doc: &Document) -> Result<Document, String> {
    let bytes = doc.model().to_bytes().map_err(|e| format!("cannot copy the part: {e}"))?;
    let model = soliddraft_model::Document::from_bytes(&bytes, doc.kernel().clone())
        .map_err(|e| format!("cannot copy the part: {e}"))?;
    // The viewport quality travels in the bytes, with the rest of the settings.
    Ok(Document::from_model(model))
}

fn status_text(doc: &Document, id: FeatureId) -> String {
    match doc.feature(id).map(|f| (&f.status, f.suppressed)) {
        Some((FeatureStatus::Ok, _)) => "ok".into(),
        Some((FeatureStatus::Error(e), _)) => format!("error: {e}"),
        Some((FeatureStatus::NeedsRepair(items), _)) => format!(
            "needs repair: {} no longer resolve{}",
            items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>().join(", "),
            if items.len() == 1 { "s" } else { "" }
        ),
        Some((FeatureStatus::Inactive, true)) => "suppressed".into(),
        Some((FeatureStatus::Inactive, false)) => "rolled back".into(),
        None => "missing".into(),
    }
}

fn broken(f: &crate::document::Feature) -> bool {
    matches!(f.status, FeatureStatus::Error(_) | FeatureStatus::NeedsRepair(_))
}

/// What a dry run found.
struct DryRun {
    rows: Vec<(String, String)>,
    effect: Value,
    preview: Preview,
}

fn dry_run(doc: &Document, lines: &[Line]) -> Result<DryRun, String> {
    let mut scratch = scratch_copy(doc)?;
    let before: Vec<(FeatureId, String, FeatureKind, bool, bool)> =
        scratch.features.iter().map(|f| (f.id, f.name.clone(), f.kind.clone(), f.suppressed, broken(f))).collect();
    let volume_before = body_mass(&scratch).map(|m| m.volume).unwrap_or(0.0);
    run_marked(&mut scratch, lines)?;
    // A proposal may not break what built before it, nor add something broken.
    let failing: Vec<String> = scratch
        .features
        .iter()
        .filter(|f| broken(f) && !before.iter().any(|b| b.0 == f.id && b.4))
        .map(|f| format!("{} ({})", f.name, status_text(&scratch, f.id)))
        .collect();
    if !failing.is_empty() {
        return Err(format!("after these lines the part does not rebuild: {}", failing.join("; ")));
    }
    let added: Vec<String> =
        scratch.features.iter().filter(|f| !before.iter().any(|b| b.0 == f.id)).map(|f| f.name.clone()).collect();
    let changed: Vec<String> = scratch
        .features
        .iter()
        .filter(|f| before.iter().any(|b| b.0 == f.id && (b.2 != f.kind || b.3 != f.suppressed)))
        .map(|f| f.name.clone())
        .collect();
    let removed: Vec<String> = before.iter().filter(|b| scratch.feature(b.0).is_none()).map(|b| b.1.clone()).collect();
    let volume_after = body_mass(&scratch).map(|m| m.volume).unwrap_or(0.0);
    let mut rows: Vec<(String, String)> = lines.iter().map(Line::describe).collect();
    let mut effect = Vec::new();
    if !added.is_empty() {
        effect.push(format!("+{} feature{}", added.len(), if added.len() == 1 { "" } else { "s" }));
    }
    if !changed.is_empty() {
        effect.push(format!("{} changed", changed.join(", ")));
    }
    if !removed.is_empty() {
        effect.push(format!("{} removed", removed.join(", ")));
    }
    rows.push(("Effect".into(), effect.join(", ")));
    rows.push(("Volume".into(), format!("{} → {} mm³", fmt_mm(volume_before.round()), fmt_mm(volume_after.round()))));
    let result = scratch.display_body().and_then(|b| scratch.meshes.get(&b).cloned());
    Ok(DryRun {
        rows,
        effect: json!({
            "features_added": added,
            "features_changed": changed,
            "features_removed": removed,
            "volume_before_mm3": round(volume_before),
            "volume_after_mm3": round(volume_after),
        }),
        preview: Preview { result, tool: None, removes: false },
    })
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Where a proposal stands.
#[derive(Clone, Debug, PartialEq)]
pub enum ProposalStatus {
    /// Waiting for the user.
    Pending,
    /// Applied as one undo step.
    Applied {
        /// The undo step's label.
        label: String,
        /// The undo stack's depth right after applying: the proposal's step is on top
        /// while the depth and label still match.
        depth: usize,
        /// Features the proposal made.
        made: Vec<FeatureId>,
    },
    /// Applied, then undone.
    Undone,
    /// Turned down by the user.
    Discarded,
}

/// A proposed change.
#[derive(Clone, Debug)]
pub struct Proposal {
    /// `P1`, `P2`, ...
    pub id: String,
    /// One-line summary from the proposer.
    pub summary: String,
    /// The recipe lines.
    pub lines: Vec<Line>,
    /// Who proposed it: the user's prompt and the model, or the MCP client.
    pub origin: Caller,
    /// What it does to the part, as rows for the card.
    pub rows: Vec<(String, String)>,
    /// The part as it would be after the proposal.
    pub preview: Preview,
    /// Where it stands.
    pub status: ProposalStatus,
}

impl Proposal {
    /// Whether it waits for the user.
    pub fn pending(&self) -> bool {
        matches!(self.status, ProposalStatus::Pending)
    }

    fn origin_text(&self) -> String {
        match &self.origin {
            Caller::Assistant { prompt, model } => format!("assistant, model {model}, prompt {prompt:?}"),
            Caller::Mcp { client } => format!("MCP client {client}"),
        }
    }
}

/// The session's proposals and the auto-apply switch.
#[derive(Default)]
pub struct ProposalBook {
    /// Every proposal of the session, oldest first.
    pub proposals: Vec<Proposal>,
    /// Whether the assistant may apply its own proposals.
    pub auto_apply: bool,
    next: u32,
}

impl ProposalBook {
    /// The proposal with this id.
    pub fn get(&self, id: &str) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id.trim())
    }

    fn index(&self, id: &str) -> Result<usize, String> {
        self.proposals.iter().position(|p| p.id == id.trim()).ok_or_else(|| format!("no proposal `{id}`"))
    }

    /// A proposal's status as the part stands now: an applied proposal whose features
    /// are gone (the user pressed Undo in the Edit menu) reads as undone.
    pub fn status(&self, doc: &Document, id: &str) -> Option<ProposalStatus> {
        let p = self.get(id)?;
        Some(match &p.status {
            ProposalStatus::Applied { made, .. }
                if !made.is_empty() && made.iter().all(|f| doc.feature(*f).is_none()) =>
            {
                ProposalStatus::Undone
            }
            s => s.clone(),
        })
    }

    /// Whether the proposal's undo step is the one on top of the undo stack.
    pub fn can_undo(&self, doc: &Document, id: &str) -> bool {
        matches!(self.get(id).map(|p| &p.status), Some(ProposalStatus::Applied { label, depth, .. })
            if doc.undo_depth() == *depth && doc.undo_label() == Some(label.as_str()))
    }

    /// Tries `lines` on a scratch copy of `doc` and records a pending proposal.
    pub fn propose(
        &mut self,
        doc: &Document,
        summary: &str,
        lines: Vec<Line>,
        origin: Caller,
    ) -> Result<(String, Value), String> {
        if lines.is_empty() {
            return Err("a proposal needs at least one line".into());
        }
        let run = dry_run(doc, &lines)?;
        self.next += 1;
        let id = format!("P{}", self.next);
        self.proposals.push(Proposal {
            id: id.clone(),
            summary: summary.trim().to_owned(),
            lines,
            origin,
            rows: run.rows,
            preview: run.preview,
            status: ProposalStatus::Pending,
        });
        Ok((id, run.effect))
    }

    /// Applies a pending proposal through the command layer, as one undo step. On any
    /// failure the part is put back as it was.
    pub fn apply(&mut self, doc: &mut Document, id: &str) -> Result<Vec<FeatureId>, String> {
        let i = self.index(id)?;
        let p = &self.proposals[i];
        match p.status {
            ProposalStatus::Pending => {}
            ProposalStatus::Applied { .. } => return Err(format!("{} is already applied", p.id)),
            ProposalStatus::Undone => return Err(format!("{} was applied and undone; propose it again", p.id)),
            ProposalStatus::Discarded => return Err(format!("{} was discarded", p.id)),
        }
        let depth = doc.undo_depth();
        let lines = p.lines.clone();
        let made = match run_marked(doc, &lines) {
            Ok(made) => made,
            Err(e) => {
                doc.abandon(depth).ok();
                return Err(e);
            }
        };
        let label = format!("Apply {}: {}", p.id, p.summary);
        doc.group_undo(depth, label.clone());
        doc.log_note(format!("apply {} \"{}\" ({})", p.id, p.summary, p.origin_text()));
        for line in &lines {
            let (k, v) = line.describe();
            doc.log_note(format!("  {k}: {v}"));
        }
        if let Some(last) = made.iter().rev().find(|f| doc.feature(**f).is_some_and(|f| !f.is_sketch())) {
            doc.selection = Some(Selection::Feature(*last));
        }
        self.proposals[i].status = ProposalStatus::Applied { label, depth: doc.undo_depth(), made: made.clone() };
        Ok(made)
    }

    /// Undoes an applied proposal, if its step is the last one on the undo stack.
    pub fn undo(&mut self, doc: &mut Document, id: &str) -> Result<(), String> {
        let i = self.index(id)?;
        if !self.can_undo(doc, id) {
            return Err(match self.proposals[i].status {
                ProposalStatus::Applied { .. } => {
                    format!("the part changed after {} was applied; undo the later changes first", self.proposals[i].id)
                }
                _ => format!("{} is not applied", self.proposals[i].id),
            });
        }
        doc.undo()?;
        doc.log_note(format!("undo {}", self.proposals[i].id));
        self.proposals[i].status = ProposalStatus::Undone;
        Ok(())
    }

    /// Discards a pending proposal.
    pub fn discard(&mut self, id: &str) -> Result<(), String> {
        let i = self.index(id)?;
        if !self.proposals[i].pending() {
            return Err(format!("{} is not pending", self.proposals[i].id));
        }
        self.proposals[i].status = ProposalStatus::Discarded;
        Ok(())
    }
}

/// Which set of tools a caller gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The in-app assistant: read tools, `propose` and `apply`.
    Assistant,
    /// An MCP client: the same, plus every verb as a direct tool, `undo` and `redo`.
    Mcp,
}

/// The command layer as a [`ToolHost`].
pub struct CommandTools<'a> {
    /// The part.
    pub doc: &'a mut Document,
    /// The session's proposals.
    pub book: &'a mut ProposalBook,
    /// Which tools to offer.
    pub surface: Surface,
}

/// What an MCP client is told about this server in `initialize`.
pub const MCP_INSTRUCTIONS: &str = "SolidDraft, a parametric CAD application. The tools drive the open part through \
the same command layer as the ribbon. Units are millimetres and Z is up. Read the part with model_summary and \
feature; change it with the verb tools (each runs at once as one undo step) or with propose and apply; undo and \
redo step through the undo stack. Entities are named as the interface names them: Boss-Extrude1 for a feature, \
Boss-Extrude1 · End cap for a face, Boss-Extrude1 · Edge 3 for an edge.";

/// How agents are told to name entities, for the tools that take names and their errors.
macro_rules! names_help {
    () => {
        "Entities are named as the interface names them: a feature by its tree name (Boss-Extrude1), a face or edge \
         as `<feature> · <label>` (Boss-Extrude1 · End cap, Boss-Extrude1 · Edge 3), a vertex as `<feature> · \
         Vertex <n>`."
    };
}

const NAMES_HELP: &str = names_help!();

/// The tools for a surface, in the registry's order.
pub fn tool_specs(surface: Surface) -> Vec<ToolSpec> {
    commands::for_agents()
        .filter(|c| surface == Surface::Mcp || c.agents == Agents::Tool)
        .map(CommandSpec::tool_spec)
        .collect()
}

fn vec3(v: Vec3) -> Value {
    json!([round(v.x as f64), round(v.y as f64), round(v.z as f64)])
}

fn kvec(v: soliddraft_kernel_api::Vec3) -> Value {
    json!([round(v.x), round(v.y), round(v.z)])
}

/// Finds an entity by the name the interface gives it.
pub fn find_entity(doc: &Document, name: &str) -> Option<Selection> {
    let name = name.trim();
    if let Some(f) = doc.features.iter().find(|f| f.name == name) {
        return Some(Selection::Feature(f.id));
    }
    if let Some(face) = face_by_name(doc, name) {
        return Some(Selection::Face(face));
    }
    if let Some(edge) = edge_by_name(doc, name) {
        return Some(Selection::Edge(edge));
    }
    let (feature, label) = split_name(name)?;
    let f = doc.features.iter().find(|f| f.name == feature)?;
    let n: u32 = label.to_ascii_lowercase().strip_prefix("vertex")?.trim().parse().ok()?;
    let key = VertexKey { feature: f.id, index: n.checked_sub(1)? };
    doc.vertex_position(key).map(|_| Selection::Vertex(key))
}

/// The point an entity is measured from.
fn reference_point(doc: &Document, sel: Selection) -> Option<Vec3> {
    match sel {
        Selection::Feature(id) => {
            doc.mass_properties(id).map(|m| Vec3::new(m.centroid.x as f32, m.centroid.y as f32, m.centroid.z as f32))
        }
        Selection::Face(face) => doc.named_face(face).map(|x| x.centroid),
        Selection::Edge(edge) => doc.named_edge(edge).map(|x| x.midpoint),
        Selection::Vertex(key) => doc.vertex_position(key),
    }
}

/// Whether an edge is an outside (convex) edge or an inside corner (concave), from the
/// display mesh. One of its faces must be planar: a point of the other face next to the
/// edge lies outside the planar face's plane exactly when the corner is concave.
fn corner(doc: &Document, edge_mid: Vec3, a: FaceId, b: FaceId) -> &'static str {
    let planar = |f: FaceId| doc.named_face(f).is_some_and(|n| n.surface == SurfaceKind::Plane);
    let (a, b) = match (planar(a), planar(b)) {
        (true, _) => (a, b),
        (false, true) => (b, a),
        (false, false) => return "curved",
    };
    let Some(pa) = doc.face_plane(a) else { return "unknown" };
    let na = Vec3::new(pa.normal.x as f32, pa.normal.y as f32, pa.normal.z as f32);
    if planar(b)
        && let Some(pb) = doc.face_plane(b)
        && na.dot(Vec3::new(pb.normal.x as f32, pb.normal.y as f32, pb.normal.z as f32)) > 0.999
    {
        return "smooth";
    }
    let (Some(mesh), Some(pick)) = (doc.meshes.values().next(), doc.pick_of_face(b)) else { return "unknown" };
    let near = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| mesh.face_picks[t[0] as usize] == pick)
        .map(|t| (mesh.positions[t[0] as usize] + mesh.positions[t[1] as usize] + mesh.positions[t[2] as usize]) / 3.0)
        .min_by(|x, y| x.distance(edge_mid).total_cmp(&y.distance(edge_mid)));
    match near {
        Some(c) if (c - edge_mid).dot(na) > 1e-4 => "concave",
        Some(_) => "convex",
        None => "unknown",
    }
}

fn face_json(doc: &Document, face: FaceId) -> Value {
    let Some(nf) = doc.named_face(face) else { return Value::Null };
    let mut v = json!({
        "name": name_of_face(doc, face),
        "surface": format!("{:?}", nf.surface).to_lowercase(),
        "area_mm2": round(nf.area),
        "centroid": vec3(nf.centroid),
    });
    if nf.surface == SurfaceKind::Plane
        && let Some(p) = doc.face_plane(face)
    {
        v["normal"] = kvec(p.normal);
        v["sketch_frame"] = json!({ "origin": kvec(p.origin), "u": kvec(p.x_dir), "v": kvec(p.y_dir()) });
    }
    v
}

fn edge_json(
    doc: &Document,
    edge: soliddraft_kernel_api::EdgeId,
    topology: Option<&soliddraft_kernel_api::Topology>,
) -> Value {
    let Some(ne) = doc.named_edge(edge) else { return Value::Null };
    let mut v = json!({
        "name": name_of_edge(doc, edge),
        "length_mm": round(ne.length),
        "midpoint": vec3(ne.midpoint),
    });
    if let Some(info) = topology.and_then(|t| t.edges.iter().find(|e| e.id == edge)) {
        v["faces"] = info.faces.iter().filter_map(|f| name_of_face(doc, *f)).collect::<Vec<_>>().into();
        match info.faces[..] {
            // The seam of a cylinder or cone joins a face to itself.
            [a, b] if a == b => v["corner"] = "seam".into(),
            [a, b] => v["corner"] = corner(doc, ne.midpoint, a, b).into(),
            [_] => v["corner"] = "seam".into(),
            _ => {}
        }
    }
    v
}

fn selection_json(doc: &Document, sel: Selection) -> Value {
    let Some(info) = doc.describe(sel) else { return Value::Null };
    let topology = doc.display_body().and_then(|b| doc.kernel().topology(b).ok());
    let mut v = match sel {
        Selection::Face(f) => face_json(doc, f),
        Selection::Edge(e) => edge_json(doc, e, topology.as_ref()),
        _ => json!({}),
    };
    v["name"] = info.title.into();
    v["kind"] = match sel {
        Selection::Feature(_) => "feature",
        Selection::Face(_) => "face",
        Selection::Edge(_) => "edge",
        Selection::Vertex(_) => "vertex",
    }
    .into();
    v["details"] =
        info.details.into_iter().map(|(k, v)| (k, Value::String(v))).collect::<serde_json::Map<_, _>>().into();
    v
}

fn params_json(doc: &Document, kind: &FeatureKind) -> Value {
    let name = |id: FeatureId| doc.feature(id).map(|f| f.name.clone()).unwrap_or_else(|| "missing".into());
    let labels = |refs: &[EdgeRef]| refs.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
    match kind {
        FeatureKind::Sketch(s) => json!({
            "type": "sketch",
            "plane": s.plane.label(),
            "entities": s.sketch.drawn().count(),
            "state": s.sketch.status.label(),
        }),
        FeatureKind::Extrude(p) => match p.sketch {
            Some(s) => json!({ "type": "extrude", "sketch": name(s), "depth": p.depth, "flip": p.flip }),
            None => json!({
                "type": "extrude", "profile": p.profile.summary(), "plane": p.plane.label(), "depth": p.depth, "flip": p.flip,
            }),
        },
        FeatureKind::Cut(p) => json!({
            "type": "cut", "sketch": name(p.sketch), "through_all": p.through_all, "depth": p.depth, "flip": p.flip,
        }),
        FeatureKind::Fillet(p) => json!({ "type": "fillet", "edges": labels(&p.edges), "radius": p.radius }),
        FeatureKind::Chamfer(p) => json!({ "type": "chamfer", "edges": labels(&p.edges), "distance": p.distance }),
    }
}

impl CommandTools<'_> {
    /// The part summary the assistant sees at the start of every message.
    pub fn summary(&self) -> Value {
        let doc = &*self.doc;
        let (lo, hi) = doc.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let mass = body_mass(doc);
        json!({
            "part": doc.name,
            "units": "mm",
            "features": doc.features.iter().map(|f| {
                let mut v = params_json(doc, &f.kind);
                v["name"] = f.name.clone().into();
                v["status"] = status_text(doc, f.id).into();
                if f.via_assistant {
                    v["via_assistant"] = true.into();
                }
                v
            }).collect::<Vec<_>>(),
            "body": {
                "bounding_box": { "min": vec3(lo), "max": vec3(hi) },
                "volume_mm3": mass.map(|m| round(m.volume)),
                "area_mm2": mass.map(|m| round(m.area)),
                "faces": doc.faces.len(),
                "edges": doc.edges.len(),
            },
            "selection": doc.selection.map(|s| selection_json(doc, s)).unwrap_or(Value::Null),
            "pending_proposals": self.book.proposals.iter().filter(|p| p.pending())
                .map(|p| json!({ "id": p.id, "summary": p.summary })).collect::<Vec<_>>(),
            "auto_apply": self.book.auto_apply,
            "undo": doc.undo_label(),
        })
    }

    fn feature(&self, name: &str) -> Result<Value, String> {
        let doc = &*self.doc;
        let id = feature_by_name(doc, name)?;
        let f = doc.feature(id).expect("found by name");
        let mut v = params_json(doc, &f.kind);
        v["name"] = f.name.clone().into();
        v["status"] = status_text(doc, id).into();
        let topology = doc.display_body().and_then(|b| doc.kernel().topology(b).ok());
        v["faces"] = doc.faces.iter().filter(|x| x.feature == id).map(|x| face_json(doc, x.id)).collect();
        v["edges"] =
            doc.edges.iter().filter(|x| x.feature == id).map(|x| edge_json(doc, x.id, topology.as_ref())).collect();
        if let Some(plane) = f.plane {
            v["sketch_frame"] = json!({
                "origin": kvec(plane.origin), "normal": kvec(plane.normal), "u": kvec(plane.x_dir), "v": kvec(plane.y_dir()),
            });
        }
        Ok(v)
    }

    fn measure(&self, a: &str, b: &str) -> Result<Value, String> {
        let doc = &*self.doc;
        let find = |name: &str| find_entity(doc, name).ok_or_else(|| format!("no entity named `{name}`. {NAMES_HELP}"));
        let (sa, sb) = (find(a)?, find(b)?);
        let point = |name: &str, s| reference_point(doc, s).ok_or_else(|| format!("`{name}` has no reference point"));
        let (pa, pb) = (point(a, sa)?, point(b, sb)?);
        let d = pb - pa;
        let mut v = json!({
            "a": { "name": a, "point": vec3(pa) },
            "b": { "name": b, "point": vec3(pb) },
            "distance_mm": round(d.length() as f64),
            "dx_mm": round(d.x as f64), "dy_mm": round(d.y as f64), "dz_mm": round(d.z as f64),
        });
        if let (Selection::Face(fa), Selection::Face(fb)) = (sa, sb)
            && let (Some(qa), Some(qb)) = (doc.face_plane(fa), doc.face_plane(fb))
            && qa.normal.dot(qb.normal).abs() > 0.9999
        {
            v["gap_mm"] = round((qb.origin - qa.origin).dot(qa.normal).abs()).into();
        }
        Ok(v)
    }

    fn propose(&mut self, input: &Value, caller: &Caller) -> Result<Value, String> {
        let lines: Vec<Line> =
            serde_json::from_value(input["commands"].clone()).map_err(|e| format!("bad commands: {e}"))?;
        self.propose_lines(input["summary"].as_str().unwrap_or_default(), lines, caller)
    }

    fn propose_lines(&mut self, summary: &str, lines: Vec<Line>, caller: &Caller) -> Result<Value, String> {
        let (id, effect) = self.book.propose(self.doc, summary, lines, caller.clone())?;
        let auto = match caller {
            Caller::Assistant { .. } => self.book.auto_apply,
            Caller::Mcp { .. } => false,
        };
        if auto {
            let made = self.book.apply(self.doc, &id)?;
            return Ok(json!({
                "proposal_id": id, "status": "applied", "effect": effect,
                "made": self.names(&made),
                "note": "Auto-apply is on, so the proposal was applied. The user can undo it from its card.",
            }));
        }
        Ok(json!({
            "proposal_id": id,
            "status": "pending",
            "effect": effect,
            "note": match caller {
                Caller::Assistant { .. } => "The user now sees a card with Preview, Apply and Discard. Do not call apply; tell the user what the proposal does.",
                Caller::Mcp { .. } => "Call apply with this id to run it.",
            },
        }))
    }

    fn apply(&mut self, id: &str, caller: &Caller) -> Result<Value, String> {
        if matches!(caller, Caller::Assistant { .. }) && !self.book.auto_apply {
            return Err("only the user can apply proposals in this session: they apply it from its card. \
                        Tell them the proposal is ready."
                .into());
        }
        let made = self.book.apply(self.doc, id)?;
        Ok(json!({ "proposal_id": id, "status": "applied", "made": self.names(&made) }))
    }

    fn names(&self, ids: &[FeatureId]) -> Vec<String> {
        ids.iter().filter_map(|id| self.doc.feature(*id)).map(|f| f.name.clone()).collect()
    }
}

impl ToolHost for CommandTools<'_> {
    fn tools(&self) -> Vec<ToolSpec> {
        tool_specs(self.surface)
    }

    fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput {
        let mcp = self.surface == Surface::Mcp;
        let result = match name {
            "model_summary" => Ok(self.summary()),
            "feature" => self.feature(input["name"].as_str().unwrap_or_default()),
            "selection" => Ok(json!({
                "selection": self.doc.selection.map(|s| selection_json(self.doc, s)).unwrap_or(Value::Null)
            })),
            "measure" => self.measure(input["a"].as_str().unwrap_or_default(), input["b"].as_str().unwrap_or_default()),
            "propose" => self.propose(&input, caller),
            "apply" => self.apply(input["proposal_id"].as_str().unwrap_or_default(), caller),
            "undo" if mcp => self.doc.undo().map(|label| json!({ "undone": label, "undo": self.doc.undo_label() })),
            "redo" if mcp => self.doc.redo().map(|label| json!({ "redone": label })),
            verb if mcp && commands::verbs().any(|v| v.id == verb) => {
                let mut line = input.clone();
                line["op"] = json!(verb);
                match serde_json::from_value::<Line>(line) {
                    Ok(line) => {
                        let (k, v) = line.describe();
                        self.propose_lines(&format!("{k}: {v}"), vec![line], caller).and_then(|r| {
                            let id = r["proposal_id"].as_str().unwrap_or_default().to_owned();
                            self.apply(&id, caller).map(|mut a| {
                                a["effect"] = r["effect"].clone();
                                a
                            })
                        })
                    }
                    Err(e) => Err(format!("bad input: {e}")),
                }
            }
            other => Err(format!("unknown tool `{other}`")),
        };
        match result {
            Ok(v) => ToolOutput::json(&v),
            Err(e) => ToolOutput::error(e),
        }
    }
}

/// The sample L-bracket before the assistant's three exchanges: a 120 × 90 × 10 base
/// plate from the origin, a 10 mm flange 60 mm tall along its front edge, and a Ø24
/// boss with a Ø12 bore. The fillet, the hole pattern and the slot are left for the
/// assistant.
pub fn board_bracket() -> Document {
    board_bracket_on(crate::kernel())
}

/// Builds the assistant sample on an explicit backend for regression scenarios.
pub fn board_bracket_on(kernel: std::sync::Arc<dyn soliddraft_kernel_api::Kernel>) -> Document {
    let mut doc = Document::new("Part1", kernel);
    let rect = |center: Point, width: f64, height: f64| Shape::Rectangle { center, width, height };
    let circle = |center: Point, diameter: f64| Shape::Circle { center, diameter };
    let steps = [
        vec![
            Line::Sketch { plane: "top".into(), shapes: vec![rect([60.0, 45.0, 0.0], 120.0, 90.0)] },
            Line::Extrude { sketch: "previous".into(), depth: 10.0, flip: false },
        ],
        vec![
            Line::Sketch { plane: "front".into(), shapes: vec![rect([60.0, 0.0, 30.0], 120.0, 60.0)] },
            Line::Extrude { sketch: "previous".into(), depth: 10.0, flip: true },
        ],
        vec![
            Line::Sketch { plane: "Boss-Extrude1 · End cap".into(), shapes: vec![circle([85.0, 55.0, 10.0], 24.0)] },
            Line::Extrude { sketch: "previous".into(), depth: 24.0, flip: false },
        ],
        vec![
            Line::Sketch { plane: "Boss-Extrude3 · End cap".into(), shapes: vec![circle([85.0, 55.0, 34.0], 12.0)] },
            Line::Cut { sketch: "previous".into(), through_all: true, depth: 1.0, flip: false },
        ],
    ];
    for lines in steps {
        run_lines(&mut doc, &lines).expect("the sample bracket builds");
    }
    doc.selection = None;
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use soliddraft_assistant::tools::{call_checked, check_strict};

    fn assistant() -> Caller {
        Caller::Assistant { prompt: "add holes".into(), model: soliddraft_assistant::DEFAULT_MODEL.into() }
    }

    #[test]
    fn every_schema_is_strict_and_within_the_limits() {
        for surface in [Surface::Assistant, Surface::Mcp] {
            for t in tool_specs(surface) {
                check_strict(&t.input_schema).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            }
        }
        let tools = tool_specs(Surface::Assistant);
        assert!(tools.len() <= 20, "strict tools per request");
        let text = serde_json::to_string(&tools.iter().map(|t| &t.input_schema).collect::<Vec<_>>()).unwrap();
        assert!(text.matches("anyOf").count() <= 16, "union parameters");
    }

    #[test]
    fn every_recipe_line_is_a_registry_verb() {
        let feature = || "Boss-Extrude1".to_owned();
        let lines = [
            Line::Sketch { plane: "top".into(), shapes: Vec::new() },
            Line::Extrude { sketch: "previous".into(), depth: 1.0, flip: false },
            Line::Cut { sketch: "previous".into(), through_all: true, depth: 1.0, flip: false },
            Line::Fillet { edges: Vec::new(), radius: 1.0 },
            Line::Chamfer { edges: Vec::new(), distance: 1.0 },
            Line::EditExtrude { feature: feature(), depth: 1.0, flip: false },
            Line::EditCut { feature: feature(), through_all: false, depth: 1.0, flip: false },
            Line::EditFillet { feature: feature(), radius: 1.0 },
            Line::EditChamfer { feature: feature(), distance: 1.0 },
            Line::Suppress { feature: feature(), suppressed: true },
            Line::DeleteFeature { feature: feature() },
        ];
        let mut ops: Vec<&str> = lines.iter().map(Line::op).collect();
        for line in &lines {
            assert_eq!(serde_json::to_value(line).unwrap()["op"], line.op(), "the serde tag is the op");
        }
        let mut verbs: Vec<&str> = commands::verbs().map(|c| c.id).collect();
        ops.sort();
        verbs.sort();
        assert_eq!(ops, verbs, "every line kind has a verb in the registry, and every verb a line kind");
        for c in commands::verbs() {
            let names: Vec<&str> = (c.params)().into_iter().map(|(n, _)| n).collect();
            assert!(!names.is_empty(), "{} is a verb without parameters", c.id);
        }
    }

    #[test]
    fn the_tool_lists_come_from_the_registry() {
        let names = |s: Surface| tool_specs(s).into_iter().map(|t| t.name).collect::<Vec<_>>();
        let assistant: Vec<&str> = commands::all().iter().filter(|c| c.agents == Agents::Tool).map(|c| c.id).collect();
        assert_eq!(names(Surface::Assistant), assistant);
        let mcp: Vec<&str> = commands::for_agents().map(|c| c.id).collect();
        assert_eq!(names(Surface::Mcp), mcp);
        for t in tool_specs(Surface::Mcp) {
            let c = commands::get(&t.name).unwrap();
            assert!(t.description.starts_with(c.doc), "{}: the description starts with the registry's doc", t.name);
        }
        for name in ["model_summary", "feature", "selection", "measure", "propose", "apply", "undo", "redo"] {
            assert!(names(Surface::Mcp).iter().any(|n| n == name), "MCP lists {name}");
        }
        assert!(!names(Surface::Assistant).iter().any(|n| n == "undo" || n == "extrude"));
    }

    #[test]
    fn the_board_bracket_builds() {
        let doc = board_bracket_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()));
        let names: Vec<&str> = doc.features.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Sketch1",
                "Boss-Extrude1",
                "Sketch2",
                "Boss-Extrude2",
                "Sketch3",
                "Boss-Extrude3",
                "Sketch4",
                "Cut-Extrude1"
            ]
        );
        assert!(
            doc.features.iter().all(|f| f.status == FeatureStatus::Ok),
            "{:?}",
            doc.features.iter().map(|f| (&f.name, &f.status)).collect::<Vec<_>>()
        );
        let v = body_mass(&doc).unwrap().volume;
        let pi = std::f64::consts::PI;
        let expected = 120.0 * 90.0 * 10.0 + 120.0 * 10.0 * 50.0 + pi * 144.0 * 24.0 - pi * 36.0 * 34.0;
        assert!((v - expected).abs() < 50.0, "volume {v}, expected {expected}");
    }

    #[test]
    fn propose_apply_undo_is_one_step() {
        let mut doc = board_bracket_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()));
        let mut book = ProposalBook::default();
        let before = doc.features.len();
        let depth = doc.undo_depth();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant };
        let input = json!({
            "summary": "Four Ø9 holes on 90 × 44",
            "commands": [
                { "op": "sketch", "plane": "Boss-Extrude1 · End cap", "shapes": [
                    { "shape": "circle", "center": [15, 28, 10], "diameter": 9 },
                    { "shape": "circle", "center": [105, 28, 10], "diameter": 9 },
                    { "shape": "circle", "center": [15, 72, 10], "diameter": 9 },
                    { "shape": "circle", "center": [105, 72, 10], "diameter": 9 }
                ]},
                { "op": "cut", "sketch": "previous", "through_all": true, "depth": 10, "flip": false }
            ]
        });
        let out = call_checked(&mut tools, "propose", input, &assistant());
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["effect"]["features_added"], json!(["Sketch5", "Cut-Extrude2"]));
        let removed =
            v["effect"]["volume_before_mm3"].as_f64().unwrap() - v["effect"]["volume_after_mm3"].as_f64().unwrap();
        assert!((removed - 4.0 * std::f64::consts::PI * 4.5 * 4.5 * 10.0).abs() < 5.0, "removed {removed}");
        assert_eq!(tools.doc.features.len(), before, "proposing changes nothing");
        assert!(tools.book.get("P1").unwrap().preview.result.is_some());

        assert!(call_checked(&mut tools, "apply", json!({ "proposal_id": "P1" }), &assistant()).is_error);
        tools.book.apply(tools.doc, "P1").unwrap();
        assert_eq!(tools.doc.features.len(), before + 2);
        assert_eq!(tools.doc.undo_depth(), depth + 1, "one undo step");
        assert!(tools.doc.features.iter().rev().take(2).all(|f| f.via_assistant));
        let log = tools.doc.command_log().join("\n");
        assert!(log.contains("prompt \"add holes\"") && log.contains("model claude-opus-5-5"), "{log}");
        tools.book.undo(tools.doc, "P1").unwrap();
        assert_eq!(tools.doc.features.len(), before);
        assert_eq!(tools.book.status(tools.doc, "P1"), Some(ProposalStatus::Undone));
    }

    #[test]
    fn unresolved_references_are_rejected_before_preview() {
        let mut doc = board_bracket_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()));
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant };
        for (commands, expect) in [
            (json!([{ "op": "fillet", "edges": ["Boss-Extrude2 · Edge 99"], "radius": 8 }]), "no edge named"),
            (json!([{ "op": "delete_feature", "feature": "Fillet9" }]), "no feature named `Fillet9`"),
            (
                json!([{ "op": "cut", "sketch": "previous", "through_all": true, "depth": 1, "flip": false }]),
                "previous",
            ),
            (
                json!([{ "op": "sketch", "plane": "top", "shapes": [{ "shape": "circle", "center": [0, 0, 5], "diameter": 4 }] }]),
                "off the sketch plane",
            ),
        ] {
            let out =
                call_checked(&mut tools, "propose", json!({ "summary": "x", "commands": commands }), &assistant());
            assert!(out.is_error && out.content.contains(expect), "{expect}: {}", out.content);
        }
        assert!(tools.book.proposals.is_empty());
    }

    #[test]
    fn the_inside_corner_reads_as_concave() {
        let mut doc = board_bracket_on(std::sync::Arc::new(soliddraft_kernel_smlib::SmlibKernel::new()));
        let mut book = ProposalBook::default();
        let tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant };
        let all: Vec<Value> = tools
            .doc
            .features
            .iter()
            .flat_map(|f| tools.feature(&f.name).unwrap()["edges"].as_array().cloned().unwrap_or_default())
            .collect();
        let inside: Vec<&Value> = all.iter().filter(|e| e["corner"] == "concave").collect();
        let long: Vec<&&Value> = inside.iter().filter(|e| e["length_mm"].as_f64() == Some(120.0)).collect();
        assert_eq!(long.len(), 1, "{inside:#?}");
        assert_eq!(long[0]["midpoint"], json!([60.0, 10.0, 10.0]));
        // The bore's rims are outside edges; the boss's foot on the plate is an inside corner.
        let of = |name: &str| tools.feature(name).unwrap()["edges"].as_array().cloned().unwrap_or_default();
        let bore = of("Cut-Extrude1");
        assert!(bore.iter().filter(|e| e["corner"] == "convex").count() >= 2, "{bore:#?}");
        assert!(bore.iter().all(|e| e["corner"] == "convex" || e["corner"] == "seam"), "{bore:#?}");
        let foot: Vec<Value> =
            of("Boss-Extrude3").into_iter().filter(|e| e["midpoint"][2].as_f64() == Some(10.0)).collect();
        assert!(!foot.is_empty() && foot.iter().all(|e| e["corner"] == "concave"), "{foot:#?}");
    }

    #[test]
    fn mcp_verbs_run_directly_and_undo() {
        let mut doc = crate::new_document();
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp };
        let mcp = Caller::Mcp { client: "test".into() };
        let out = call_checked(
            &mut tools,
            "sketch",
            json!({ "plane": "top", "shapes": [{ "shape": "rectangle", "center": [0, 0, 0], "width": 40, "height": 30 }] }),
            &mcp,
        );
        assert!(!out.is_error, "{}", out.content);
        let out = call_checked(&mut tools, "extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }), &mcp);
        assert!(!out.is_error, "{}", out.content);
        let m = call_checked(
            &mut tools,
            "measure",
            json!({ "a": "Boss-Extrude1 · Start cap", "b": "Boss-Extrude1 · End cap" }),
            &mcp,
        );
        let v: Value = serde_json::from_str(&m.content).unwrap();
        assert_eq!(v["gap_mm"], 10.0, "{v}");
        assert!(!call_checked(&mut tools, "undo", json!({}), &mcp).is_error);
        assert_eq!(tools.doc.features.len(), 1);
        assert!(!call_checked(&mut tools, "redo", json!({}), &mcp).is_error);
        assert_eq!(tools.doc.features.len(), 2);
    }
}
