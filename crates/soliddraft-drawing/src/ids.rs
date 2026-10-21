//! Identifiers of the things a drawing holds.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A view's id, unique within the drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewId(pub u32);

/// The view id annotations placed on the sheet itself use: their points are sheet
/// millimetres.
pub const SHEET: ViewId = ViewId(0);

/// An annotation's id, unique within the drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AnnotationId(pub u32);

/// A template's id, such as `"iso-a3"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TemplateId(pub String);

impl TemplateId {
    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TemplateId {
    fn from(id: &str) -> TemplateId {
        TemplateId(id.to_owned())
    }
}

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "view {}", self.0)
    }
}

impl fmt::Display for AnnotationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "annotation {}", self.0)
    }
}

impl fmt::Display for TemplateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
