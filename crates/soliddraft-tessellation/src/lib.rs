//! SolidDraft display quality: how finely bodies are tessellated for the screen, and the
//! cache that lets a million placements share a definition's meshes.
//!
//! - **The policy** ([`policy`]): the automatic chord height relative to a body's size,
//!   15 degree angles, and three levels per definition at 4, 1 and 1/4 of the standard
//!   chord height ([`level`]).
//! - **The setting** ([`quality`]): automatic, a slider position or advanced raw values,
//!   saved in each document; an assembly component can override its own.
//! - **The choice** ([`select`]): each occurrence moves to a finer level when its error
//!   covers more than about a pixel and back after it has stayed under about a tenth of
//!   one, never past the ceiling the device budget sets ([`budget`]).
//! - **The cache** ([`cache`]): meshes by definition and exact parameters, levels that
//!   come out the same shared, trimmed to the app's budget.
//! - **The worker** ([`service`]): tessellation off the interface thread, one kernel
//!   call per body for all its levels, with superseded requests dropped.
//! - **The settings** ([`settings`]): the document's `settings.json`, and the four
//!   tolerances (viewport, drawing projection, export, modelling) kept apart.
//! - **The report** ([`report`]): each level's deviation in millimetres and triangle
//!   count, for the quality slider and the command line.
//! - **Display meshes** ([`mesh`]): meshes with faces named by persistent references,
//!   and their binary form, which part files and assembly copies both keep ([`binary`]).
//!
//! See `docs/tessellation.md`.

pub mod binary;
pub mod budget;
pub mod cache;
pub mod level;
pub mod mesh;
pub mod policy;
pub mod quality;
pub mod report;
pub mod select;
pub mod service;
pub mod settings;

pub use budget::{DeviceBudget, Usage};
pub use cache::{DisplayCache, Weigh};
pub use level::Level;
pub use mesh::{DisplayMesh, MeshFace, SharedMesh};
pub use policy::{Levels, ParamsKey};
pub use quality::{Advanced, Quality};
pub use report::{LevelReport, Report, report};
pub use select::{Choice, Projection, Thresholds};
pub use service::{Done, Tessellator};
pub use settings::{DrawingTolerance, ExportTolerance, QualityOverride, Settings};
