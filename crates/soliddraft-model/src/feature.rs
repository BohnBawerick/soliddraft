//! Features: the steps of a part, in tree order.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use soliddraft_kernel_api::{Decision, HoleDepth, HoleHead, HoleTip, Plane, Vec3, question_index};
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
    /// A body read from an external STEP file, with the answers to its repair questions.
    ImportStep(ImportStepFeature),
    /// Turns the finished body into a lattice. It is always the last feature.
    Lattice(LatticeFeature),
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
            FeatureKind::ImportStep(_) => "Import",
            FeatureKind::Lattice(_) => "Lattice",
        }
    }

    /// True for the terminal lattice, which no CAD feature may follow.
    pub fn is_lattice(&self) -> bool {
        matches!(self, FeatureKind::Lattice(_))
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
            FeatureKind::Pattern(_) | FeatureKind::ImportStep(_) | FeatureKind::Lattice(_) => {}
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

/// The final lattice stage. It reads the whole finished body, so it references nothing.
/// Cell, density and output settings are saved with the feature.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatticeFeature {
    /// Missing in legacy files, which keep their earlier body visible until configured.
    #[serde(default)]
    pub cell: Option<soliddraft_kernel_api::LatticeCell>,
    /// Cubic cell edge length, in millimetres.
    #[serde(default)]
    pub cell_size_mm: Option<f64>,
    /// Density expressed as a constant beam radius, in millimetres.
    #[serde(default)]
    pub radius_mm: Option<f64>,
    /// Grid fixed to the part origin and axes.
    #[serde(default)]
    pub frame: soliddraft_kernel_api::LatticeFrame,
    /// Accuracy the lattice is proven to, in millimetres.
    pub accuracy_mm: f64,
    /// The smallest detail the lattice keeps, in millimetres.
    pub min_detail_mm: f64,
}

impl Default for LatticeFeature {
    fn default() -> Self {
        LatticeFeature {
            cell: Some(soliddraft_kernel_api::LatticeCell::Cubic),
            cell_size_mm: Some(5.0),
            radius_mm: Some(1.0),
            frame: soliddraft_kernel_api::LatticeFrame::Part,
            accuracy_mm: 0.1,
            min_detail_mm: 0.5,
        }
    }
}

/// An imported body: the source file, kept unchanged, and what the user decided about
/// its proposed repairs. The body is derived from the two and rebuilds from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportStepFeature {
    /// The file name as imported, for display. It is never used as a path.
    pub source_name: String,
    /// The sha256 of `source`, in lowercase hex.
    pub sha256: String,
    /// The file bytes, unchanged.
    #[serde(with = "hex_bytes")]
    pub source: Vec<u8>,
    /// Answers to repair questions, by question id: `accept:<tolerance_mm>` or `decline`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub decisions: BTreeMap<String, String>,
    /// Who accepted the proposals, as the app supplied it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approver: String,
}

impl ImportStepFeature {
    /// An import of `source`, hashed, with no decisions.
    pub fn new(source_name: impl Into<String>, source: Vec<u8>) -> ImportStepFeature {
        ImportStepFeature {
            source_name: source_name.into(),
            sha256: sha256_hex(&source),
            source,
            decisions: BTreeMap::new(),
            approver: String::new(),
        }
    }

    /// True when `sha256` is the hash of `source`.
    pub fn hash_matches(&self) -> bool {
        self.sha256 == sha256_hex(&self.source)
    }

    /// This import with `question` accepted at `tolerance_mm`, replacing any earlier answer.
    pub fn accepted(&self, question: &str, tolerance_mm: &str, approver: &str) -> ImportStepFeature {
        let mut out = self.clone();
        out.decisions.insert(question.to_string(), format!("accept:{tolerance_mm}"));
        out.approver = approver.to_string();
        out
    }

    /// This import with `question` declined, replacing any earlier answer.
    pub fn declined(&self, question: &str) -> ImportStepFeature {
        let mut out = self.clone();
        out.decisions.insert(question.to_string(), "decline".to_string());
        out
    }

    /// This import with the answer to `question` removed, so it is asked again.
    pub fn unanswered(&self, question: &str) -> ImportStepFeature {
        let mut out = self.clone();
        out.decisions.remove(question);
        out
    }

    /// The saved answers as the kernel reads them, or why one cannot be read. A question
    /// must name a gap of this source, a tolerance must be a plain decimal, and an
    /// acceptance needs an approver.
    pub fn kernel_decisions(&self) -> Result<Vec<Decision>, String> {
        let mut out = Vec::new();
        for (question, answer) in &self.decisions {
            let Some(index) = question_index(&self.sha256, question) else {
                return Err(format!("{question} is not a question about this file"));
            };
            let id = format!("g{index}");
            match answer.strip_prefix("accept:") {
                _ if answer == "decline" => out.push(Decision::Decline { id }),
                Some(tol) if is_decimal(tol) && !self.approver.is_empty() => {
                    out.push(Decision::Accept { id, tolerance_mm: tol.to_string(), approver: self.approver.clone() });
                }
                Some(tol) if is_decimal(tol) => return Err(format!("{question} is accepted without an approver")),
                _ => return Err(format!("{question} has an unreadable answer {answer:?}")),
            }
        }
        Ok(out)
    }
}

/// True for a plain non-negative decimal such as `0.02`.
fn is_decimal(text: &str) -> bool {
    let (whole, frac) = text.split_once('.').unwrap_or((text, "0"));
    !whole.is_empty()
        && !frac.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

/// The sha256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Source bytes in a part file: lowercase hex, so the text file stays deterministic.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() % 2 != 0 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(D::Error::custom("source bytes must be lowercase hex"));
        }
        Ok((0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("checked hex")).collect())
    }
}
