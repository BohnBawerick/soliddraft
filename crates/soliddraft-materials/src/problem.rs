//! Problems found when checking a material or an appearance.

use std::fmt;

/// One thing wrong with a material or an appearance: which field, and what is wrong
/// with it, in words a properties panel can show beside the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The field's name as written in files: "density_kg_m3".
    pub field: &'static str,
    /// What is wrong: "is 0; it must be above 0".
    pub message: String,
}

impl Problem {
    /// A problem with `field`.
    pub fn new(field: &'static str, message: impl Into<String>) -> Problem {
        Problem { field, message: message.into() }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.field, self.message)
    }
}
