//! The recipe file format, `soliddraft-recipe/1`.
//!
//! A recipe is an ordered list of steps. Each step has an id, runs one kernel command,
//! and names its inputs by the ids of earlier steps. Faces and edges are never named by
//! kernel ids, which differ between backends; they are chosen by selectors that are
//! evaluated against the body and its history when the step runs.
//!
//! ```json
//! {
//!   "format": "soliddraft-recipe/1",
//!   "name": "box",
//!   "steps": [
//!     { "id": "sketch", "op": "profile", "plane": "XY",
//!       "rectangle": { "center": [0, 0], "width": 40, "height": 30 } },
//!     { "id": "base", "op": "extrude", "profile": "sketch", "distance": 10 }
//!   ],
//!   "result": "base"
//! }
//! ```

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{BooleanOp, Loop, Plane, Point2, Vec3};

/// The format identifier every recipe carries.
pub const FORMAT: &str = "soliddraft-recipe/1";

/// A replayable model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Always [`FORMAT`].
    pub format: String,
    /// Short name, used in reports.
    pub name: String,
    /// What the recipe builds, for people.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The steps, in order.
    pub steps: Vec<Step>,
    /// The id of the step whose body is the recipe's result.
    pub result: String,
}

/// Whether a step is meant to succeed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The step must succeed.
    #[default]
    Ok,
    /// The step must fail, and must leave its input bodies unchanged. It produces no body.
    Error,
}

/// One command with its id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Unique id; later steps refer to the body by it, and history records carry it as
    /// their tag.
    pub id: String,
    /// The command.
    #[serde(flatten)]
    pub command: Command,
    /// Whether the step is meant to succeed.
    #[serde(default, skip_serializing_if = "is_ok")]
    pub expect: Outcome,
}

fn is_ok(o: &Outcome) -> bool {
    *o == Outcome::Ok
}

/// A kernel command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// A deferred closed profile definition.
    Profile {
        /// The sketch plane.
        plane: PlaneSpec,
        /// The region.
        #[serde(flatten)]
        shape: Shape,
    },
    /// A linear extrusion of a profile.
    Extrude {
        /// Id of the profile step.
        profile: String,
        /// Distance, greater than zero.
        distance: f64,
        /// Direction; the profile plane's normal when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<Vec3>,
    },
    /// A revolution of a profile.
    Revolve {
        /// Id of the profile step.
        profile: String,
        /// A point on the axis.
        axis_origin: Vec3,
        /// Axis direction, in the profile plane.
        axis_direction: Vec3,
        /// Angle in degrees, in (0, 360].
        angle_deg: f64,
    },
    /// A boolean of two solids.
    Boolean {
        /// Union, subtract or intersect.
        kind: BooleanOp,
        /// Id of the target step.
        target: String,
        /// Id of the tool step.
        tool: String,
    },
    /// A circular fillet.
    Fillet {
        /// Id of the input step.
        body: String,
        /// Which edges.
        edges: Selector,
        /// Radius.
        radius: f64,
    },
    /// A flat chamfer.
    Chamfer {
        /// Id of the input step.
        body: String,
        /// Which edges.
        edges: Selector,
        /// Chamfer width.
        distance: f64,
    },
    /// A shell.
    Shell {
        /// Id of the input step.
        body: String,
        /// Faces to leave open.
        open_faces: Selector,
        /// Wall thickness.
        thickness: f64,
    },
}

impl Command {
    /// Ids of the steps this command reads.
    pub fn inputs(&self) -> Vec<&str> {
        match self {
            Command::Profile { .. } => vec![],
            Command::Extrude { profile, .. } | Command::Revolve { profile, .. } => vec![profile],
            Command::Boolean { target, tool, .. } => vec![target, tool],
            Command::Fillet { body, .. } | Command::Chamfer { body, .. } | Command::Shell { body, .. } => vec![body],
        }
    }
}

/// A named world plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedPlane {
    /// Normal +Z.
    XY,
    /// Normal +X.
    YZ,
    /// Normal +Y.
    ZX,
}

/// A sketch plane in a recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlaneSpec {
    /// A world plane through the origin: `"XY"`.
    Named(NamedPlane),
    /// A world plane moved to a new origin: `{ "base": "XY", "origin": [0, 0, 5] }`.
    Offset {
        /// The world plane.
        base: NamedPlane,
        /// The new origin.
        origin: Vec3,
    },
    /// Any plane: `{ "origin": [...], "normal": [...], "x_dir": [...] }`.
    Full(Plane),
}

impl PlaneSpec {
    /// The plane this spec describes.
    pub fn plane(&self) -> Plane {
        let named = |n: NamedPlane| match n {
            NamedPlane::XY => Plane::XY,
            NamedPlane::YZ => Plane::YZ,
            NamedPlane::ZX => Plane::ZX,
        };
        match self {
            PlaneSpec::Named(n) => named(*n),
            PlaneSpec::Offset { base, origin } => named(*base).with_origin(*origin),
            PlaneSpec::Full(p) => *p,
        }
    }
}

/// The region of a profile step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// An axis-aligned rectangle in plane coordinates.
    Rectangle {
        /// Centre.
        center: Point2,
        /// Size along the plane's X direction.
        width: f64,
        /// Size along the plane's Y direction.
        height: f64,
    },
    /// A circle.
    Circle {
        /// Centre.
        center: Point2,
        /// Radius.
        radius: f64,
    },
    /// Explicit loops.
    Loops {
        /// The outer loop.
        outer: Loop,
        /// Hole loops.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        holes: Vec<Loop>,
    },
}

/// Chooses faces or edges of a body when a step runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selector {
    /// Every face or edge: `"all"`.
    All,
    /// The entities a step generated that still exist, followed through later splits:
    /// `{ "created_by": "joined" }`. This is the persistent-naming path: it uses only
    /// the kernel's history, never ids.
    CreatedBy(String),
    /// The one entity whose centroid (faces) or midpoint (edges) is nearest:
    /// `{ "nearest": [x, y, z] }`.
    Nearest(Vec3),
    /// Entities whose bounding box lies inside a box:
    /// `{ "within": { "min": [...], "max": [...] } }`.
    Within {
        /// Minimum corner.
        min: Vec3,
        /// Maximum corner.
        max: Vec3,
    },
    /// Entities every listed selector chooses: `{ "all_of": [ ... ] }`.
    AllOf(Vec<Selector>),
}
