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
    ChamferParams, Command, CutParams, Document, EndKind, ExtrudeParams, FeatureId, FeatureKind, FeatureStatus,
    FilletParams, PlaneKind, Preview, Selection, SketchPlane, VertexKey, fmt_mm, hole_depth_text, hole_head_text,
    plane_ref_label,
};
use crate::sketch::{self, ConstraintKind, EntityKind, P2, PointRef, Sketch};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{
    any_of, array, boolean, constant, integer, non_empty_array, number, object, optional, string,
};
use soliddraft_assistant::{Caller, ToolHost, ToolOutput, ToolSpec};
use soliddraft_kernel_api::{FaceId, Plane, SurfaceKind};
use soliddraft_kernel_api::{HoleDepth, HoleHead, HoleTip};
use soliddraft_model::{DatumPlane, PatternSpec, PlaneRef};

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
    /// Adds an inactive configuration row.
    AddConfiguration { name: String },
    /// Changes one row override.
    SetConfigurationValue { configuration: String, name: String, expression: String },
    /// Selects a row and resolves its feature inputs.
    ActivateConfiguration { name: String },
    /// Creates or changes a named value.
    SetValue {
        /// The value identifier.
        name: String,
        /// Authored expression text.
        expression: String,
    },
    /// Renames a value and its expression references.
    RenameValue {
        /// The existing identifier.
        name: String,
        /// The new identifier.
        to: String,
    },
    /// Drives a sketch dimension or an extrusion depth.
    Drive {
        /// Sketch name, for a dimension.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sketch: Option<String>,
        /// Dimension number from 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dimension: Option<usize>,
        /// Extrusion name, for a depth.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feature: Option<String>,
        /// The parameter name, depth.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parameter: Option<String>,
        /// Authored driver text.
        expression: String,
    },
    /// A sketch of closed shapes on a reference plane or a planar face.
    Sketch {
        /// `top`, `front`, `right`, or a planar face's name.
        plane: String,
        /// Signed datum offset along its normal, mm; omitted means zero. Faces require zero.
        #[serde(default)]
        offset: f64,
        /// The shapes.
        shapes: Vec<Shape>,
    },
    /// A sketch built with the analytic geometry tools, in plane coordinates.
    SketchGeometry {
        /// Reference plane or planar face name.
        plane: String,
        /// Signed datum offset along its normal, mm; omitted means zero. Faces require zero.
        #[serde(default)]
        offset: f64,
        /// Operations in order; entity ids start at 1 and are never reused.
        operations: Vec<sketch::tools::Operation>,
    },
    /// Adds material: extrudes a sketch's profiles and joins them to the part.
    Extrude {
        /// The sketch's name, or `previous` for the sketch an earlier line made.
        sketch: String,
        /// Depth, mm.
        depth: f64,
        /// Extrude against the sketch normal.
        flip: bool,
        /// Stop at the first place the sweep crosses the part's boundary; `depth` is ignored.
        #[serde(default, skip_serializing_if = "is_false")]
        up_to_next: bool,
        /// Stop at the first hit on this face; `depth` is ignored.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_to_face: Option<String>,
    },
    /// Revolves a meridian sketch around a model-space axis.
    Revolve { sketch: String, axis_origin: [f64; 3], axis_direction: [f64; 3], angle_deg: f64 },
    /// Changes a revolution angle without changing its sketch or axis.
    EditRevolve { feature: String, angle_deg: f64 },
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
        /// Stop at the first place the sweep crosses the part's boundary.
        #[serde(default, skip_serializing_if = "is_false")]
        up_to_next: bool,
        /// Stop at the first hit on this face.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_to_face: Option<String>,
    },
    /// Drills holes into a planar face.
    Hole {
        /// The planar face's name.
        face: String,
        /// Hole centres: world points on the face.
        points: Vec<Point>,
        /// Diameter, mm.
        diameter: f64,
        /// Depth, mm; absent drills through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<f64>,
        /// Full drill-point angle of a blind hole, degrees; absent leaves a flat floor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tip_angle: Option<f64>,
        /// A counterbore at the entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        counterbore: Option<Counterbore>,
        /// A countersink at the entry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        countersink: Option<Countersink>,
    },
    /// Copies the part's body along a direction.
    LinearPattern {
        /// Direction, [x, y, z].
        direction: Point,
        /// Distance between copies, mm.
        spacing: f64,
        /// Copies, including the original.
        count: u32,
    },
    /// Copies the part's body around an axis.
    CircularPattern {
        /// A point on the axis.
        axis_origin: Point,
        /// The axis direction.
        axis_direction: Point,
        /// Copies, including the original.
        count: u32,
        /// Angle between copies, degrees.
        angle: f64,
    },
    /// Reflects the part's body in a plane.
    Mirror {
        /// `top`, `front`, `right`, or a planar face's name.
        plane: String,
        /// Unite the reflection with the original.
        keep_original: bool,
    },
    /// Replaces the finished body with a terminal cubic lattice.
    Lattice { cell: soliddraft_kernel_api::LatticeCell, cell_size: f64, radius: f64, accuracy: f64, min_detail: f64 },
    /// Edits only the supplied lattice settings.
    EditLattice {
        feature: String,
        #[serde(default)]
        cell_size: Option<f64>,
        #[serde(default)]
        radius: Option<f64>,
        #[serde(default)]
        accuracy: Option<f64>,
        #[serde(default)]
        min_detail: Option<f64>,
    },
    /// Replaces the finished body with a terminal network gyroid.
    Gyroid { kind: soliddraft_kernel_api::GyroidKind, period: f64, level: f64, accuracy: f64, min_detail: f64 },
    /// Edits only the supplied gyroid settings.
    EditGyroid {
        feature: String,
        #[serde(default)]
        period: Option<f64>,
        #[serde(default)]
        level: Option<f64>,
        #[serde(default)]
        accuracy: Option<f64>,
        #[serde(default)]
        min_detail: Option<f64>,
    },
    /// Grades a terminal field linearly along one part axis: a lattice's beam radius or a
    /// gyroid's level is `start` at part coordinate `from` and `end` at `to`.
    GradeField { feature: String, axis: soliddraft_kernel_api::GradeAxis, from: f64, to: f64, start: f64, end: f64 },
    /// Hollows the part, leaving named faces open.
    Shell {
        /// Faces left open; empty requests a closed shell.
        open_faces: Vec<String>,
        /// Positive wall thickness, mm.
        thickness: f64,
    },
    /// Tilts faces about a neutral plane.
    Draft {
        /// Face names.
        faces: Vec<String>,
        /// `top`, `front`, `right`, or a planar face's name.
        neutral: String,
        /// Draft angle, degrees.
        angle: f64,
        /// Pull against the neutral plane's normal.
        reverse: bool,
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
    /// Changes a hole's diameter and depth.
    EditHole {
        /// The feature's name.
        feature: String,
        /// New diameter, mm.
        diameter: f64,
        /// New depth, mm; absent drills through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<f64>,
        /// A new countersink at the entry; absent keeps the hole's head.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        countersink: Option<Countersink>,
    },
    /// Changes a pattern's copies and spacing or angle.
    EditPattern {
        /// The feature's name.
        feature: String,
        /// Copies, including the original.
        count: u32,
        /// Spacing in mm for a linear pattern, angle in degrees for a circular one.
        step: f64,
    },
    /// Changes whether a mirror keeps the original.
    EditMirror {
        /// The feature's name.
        feature: String,
        /// Unite the reflection with the original.
        keep_original: bool,
    },
    /// Changes a shell's wall thickness, retaining its openings.
    EditShell {
        /// The shell feature's name.
        feature: String,
        /// Positive wall thickness, mm.
        thickness: f64,
    },
    /// Changes a draft's angle.
    EditDraft {
        /// The feature's name.
        feature: String,
        /// New angle, degrees.
        angle: f64,
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

/// A counterbore in a hole line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counterbore {
    /// Diameter, mm.
    pub diameter: f64,
    /// Depth, mm.
    pub depth: f64,
}

/// A countersink in a hole line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Countersink {
    /// Diameter at the face, mm.
    pub diameter: f64,
    /// Full angle, degrees.
    pub angle: f64,
}

fn is_false(b: &bool) -> bool {
    !*b
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

/// The canonical command tool's parameters.
pub fn document_command_params() -> Vec<(&'static str, Value)> {
    vec![(
        "command_json",
        string("Canonical journal command as JSON: op execute, undo, redo, rollback, settings, group_undo or abandon"),
    )]
}

/// The rollback tool's parameters.
pub fn rollback_params() -> Vec<(&'static str, Value)> {
    vec![("before", any_of(vec![string("First inactive feature, or null for the end"), json!({"type":"null"})]))]
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
        ("offset", optional(number("Signed datum offset along its normal, mm; default 0. Not supported on faces"))),
        ("shapes", non_empty_array(shape_schema(), "The closed shapes")),
    ]
}

/// Parameters for sketches made by polygon and line/arc editing tools.
pub fn sketch_geometry_params() -> Vec<(&'static str, Value)> {
    vec![
        ("plane", string("top, front, right, or a planar face name")),
        ("offset", optional(number("Signed datum offset along its normal, mm; default 0. Not supported on faces"))),
        (
            "operations",
            non_empty_array(
                crate::sketch_tools::operation_schema(),
                "Sketch tools in order, using u/v plane coordinates in mm. Entity ids start at 1; operations return independent copies. Offset chains use selection walking order. Trim/extend discard target relations.",
            ),
        ),
    ]
}

/// The `extrude` verb's parameters.
pub fn extrude_params() -> Vec<(&'static str, Value)> {
    let mut params = vec![
        sketch_param(),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true extrudes against the sketch normal")),
    ];
    params.extend(up_to_params());
    params
}

/// The `revolve` verb's parameters.
pub fn revolve_params() -> Vec<(&'static str, Value)> {
    vec![
        sketch_param(),
        ("axis_origin", array(number("component"), "Point on the axis [x, y, z] in model millimetres")),
        ("axis_direction", array(number("component"), "Nonzero model-space axis direction [x, y, z]")),
        ("angle_deg", number("Angle in degrees, greater than 0 and at most 360")),
    ]
}

