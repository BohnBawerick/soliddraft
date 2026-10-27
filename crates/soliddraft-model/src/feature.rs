//! Features: the steps of a part, in tree order.

use std::fmt;

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{HoleDepth, HoleHead, HoleTip, Plane, Vec3};
use soliddraft_sketch::{P2, Sketch};

use crate::reference::{EdgeRef, FaceRef};

/// Id of a feature. It never changes and is never reused within a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureId(pub u32);

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "F{}", self.0)
    }
}

/// One step of the part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    /// Stable id.
    pub id: FeatureId,
    /// The name the tree shows, such as "Boss-Extrude1".
    pub name: String,
    /// What the feature does.
    pub kind: FeatureKind,
    /// A suppressed feature stays in the tree but is not built.
    #[serde(default, skip_serializing_if = "is_false")]
    pub suppressed: bool,
    /// The assistant proposed the feature and the user applied it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub via_assistant: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// What a feature does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeatureKind {
    /// A 2D sketch on a plane.
    Sketch(SketchFeature),
    /// A boss or cut swept from a sketch's closed regions.
    Extrude(ExtrudeFeature),
    /// A sketch swept around a model-space axis.
    Revolve(RevolveFeature),
    /// Rounds edges.
    Fillet(BlendFeature),
    /// Bevels edges.
    Chamfer(BlendFeature),
    /// Drills holes into a planar face of the part.
    Hole(HoleFeature),
    /// Copies the part's body along a line or around an axis.
    Pattern(PatternFeature),
    /// Reflects the part's body in a plane.
    Mirror(MirrorFeature),
    /// Tilts faces of the part away from a pull direction.
    Draft(DraftFeature),
    /// Hollows the body, leaving the selected faces open.
    Shell(ShellFeature),
}

impl FeatureKind {
    /// The tree's base name for the kind, numbered to make a feature name.
    pub fn base_name(&self) -> &'static str {
        match self {
            FeatureKind::Sketch(_) => "Sketch",
            FeatureKind::Revolve(_) => "Revolve",
            FeatureKind::Extrude(e) if e.operation == ExtrudeOp::Cut => "Cut-Extrude",
            FeatureKind::Extrude(_) => "Boss-Extrude",
            FeatureKind::Fillet(_) => "Fillet",
            FeatureKind::Chamfer(_) => "Chamfer",
            FeatureKind::Hole(_) => "Hole",
            FeatureKind::Pattern(p) => match p.layout {
                PatternSpec::Linear { .. } => "LPattern",
                PatternSpec::Circular { .. } => "CirPattern",
            },
            FeatureKind::Mirror(_) => "Mirror",
            FeatureKind::Draft(_) => "Draft",
            FeatureKind::Shell(_) => "Shell",
        }
    }

    /// The features this one reads: its sketch, and the features that made the faces
    /// and edges it references.
    pub fn dependencies(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        match self {
            FeatureKind::Sketch(s) => {
                if let SketchPlane::Face { face } = &s.plane {
                    out.push(face.feature);
                }
            }
            FeatureKind::Revolve(r) => out.push(r.sketch),
            FeatureKind::Extrude(e) => {
                if let ProfileSource::Sketch { feature } = e.profile {
                    out.push(feature);
                }
                if let EndCondition::UpToFace { face } = &e.end {
                    out.push(face.feature);
                }
            }
            FeatureKind::Fillet(b) | FeatureKind::Chamfer(b) => {
                out.extend(b.edges.iter().flat_map(|e| e.faces.iter().map(|f| f.feature)));
            }
            FeatureKind::Hole(h) => out.push(h.face.feature),
            FeatureKind::Pattern(_) => {}
            FeatureKind::Mirror(m) => {
                if let PlaneRef::Face { face } = &m.plane {
                    out.push(face.feature);
                }
            }
            FeatureKind::Shell(s) => out.extend(s.open_faces.iter().map(|f| f.feature)),
            FeatureKind::Draft(d) => {
                out.extend(d.faces.iter().map(|f| f.feature));
                if let PlaneRef::Face { face } = &d.neutral {
                    out.push(face.feature);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// A boss swept around an exact axis through the kernel's revolution operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevolveFeature {
    /// The closed meridian sketch.
    pub sketch: FeatureId,
    /// Point on the axis, in model millimetres.
    pub axis_origin: [f64; 3],
    /// Nonzero axis direction, in model coordinates.
    pub axis_direction: [f64; 3],
    /// Sweep angle in degrees, greater than zero and at most 360.
    pub angle_deg: f64,
}

/// A sketch and the plane it is drawn on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchFeature {
    /// Where the sketch lies.
    pub plane: SketchPlane,
    /// The geometry and constraints, in the plane's coordinates.
    pub sketch: Sketch,
}

/// Where a sketch lies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum SketchPlane {
    /// A reference plane, moved along its normal by `offset` millimetres.
    Datum {
        /// Which reference plane.
        datum: DatumPlane,
        /// Distance along the plane's normal.
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f64,
    },
    /// A planar face of the part, found again on every rebuild by its persistent name.
    /// The sketch's X axis is horizontal when the face is not, and the world X axis
    /// projected onto the face when it is; the origin is the world origin projected onto
    /// the face. The normal points out of the material.
    Face {
        /// The face.
        face: FaceRef,
    },
}

/// The three reference planes. Z is up; the Front plane faces a viewer standing on -Y.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatumPlane {
    /// The XY plane, normal +Z.
    Top,
    /// The XZ plane, normal -Y, X to the right and Z up.
    Front,
    /// The YZ plane, normal +X, Y to the right and Z up.
    Right,
}

