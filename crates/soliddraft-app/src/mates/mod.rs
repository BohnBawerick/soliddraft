//! Mates in the window and over MCP. What a mate means, how it solves and how it commits
//! are `soliddraft-assembly`'s (see `docs/mates.md`); this module holds the commands, the
//! Mate panel and the rows `model_summary` reports.

pub(crate) mod panel;
pub(crate) mod registry;
pub(crate) mod tools;

pub use panel::{Draft, MateAction, PickedFace, apply};