/// The `edit_revolve` verb's parameters.
pub fn edit_revolve_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("angle_deg", number("Angle in degrees, greater than 0 and at most 360"))]
}

/// The `cut` verb's parameters.
pub fn cut_params() -> Vec<(&'static str, Value)> {
    let mut params = vec![
        sketch_param(),
        ("through_all", boolean("true cuts through the whole part; depth is then ignored")),
        ("depth", number("Depth, mm, greater than 0")),
        ("flip", boolean("true cuts along the sketch normal instead of into the face")),
    ];
    params.extend(up_to_params());
    params
}

fn up_to_params() -> [(&'static str, Value); 2] {
    [
        ("up_to_next", optional(boolean("true stops at the first place the sweep crosses the part's boundary"))),
        ("up_to_face", optional(string("Stops at the first hit on this face, by name"))),
    ]
}

/// The `hole` verb's parameters.
pub fn hole_params() -> Vec<(&'static str, Value)> {
    vec![
        ("face", string("The planar face the holes start on, such as Boss-Extrude1 · End cap")),
        ("points", non_empty_array(point("A hole centre"), "Hole centres on the face")),
        ("diameter", number("Diameter, mm, greater than 0")),
        ("depth", optional(number("Depth, mm; leave out to drill through"))),
        ("tip_angle", optional(number("Drill-point angle of a blind hole, degrees, such as 118; leave out for flat"))),
        ("counterbore", optional(object(&[("diameter", number("Diameter, mm")), ("depth", number("Depth, mm"))]))),
        ("countersink", optional(countersink_param())),
    ]
}

fn countersink_param() -> Value {
    object(&[("diameter", number("Diameter at the face, mm")), ("angle", number("Full angle, degrees: 82, 90 or 100"))])
}

/// The `linear_pattern` verb's parameters.
pub fn linear_pattern_params() -> Vec<(&'static str, Value)> {
    vec![
        ("direction", array(number("component"), "Direction [x, y, z]")),
        ("spacing", number("Distance between copies, mm, greater than 0")),
        ("count", number("Copies including the original, at least 2")),
    ]
}

/// The `circular_pattern` verb's parameters.
pub fn circular_pattern_params() -> Vec<(&'static str, Value)> {
    vec![
        ("axis_origin", point("A point on the axis")),
        ("axis_direction", array(number("component"), "Axis direction [x, y, z]")),
        ("count", number("Copies including the original, at least 2")),
        ("angle", number("Angle between copies, degrees")),
    ]
}

/// The `mirror` verb's parameters.
pub fn mirror_params() -> Vec<(&'static str, Value)> {
    vec![
        ("plane", string("`top`, `front`, `right`, or a planar face's name")),
        ("keep_original", boolean("true unites the reflection with the original")),
    ]
}

/// The `lattice` verb's parameters.
pub fn lattice_params() -> Vec<(&'static str, Value)> {
    vec![
        ("cell", constant("cubic")),
        ("cell_size", number("Cell size in millimetres")),
        ("radius", number("Density as beam radius in millimetres")),
        ("accuracy", number("Certified export accuracy in millimetres")),
        ("min_detail", number("Minimum preserved detail in millimetres")),
    ]
}

/// The `edit_lattice` verb's parameters.
pub fn edit_lattice_params() -> Vec<(&'static str, Value)> {
    let mut params = vec![feature_param()];
    params.extend(
        lattice_params().into_iter().filter(|(key, _)| *key != "cell").map(|(key, value)| (key, optional(value))),
    );
    params
}

/// The `gyroid` verb's parameters.
pub fn gyroid_params() -> Vec<(&'static str, Value)> {
    vec![
        ("kind", constant("network")),
        ("period", number("Length of one period in millimetres")),
        ("level", number("Constant dimensionless level of the gyroid function, which may be zero or negative")),
        ("accuracy", number("Certified export accuracy in millimetres")),
        ("min_detail", number("Minimum preserved detail in millimetres")),
    ]
}

/// The `grade_field` verb's parameters.
pub fn grade_field_params() -> Vec<(&'static str, Value)> {
    vec![
        ("feature", string("The lattice or gyroid's tree name, such as Lattice1")),
        ("axis", json!({"type": "string", "enum": ["x", "y", "z"], "description": "The part axis it changes along"})),
        ("from", number("Part coordinate on the axis where the value is start, in millimetres")),
        ("to", number("Part coordinate on the axis where the value is end, in millimetres; not from")),
        ("start", number("The value at from and before it: beam radius in millimetres, or the dimensionless level")),
        ("end", number("The value at to and beyond it: beam radius in millimetres, or the dimensionless level")),
    ]
}

/// The `edit_gyroid` verb's parameters.
pub fn edit_gyroid_params() -> Vec<(&'static str, Value)> {
    let mut params = vec![feature_param()];
    params.extend(
        gyroid_params().into_iter().filter(|(key, _)| *key != "kind").map(|(key, value)| (key, optional(value))),
    );
    params
}

/// The `shell` verb's parameters.
pub fn shell_params() -> Vec<(&'static str, Value)> {
    vec![
        ("open_faces", array(string("A face name"), "Faces left open; empty for a closed shell")),
        ("thickness", number("Wall thickness, mm, greater than 0")),
    ]
}

/// The `edit_shell` verb's parameters.
pub fn edit_shell_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("thickness", number("Wall thickness, mm, greater than 0"))]
}

/// The `draft` verb's parameters.
pub fn draft_params() -> Vec<(&'static str, Value)> {
    vec![
        ("faces", non_empty_array(string("A face name"), "The faces to tilt")),
        ("neutral", string("The neutral plane: `top`, `front`, `right`, or a planar face's name")),
        ("angle", number("Draft angle, degrees, between -90 and 90")),
        ("reverse", boolean("true pulls against the neutral plane's normal")),
    ]
}

/// The `edit_hole` verb's parameters.
pub fn edit_hole_params() -> Vec<(&'static str, Value)> {
    vec![
        feature_param(),
        ("diameter", number("Diameter, mm, greater than 0")),
        ("depth", optional(number("Depth, mm; leave out to drill through"))),
        ("countersink", optional(countersink_param())),
    ]
}

/// The `edit_pattern` verb's parameters.
pub fn edit_pattern_params() -> Vec<(&'static str, Value)> {
    vec![
        feature_param(),
        ("count", number("Copies including the original, at least 2")),
        ("step", number("Spacing in mm for a linear pattern, angle in degrees for a circular one")),
    ]
}

/// The `edit_mirror` verb's parameters.
pub fn edit_mirror_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("keep_original", boolean("true unites the reflection with the original"))]
}

/// The `edit_draft` verb's parameters.
pub fn edit_draft_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("angle", number("Draft angle, degrees"))]
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

/// The `rename` tool's parameters.
pub fn rename_params() -> Vec<(&'static str, Value)> {
    vec![feature_param(), ("name", string("The new tree name, not empty and not used by another feature"))]
}

/// The `rename_plane` tool's parameters.
pub fn rename_plane_params() -> Vec<(&'static str, Value)> {
    vec![("plane", string("top, front, right, or the plane's current name")), ("name", string("The new display name"))]
}

/// Parameters for a named value expression.
pub fn set_value_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("Value identifier")), ("expression", string("Expression in document units"))]
}

/// Parameters for renaming a value.
pub fn rename_value_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("Existing value name")), ("to", string("New identifier"))]
}

/// Parameters for binding a sketch dimension or extrusion depth.
pub fn drive_params() -> Vec<(&'static str, Value)> {
    vec![
        ("sketch", optional(string("Sketch name"))),
        ("dimension", optional(integer("Dimension number from 1"))),
        ("feature", optional(string("Extrude name"))),
        ("parameter", optional(string("depth"))),
        ("expression", string("Driver expression")),
    ]
}

/// The `edit_dimension` tool's parameters.
pub fn edit_dimension_params() -> Vec<(&'static str, Value)> {
    vec![
        ("sketch", string("The sketch's tree name")),
        ("dimension", integer("The dimension's number, from 1, as `feature` lists the sketch's dimensions")),
        (
            "value",
            number("The new value: mm for lengths, a diameter or radius as the dimension is, degrees for angles"),
        ),
    ]
}

/// The `save` and `open` tools' parameters.
pub fn part_path_params() -> Vec<(&'static str, Value)> {
    vec![("path", string("A part file path ending in .dwprt"))]
}

/// The `feature` tool's parameters.
pub fn feature_params() -> Vec<(&'static str, Value)> {
    vec![("name", string("The feature's tree name, such as Boss-Extrude1"))]
}

/// The `measure` tool's parameters.
pub fn measure_params() -> Vec<(&'static str, Value)> {
    crate::measure::tools::pair_params()
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

/// The largest length, in millimetres, an agent's command takes: a size, a depth, a radius
/// or a coordinate. The kernel meshes every result, and the time and memory it takes grow
/// with the size of the shape (about 0.3 GB and 7 s for a 10 m circle, 0.9 GB and 40 s for
/// a 100 m one, and several gigabytes for a 1,000 km one), so a size no part has is refused
/// by name before it reaches the kernel.
pub const MAX_LENGTH_MM: f64 = 100_000.0;

fn positive(what: &str, v: f64) -> Result<f64, String> {
    if !v.is_finite() || v <= 0.0 {
        return Err(format!("{what} must be greater than 0, got {v}"));
    }
    if v > MAX_LENGTH_MM {
        return Err(format!("{what} must be at most {MAX_LENGTH_MM} mm, got {v}"));
    }
    Ok(v)
}

fn feature_names(doc: &Document) -> String {
    let names: Vec<&str> = doc.features.iter().map(|f| f.name.as_str()).collect();
    if names.is_empty() { "nothing".into() } else { names.join(", ") }
}

/// A sketch's dimensions, in the order `feature` numbers them.
fn dimensions(sketch: &Sketch) -> impl Iterator<Item = &sketch::Constraint> {
    sketch.constraints.iter().filter(|c| c.is_dimension())
}

fn dimension_kind(c: &sketch::Constraint) -> &'static str {
    match c.kind {
        ConstraintKind::Radius { diameter: true, .. } => "diameter",
        ConstraintKind::Radius { .. } => "radius",
        ConstraintKind::Angle { .. } => "angle",
        _ => "distance",
    }
}

