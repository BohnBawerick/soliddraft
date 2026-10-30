//! Terminal field inputs that vary over the part, and the explicit outputs a field has
//! besides its labelled mesh: certified moments of the field and a mass stand-in.

use serde::{Deserialize, Serialize};

/// A part axis a linear grade runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeAxis {
    /// The part's x axis.
    X,
    /// The part's y axis.
    Y,
    /// The part's z axis.
    Z,
}

impl GradeAxis {
    /// Every axis, in the order the editor offers them.
    pub const ALL: [GradeAxis; 3] = [GradeAxis::X, GradeAxis::Y, GradeAxis::Z];

    /// The axis's index in a coordinate triple.
    pub fn index(self) -> usize {
        match self {
            GradeAxis::X => 0,
            GradeAxis::Y => 1,
            GradeAxis::Z => 2,
        }
    }

    /// The lowercase name files and tools use.
    pub fn name(self) -> &'static str {
        match self {
            GradeAxis::X => "x",
            GradeAxis::Y => "y",
            GradeAxis::Z => "z",
        }
    }
}

/// A value graded linearly along one part axis: `start` at part coordinate `from` on
/// `axis`, `end` at `to`, linear in between and clamped beyond. The anchors are part
/// coordinates, so an upstream size edit does not move them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearGrade {
    /// The part axis the value changes along.
    pub axis: GradeAxis,
    /// Coordinate on `axis` where the value is `start`.
    pub from: f64,
    /// Coordinate on `axis` where the value is `end`.
    pub to: f64,
    /// The value at `from` and before it.
    pub start: f64,
    /// The value at `to` and beyond it.
    pub end: f64,
}

impl LinearGrade {
    /// The value at a part position, as the kernel defines it.
    pub fn at(&self, position: [f64; 3]) -> f64 {
        let t = ((position[self.axis.index()] - self.from) / (self.to - self.from)).clamp(0.0, 1.0);
        self.start + (self.end - self.start) * t
    }

    /// The anchor points in part coordinates: `from` and `to` on the axis, zero elsewhere.
    pub fn anchors(&self) -> [[f64; 3]; 2] {
        let mut from = [0.0; 3];
        let mut to = [0.0; 3];
        from[self.axis.index()] = self.from;
        to[self.axis.index()] = self.to;
        [from, to]
    }

    /// The change of value per millimetre along the axis.
    pub fn slope(&self) -> f64 {
        ((self.end - self.start) / (self.to - self.from)).abs()
    }
}

/// How a terminal field parameter varies over the part. A constant is written as a bare
/// number, as files always had it; a function is an object that names it, so a reader
/// that does not know a function refuses the file instead of reading it as a constant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "GradeFile", into = "GradeFile")]
pub enum Grade {
    /// The same value everywhere.
    Constant(f64),
    /// Linear along one part axis, clamped beyond its anchors.
    Linear(LinearGrade),
}

impl Grade {
    /// Every value the grade is built from, for finiteness checks.
    pub fn values(&self) -> Vec<f64> {
        match self {
            Grade::Constant(v) => vec![*v],
            Grade::Linear(l) => vec![l.from, l.to, l.start, l.end],
        }
    }

    /// The values the grade takes: the constant, or both endpoints.
    pub fn endpoints(&self) -> Vec<f64> {
        match self {
            Grade::Constant(v) => vec![*v],
            Grade::Linear(l) => vec![l.start, l.end],
        }
    }

    /// The constant, when the grade is one.
    pub fn constant(&self) -> Option<f64> {
        match self {
            Grade::Constant(v) => Some(*v),
            Grade::Linear(_) => None,
        }
    }
}

impl From<f64> for Grade {
    fn from(value: f64) -> Self {
        Grade::Constant(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum GradeFile {
    Constant(f64),
    Function(GradeFunction),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GradeFunction {
    Linear(LinearGrade),
}

impl From<GradeFile> for Grade {
    fn from(file: GradeFile) -> Self {
        match file {
            GradeFile::Constant(v) => Grade::Constant(v),
            GradeFile::Function(GradeFunction::Linear(l)) => Grade::Linear(l),
        }
    }
}

impl From<Grade> for GradeFile {
    fn from(grade: Grade) -> Self {
        match grade {
            Grade::Constant(v) => GradeFile::Constant(v),
            Grade::Linear(l) => GradeFile::Function(GradeFunction::Linear(l)),
        }
    }
}

/// The largest widths a field's moment enclosures may have. Each is absolute, with no
/// relative allowance. The defaults are the widths the graded-field design requires.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldMassAccuracy {
    /// Volume width in cubic millimetres.
    pub volume_mm3: f64,
    /// Width of each centroid coordinate in millimetres.
    pub centroid_mm: f64,
    /// Width of each centroidal inertia component in millimetres to the fifth power.
    pub inertia_mm5: f64,
}

impl Default for FieldMassAccuracy {
    /// 0.01 mm³, 0.001 mm and 0.1 mm⁵.
    fn default() -> Self {
        FieldMassAccuracy { volume_mm3: 0.01, centroid_mm: 0.001, inertia_mm5: 0.1 }
    }
}

/// A proven enclosure of an exact quantity, as exact rationals such as `"-7/25"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactRange {
    /// Lower end.
    pub lo: String,
    /// Upper end.
    pub hi: String,
}

/// Unit-density moments of a field's own definition, proven by interval integration of
/// the field. No mesh, preview or display region contributes to them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldMass {
    /// Always `original-field-definition`.
    pub scope: String,
    /// The requested widths, as exact rationals: volume, centroid and inertia.
    pub requested_widths: [String; 3],
    /// Volume in cubic millimetres.
    pub volume_mm3: ExactRange,
    /// Centre of gravity in millimetres; absent exactly for a zero volume.
    pub centroid_mm: Option<[ExactRange; 3]>,
    /// Inertia about the centre of gravity along the part axes, at unit density, in
    /// millimetres to the fifth power; absent exactly for a zero volume.
    pub inertia_mm5: Option<[[ExactRange; 3]; 3]>,
    /// Classified boxes and analytic intervals the integration used.
    pub cells: usize,
    /// Deterministic work units the request used.
    pub work: u64,
}

/// The explicit mass stand-in of a field: a STEP file with the field's source envelope
/// as a massless surface model and unit-density proxy solids whose moments lie inside
/// the field's enclosures.
#[derive(Clone, Debug)]
pub struct StandInExport {
    /// The STEP AP242 file.
    pub bytes: Vec<u8>,
    /// What it holds and the proof that its mass bodies match the field.
    pub report: StandInReport,
}

/// Moments of the proxy solids, as exact rationals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProxyMoments {
    /// Volume in cubic millimetres.
    pub volume_mm3: String,
    /// Centre of gravity in millimetres.
    pub centroid_mm: [String; 3],
    /// Centroidal inertia in millimetres to the fifth power.
    pub inertia_mm5: [[String; 3]; 3],
}

