//! SolidDraft recipes: a model as an ordered list of kernel commands with named
//! references, replayable on any backend of the kernel trait.
//!
//! Recipes are the conformance corpus: replaying one reports face and edge counts,
//! volume, area, validity and a geometric digest, so two runs, or two backends, can be
//! compared by their reports.

pub mod digest;
mod format;
mod replay;

pub use format::{Command, FORMAT, NamedPlane, Outcome, PlaneSpec, Recipe, Selector, Shape, Step};
pub use replay::{
    BodySummary, ProfileSummary, RecipeError, ReplayReport, StepOutcome, StepReport, build, check, parse, replay,
    summarize,
};