/// The part file a `save` or `open` call names.
fn part_path(input: &Value) -> Result<std::path::PathBuf, String> {
    let path = std::path::PathBuf::from(input["path"].as_str().unwrap_or_default());
    if path.extension().is_none_or(|e| e != "dwprt") {
        return Err("the path must name a .dwprt part file".into());
    }
    Ok(path)
}

pub(crate) fn feature_by_name(doc: &Document, name: &str) -> Result<FeatureId, String> {
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

fn datum_by_name(doc: &Document, name: &str) -> Option<PlaneKind> {
    let key = name.trim().to_ascii_lowercase();
    let key = key.trim_end_matches(" plane");
    PlaneKind::ALL.into_iter().find(|p| {
        [doc.model().datum_name(p.to_model()), p.label()]
            .iter()
            .any(|label| label.to_ascii_lowercase().trim_end_matches(" plane") == key)
    })
}

fn sketch_plane(doc: &Document, plane: &str, offset: f64) -> Result<SketchPlane, String> {
    if !offset.is_finite() || offset.abs() > MAX_LENGTH_MM {
        return Err(format!("datum offset must be finite and within {MAX_LENGTH_MM} mm of zero"));
    }
    if let Some(plane) = datum_by_name(doc, plane) {
        return Ok(SketchPlane::Reference { plane, offset });
    }
    let face = face_by_name(doc, plane).ok_or_else(|| {
        format!("`{plane}` is neither top, front, right nor a face name; `feature` lists each feature's faces")
    })?;
    if offset != 0.0 {
        return Err("datum offset is supported only on reference planes, not on faces".into());
    }
    let face = doc.face_ref(face).ok_or_else(|| format!("`{plane}` is not planar, so it cannot carry a sketch"))?;
    Ok(SketchPlane::Face { face })
}

/// Any face by name, as a persistent reference.
fn face_ref_by_name(doc: &Document, name: &str) -> Result<crate::document::FaceRef, String> {
    let face = face_by_name(doc, name)
        .ok_or_else(|| format!("no face named `{name}`; `feature` lists each feature's faces"))?;
    doc.any_face_ref(face).ok_or_else(|| format!("`{name}` cannot be referenced"))
}

/// A reference plane or a planar face, by name.
fn plane_ref(doc: &Document, plane: &str) -> Result<PlaneRef, String> {
    Ok(match sketch_plane(doc, plane, 0.0)? {
        SketchPlane::Reference { plane, offset } => PlaneRef::Datum {
            datum: match plane {
                PlaneKind::Top => DatumPlane::Top,
                PlaneKind::Front => DatumPlane::Front,
                PlaneKind::Right => DatumPlane::Right,
            },
            offset,
        },
        SketchPlane::Face { face } => PlaneRef::Face { face },
    })
}

/// The end of an extrude or cut line.
fn end_of(doc: &Document, up_to_next: bool, up_to_face: &Option<String>) -> Result<Option<EndKind>, String> {
    match (up_to_next, up_to_face) {
        (true, Some(_)) => Err("choose up_to_next or up_to_face, not both".into()),
        (true, None) => Ok(Some(EndKind::UpToNext)),
        (false, Some(face)) => Ok(Some(EndKind::UpToFace(Some(face_ref_by_name(doc, face)?)))),
        (false, None) => Ok(None),
    }
}

fn vec3_of(p: Point, what: &str) -> Result<soliddraft_kernel_api::Vec3, String> {
    let v = soliddraft_kernel_api::Vec3::new(p[0], p[1], p[2]);
    if !v.is_finite() || v.normalized().is_none() {
        return Err(format!("{what} must be a finite, non-zero vector"));
    }
    Ok(v)
}

/// Projects a world point into the plane's (u, v) frame. Points more than half a
/// millimetre off the plane are refused: they mean the plane or the point is wrong.
fn to_plane(plane: &Plane, p: Point, what: &str) -> Result<P2, String> {
    let w = soliddraft_kernel_api::Vec3::new(p[0], p[1], p[2]);
    if !w.is_finite() {
        return Err(format!("{what} is not a finite point"));
    }
    if p.iter().any(|c| c.abs() > MAX_LENGTH_MM) {
        return Err(format!("{what} must lie within {MAX_LENGTH_MM} mm of the origin on every axis"));
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
            let (width, height) = (positive("width", width)?, positive("height", height)?);
            sketch.rectangle(c, width, height).map_err(|e| format!("{}: {e}", what("rectangle")))?;
        }
        Shape::Circle { center, diameter } => {
            let c = to_plane(plane, center, &what("centre"))?;
            let diameter = positive("diameter", diameter)?;
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

fn value_command(doc: &Document, name: &str, input: &Value) -> Result<Command, String> {
    match name {
        "set_value" => {
            let name = input["name"].as_str().ok_or("missing name")?.to_owned();
            let expression = input["expression"].as_str().ok_or("missing expression")?.to_owned();
            Ok(Command::SetValue { name, expression })
        }
        "rename_value" => {
            let name = input["name"].as_str().ok_or("missing name")?.to_owned();
            let to = input["to"].as_str().ok_or("missing to")?.to_owned();
            Ok(Command::RenameValue { name, to })
        }
        "drive" => {
            let expression = input["expression"].as_str().ok_or("missing expression")?.to_owned();
            let target = if let Some(sketch) = input["sketch"].as_str() {
                if input.get("feature").is_some() || input.get("parameter").is_some() {
                    return Err("choose a sketch or feature input".into());
                }
                let id = feature_by_name(doc, sketch)?;
                let Some(FeatureKind::Sketch(s)) = doc.feature(id).map(|f| &f.kind) else {
                    return Err("not a sketch".into());
                };
                let number = input["dimension"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or("dimension must be a positive index")?;
                let constraint =
                    dimensions(&s.sketch).nth(number - 1).map(|c| c.id).ok_or("dimension number is out of range")?;
                soliddraft_model::values::Input::Dimension {
                    feature: soliddraft_model::FeatureId(id as u32),
                    constraint,
                }
            } else {
                if input.get("dimension").is_some() || input.get("sketch").is_some() {
                    return Err("choose a sketch or feature input".into());
                }
                let feature = input["feature"].as_str().ok_or("missing feature")?;
                if input["parameter"].as_str() != Some("depth") {
                    return Err("parameter must be depth".into());
                }
                soliddraft_model::values::Input::Depth {
                    feature: soliddraft_model::FeatureId(feature_by_name(doc, feature)? as u32),
                }
            };
            Ok(Command::Drive { input: target, expression })
        }
        _ => Err("unknown value command".into()),
    }
}

impl Line {
    /// Runs the line on `doc` through the command layer.
    fn run(&self, doc: &mut Document, run: &mut Run) -> Result<(), String> {
        // An active field is last: a CAD add is refused before any of its inputs are read.
        if self.adds_cad()
            && let Some(refusal) = doc.model().cad_after_field()
        {
            return Err(refusal.to_string());
        }
        match self {
            Line::AddConfiguration { .. } | Line::SetConfigurationValue { .. } | Line::ActivateConfiguration { .. } => {
                run.apply(doc, crate::grid::tools::command(self)?)?;
            }
            Line::SetValue { .. } | Line::RenameValue { .. } | Line::Drive { .. } => {
                let input = serde_json::to_value(self).map_err(|e| e.to_string())?;
                let command = value_command(doc, self.op(), &input)?;
                run.apply(doc, command)?;
            }
            Line::Sketch { plane, offset, shapes } => {
                if shapes.is_empty() {
                    return Err("a sketch needs at least one shape".into());
                }
                let at = sketch_plane(doc, plane, *offset)?;
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
            Line::SketchGeometry { plane, offset, operations } => {
                if operations.is_empty() {
                    return Err("a sketch needs at least one geometry operation".into());
                }
                let at = sketch_plane(doc, plane, *offset)?;
                let id = run.apply(doc, Command::AddSketch { plane: at })?.ok_or("the sketch was not created")?;
                let mut s = Sketch::new();
                for (i, operation) in operations.iter().enumerate() {
                    s.apply_operation(operation).map_err(|e| format!("sketch tool {}: {e}", i + 1))?;
                }
                run.apply(doc, Command::EditSketch { id, sketch: s })?;
                run.last_sketch = Some(id);
            }
            Line::Extrude { sketch, depth, flip, up_to_next, up_to_face } => {
                let end = end_of(doc, *up_to_next, up_to_face)?;
                let params = ExtrudeParams {
                    sketch: Some(run.sketch(doc, sketch)?),
                    depth: if end.is_some() { depth.clamp(1.0, MAX_LENGTH_MM) } else { positive("depth", *depth)? },
                    flip: *flip,
                    end: end.unwrap_or_default(),
                    ..Default::default()
                };
                run.apply(doc, Command::AddExtrude { params, via_assistant: true })?;
            }
            Line::Revolve { sketch, axis_origin, axis_direction, angle_deg } => {
                if axis_direction.iter().any(|v| !v.is_finite()) || axis_direction.iter().all(|v| *v == 0.0) {
                    return Err("axis direction must be finite and nonzero".into());
                }
                if axis_origin.iter().any(|v| !v.is_finite() || v.abs() > MAX_LENGTH_MM) {
                    return Err(format!("axis origin must lie within {MAX_LENGTH_MM} mm on each axis"));
                }
                let r = soliddraft_model::RevolveFeature {
                    sketch: soliddraft_model::FeatureId(run.sketch(doc, sketch)? as u32),
                    axis_origin: *axis_origin,
                    axis_direction: *axis_direction,
                    angle_deg: *angle_deg,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Revolve(r)))?;
            }
            Line::EditRevolve { feature, angle_deg } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Revolve(mut r)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a revolve"));
                };
                r.angle_deg = *angle_deg;
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Revolve(r) })?;
            }
            Line::Cut { sketch, through_all, depth, flip, up_to_next, up_to_face } => {
                let end = end_of(doc, *up_to_next, up_to_face)?;
                let free = *through_all || end.is_some();
                let params = CutParams {
                    sketch: run.sketch(doc, sketch)?,
                    depth: if free { depth.clamp(1.0, MAX_LENGTH_MM) } else { positive("depth", *depth)? },
                    through_all: *through_all && end.is_none(),
                    flip: *flip,
                    up_to: end,
                };
                run.apply(doc, Command::AddCut { params })?;
            }
            Line::Hole { face, points, diameter, depth, tip_angle, counterbore, countersink } => {
                let id = face_by_name(doc, face).ok_or_else(|| format!("no face named `{face}`"))?;
                let r =
                    doc.face_ref(id).ok_or_else(|| format!("`{face}` is not planar, so holes cannot start on it"))?;
                let plane = doc.face_plane(id).ok_or_else(|| format!("`{face}` has no plane"))?;
                let points = points
                    .iter()
                    .enumerate()
                    .map(|(i, p)| to_plane(&plane, *p, &format!("hole {}", i + 1)))
                    .collect::<Result<Vec<_>, _>>()?;
                let head = match (counterbore, countersink) {
                    (Some(_), Some(_)) => return Err("a hole has a counterbore or a countersink, not both".into()),
                    (Some(c), None) => Some(HoleHead::Counterbore {
                        diameter: positive("counterbore diameter", c.diameter)?,
                        depth: positive("counterbore depth", c.depth)?,
                    }),
                    (None, Some(c)) => Some(HoleHead::Countersink {
                        diameter: positive("countersink diameter", c.diameter)?,
                        angle_deg: positive("countersink angle", c.angle)?,
                    }),
                    (None, None) => None,
                };
                let hole = soliddraft_model::HoleFeature {
                    face: r,
                    points,
                    diameter: positive("diameter", *diameter)?,
                    depth: match depth {
                        Some(d) => HoleDepth::Blind { depth: positive("depth", *d)? },
                        None => HoleDepth::Through,
                    },
                    tip: match tip_angle {
                        Some(a) => HoleTip::Cone { angle_deg: positive("tip angle", *a)? },
                        None => HoleTip::Flat,
                    },
                    head,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Hole(hole)))?;
            }
            Line::LinearPattern { direction, spacing, count } => {
                let layout = PatternSpec::Linear {
                    direction: vec3_of(*direction, "direction")?,
                    spacing: positive("spacing", *spacing)?,
                    count: *count,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Pattern(soliddraft_model::PatternFeature { layout })))?;
            }
            Line::CircularPattern { axis_origin, axis_direction, count, angle } => {
                let layout = PatternSpec::Circular {
                    axis_origin: soliddraft_kernel_api::Vec3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
                    axis_direction: vec3_of(*axis_direction, "axis direction")?,
                    count: *count,
                    angle: *angle,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Pattern(soliddraft_model::PatternFeature { layout })))?;
            }
            Line::Mirror { plane, keep_original } => {
                let mirror =
                    soliddraft_model::MirrorFeature { plane: plane_ref(doc, plane)?, keep_original: *keep_original };
                run.apply(doc, Command::AddFeature(FeatureKind::Mirror(mirror)))?;
            }
            Line::Lattice { cell, cell_size, radius, accuracy, min_detail } => {
                let lattice = soliddraft_model::LatticeFeature {
                    cell: Some(*cell),
                    cell_size_mm: Some(positive("cell size", *cell_size)?),
                    radius_mm: Some(positive("density", *radius)?.into()),
                    accuracy_mm: *accuracy,
                    min_detail_mm: *min_detail,
                    ..Default::default()
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Lattice(lattice)))?;
            }
            Line::EditLattice { feature, cell_size, radius, accuracy, min_detail } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Lattice(mut lattice)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a lattice"));
                };
                lattice.cell = Some(soliddraft_kernel_api::LatticeCell::Cubic);
                if let Some(v) = cell_size {
                    lattice.cell_size_mm = Some(positive("cell size", *v)?);
                }
                if let Some(v) = radius {
                    lattice.radius_mm = Some(positive("density", *v)?.into());
                }
                if let Some(v) = accuracy {
                    lattice.accuracy_mm = *v;
                }
                if let Some(v) = min_detail {
                    lattice.min_detail_mm = *v;
                }
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Lattice(lattice) })?;
            }
            Line::Gyroid { kind, period, level, accuracy, min_detail } => {
                let gyroid = soliddraft_model::GyroidFeature {
                    kind: *kind,
                    period_mm: positive("period", *period)?,
                    level: (*level).into(),
                    accuracy_mm: *accuracy,
                    min_detail_mm: *min_detail,
                    ..Default::default()
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Gyroid(gyroid)))?;
            }
            Line::EditGyroid { feature, period, level, accuracy, min_detail } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Gyroid(mut gyroid)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a gyroid"));
                };
                if let Some(v) = period {
                    gyroid.period_mm = positive("period", *v)?;
                }
                if let Some(v) = level {
                    gyroid.level = (*v).into();
                }
                if let Some(v) = accuracy {
                    gyroid.accuracy_mm = *v;
                }
                if let Some(v) = min_detail {
                    gyroid.min_detail_mm = *v;
                }
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Gyroid(gyroid) })?;
            }
            Line::GradeField { feature, axis, from, to, start, end } => {
                let id = feature_by_name(doc, feature)?;
                let grade = soliddraft_kernel_api::Grade::Linear(soliddraft_kernel_api::LinearGrade {
                    axis: *axis,
                    from: *from,
                    to: *to,
                    start: *start,
                    end: *end,
                });
                let kind = match doc.feature(id).map(|f| f.kind.clone()) {
                    Some(FeatureKind::Lattice(lattice)) => {
                        FeatureKind::Lattice(soliddraft_model::LatticeFeature { radius_mm: Some(grade), ..lattice })
                    }
                    Some(FeatureKind::Gyroid(gyroid)) => {
                        FeatureKind::Gyroid(soliddraft_model::GyroidFeature { level: grade, ..gyroid })
                    }
                    _ => return Err(format!("{feature} is not a lattice or a gyroid")),
                };
                run.apply(doc, Command::EditFeature { id, kind })?;
            }
            Line::Shell { open_faces, thickness } => {
                let shell = soliddraft_model::ShellFeature {
                    open_faces: open_faces.iter().map(|f| face_ref_by_name(doc, f)).collect::<Result<Vec<_>, _>>()?,
                    thickness: positive("wall thickness", *thickness)?,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Shell(shell)))?;
            }
            Line::Draft { faces, neutral, angle, reverse } => {
                let draft = soliddraft_model::DraftFeature {
                    faces: faces.iter().map(|f| face_ref_by_name(doc, f)).collect::<Result<Vec<_>, _>>()?,
                    neutral: plane_ref(doc, neutral)?,
                    angle: *angle,
                    reverse: *reverse,
                };
                run.apply(doc, Command::AddFeature(FeatureKind::Draft(draft)))?;
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
                let params = ExtrudeParams { depth: positive("depth", *depth)?, flip: *flip, end: EndKind::Blind, ..p };
                run.apply(doc, Command::EditExtrude { id, params })?;
            }
            Line::EditCut { feature, through_all, depth, flip } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Cut(p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a cut"));
                };
                let depth = if *through_all { depth.clamp(1.0, MAX_LENGTH_MM) } else { positive("depth", *depth)? };
                let params = CutParams { through_all: *through_all, depth, flip: *flip, up_to: None, ..p };
                run.apply(doc, Command::EditCut { id, params })?;
            }
            Line::EditHole { feature, diameter, depth, countersink } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Hole(mut h)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a hole"));
                };
                h.diameter = positive("diameter", *diameter)?;
                h.depth = match depth {
                    Some(d) => HoleDepth::Blind { depth: positive("depth", *d)? },
                    None => HoleDepth::Through,
                };
                if let Some(c) = countersink {
                    h.head = Some(HoleHead::Countersink {
                        diameter: positive("countersink diameter", c.diameter)?,
                        angle_deg: positive("countersink angle", c.angle)?,
                    });
                }
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Hole(h) })?;
            }
            Line::EditPattern { feature, count, step } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Pattern(mut p)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a pattern"));
                };
                match &mut p.layout {
                    PatternSpec::Linear { spacing, count: c, .. } => {
                        *spacing = positive("spacing", *step)?;
                        *c = *count;
                    }
                    PatternSpec::Circular { angle, count: c, .. } => {
                        *angle = *step;
                        *c = *count;
                    }
                }
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Pattern(p) })?;
            }
            Line::EditMirror { feature, keep_original } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Mirror(mut m)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a mirror"));
                };
                m.keep_original = *keep_original;
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Mirror(m) })?;
            }
            Line::EditShell { feature, thickness } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Shell(mut shell)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a shell"));
                };
                shell.thickness = positive("wall thickness", *thickness)?;
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Shell(shell) })?;
            }
            Line::EditDraft { feature, angle } => {
                let id = feature_by_name(doc, feature)?;
                let Some(FeatureKind::Draft(mut d)) = doc.feature(id).map(|f| f.kind.clone()) else {
                    return Err(format!("{feature} is not a draft"));
                };
                d.angle = *angle;
                run.apply(doc, Command::EditFeature { id, kind: FeatureKind::Draft(d) })?;
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

    /// Whether the line adds a CAD feature, which an active terminal field refuses. A
    /// second field meets the document's one-field rule instead.
    fn adds_cad(&self) -> bool {
        matches!(
            self,
            Line::Sketch { .. }
                | Line::SketchGeometry { .. }
                | Line::Extrude { .. }
                | Line::Revolve { .. }
                | Line::Cut { .. }
                | Line::Fillet { .. }
                | Line::Chamfer { .. }
                | Line::Hole { .. }
                | Line::LinearPattern { .. }
                | Line::CircularPattern { .. }
                | Line::Mirror { .. }
                | Line::Draft { .. }
                | Line::Shell { .. }
        )
    }

    /// The line's `op`: the id of its verb in the command registry.
    pub fn op(&self) -> &'static str {
        match self {
            Line::AddConfiguration { .. } => "add_configuration",
            Line::SetConfigurationValue { .. } => "set_configuration_value",
            Line::ActivateConfiguration { .. } => "activate_configuration",
            Line::SetValue { .. } => "set_value",
            Line::RenameValue { .. } => "rename_value",
            Line::Drive { .. } => "drive",
            Line::Sketch { .. } => "sketch",
            Line::SketchGeometry { .. } => "sketch_geometry",
            Line::Extrude { .. } => "extrude",
            Line::Revolve { .. } => "revolve",
            Line::EditRevolve { .. } => "edit_revolve",
            Line::Cut { .. } => "cut",
            Line::Fillet { .. } => "fillet",
            Line::Chamfer { .. } => "chamfer",
            Line::EditExtrude { .. } => "edit_extrude",
            Line::EditCut { .. } => "edit_cut",
            Line::EditFillet { .. } => "edit_fillet",
            Line::EditChamfer { .. } => "edit_chamfer",
            Line::Hole { .. } => "hole",
            Line::LinearPattern { .. } => "linear_pattern",
            Line::CircularPattern { .. } => "circular_pattern",
            Line::Mirror { .. } => "mirror",
            Line::Draft { .. } => "draft",
            Line::Lattice { .. } => "lattice",
            Line::EditLattice { .. } => "edit_lattice",
            Line::Gyroid { .. } => "gyroid",
            Line::EditGyroid { .. } => "edit_gyroid",
            Line::GradeField { .. } => "grade_field",
            Line::Shell { .. } => "shell",
            Line::EditShell { .. } => "edit_shell",
            Line::EditHole { .. } => "edit_hole",
            Line::EditPattern { .. } => "edit_pattern",
            Line::EditMirror { .. } => "edit_mirror",
            Line::EditDraft { .. } => "edit_draft",
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
        let up_to = |next: bool, face: &Option<String>, otherwise: String| match (next, face) {
            (true, _) => "up to next".to_owned(),
            (_, Some(f)) => format!("up to {f}"),
            _ => otherwise,
        };
        match self {
            Line::Sketch { plane, offset, shapes } => {
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
                let title = if *offset == 0.0 {
                    format!("Sketch on {plane}")
                } else {
                    format!("Sketch on {plane}, offset {} mm", fmt_mm(*offset))
                };
                (title, list.join("; "))
            }
            Line::SketchGeometry { plane, offset, operations } => {
                let title = if *offset == 0.0 {
                    format!("Sketch geometry on {plane}")
                } else {
                    format!("Sketch geometry on {plane}, offset {} mm", fmt_mm(*offset))
                };
                (title, format!("{} tool operations", operations.len()))
            }
            Line::Revolve { sketch, angle_deg, .. } => ("Revolve".into(), format!("{sketch}, {}°", fmt_mm(*angle_deg))),
            Line::EditRevolve { feature, angle_deg } => (format!("Edit {feature}"), format!("{}°", fmt_mm(*angle_deg))),
            Line::Extrude { sketch, depth, flip, up_to_next, up_to_face } => {
                ("Extrude".into(), format!("{sketch}, {}", up_to(*up_to_next, up_to_face, end(false, *depth, *flip))))
            }
            Line::Cut { sketch, through_all, depth, flip, up_to_next, up_to_face } => (
                "Cut".into(),
                format!("{sketch}, {}", up_to(*up_to_next, up_to_face, end(*through_all, *depth, *flip))),
            ),
            Line::Hole { face, points, diameter, depth, tip_angle, counterbore, countersink } => {
                let depth = match depth {
                    Some(d) => {
                        let tip = tip_angle.map(|a| HoleTip::Cone { angle_deg: a }).unwrap_or(HoleTip::Flat);
                        hole_depth_text(&HoleDepth::Blind { depth: *d }, &tip)
                    }
                    None => "through".into(),
                };
                let head = match (counterbore, countersink) {
                    (Some(c), _) => {
                        format!(", {}", hole_head_text(&HoleHead::Counterbore { diameter: c.diameter, depth: c.depth }))
                    }
                    (_, Some(c)) => format!(
                        ", {}",
                        hole_head_text(&HoleHead::Countersink { diameter: c.diameter, angle_deg: c.angle })
                    ),
                    _ => String::new(),
                };
                ("Hole".into(), format!("{} × Ø{} {depth}{head} on {face}", points.len(), fmt_mm(*diameter)))
            }
            Line::LinearPattern { direction, spacing, count } => {
                ("Linear pattern".into(), format!("{count} × {} mm along {}", fmt_mm(*spacing), pt(direction)))
            }
            Line::CircularPattern { axis_direction, count, angle, .. } => {
                ("Circular pattern".into(), format!("{count} × {}° about {}", fmt_mm(*angle), pt(axis_direction)))
            }
            Line::Mirror { plane, keep_original } => {
                ("Mirror".into(), format!("in {plane}{}", if *keep_original { ", keeping the original" } else { "" }))
            }
            Line::Lattice { cell_size, radius, .. } => {
                ("Lattice".into(), format!("Cubic cell {} mm, beam radius {} mm", fmt_mm(*cell_size), fmt_mm(*radius)))
            }
            Line::EditLattice { feature, .. } => (format!("Edit {feature}"), "Change lattice settings".into()),
            Line::Gyroid { period, level, .. } => {
                ("Gyroid".into(), format!("Network, period {} mm, level {}", fmt_mm(*period), fmt_mm(*level)))
            }
            Line::EditGyroid { feature, .. } => (format!("Edit {feature}"), "Change gyroid settings".into()),
            Line::GradeField { feature, axis, from, to, start, end } => {
                (format!("Grade {feature}"), format!("{start} to {end} along {} from {from} to {to} mm", axis.name()))
            }
            Line::Shell { open_faces, thickness } => (
                "Shell".into(),
                format!(
                    "{} mm wall, {}",
                    fmt_mm(*thickness),
                    if open_faces.is_empty() { "closed".into() } else { format!("open at {}", open_faces.join(", ")) }
                ),
            ),
            Line::EditShell { feature, thickness } => {
                (format!("Edit {feature}"), format!("{} mm wall", fmt_mm(*thickness)))
            }
            Line::Draft { faces, neutral, angle, .. } => {
                ("Draft".into(), format!("{}° on {} about {neutral}", fmt_mm(*angle), faces.join(", ")))
            }
            Line::EditHole { feature, diameter, depth, countersink } => (
                format!("Edit {feature}"),
                format!(
                    "Ø{}, {}{}",
                    fmt_mm(*diameter),
                    depth.map_or("through".into(), |d| format!("{} mm", fmt_mm(d))),
                    countersink.map_or(String::new(), |c| format!(
                        ", {}",
                        hole_head_text(&HoleHead::Countersink { diameter: c.diameter, angle_deg: c.angle })
                    ))
                ),
            ),
            Line::EditPattern { feature, count, step } => {
                (format!("Edit {feature}"), format!("{count} × {}", fmt_mm(*step)))
            }
            Line::EditMirror { feature, keep_original } => {
                (format!("Edit {feature}"), if *keep_original { "keep the original" } else { "reflection only" }.into())
            }
            Line::EditDraft { feature, angle } => (format!("Edit {feature}"), format!("{}°", fmt_mm(*angle))),
            Line::Fillet { edges, radius } => {
                ("Fillet".into(), format!("R{} on {}", fmt_mm(*radius), edges.join(", ")))
            }
            Line::Chamfer { edges, distance } => {
                ("Chamfer".into(), format!("{} mm on {}", fmt_mm(*distance), edges.join(", ")))
            }
            Line::AddConfiguration { name } => ("Add configuration".into(), name.clone()),
            Line::SetConfigurationValue { configuration, name, expression } => {
                (format!("Set {configuration} {name}"), expression.clone())
            }
            Line::ActivateConfiguration { name } => ("Activate configuration".into(), name.clone()),
            Line::SetValue { name, expression } => (format!("Set {name}"), expression.clone()),
            Line::RenameValue { name, to } => (format!("Rename {name}"), to.clone()),
            Line::Drive { sketch, feature, expression, .. } => {
                (format!("Drive {}", sketch.as_deref().or(feature.as_deref()).unwrap_or("input")), expression.clone())
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

/// Previously requested properties; inspection must not start a measurement.
fn cached_body_mass(doc: &Document) -> Option<soliddraft_kernel_api::MassProperties> {
    if !doc.proof_state().current_proved_model() {
        return None;
    }
    doc.kernel().cached_mass_properties(doc.model().body()?)
}

/// An independent proposal copy sharing the part's immutable bodies and meshes. It makes
/// no new display mesh until the dry run asks for its preview.
fn scratch_copy(doc: &Document) -> Document {
    doc.scratch()
}

fn status_text(doc: &Document, id: FeatureId) -> String {
    match doc.feature(id).map(|f| (&f.status, f.suppressed)) {
        Some((FeatureStatus::Ok, _)) => "ok".into(),
        Some((FeatureStatus::Pending, _)) => "proof pending".into(),
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
    builds: PreparedBuilds,
}

fn dry_run(doc: &Document, lines: &[Line], preview: bool) -> Result<DryRun, String> {
    let mut scratch = scratch_copy(doc);
    let before: Vec<(FeatureId, String, FeatureKind, bool, bool)> =
        scratch.features.iter().map(|f| (f.id, f.name.clone(), f.kind.clone(), f.suppressed, broken(f))).collect();
    let volume_before = cached_body_mass(&scratch).map(|m| m.volume);
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
    let volume_after = cached_body_mass(&scratch).map(|m| m.volume);
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
    if let (Some(before), Some(after)) = (volume_before, volume_after) {
        rows.push(("Volume".into(), format!("{} → {} mm³", fmt_mm(before.round()), fmt_mm(after.round()))));
    }
    let result = if preview {
        scratch.show();
        scratch.display_body().and_then(|b| scratch.meshes.get(&b).cloned())
    } else {
        None
    };
    Ok(DryRun {
        rows,
        effect: json!({
            "features_added": added,
            "features_changed": changed,
            "features_removed": removed,
            "volume_before_mm3": volume_before.map(round),
            "volume_after_mm3": volume_after.map(round),
        }),
        preview: Preview { result, tool: None, removes: false },
        builds: PreparedBuilds(std::sync::Arc::new(scratch.into_model())),
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

#[derive(Clone)]
struct PreparedBuilds(std::sync::Arc<soliddraft_model::Document>);

impl std::fmt::Debug for PreparedBuilds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedBuilds").field("features", &self.0.features().len()).finish_non_exhaustive()
    }
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
    builds: Option<PreparedBuilds>,
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

    /// Tries `lines` on a scratch copy of `doc` and records a pending proposal. A
    /// proposal that is applied at once can skip the `preview` mesh of its result.
    pub fn propose(
        &mut self,
        doc: &Document,
        summary: &str,
        lines: Vec<Line>,
        origin: Caller,
        preview: bool,
    ) -> Result<(String, Value), String> {
        if lines.is_empty() {
            return Err("a proposal needs at least one line".into());
        }
        let run = dry_run(doc, &lines, preview)?;
        self.next += 1;
        let id = format!("P{}", self.next);
        self.proposals.push(Proposal {
            id: id.clone(),
            summary: summary.trim().to_owned(),
            lines,
            origin,
            rows: run.rows,
            preview: run.preview,
            builds: Some(run.builds),
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
        if let Some(builds) = &p.builds {
            doc.reuse_builds_from(&builds.0);
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
        self.proposals[i].builds = None;
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
        self.proposals[i].builds = None;
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
    /// The open drawing, when this command host has one.
    pub drawing: Option<&'a mut crate::drawing::document::DrawingDoc>,
}

/// What an MCP client is told about this server in `initialize`.
pub const MCP_INSTRUCTIONS: &str = "SolidDraft, a parametric CAD application. The tools drive the open part through \
the same command layer as the ribbon. Units are millimetres, Z is up, and no length or coordinate may exceed 100000. Read the part with model_summary and \
feature; change it with the verb tools (each runs at once as one undo step) or with propose and apply; undo and \
redo step through the undo stack. Entities are named as the interface names them: Boss-Extrude1 for a feature, \
Boss-Extrude1 · End cap for a face, Boss-Extrude1 · Edge 3 for an edge. make_drawing or opening a .dwdrw file \
gives a drawing, which model_summary lists by sheet, view and annotation for the drawing_ tools. Every command of \
the ribbon is a tool here except those that need the window; `soliddraft commands` says why for each.";

/// How agents are told to name entities, for the tools that take names and their errors.
macro_rules! names_help {
    () => {
        "Entities are named as the interface names them: a feature by its tree name (Boss-Extrude1), a face or edge \
         as `<feature> · <label>` (Boss-Extrude1 · End cap, Boss-Extrude1 · Edge 3), a vertex as `<feature> · \
         Vertex <n>`."
    };
}

pub(crate) const NAMES_HELP: &str = names_help!();

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
    if name.eq_ignore_ascii_case(crate::document::BODY_NAME) {
        return doc.display_body().map(|_| Selection::Body);
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
    if doc.model().active_field().is_some_and(|f| doc.model().status(f.id).is_some_and(|s| s.is_ok())) {
        // A field's one display region is a placeholder with no measured area or centroid.
        return json!({
            "name": name_of_face(doc, face),
            "surface": "field display region",
            "area_mm2": null,
            "centroid": null,
            "measured": "field_mass proves the field's moments; the display region has none",
        });
    }
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

/// Says when the centroids, midpoints and lengths shown come from the display mesh rather
/// than the kernel's proofs, which only `measure` asks for.
fn mark_estimates(v: &mut Value, topology: Option<&soliddraft_kernel_api::Topology>) {
    if let Some(chord) = topology.and_then(|t| t.display_chord_height) {
        v["estimates"] = format!(
            "centroids, midpoints and lengths are estimated from the display mesh, within {chord:.3} mm; measure_entity proves edge length or face area; measure reports reference-point offsets"
        )
        .into();
    }
}

fn selection_json(doc: &Document, sel: Selection) -> Value {
    let Some(info) = doc.describe(sel) else { return Value::Null };
    let topology = doc.display_body().and_then(|b| doc.kernel().topology(b).ok());
    let mut v = match sel {
        Selection::Face(f) => face_json(doc, f),
        Selection::Edge(e) => edge_json(doc, e, topology.as_ref()),
        _ => json!({}),
    };
    if matches!(sel, Selection::Face(_) | Selection::Edge(_)) {
        mark_estimates(&mut v, topology.as_ref());
    }
    v["name"] = info.title.into();
    v["kind"] = match sel {
        Selection::Body => "body",
        Selection::Feature(_) => "feature",
        Selection::Face(_) => "face",
        Selection::Edge(_) => "edge",
        Selection::Vertex(_) => "vertex",
        Selection::Reference(_) => "reference",
    }
    .into();
    v["details"] =
        info.details.into_iter().map(|(k, v)| (k, Value::String(v))).collect::<serde_json::Map<_, _>>().into();
    v
}

fn end_json(v: &mut Value, end: &EndKind) {
    match end {
        EndKind::UpToNext => v["end"] = "up to next".into(),
        EndKind::UpToFace(face) => {
            v["end"] = "up to face".into();
            v["up_to_face"] = face.as_ref().map(|f| f.label.clone()).into();
        }
        EndKind::Blind | EndKind::ThroughAll => {}
    }
}

fn params_json(doc: &Document, kind: &FeatureKind) -> Value {
    let name = |id: FeatureId| doc.feature(id).map(|f| f.name.clone()).unwrap_or_else(|| "missing".into());
    let labels = |refs: &[EdgeRef]| refs.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
    match kind {
        FeatureKind::Sketch(s) => json!({
            "type": "sketch",
            "plane": match &s.plane {
                SketchPlane::Reference { plane, .. } => doc.model().datum_name(plane.to_model()).to_owned(),
                other => other.label(),
            },
            "offset": match &s.plane {
                SketchPlane::Reference { offset, .. } => Some(*offset),
                SketchPlane::Face { .. } => None,
            },
            "entities": s.sketch.drawn().count(),
            "state": s.sketch.status.label(),
        }),
        FeatureKind::Extrude(p) => {
            let mut v = match p.sketch {
                Some(s) => json!({ "type": "extrude", "sketch": name(s), "depth": p.depth, "flip": p.flip }),
                None => json!({
                    "type": "extrude", "profile": p.profile.summary(), "plane": doc.model().datum_name(p.plane.to_model()), "depth": p.depth, "flip": p.flip,
                }),
            };
            end_json(&mut v, &p.end);
            v
        }
        FeatureKind::Cut(p) => {
            let mut v = json!({
                "type": "cut", "sketch": name(p.sketch), "through_all": p.through_all, "depth": p.depth, "flip": p.flip,
            });
            end_json(&mut v, &p.end());
            v
        }
        FeatureKind::Revolve(r) => {
            json!({"type":"revolve","sketch":name(r.sketch.0 as u64),"axis_origin":r.axis_origin,"axis_direction":r.axis_direction,"angle_deg":r.angle_deg})
        }
        FeatureKind::Fillet(p) => json!({ "type": "fillet", "edges": labels(&p.edges), "radius": p.radius }),
        FeatureKind::Chamfer(p) => json!({ "type": "chamfer", "edges": labels(&p.edges), "distance": p.distance }),
        FeatureKind::Hole(h) => {
            let mut v = json!({
                "type": "hole", "face": h.face.label, "holes": h.points.len(), "diameter": h.diameter,
                "depth": match h.depth { HoleDepth::Blind { depth } => json!(depth), HoleDepth::Through => json!("through") },
            });
            if let HoleTip::Cone { angle_deg } = h.tip {
                v["tip_angle"] = angle_deg.into();
            }
            match h.head {
                Some(HoleHead::Counterbore { diameter, depth }) => {
                    v["counterbore"] = json!({ "diameter": diameter, "depth": depth });
                }
                Some(HoleHead::Countersink { diameter, angle_deg }) => {
                    v["countersink"] = json!({ "diameter": diameter, "angle": angle_deg });
                }
                None => {}
            }
            v
        }
        FeatureKind::Pattern(p) => match p.layout {
            PatternSpec::Linear { direction, spacing, count } => json!({
                "type": "linear_pattern", "direction": kvec(direction), "spacing": spacing, "count": count,
            }),
            PatternSpec::Circular { axis_origin, axis_direction, count, angle } => json!({
                "type": "circular_pattern", "axis_origin": kvec(axis_origin), "axis_direction": kvec(axis_direction),
                "count": count, "angle": angle,
            }),
        },
        FeatureKind::Mirror(m) => {
            json!({ "type": "mirror", "plane": plane_ref_label(&m.plane), "keep_original": m.keep_original })
        }
        FeatureKind::Shell(s) => json!({
            "type": "shell", "open_faces": s.open_faces.iter().map(|f| f.label.clone()).collect::<Vec<_>>(),
            "thickness": s.thickness,
        }),
        FeatureKind::Draft(d) => json!({
            "type": "draft", "faces": d.faces.iter().map(|f| f.label.clone()).collect::<Vec<_>>(),
            "neutral": plane_ref_label(&d.neutral), "angle": d.angle, "reverse": d.reverse,
        }),
        FeatureKind::Lattice(l) => json!({
            "type": "lattice", "cell": l.cell, "cell_size_mm": l.cell_size_mm, "radius_mm": l.radius_mm,
            "frame": l.frame, "accuracy_mm": l.accuracy_mm, "min_detail_mm": l.min_detail_mm,
            "display": "unproved preview",
        }),
        FeatureKind::Gyroid(g) => json!({
            "type": "gyroid", "kind": g.kind, "period_mm": g.period_mm, "level": g.level,
            "frame": g.frame, "accuracy_mm": g.accuracy_mm, "min_detail_mm": g.min_detail_mm,
            "display": "unproved preview",
        }),
        FeatureKind::ImportStep(i) => json!({
            "type": "import_step", "source_name": i.source_name, "sha256": i.sha256,
            "bytes": i.source.len(), "decisions": i.decisions,
        }),
    }
}

impl CommandTools<'_> {
    /// The part summary the assistant sees at the start of every message.
    pub fn summary(&self) -> Value {
        let doc = &*self.doc;
        let (lo, hi) = doc.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
        let proof = doc.proof_state();
        let current = proof.current_proved_display();
        let mass = if current && doc.model().active_field().is_none() { cached_body_mass(doc) } else { None };
        json!({
            "part": doc.name,
            "units": "mm",
            "values": crate::grid::tools::values(crate::measure::values(doc), doc.model().values().summary()),
            "configurations": doc.model().configuration_summary(),
            "edit_state": proof,
            "planes": PlaneKind::ALL.map(|p| doc.model().datum_name(p.to_model())),
            "features": doc.features.iter().map(|f| {
                let mut v = params_json(doc, &f.kind);
                if let Some(feature) = doc.model().feature(soliddraft_model::FeatureId(f.id as u32)) {
                    let expressions = doc.model().values().expressions(feature);
                    if !expressions.is_empty() { v["expressions"] = json!(expressions); }
                }
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
                "faces": current.then_some(doc.faces.len()),
                "edges": current.then_some(doc.edges.len()),
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
        mark_estimates(&mut v, topology.as_ref());
        if let FeatureKind::Sketch(s) = &f.kind {
            v["dimensions"] = dimensions(&s.sketch)
                .enumerate()
                .map(|(i, c)| json!({ "number": i + 1, "kind": dimension_kind(c), "value": c.value() }))
                .collect();
        }
        if let Some(plane) = f.plane {
            v["sketch_frame"] = json!({
                "origin": kvec(plane.origin), "normal": kvec(plane.normal), "u": kvec(plane.x_dir), "v": kvec(plane.y_dir()),
            });
        }
        Ok(v)
    }

    fn mass_properties(&self, name: &str) -> Result<Value, String> {
        let doc = &*self.doc;
        let id = feature_by_name(doc, name)?;
        let mass = doc.mass_properties(id).ok_or_else(|| format!("`{name}` has no measurable body"))?;
        Ok(json!({
            "feature": name,
            "volume_mm3": round(mass.volume),
            "area_mm2": round(mass.area),
            "centre_of_mass": kvec(mass.centroid),
        }))
    }

    fn propose(&mut self, input: &Value, caller: &Caller) -> Result<Value, String> {
        let lines: Vec<Line> =
            serde_json::from_value(input["commands"].clone()).map_err(|e| format!("bad commands: {e}"))?;
        self.propose_lines(input["summary"].as_str().unwrap_or_default(), lines, caller, true)
    }

    fn propose_lines(
        &mut self,
        summary: &str,
        lines: Vec<Line>,
        caller: &Caller,
        preview: bool,
    ) -> Result<Value, String> {
        let (id, effect) = self.book.propose(self.doc, summary, lines, caller.clone(), preview)?;
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

    fn edit_dimension(&mut self, input: &Value) -> Result<Value, String> {
        let name = input["sketch"].as_str().unwrap_or_default();
        let id = feature_by_name(self.doc, name)?;
        let Some(FeatureKind::Sketch(s)) = self.doc.feature(id).map(|f| &f.kind) else {
            return Err(format!("`{name}` is not a sketch"));
        };
        let mut sketch = s.sketch.clone();
        let number = input["dimension"].as_u64().unwrap_or(0) as usize;
        let value = input["value"].as_f64().filter(|v| v.is_finite()).ok_or("the value must be a finite number")?;
        let target = dimensions(&sketch)
            .nth(number.wrapping_sub(1))
            .map(|c| c.id)
            .ok_or_else(|| format!("`{name}` has {} dimensions; number them from 1", dimensions(&sketch).count()))?;
        sketch.constraint_mut(target).expect("listed by dimensions").set_value(value);
        self.doc.apply(Command::EditSketch { id, sketch })?;
        Ok(json!({ "sketch": name, "dimension": number, "value": value }))
    }

    fn value_command(&mut self, name: &str, input: &Value) -> Result<Value, String> {
        let command = value_command(self.doc, name, input)?;
        self.doc.apply(command)?;
        Ok(self.summary())
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
        let drawing_key = self.drawing.as_deref().map(|d| d.journal_key());
        let journal = self.doc.model().journal_command(|| {
            json!({
                "op":"tool", "name":name, "input":input,
                "assistant":matches!(caller, Caller::Assistant { .. }),
                "auto_apply":self.book.auto_apply,
                "drawing":drawing_key,
                "surface":if self.surface == Surface::Mcp {"mcp"} else {"assistant"}
            })
        });
        let mcp = self.surface == Surface::Mcp;
        let result = match name {
            "document_command" if mcp => input["command_json"]
                .as_str()
                .ok_or_else(|| "missing command_json".to_owned())
                .and_then(|s| serde_json::from_str(s).map_err(|e| format!("bad command JSON: {e}")))
                .and_then(|command| self.doc.run_journal_command(command))
                .map(|()| json!({"status":"applied"})),
            "drawing_command" if mcp => (|| {
                let command = serde_json::from_str(input["command_json"].as_str().ok_or("missing command_json")?)
                    .map_err(|e| format!("bad command JSON: {e}"))?;
                let drawing = self.drawing.as_deref_mut().ok_or("no open drawing")?;
                let part = crate::drawing::part::PartContext::new(self.doc);
                drawing.run_journal_command(command, part.as_ref())?;
                Ok(json!({"status":"applied"}))
            })(),
            "model_summary" => Ok(self.summary()),
            "feature" => self.feature(input["name"].as_str().unwrap_or_default()),
            "mass_properties" => self.mass_properties(input["name"].as_str().unwrap_or_default()),
            "field_mass" => self
                .doc
                .model()
                .field_mass(&soliddraft_kernel_api::FieldMassAccuracy::default())
                .map(|mass| json!({ "field_mass": mass, "density": "unit", "units": { "volume": "mm3", "centroid": "mm", "inertia": "mm5" } })),
            "selection" => Ok(json!({
                "selection": self.doc.selection.map(|s| selection_json(self.doc, s)).unwrap_or(Value::Null)
            })),
            "measure_entity" | "track" | "measure" => crate::measure::tools::call(self.doc, name, &input),
            "propose" => self.propose(&input, caller),
            "apply" => self.apply(input["proposal_id"].as_str().unwrap_or_default(), caller),
            "rollback" if mcp => (|| {
                self.doc.rollback = match input["before"].as_str() {
                    Some(name) => {
                        let id = feature_by_name(self.doc, name)?;
                        Some(self.doc.index_of(id).ok_or("feature disappeared")?)
                    }
                    None => None,
                };
                self.doc.rebuild()?;
                Ok(json!({"before":input["before"]}))
            })(),
            "undo" if mcp => self.doc.undo().map(|label| json!({ "undone": label, "undo": self.doc.undo_label() })),
            "rename" if mcp => (|| {
                let id = feature_by_name(self.doc, input["feature"].as_str().unwrap_or_default())?;
                let name = input["name"].as_str().unwrap_or_default().trim().to_owned();
                if self.doc.features.iter().any(|f| f.name == name && f.id != id) {
                    return Err(format!("a feature is already named `{name}`"));
                }
                self.doc.apply(Command::Rename { id, name: name.clone() })?;
                Ok(json!({ "renamed": name }))
            })(),
            "rename_plane" if mcp => (|| {
                let requested = input["plane"].as_str().unwrap_or_default();
                let plane =
                    datum_by_name(self.doc, requested).ok_or_else(|| format!("no plane named `{requested}`"))?;
                let name = input["name"].as_str().unwrap_or_default().trim().to_owned();
                self.doc.apply(Command::RenamePlane { plane, name: name.clone() })?;
                Ok(json!({ "renamed_plane": name }))
            })(),
            "set_value" | "rename_value" | "drive" if mcp => self.value_command(name, &input),
            "edit_dimension" if mcp => self.edit_dimension(&input),
            "save" if mcp => (|| {
                let path = part_path(&input)?;
                // The part file alone: no backups or recent-file list, which belong to the user's session.
                crate::journal::save_part(self.doc, &path)?;
                self.doc.path = Some(path.clone());
                self.doc.dirty = false;
                Ok(json!({ "saved": path }))
            })(),
            "open" if mcp => (|| {
                let path = part_path(&input)?;
                let mut doc = crate::journal::open_part(&path, self.doc.kernel().clone(), Some(self.doc.model()))?;
                doc.path = Some(path.clone());
                *self.doc = doc;
                Ok(json!({ "opened": path }))
            })(),
            "delete_feature" | "suppress" | "grade_field" if mcp => {
                let mut line = input.clone();
                line["op"] = json!(name);
                serde_json::from_value::<Line>(line)
                    .map_err(|e| format!("bad input: {e}"))
                    .and_then(|line| run_lines(self.doc, &[line]).map(|_| self.summary()))
            }
            "redo" if mcp => self.doc.redo().map(|label| json!({ "redone": label })),
            verb if mcp && commands::verbs().any(|v| v.id == verb) => {
                let mut line = input.clone();
                line["op"] = json!(verb);
                match serde_json::from_value::<Line>(line) {
                    Ok(line) => {
                        let (k, v) = line.describe();
                        // Applied at once, so no card shows a preview of it.
                        self.propose_lines(&format!("{k}: {v}"), vec![line], caller, false).and_then(|r| {
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
        let result = journal.finish(result);
        if name == "open" && result.is_ok() {
            self.doc.model().journal_key();
        }
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
    let mut doc = Document::new("Part1", crate::kernel());
    let rect = |center: Point, width: f64, height: f64| Shape::Rectangle { center, width, height };
    let circle = |center: Point, diameter: f64| Shape::Circle { center, diameter };
    let steps = [
        vec![
            Line::Sketch { plane: "top".into(), offset: 0.0, shapes: vec![rect([60.0, 45.0, 0.0], 120.0, 90.0)] },
            Line::Extrude { sketch: "previous".into(), depth: 10.0, flip: false, up_to_next: false, up_to_face: None },
        ],
        vec![
            Line::Sketch { plane: "front".into(), offset: 0.0, shapes: vec![rect([60.0, 0.0, 30.0], 120.0, 60.0)] },
            Line::Extrude { sketch: "previous".into(), depth: 10.0, flip: true, up_to_next: false, up_to_face: None },
        ],
        vec![
            Line::Sketch {
                plane: "Boss-Extrude1 · End cap".into(),
                offset: 0.0,
                shapes: vec![circle([85.0, 55.0, 10.0], 24.0)],
            },
            Line::Extrude { sketch: "previous".into(), depth: 24.0, flip: false, up_to_next: false, up_to_face: None },
        ],
        vec![
            Line::Sketch {
                plane: "Boss-Extrude3 · End cap".into(),
                offset: 0.0,
                shapes: vec![circle([85.0, 55.0, 34.0], 12.0)],
            },
            Line::Cut {
                sketch: "previous".into(),
                through_all: true,
                depth: 1.0,
                flip: false,
                up_to_next: false,
                up_to_face: None,
            },
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
    use soliddraft_kernel_api::{
        BodyId, BooleanOp, EdgeId, EdgePolyline, Extrude, HistoryRecord, KernelResult, MassProperties, Mesh, OpResult,
        OpTag, Profile, Revolve, TessellationParams, Topology, Validation,
    };

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
    fn revolve_axes_still_require_three_components() {
        for field in ["axis_origin", "axis_direction"] {
            for malformed in [json!([]), json!([0, 1]), json!([0, 1, 0, 0])] {
                let mut input = json!({"op":"revolve", "sketch":"Sketch1",
                    "axis_origin":[0,0,0], "axis_direction":[0,1,0], "angle_deg":360});
                input[field] = malformed;
                assert!(serde_json::from_value::<Line>(input).is_err(), "{field} requires exactly three components");
            }
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
        let doc = board_bracket();
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
        let v = doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume;
        let pi = std::f64::consts::PI;
        let expected = 120.0 * 90.0 * 10.0 + 120.0 * 10.0 * 50.0 + pi * 144.0 * 24.0 - pi * 36.0 * 34.0;
        assert!((v - expected).abs() < 50.0, "volume {v}, expected {expected}");
    }

    #[test]
    fn assistant_values_require_a_proposal_and_explicit_apply() {
        let mut doc = Document::new("Part1", crate::kernel());
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
        let arguments = json!({"name":"plate_w","expression":"140"});
        assert!(call_checked(&mut tools, "set_value", arguments, &assistant()).is_error);
        let proposed = call_checked(
            &mut tools,
            "propose",
            json!({
                "summary":"Name the width", "commands":[{"op":"set_value","name":"plate_w","expression":"140"}]
            }),
            &assistant(),
        );
        assert!(!proposed.is_error, "{}", proposed.content);
        assert!(tools.doc.model().values().is_empty());
        assert!(call_checked(&mut tools, "apply", json!({"proposal_id":"P1"}), &assistant()).is_error);
        assert!(tools.doc.model().values().is_empty());
        tools.book.apply(tools.doc, "P1").unwrap();
        assert_eq!(tools.doc.model().values().summary()[0]["value"], json!(140.0));
    }

    #[test]
    fn propose_apply_undo_is_one_step() {
        let mut doc = board_bracket();
        let mut book = ProposalBook::default();
        let before = doc.features.len();
        let depth = doc.undo_depth();
        let volume_before = doc.kernel().mass_properties(doc.display_body().unwrap()).unwrap().volume;
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
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
        assert_eq!(tools.doc.features.len(), before, "proposing changes nothing");
        assert!(tools.book.get("P1").unwrap().preview.result.is_some());

        assert!(call_checked(&mut tools, "apply", json!({ "proposal_id": "P1" }), &assistant()).is_error);
        tools.book.apply(tools.doc, "P1").unwrap();
        let volume_after = tools.doc.kernel().mass_properties(tools.doc.display_body().unwrap()).unwrap().volume;
        let removed = volume_before - volume_after;
        assert!((removed - 4.0 * std::f64::consts::PI * 4.5 * 4.5 * 10.0).abs() < 5.0, "removed {removed}");
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
        let mut doc = board_bracket();
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
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
        let mut doc = board_bracket();
        let mut book = ProposalBook::default();
        let tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
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
        use soliddraft_kernel_api::counting::{Counting, Work};
        let kernel = std::sync::Arc::new(Counting::new(crate::kernel()));
        let mut doc = Document::new("Part1", kernel.clone());
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp, drawing: None };
        let mcp = Caller::Mcp { client: "test".into() };
        let out = call_checked(
            &mut tools,
            "sketch",
            json!({ "plane": "top", "shapes": [{ "shape": "rectangle", "center": [0, 0, 0], "width": 40, "height": 30 }] }),
            &mcp,
        );
        assert!(!out.is_error, "{}", out.content);
        kernel.take();
        let out = call_checked(&mut tools, "extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }), &mcp);
        assert!(!out.is_error, "{}", out.content);
        let operations = kernel.take().into_iter().filter(|w| matches!(w, Work::Operation(_))).count();
        assert_eq!(operations, 2, "the dry run's profile and extrusion must not be built again on apply");
        let body = tools.doc.display_body();
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
        assert_eq!(tools.doc.display_body(), body);
        assert!(kernel.take().iter().all(|w| !matches!(w, Work::Operation(_))), "history rebuilt the prepared body");

        let mesh = tools.doc.meshes[&body.unwrap()].clone();
        let out = call_checked(
            &mut tools,
            "sketch",
            json!({ "plane": "top", "shapes": [{ "shape": "circle", "center": [0, 0, 0], "diameter": 5 }] }),
            &mcp,
        );
        assert!(!out.is_error, "{}", out.content);
        // A direct verb is applied at once, so its dry run makes no preview.
        assert!(tools.book.proposals.last().unwrap().preview.result.is_none());
        assert!(
            std::sync::Arc::ptr_eq(&tools.doc.meshes[&body.unwrap()], &mesh),
            "a sketch preflight regenerated the solid's display buffers"
        );
    }

    /// Catallax, refusing every proof a measurement asks for.
    struct RefusesProofs(std::sync::Arc<dyn soliddraft_kernel_api::Kernel>);

    impl soliddraft_kernel_api::Kernel for RefusesProofs {
        fn backend(&self) -> soliddraft_kernel_api::BackendInfo {
            self.0.backend()
        }
        fn create_body_from_profile(&self, tag: &OpTag, profile: &Profile) -> KernelResult<OpResult> {
            self.0.create_body_from_profile(tag, profile)
        }
        fn extrude(&self, tag: &OpTag, profile: BodyId, params: &Extrude) -> KernelResult<OpResult> {
            self.0.extrude(tag, profile, params)
        }
        fn revolve(&self, tag: &OpTag, profile: BodyId, params: &Revolve) -> KernelResult<OpResult> {
            self.0.revolve(tag, profile, params)
        }
        fn boolean(&self, tag: &OpTag, op: BooleanOp, target: BodyId, tool: BodyId) -> KernelResult<OpResult> {
            self.0.boolean(tag, op, target, tool)
        }
        fn fillet(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], radius: f64) -> KernelResult<OpResult> {
            self.0.fillet(tag, body, edges, radius)
        }
        fn chamfer(&self, tag: &OpTag, body: BodyId, edges: &[EdgeId], distance: f64) -> KernelResult<OpResult> {
            self.0.chamfer(tag, body, edges, distance)
        }
        fn shell(&self, tag: &OpTag, body: BodyId, open: &[FaceId], thickness: f64) -> KernelResult<OpResult> {
            self.0.shell(tag, body, open, thickness)
        }
        fn tessellate(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Mesh> {
            self.0.tessellate(body, params)
        }
        fn tessellate_edges(&self, body: BodyId, params: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
            self.0.tessellate_edges(body, params)
        }
        fn mass_properties(&self, _body: BodyId) -> KernelResult<MassProperties> {
            Err(Self::refusal("mass_properties"))
        }
        fn validate(&self, body: BodyId) -> KernelResult<Validation> {
            self.0.validate(body)
        }
        fn topology(&self, body: BodyId) -> KernelResult<Topology> {
            self.0.topology(body)
        }
        fn measured_topology(&self, _body: BodyId) -> KernelResult<Topology> {
            Err(Self::refusal("measured_topology"))
        }
        fn measure_pair(
            &self,
            _body: BodyId,
            _a: soliddraft_kernel_api::PairEndpoint,
            _b: soliddraft_kernel_api::PairEndpoint,
            _quantity: soliddraft_kernel_api::PairQuantity,
        ) -> KernelResult<soliddraft_kernel_api::Measurement> {
            Err(Self::refusal("measure_pair"))
        }
        fn history(&self, body: BodyId) -> KernelResult<Vec<HistoryRecord>> {
            self.0.history(body)
        }
        fn release(&self, body: BodyId) -> KernelResult<()> {
            self.0.release(body)
        }
    }

    impl RefusesProofs {
        fn refusal(query: &str) -> soliddraft_kernel_api::KernelError {
            soliddraft_kernel_api::KernelError::QueryFailed {
                query: query.into(),
                code: 0,
                message: "work limit reached".into(),
            }
        }
    }

    #[test]
    fn mcp_measure_refuses_when_the_kernel_refuses_the_proof() {
        let kernel = std::sync::Arc::new(RefusesProofs(crate::kernel()));
        let mut doc = Document::new("Part1", kernel);
        let mut book = ProposalBook::default();
        let mut tools = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp, drawing: None };
        let mcp = Caller::Mcp { client: "test".into() };
        let rectangle = json!({ "shape": "rectangle", "center": [0, 0, 0], "width": 40, "height": 30 });
        assert!(!call_checked(&mut tools, "sketch", json!({ "plane": "top", "shapes": [rectangle] }), &mcp).is_error);
        let out = call_checked(&mut tools, "extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }), &mcp);
        assert!(!out.is_error, "{}", out.content);

        for (a, b) in [
            ("Boss-Extrude1 · Start cap", "Boss-Extrude1 · End cap"),
            ("Boss-Extrude1 · Edge 1", "Boss-Extrude1 · End cap"),
        ] {
            let m = call_checked(&mut tools, "measure", json!({ "a": a, "b": b }), &mcp);
            assert!(m.is_error, "{a} to {b} answered without a proof: {}", m.content);
            assert!(
                m.content.contains("could not be measured") && m.content.contains("work limit reached"),
                "{}",
                m.content
            );
        }
        // A feature has no proven pair quantity; it is refused before any proof.
        let m =
            call_checked(&mut tools, "measure", json!({ "a": "Boss-Extrude1", "b": "Boss-Extrude1 · End cap" }), &mcp);
        assert!(m.is_error && m.content.contains("select an edge or a face"), "{}", m.content);

        // The descriptions still show the display's estimates, and say so.
        let feature = call_checked(&mut tools, "feature", json!({ "name": "Boss-Extrude1" }), &mcp);
        let v: Value = serde_json::from_str(&feature.content).unwrap();
        assert!(v["estimates"].as_str().is_some_and(|s| s.contains("display mesh")), "{v}");
    }
}
