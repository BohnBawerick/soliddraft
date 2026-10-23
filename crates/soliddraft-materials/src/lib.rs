//! SolidDraft materials and appearances.
//!
//! What a part is made of and how its surfaces look are two things, as in SolidWorks,
//! Inventor and Fusion: a painted steel part stays steel.
//!
//! - A [`Material`] is physical: density, elastic and strength values, thermal values,
//!   in SI units with the unit in each field's name. Every value is optional, and an
//!   unknown value stays unknown ([`material`]).
//! - An [`Appearance`] is visual: glTF's metallic-roughness model with a base colour,
//!   metallic and roughness factors, opacity and emission ([`appearance`]). A material
//!   suggests one as its default.
//! - Colours are sRGB with a declared conversion to linear RGB ([`colour`]).
//! - Libraries are TOML files of materials and appearances ([`library`]). The
//!   [`Catalog`] holds SolidDraft's built-in libraries (about 40 materials from named
//!   public sources, and a set of finishes), the user's own library and the libraries
//!   File locations lists ([`catalog`]).
//! - A document keeps a copy of every material and appearance it uses, so a missing
//!   or edited library never changes its look or mass ([`snapshot`]).
//! - Assignments carry a configuration scope ([`scope`]).
//! - Mass comes from the kernel's unit-density properties and the material's density;
//!   with no density it is unknown ([`mass`]).
//! - Each value of a material can record its source (a library, a user override, an
//!   imported `.sldmat`, a standard reference) and the conditions it holds under, such
//!   as a temperature, form or treatment. A value that records none has an unknown
//!   source ([`provenance`]).
//! - SolidWorks `.sldmat` libraries import as SolidDraft libraries ([`sldmat`]).
//!
//! The schema and the rules are in `docs/materials.md`.

pub mod appearance;
pub mod catalog;
pub mod colour;
pub mod library;
pub mod mass;
pub mod material;
pub mod problem;
pub mod provenance;
pub mod scope;
pub mod sldmat;
pub mod snapshot;

pub use appearance::{Appearance, AppearanceId, Shading};
pub use catalog::{Catalog, CatalogIssue, LibraryMatch};
pub use colour::{ParseColourError, Srgb};
pub use library::{Library, LibraryError, Origin};
pub use mass::{Mass, Placed, Unknown};
pub use material::{Material, MaterialId};
pub use problem::Problem;
pub use provenance::{Conditions, Property, PropertySource, Provenance};
pub use snapshot::Snapshots;
