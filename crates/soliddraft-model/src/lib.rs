//! The SolidDraft part document.
//!
//! A part is an ordered list of [`Feature`]s: sketches, extrusions that add or cut
//! material, fillets and chamfers. The document stores only their definitions and
//! rebuilds them through the app-owned [`Kernel`](soliddraft_kernel_api::Kernel) trait,
//! so it works on any backend.
//!
//! - **Commands.** Every change is a [`Command`] run by [`Document::execute`], one undo
//!   entry each; [`Document::undo`] and [`Document::redo`] walk the stack. The ribbon,
//!   the assistant and scripts all go through the same commands.
//! - **Rebuild.** A command rebuilds only from the first feature it changed, and every
//!   feature build is kept by its exact inputs (see [`rebuild`]), so a rename builds
//!   nothing and undo, redo or a value put back reuse the earlier bodies. Suppressed
//!   features and features below the rollback bar pass the body through unchanged.
//! - **Persistent references.** Features name faces and edges by origin, never by kernel
//!   id (see [`reference`]). A reference that no longer resolves to exactly one matching
//!   entity stops its feature with [`FeatureStatus::RepairNeeded`] instead of guessing.
//! - **Materials and appearances.** A part's material and the appearances of the part,
//!   its features' faces and single faces, each scoped to configurations, with copies
//!   of what they use (see [`materials`]). Mass comes from the material's density.
//! - **Files.** [`Document::save`] and [`Document::open`] read and write the versioned
//!   native format (see [`file`]); a saved file round-trips exactly.
//!
//! Units are millimetres and degrees.

mod command;
pub mod display;
mod document;
mod exact;
mod feature;
pub mod file;
pub mod materials;
pub mod rebuild;
pub mod reference;

pub use command::{Command, CommandError, Executed};
pub use document::{BuiltSketch, Document, FACE_OVERLAP, FaceLabel, FeatureStatus, feature_of_tag, plane_on_face};
pub use feature::{
    BlendFeature, BuiltinShape, DatumPlane, DraftFeature, EndCondition, ExtrudeFeature, ExtrudeOp, Feature, FeatureId,
    FeatureKind, HoleFeature, ImportStepFeature, MirrorFeature, PatternFeature, PatternSpec, PlaneRef, ProfileSource,
    RevolveFeature, ShellFeature, SketchFeature, SketchPlane, sha256_hex,
};
pub use file::FileError;
pub use materials::{
    AppearanceAssignment, AppearanceTable, AppearanceTarget, LostAppearance, MassError, MaterialAssignment,
    PartMaterials,
};
pub use reference::{
    Candidate, EdgeFingerprint, EdgeRef, FaceFingerprint, FaceRef, Generator, NamedFace, NamingError, ReferenceSlot,
    RepairNeeded, RepairReason,
};

/// The sketch types features are built from.
pub use soliddraft_sketch as sketch;
