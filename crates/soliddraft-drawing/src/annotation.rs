//! Annotations: dimensions, notes and symbols attached to model entities in a view.

use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{Point2, Vec3};

use crate::entity::EntityRef;
use crate::ids::{AnnotationId, ViewId};

/// One annotation on a sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// Id, unique within the drawing.
    pub id: AnnotationId,
    /// The view it belongs to. It moves with the view.
    pub view: ViewId,
    /// What it is.
    pub body: AnnotationKind,
    /// The model entities it is attached to, in order. A linear dimension has two, a
    /// diameter one, a note's leader one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<Anchor>,
    /// Where its text sits, in view millimetres.
    pub placement: Point2,
    /// Style that differs from the template's.
    #[serde(default, skip_serializing_if = "StyleOverrides::is_empty")]
    pub style: StyleOverrides,
    /// Whether its references still resolve.
    #[serde(default)]
    pub state: AnnotationState,
}

impl Annotation {
    /// True when the annotation changed or lost a reference in the last update.
    pub fn needs_attention(&self) -> bool {
        self.state != AnnotationState::Ok
    }
}

/// What an annotation is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationKind {
    /// A dimension.
    Dimension(Dimension),
    /// Free text, with an optional leader.
    Note {
        /// The text. Lines are separated by `\n`.
        text: String,
        /// Leader bend points in view millimetres, from the text towards the anchor.
        /// Empty for a note without a leader.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        leader: Vec<Point2>,
    },
    /// A callout read from a hole feature, such as "4X ⌀9 THRU".
    HoleCallout {
        /// The callout text.
        text: String,
    },
    /// A centre mark on a circular edge.
    CentreMark,
    /// A centreline of a cylindrical face or between two edges.
    Centreline,
    /// A surface finish symbol (ISO 1302, ASME Y14.36) on a surface or its extension
    /// line. Its anchor, when it has one, is the surface; the symbol stands on that
    /// point and its text sits at the placement.
    SurfaceFinish {
        /// The roughness requirement, such as "Ra 1.6".
        text: String,
        /// What the process may do to the surface.
        #[serde(default)]
        process: SurfaceProcess,
        /// The production method, such as "Ground", written on the symbol's extension.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        method: String,
        /// The direction of the lay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lay: Option<Lay>,
        /// The machining allowance, such as "0.5".
        #[serde(default, skip_serializing_if = "String::is_empty")]
        allowance: String,
        /// The requirement holds all around the outline.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        all_around: bool,
    },
    /// A geometric tolerance: a feature control frame (ISO 1101, ASME Y14.5), with a
    /// leader to its anchor when it has one.
    Gdt {
        /// The geometric characteristic.
        characteristic: GdtCharacteristic,
        /// The tolerance value in millimetres.
        tolerance: f64,
        /// The tolerance zone is a cylinder: the value carries a diameter sign.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        diameter: bool,
        /// The material condition the tolerance applies at.
        #[serde(default, skip_serializing_if = "MaterialCondition::is_regardless")]
        material: MaterialCondition,
        /// Datum references, primary first.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        datums: Vec<DatumRef>,
    },
    /// A datum feature symbol: the letter in a frame, joined to a filled triangle on the
    /// feature it names.
    Datum {
        /// The datum letter.
        letter: String,
    },
    /// A revision symbol: the revision letter in a triangle, with a leader to its anchor
    /// when it has one.
    RevisionSymbol {
        /// The revision it marks.
        revision: String,
    },
    /// Ordinate, baseline or chain dimensions: several distances along one direction
    /// measured from the first anchor or from one anchor to the next.
    DimensionSet(DimensionSet),
}

/// Ordinate, baseline or chain dimensions as one annotation. Anchor 0 is the origin (for
/// a chain, the first end); `values[i]` belongs to anchor `i + 1`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DimensionSet {
    /// How the distances are measured and drawn.
    pub kind: SetKind,
    /// The direction distances are measured in: horizontal or vertical on the sheet.
    pub orientation: LinearOrientation,
    /// The measured distances, one per anchor after the first: from anchor 0 for
    /// ordinate and baseline sets, from the previous anchor for a chain.
    pub values: Vec<f64>,
    /// Decimal places shown.
    pub precision: u8,
    /// Whether lengths are measured in the view plane or in 3D.
    #[serde(default)]
    pub dim_type: DimType,
}

