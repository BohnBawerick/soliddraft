//! Stable ids inside an assembly, and occurrence paths.
//!
//! Every component, pattern and source of an assembly has an id from one counter
//! (`next_id`), never reused, so a rename or a delete elsewhere never moves it. A part
//! placed deep in an assembly is reached by an [`OccurrencePath`]: the steps from the
//! root, each a component of the assembly at that level, or a member a pattern
//! generated from it. Names are labels for the tree; paths never use them, so renaming a
//! component breaks no drawing anchor or mate.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! local_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

local_id!(
    /// A component of an assembly, stable within it.
    ComponentId
);
local_id!(
    /// A pattern of an assembly, stable within it.
    PatternId
);
local_id!(
    /// A referenced document (part or sub-assembly, with its configuration) of an
    /// assembly, stable within it. Components name their source by it, so a thousand
    /// placements of one bolt share one reference.
    SourceId
);

/// One step of an occurrence path: a component of the assembly at that level, or a
/// member a pattern generated from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Step {
    /// The component, or the pattern seed the member was generated from.
    pub component: ComponentId,
    /// For a generated member: the pattern and the member's index in it (1 or more;
    /// member 0 is the seed itself).
    pub member: Option<(PatternId, u32)>,
}

impl Step {
    /// The component itself.
    pub fn component(id: ComponentId) -> Step {
        Step { component: id, member: None }
    }

    /// Member `index` of `pattern`, generated from the seed `seed`.
    pub fn member(seed: ComponentId, pattern: PatternId, index: u32) -> Step {
        Step { component: seed, member: Some((pattern, index)) }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.member {
            None => write!(f, "{}", self.component),
            Some((p, i)) => write!(f, "{}:{}.{}", self.component, p, i),
        }
    }
}

/// Why a path's text did not parse.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("not an occurrence path: {0:?} (expected steps such as \"4/12\" or \"4/12:9.3\")")]
pub struct PathError(pub String);

impl FromStr for Step {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Step, PathError> {
        let bad = || PathError(s.to_owned());
        let num = |t: &str| t.parse::<u32>().map_err(|_| bad());
        match s.split_once(':') {
            None => Ok(Step::component(ComponentId(num(s)?))),
            Some((c, rest)) => {
                let (p, i) = rest.split_once('.').ok_or_else(bad)?;
                let index = num(i)?;
                if index == 0 {
                    return Err(bad());
                }
                Ok(Step::member(ComponentId(num(c)?), PatternId(num(p)?), index))
            }
        }
    }
}

/// The steps from the root assembly to one occurrence. The root itself is the empty
/// path. Written as text: steps joined by "/", such as "4/12:9.3" (component 12 of the
/// assembly placed as component 4, member 3 of pattern 9).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrencePath(pub Vec<Step>);

impl OccurrencePath {
    /// The root.
    pub fn root() -> OccurrencePath {
        OccurrencePath(Vec::new())
    }

    /// This path with `step` after it.
    pub fn child(&self, step: Step) -> OccurrencePath {
        let mut steps = self.0.clone();
        steps.push(step);
        OccurrencePath(steps)
    }

    /// The path of the containing occurrence; `None` for the root.
    pub fn parent(&self) -> Option<OccurrencePath> {
        let (_, rest) = self.0.split_last()?;
        Some(OccurrencePath(rest.to_vec()))
    }

    /// True for the root.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The steps.
    pub fn steps(&self) -> &[Step] {
        &self.0
    }

    /// True when `self` is `other` or inside it.
    pub fn starts_with(&self, other: &OccurrencePath) -> bool {
        self.0.starts_with(&other.0)
    }
}

impl From<Vec<Step>> for OccurrencePath {
    fn from(steps: Vec<Step>) -> OccurrencePath {
        OccurrencePath(steps)
    }
}

impl fmt::Display for OccurrencePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            write!(f, "{s}")?;
        }
        Ok(())
    }
}

impl FromStr for OccurrencePath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<OccurrencePath, PathError> {
        let s = s.trim().trim_matches('/');
        if s.is_empty() {
            return Ok(OccurrencePath::root());
        }
        s.split('/').map(str::parse).collect::<Result<Vec<_>, _>>().map(OccurrencePath)
    }
}

impl Serialize for OccurrencePath {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for OccurrencePath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<OccurrencePath, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_read_and_write_as_text() {
        let p: OccurrencePath = "4/12:9.3".parse().unwrap();
        assert_eq!(p.steps(), [Step::component(ComponentId(4)), Step::member(ComponentId(12), PatternId(9), 3)]);
        assert_eq!(p.to_string(), "4/12:9.3");
        assert_eq!(serde_json::to_string(&p).unwrap(), r#""4/12:9.3""#);
        assert_eq!(serde_json::from_str::<OccurrencePath>(r#""/4/12:9.3/""#).unwrap(), p);
        assert_eq!("".parse::<OccurrencePath>().unwrap(), OccurrencePath::root());
        assert_eq!(p.parent().unwrap().to_string(), "4");
        assert!(p.starts_with(&p.parent().unwrap()) && !p.parent().unwrap().starts_with(&p));
        for bad in ["a", "4/x", "4:9", "4:9.0", "4:9.x", "4//5"] {
            assert!(bad.parse::<OccurrencePath>().is_err(), "{bad}");
        }
    }
}
