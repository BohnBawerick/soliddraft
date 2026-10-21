//! Opaque handles and entity ids.

use serde::{Deserialize, Serialize};

macro_rules! opaque_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// Wraps a backend's raw value. Only backends create ids; callers treat them
            /// as opaque and never compare ids from different kernel sessions.
            pub const fn from_raw(raw: u64) -> Self {
                $name(raw)
            }

            /// The backend's raw value.
            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

opaque_id!(
    /// A body owned by a kernel backend. Operations never change a body: they return a
    /// new one, so a handle stays valid, with the same geometry, until it is released.
    BodyId,
    "body#"
);

opaque_id!(
    /// A face of a body. The id follows the face through later operations for as long as
    /// the history reports it as unchanged or modified.
    FaceId,
    "face#"
);

opaque_id!(
    /// An edge of a body. The id follows the edge through later operations for as long
    /// as the history reports it as unchanged or modified.
    EdgeId,
    "edge#"
);

opaque_id!(
    /// A vertex of a body. The id follows the vertex through later operations for as long
    /// as the history reports it as unchanged or modified.
    VertexId,
    "vertex#"
);

/// The caller's name for one operation, such as a feature id ("Fillet1") or a recipe
/// step id. History records are keyed by it. The kernel does not interpret it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpTag(pub String);

impl OpTag {
    /// Builds a tag.
    pub fn new(tag: impl Into<String>) -> Self {
        OpTag(tag.into())
    }

    /// The tag text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OpTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for OpTag {
    fn from(s: &str) -> Self {
        OpTag::new(s)
    }
}