impl DimensionSet {
    /// The values as the set displays them, joined: what "changed" compares.
    pub fn values_text(&self) -> String {
        self.values.iter().map(|v| crate::measure::format_value(*v, self.precision)).collect::<Vec<_>>().join(", ")
    }
}

/// How a set of dimensions is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetKind {
    /// Each point's distance from the origin, written at the end of a leader, the origin
    /// marked 0.
    Ordinate,
    /// Stacked dimension lines, all from the origin.
    Baseline,
    /// Dimension lines end to end on one line.
    Chain,
}

/// What a surface finish allows the process to do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProcess {
    /// Any process: the basic symbol.
    #[default]
    Any,
    /// Material must be removed: the symbol with a bar.
    Removal,
    /// Material must not be removed: the symbol with a circle.
    NoRemoval,
}

/// The direction of the lay on a surface finish symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lay {
    /// Parallel to the projection plane of the view: "=".
    Parallel,
    /// Perpendicular to it: "⊥".
    Perpendicular,
    /// Crossed in two directions: "X".
    Crossed,
    /// Many directions: "M".
    Multidirectional,
    /// Roughly circular about the surface's centre: "C".
    Circular,
    /// Roughly radial: "R".
    Radial,
    /// Particulate, non-directional: "P".
    Particulate,
}

impl Lay {
    /// The letter or sign written on the symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            Lay::Parallel => "=",
            Lay::Perpendicular => "⊥",
            Lay::Crossed => "X",
            Lay::Multidirectional => "M",
            Lay::Circular => "C",
            Lay::Radial => "R",
            Lay::Particulate => "P",
        }
    }
}

/// A geometric characteristic of ISO 1101 and ASME Y14.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GdtCharacteristic {
    /// Form: straightness.
    Straightness,
    /// Form: flatness.
    Flatness,
    /// Form: circularity (roundness).
    Circularity,
    /// Form: cylindricity.
    Cylindricity,
    /// Profile of a line.
    ProfileOfLine,
    /// Profile of a surface.
    ProfileOfSurface,
    /// Orientation: angularity.
    Angularity,
    /// Orientation: perpendicularity.
    Perpendicularity,
    /// Orientation: parallelism.
    Parallelism,
    /// Location: position.
    Position,
    /// Location: concentricity (coaxiality).
    Concentricity,
    /// Location: symmetry.
    Symmetry,
    /// Runout: circular runout.
    CircularRunout,
    /// Runout: total runout.
    TotalRunout,
}

impl GdtCharacteristic {
    /// Every characteristic, in the standards' order.
    pub const ALL: [GdtCharacteristic; 14] = [
        GdtCharacteristic::Straightness,
        GdtCharacteristic::Flatness,
        GdtCharacteristic::Circularity,
        GdtCharacteristic::Cylindricity,
        GdtCharacteristic::ProfileOfLine,
        GdtCharacteristic::ProfileOfSurface,
        GdtCharacteristic::Angularity,
        GdtCharacteristic::Perpendicularity,
        GdtCharacteristic::Parallelism,
        GdtCharacteristic::Position,
        GdtCharacteristic::Concentricity,
        GdtCharacteristic::Symmetry,
        GdtCharacteristic::CircularRunout,
        GdtCharacteristic::TotalRunout,
    ];