/// Bounds of the proxy's moments after the file's coordinate rounding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExchangedMoments {
    /// Volume in cubic millimetres.
    pub volume_mm3: ExactRange,
    /// Centre of gravity in millimetres.
    pub centroid_mm: [ExactRange; 3],
    /// Centroidal inertia in millimetres to the fifth power.
    pub inertia_mm5: [[ExactRange; 3]; 3],
}

/// What a mass stand-in wrote and why its mass bodies stand for the field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StandInReport {
    /// Always `mass stand-in — not field boundary geometry`.
    pub label: String,
    /// Always `massless-surface-envelope-plus-unit-density-solids`.
    pub representation: String,
    /// The field's own moments the proxy was matched to.
    pub field: FieldMass,
    /// The proxy solids' exact moments.
    pub proxy: ProxyMoments,
    /// The proxy's moments once written, including the file's rounding; each lies inside
    /// the field's enclosure.
    pub exchanged: ExchangedMoments,
    /// The proxy's exact inertia residual against its target, in millimetres to the fifth power.
    pub inertia_residual: String,
    /// Unit-density proxy solids written.
    pub mass_bodies: usize,
    /// Massless surface envelopes written: the field's source region.
    pub surface_envelopes: usize,
    /// Proven bound, in millimetres, of the file's coordinate rounding.
    pub max_deviation_mm: String,
    /// The file states no density or material.
    pub density_metadata: bool,
    /// Deterministic work units the request used, the field moments included.
    pub work: u64,
    /// SHA-256 of the file.
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_stay_bare_numbers_and_functions_name_themselves() {
        assert_eq!(serde_json::to_string(&Grade::Constant(1.5)).unwrap(), "1.5");
        let linear = Grade::Linear(LinearGrade { axis: GradeAxis::Z, from: 20.0, to: 30.0, start: 1.0, end: 2.0 });
        let text = serde_json::to_string(&linear).unwrap();
        assert_eq!(text, r#"{"linear":{"axis":"z","from":20.0,"to":30.0,"start":1.0,"end":2.0}}"#);
        assert_eq!(serde_json::from_str::<Grade>(&text).unwrap(), linear);
        assert_eq!(serde_json::from_str::<Grade>("-0.1").unwrap(), Grade::Constant(-0.1));
    }

    #[test]
    fn unknown_grade_functions_and_keys_are_refused() {
        for text in [
            r#"{"radial":{"axis":"z","from":0,"to":1,"start":1,"end":2}}"#,
            r#"{"linear":{"axis":"z","from":0,"to":1,"start":1,"end":2,"span":3}}"#,
            r#"{"linear":{"axis":"w","from":0,"to":1,"start":1,"end":2}}"#,
            r#"{"linear":{"axis":"z","from":0,"to":1,"start":1}}"#,
            r#""sin(z)""#,
            "null",
        ] {
            assert!(serde_json::from_str::<Grade>(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_linear_grade_clamps_beyond_its_anchors() {
        let grade = LinearGrade { axis: GradeAxis::Z, from: 20.0, to: 30.0, start: 1.0, end: 2.0 };
        assert_eq!(grade.at([0.0, 0.0, 22.0]), 1.2);
        assert_eq!(grade.at([5.0, -5.0, 10.0]), 1.0);
        assert_eq!(grade.at([0.0, 0.0, 31.0]), 2.0);
        assert_eq!(grade.anchors(), [[0.0, 0.0, 20.0], [0.0, 0.0, 30.0]]);
        assert_eq!(grade.slope(), 0.1);
        let x = LinearGrade { axis: GradeAxis::X, ..grade };
        assert_eq!(x.anchors(), [[20.0, 0.0, 0.0], [30.0, 0.0, 0.0]]);
    }
}
