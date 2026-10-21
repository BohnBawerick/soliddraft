//! The three quantised levels every definition is tessellated at.

use serde::{Deserialize, Serialize};

/// One of a definition's three meshes. Each level's chord height is a fixed multiple of
/// the standard one, so the levels of every definition line up and an occurrence's
/// choice is one byte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Four times the standard chord height: far away, or a lightweight open.
    Coarse,
    /// The document's quality setting.
    #[default]
    Standard,
    /// A quarter of the standard chord height: close up.
    Fine,
}

impl Level {
    /// Every level, coarsest first.
    pub const ALL: [Level; 3] = [Level::Coarse, Level::Standard, Level::Fine];

    /// The level's place in [`Level::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The chord height of this level as a multiple of the standard level's.
    pub fn chord_factor(self) -> f64 {
        match self {
            Level::Coarse => 4.0,
            Level::Standard => 1.0,
            Level::Fine => 0.25,
        }
    }

    /// The next finer level.
    pub fn finer(self) -> Option<Level> {
        match self {
            Level::Coarse => Some(Level::Standard),
            Level::Standard => Some(Level::Fine),
            Level::Fine => None,
        }
    }

    /// The next coarser level.
    pub fn coarser(self) -> Option<Level> {
        match self {
            Level::Coarse => None,
            Level::Standard => Some(Level::Coarse),
            Level::Fine => Some(Level::Standard),
        }
    }

    /// The level as one word: "coarse", "standard" or "fine".
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Coarse => "coarse",
            Level::Standard => "standard",
            Level::Fine => "fine",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> Result<Level, String> {
        Level::ALL
            .into_iter()
            .find(|l| l.as_str() == s)
            .ok_or_else(|| format!("no level {s:?}: coarse, standard or fine"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_step_by_four() {
        for w in Level::ALL.windows(2) {
            assert_eq!(w[0].finer(), Some(w[1]));
            assert_eq!(w[1].coarser(), Some(w[0]));
            assert_eq!(w[0].chord_factor() / w[1].chord_factor(), 4.0);
        }
        assert_eq!(Level::Fine.finer(), None);
        assert_eq!(Level::Coarse.coarser(), None);
        for l in Level::ALL {
            assert_eq!(l.as_str().parse::<Level>(), Ok(l));
            assert_eq!(Level::ALL[l.index()], l);
        }
        assert!("medium".parse::<Level>().is_err());
    }
}
