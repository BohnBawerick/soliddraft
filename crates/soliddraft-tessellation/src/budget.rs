//! The device budget: how much display data the app lets itself hold.
//!
//! The app owns the budget, not the document: a file opens the same on every machine,
//! and a smaller device shows it at coarser levels. The display cache counts what it
//! holds against the budget and sets the ceiling the level choice obeys.

use crate::level::Level;

/// Display data held, counted once per distinct mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Triangles.
    pub triangles: u64,
    /// Bytes of vertex and index data.
    pub bytes: u64,
}

/// The most display data the app holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceBudget {
    /// Triangles.
    pub triangles: u64,
    /// Bytes of vertex and index data.
    pub bytes: u64,
}

/// Above this share of the budget, nothing is promoted to the fine level.
const FINE_SHARE: f64 = 0.8;

impl DeviceBudget {
    /// No limit.
    pub const UNLIMITED: DeviceBudget = DeviceBudget { triangles: u64::MAX, bytes: u64::MAX };

    /// A desktop with a discrete or recent integrated GPU: 40 million triangles in
    /// 2 GiB.
    pub const DESKTOP: DeviceBudget = DeviceBudget { triangles: 40_000_000, bytes: 2 << 30 };

    /// A browser tab, whose memory and WebGL2 buffers are tighter: 8 million triangles
    /// in 512 MiB.
    pub const BROWSER: DeviceBudget = DeviceBudget { triangles: 8_000_000, bytes: 512 << 20 };

    /// The finest level the level choice may promote to with `usage` held: fine below
    /// 80 percent of the budget, standard below all of it, coarse beyond.
    pub fn ceiling(&self, usage: Usage) -> Level {
        let share = self.share(usage);
        if share < FINE_SHARE {
            Level::Fine
        } else if share < 1.0 {
            Level::Standard
        } else {
            Level::Coarse
        }
    }

    /// True when `usage` is within the budget.
    pub fn fits(&self, usage: Usage) -> bool {
        usage.triangles <= self.triangles && usage.bytes <= self.bytes
    }

    /// The larger of the triangle and byte shares `usage` takes of the budget.
    pub fn share(&self, usage: Usage) -> f64 {
        let part = |used: u64, limit: u64| if limit == 0 { f64::INFINITY } else { used as f64 / limit as f64 };
        part(usage.triangles, self.triangles).max(part(usage.bytes, self.bytes))
    }
}

impl Default for DeviceBudget {
    fn default() -> DeviceBudget {
        if cfg!(target_arch = "wasm32") { DeviceBudget::BROWSER } else { DeviceBudget::DESKTOP }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_drops_as_the_budget_fills() {
        let b = DeviceBudget { triangles: 1000, bytes: 1 << 20 };
        assert_eq!(b.ceiling(Usage::default()), Level::Fine);
        assert_eq!(b.ceiling(Usage { triangles: 799, bytes: 0 }), Level::Fine);
        assert_eq!(b.ceiling(Usage { triangles: 800, bytes: 0 }), Level::Standard);
        assert_eq!(b.ceiling(Usage { triangles: 0, bytes: 1 << 20 }), Level::Coarse, "bytes count too");
        assert!(b.fits(Usage { triangles: 1000, bytes: 1 << 20 }));
        assert!(!b.fits(Usage { triangles: 1001, bytes: 0 }));
        assert_eq!(DeviceBudget::UNLIMITED.ceiling(Usage { triangles: u64::MAX / 2, bytes: 0 }), Level::Fine);
        assert_eq!(DeviceBudget { triangles: 0, bytes: 0 }.ceiling(Usage::default()), Level::Coarse);
    }
}