impl DatumPlane {
    /// The plane through the origin.
    pub fn plane(self) -> Plane {
        match self {
            DatumPlane::Top => Plane::XY,
            DatumPlane::Front => Plane { origin: Vec3::ZERO, normal: -Vec3::Y, x_dir: Vec3::X },
            DatumPlane::Right => Plane::YZ,
        }
    }

    /// Name for people.
    pub fn label(self) -> &'static str {
        match self {
            DatumPlane::Top => "Top plane",
            DatumPlane::Front => "Front plane",
            DatumPlane::Right => "Right plane",
        }
    }
}

/// Whether an extrusion adds or removes material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeOp {
    /// Adds material: the first boss makes the part's body, later ones join it.
    Boss,
    /// Removes material from the part's body.
    Cut,
}

/// How far an extrusion goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EndCondition {
    /// A fixed depth from the sketch plane.
    Blind {
        /// Depth in millimetres, greater than zero.
        depth: f64,
    },
    /// Through everything beyond the sketch plane, in the extrusion direction.
    ThroughAll,
    /// To the first place the sweep crosses the part's boundary.
    UpToNext,
    /// To the first hit on a face of the part.
    UpToFace {
        /// The face, by persistent name.
        face: FaceRef,
    },
}

/// Where an extrusion's profile comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum ProfileSource {
    /// Every closed region of a sketch feature.
    Sketch {
        /// The sketch feature.
        feature: FeatureId,
    },
    /// A built-in shape centred on a reference plane's origin: a quick extrude without
    /// a sketch feature.
    Shape {
        /// The reference plane.
        datum: DatumPlane,
        /// The shape.
        shape: BuiltinShape,
    },
}

/// A built-in profile, centred on its plane's origin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuiltinShape {
    /// An axis-aligned rectangle.
    Rectangle {
        /// Size along the plane's X axis.
        width: f64,
        /// Size along the plane's Y axis.
        height: f64,
    },
    /// A circle.
    Circle {
        /// Diameter.
        diameter: f64,
    },
}

impl BuiltinShape {
    /// The shape as a fully constrained sketch. The same shape always gives the same
    /// entity ids, so faces named after its entities keep their names.
    pub fn sketch(&self) -> Result<Sketch, String> {
        let mut sketch = Sketch::new();
        let made = match *self {
            BuiltinShape::Rectangle { width, height } => sketch.rectangle(P2::ZERO, width, height),
            BuiltinShape::Circle { diameter } => sketch.circle_at(P2::ZERO, diameter),
        };
        made.map_err(|e| e.to_string())?;
        soliddraft_sketch::solve::solve(&mut sketch, None, false);
        Ok(sketch)
    }
}

/// A boss or cut swept from the closed regions of a profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtrudeFeature {
    /// The profile.
    pub profile: ProfileSource,
    /// Add or remove material.
    pub operation: ExtrudeOp,
    /// How far.
    pub end: EndCondition,
    /// Sweep against the sketch plane's normal instead of along it. A cut on a face of
    /// the part usually needs this, since the face normal points out of the material.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
}

/// A fillet or chamfer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlendFeature {
    /// The edges, by persistent name.
    pub edges: Vec<EdgeRef>,
    /// Fillet radius or chamfer distance, in millimetres.
    pub size: f64,
}

/// Holes drilled into a planar face, one per point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HoleFeature {
    /// The planar face the holes start on. They drill against its outward normal.
    pub face: FaceRef,
    /// Hole centres in the face's sketch frame (see
    /// [`plane_on_face`](crate::plane_on_face)), in millimetres.
    pub points: Vec<P2>,
    /// Diameter of the main wall.
    pub diameter: f64,
    /// How deep the main wall goes.
    pub depth: HoleDepth,
    /// The bottom of a blind hole.
    pub tip: HoleTip,
    /// A counterbore or countersink at the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<HoleHead>,
}

/// Where a pattern's copies go.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PatternSpec {
    /// Copies along a direction.
    Linear {
        /// The direction, of any length.
        direction: Vec3,
        /// Distance between neighbouring copies, in millimetres.
        spacing: f64,
        /// Number of copies, including the original.
        count: u32,
    },
    /// Copies around an axis.
    Circular {
        /// A point on the axis.
        axis_origin: Vec3,
        /// The axis direction; positive angles turn counter-clockwise about it.
        axis_direction: Vec3,
        /// Number of copies, including the original.
        count: u32,
        /// Angle between neighbouring copies, in degrees.
        angle: f64,
    },
}

/// Copies of the part's body, united with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternFeature {
    /// Where the copies go.
    pub layout: PatternSpec,
}

/// A plane a feature reads: a reference plane or a planar face of the part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum PlaneRef {
    /// A reference plane, moved along its normal by `offset` millimetres.
    Datum {
        /// Which reference plane.
        datum: DatumPlane,
        /// Distance along the plane's normal.
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: f64,
    },
    /// A planar face of the part, with its outward normal.
    Face {
        /// The face.
        face: FaceRef,
    },
}

/// The part's body reflected in a plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirrorFeature {
    /// The mirror plane.
    pub plane: PlaneRef,
    /// Unites the reflection with the original when true; keeps only the reflection
    /// when false.
    pub keep_original: bool,
}

/// Faces tilted by an angle about a neutral plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DraftFeature {
    /// The faces to tilt.
    pub faces: Vec<FaceRef>,
    /// Where the drafted faces keep their place. The pull direction is its normal.
    pub neutral: PlaneRef,
    /// Draft angle in degrees, in (-90, 90).
    pub angle: f64,
    /// Pull against the neutral plane's normal instead of along it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
}

/// A constant-thickness shell with persistent references to its openings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellFeature {
    /// Faces removed to open the shell; empty means a closed shell.
    pub open_faces: Vec<FaceRef>,
    /// Positive, finite wall thickness in millimetres.
    pub thickness: f64,
}