    /// The name in words, as in pickers.
    pub fn name(self) -> &'static str {
        match self {
            GdtCharacteristic::Straightness => "Straightness",
            GdtCharacteristic::Flatness => "Flatness",
            GdtCharacteristic::Circularity => "Circularity",
            GdtCharacteristic::Cylindricity => "Cylindricity",
            GdtCharacteristic::ProfileOfLine => "Profile of a line",
            GdtCharacteristic::ProfileOfSurface => "Profile of a surface",
            GdtCharacteristic::Angularity => "Angularity",
            GdtCharacteristic::Perpendicularity => "Perpendicularity",
            GdtCharacteristic::Parallelism => "Parallelism",
            GdtCharacteristic::Position => "Position",
            GdtCharacteristic::Concentricity => "Concentricity",
            GdtCharacteristic::Symmetry => "Symmetry",
            GdtCharacteristic::CircularRunout => "Circular runout",
            GdtCharacteristic::TotalRunout => "Total runout",
        }
    }

    /// The snake-case key, as in files and on the command line.
    pub fn key(self) -> &'static str {
        match self {
            GdtCharacteristic::Straightness => "straightness",
            GdtCharacteristic::Flatness => "flatness",
            GdtCharacteristic::Circularity => "circularity",
            GdtCharacteristic::Cylindricity => "cylindricity",
            GdtCharacteristic::ProfileOfLine => "profile_of_line",
            GdtCharacteristic::ProfileOfSurface => "profile_of_surface",
            GdtCharacteristic::Angularity => "angularity",
            GdtCharacteristic::Perpendicularity => "perpendicularity",
            GdtCharacteristic::Parallelism => "parallelism",
            GdtCharacteristic::Position => "position",
            GdtCharacteristic::Concentricity => "concentricity",
            GdtCharacteristic::Symmetry => "symmetry",
            GdtCharacteristic::CircularRunout => "circular_runout",
            GdtCharacteristic::TotalRunout => "total_runout",
        }
    }

    /// The characteristic with this key.
    pub fn from_key(key: &str) -> Option<GdtCharacteristic> {
        GdtCharacteristic::ALL.into_iter().find(|c| c.key() == key)
    }

    /// Form tolerances never take datums; orientation and runout tolerances always do.
    pub fn takes_datums(self) -> Option<bool> {
        match self {
            GdtCharacteristic::Straightness
            | GdtCharacteristic::Flatness
            | GdtCharacteristic::Circularity
            | GdtCharacteristic::Cylindricity => Some(false),
            GdtCharacteristic::Angularity
            | GdtCharacteristic::Perpendicularity
            | GdtCharacteristic::Parallelism
            | GdtCharacteristic::CircularRunout
            | GdtCharacteristic::TotalRunout => Some(true),
            _ => None,
        }
    }
}

/// The material condition a tolerance or datum reference applies at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialCondition {
    /// Regardless of feature size: no modifier.
    #[default]
    Regardless,
    /// Maximum material condition: Ⓜ.
    Maximum,
    /// Least material condition: Ⓛ.
    Least,
}

impl MaterialCondition {
    /// True for regardless of feature size, which writes nothing.
    pub fn is_regardless(&self) -> bool {
        *self == MaterialCondition::Regardless
    }

    /// The modifier letter drawn in a circle: "M", "L" or "".
    pub fn letter(self) -> &'static str {
        match self {
            MaterialCondition::Regardless => "",
            MaterialCondition::Maximum => "M",
            MaterialCondition::Least => "L",
        }
    }
}

/// One datum reference of a feature control frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatumRef {
    /// The datum letter, or a common datum such as "A-B".
    pub letter: String,
    /// The material condition it applies at.
    #[serde(default, skip_serializing_if = "MaterialCondition::is_regardless")]
    pub material: MaterialCondition,
}

impl DatumRef {
    /// A datum reference regardless of feature size.
    pub fn new(letter: &str) -> DatumRef {
        DatumRef { letter: letter.to_owned(), material: MaterialCondition::Regardless }
    }
}

/// Where an annotation attaches to a model entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    /// The entity.
    pub entity: EntityRef,
    /// Which point of the entity.
    pub attach: Attach,
    /// The attachment point in view millimetres at the last resolve.
    pub cached: Point2,
}

/// Which point of an entity an anchor attaches to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Attach {
    /// Start of an edge.
    Start,
    /// End of an edge.
    End,
    /// Middle of an edge, or the edge as a whole for a distance between parallel edges.
    Mid,
    /// Centre of a circle or arc.
    Centre,
    /// The near side of a circle or arc along the dimension direction.
    Min,
    /// The far side of a circle or arc along the dimension direction.
    Max,
    /// The silhouette of a curved face.
    Silhouette,
    /// A point on the entity.
    Point {
        /// The point in model space.
        at: Vec3,
    },
}

/// Style that differs from the template's. Every field left `None` uses the template.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StyleOverrides {
    /// Text height in sheet millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_height: Option<f64>,
    /// Arrow length in sheet millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrow_size: Option<f64>,
    /// Line width in sheet millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_width: Option<f64>,
}

impl StyleOverrides {
    /// True when nothing is overridden.
    pub fn is_empty(&self) -> bool {
        *self == StyleOverrides::default()
    }
}

/// Whether an annotation's references still resolve after the last update.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationState {
    /// Every reference resolved and nothing changed.
    #[default]
    Ok,
    /// Every reference resolved but the displayed value changed. Highlighted until the
    /// user accepts it.
    Changed {
        /// The displayed text before the update, such as "8".
        previous: String,
    },
    /// At least one reference no longer resolves. The annotation stays on the sheet with
    /// its last geometry.
    Dangling {
        /// Indices into `anchors` of the references that were lost.
        lost: Vec<usize>,
        /// The cause in words, for the Needs attention panel.
        cause: String,
    },
}

/// A dimension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    /// What it measures.
    pub kind: DimensionKind,
    /// The value.
    pub value: DimValue,
    /// The tolerance.
    #[serde(default)]
    pub tolerance: Tolerance,
    /// Decimal places shown.
    pub precision: u8,
    /// The displayed text. `<DIM>` stands for the formatted value.
    #[serde(default = "Dimension::default_text")]
    pub text: String,
    /// Whether a length is measured in the view plane or in 3D.
    #[serde(default)]
    pub dim_type: DimType,
}

impl Dimension {
    /// The text that shows only the value.
    pub const VALUE_TEXT: &'static str = "<DIM>";

    fn default_text() -> String {
        Dimension::VALUE_TEXT.to_owned()
    }

    /// A measured dimension with no tolerance, the default text and the given precision.
    pub fn measured(kind: DimensionKind, value: f64, precision: u8) -> Dimension {
        Dimension {
            kind,
            value: DimValue::Measured { value },
            tolerance: Tolerance::None,
            precision,
            text: Dimension::default_text(),
            dim_type: DimType::Projected,
        }
    }
}

/// What a dimension measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DimensionKind {
    /// A distance between two anchors, or the length of one edge.
    Linear {
        /// The direction the distance is measured in.
        orientation: LinearOrientation,
    },
    /// The diameter of a circle.
    Diameter,
    /// The radius of an arc.
    Radius,
    /// The angle between two edges.
    Angular,
    /// A distance from an origin anchor.
    Ordinate,
    /// A chamfer's size and angle.
    Chamfer,
}

/// The direction a linear dimension is measured in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinearOrientation {
    /// Along the sheet's X.
    Horizontal,
    /// Along the sheet's Y.
    Vertical,
    /// Along the line between the two anchors.
    Aligned,
}

/// The value of a dimension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DimValue {
    /// A reference dimension: measured from the model, never drives it.
    Measured {
        /// Millimetres or degrees.
        value: f64,
    },
    /// A dimension that drives a model parameter.
    Driving {
        /// The parameter.
        parameter: ParameterRef,
    },
}

/// A model parameter. A placeholder until the sketch parameters land.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterRef {
    /// Tag of the feature that owns the parameter.
    pub feature: String,
    /// The parameter's name.
    pub name: String,
}

/// The tolerance of a dimension, in millimetres or degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tolerance {
    /// No tolerance shown.
    #[default]
    None,
    /// Plus or minus one value.
    Symmetric {
        /// The deviation, greater than zero.
        value: f64,
    },
    /// Separate upper and lower deviations.
    Deviation {
        /// Upper deviation.
        upper: f64,
        /// Lower deviation, negative below the nominal value.
        lower: f64,
    },
    /// The deviations shown as upper and lower limits.
    Limits {
        /// Upper deviation.
        upper: f64,
        /// Lower deviation, negative below the nominal value.
        lower: f64,
    },
}

/// Whether a length is measured as projected in the view or in 3D.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimType {
    /// Measured in the view plane. The default for orthographic views.
    #[default]
    Projected,
    /// The true 3D length.
    True,
}
