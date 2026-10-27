//! The side area: the feature tree on the left. The assistant has no side area; it
//! opens as a session above the command line at the bottom (see `command_line.rs`).
//!
//! The side area can be hidden and resized. What the user chose is kept in the app's
//! settings file (see `settings.rs`) and restored at the next start.

use serde::{Deserialize, Serialize};

/// The side areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// The feature tree with the property panel under it, left of the viewport.
    Tree,
}

impl Side {
    pub const ALL: [Side; 1] = [Side::Tree];

    pub fn title(self) -> &'static str {
        match self {
            Side::Tree => "Feature tree",
        }
    }

    /// The title as it reads inside a sentence.
    pub fn name(self) -> &'static str {
        match self {
            Side::Tree => "feature tree",
        }
    }

    /// The command that shows or hides the side area.
    pub fn command(self) -> &'static str {
        match self {
            Side::Tree => "toggle_tree",
        }
    }

    /// The show-or-hide shortcut, from the command registry. F9 hides the FeatureManager
    /// in SolidWorks.
    pub fn shortcut(self) -> String {
        crate::commands::get(self.command()).and_then(|c| c.shortcut_text()).unwrap_or_default()
    }

    pub fn min_width(self) -> f32 {
        match self {
            Side::Tree => 200.0,
        }
    }

    pub fn default_width(self) -> f32 {
        match self {
            Side::Tree => 380.0,
        }
    }

    /// The egui id of the panel while it is shown; its width lives under this id.
    pub fn panel_id(self) -> egui::Id {
        egui::Id::new(("side_panel", self))
    }

    /// The egui id of the strip left at the edge while the panel is hidden.
    pub fn strip_id(self) -> egui::Id {
        egui::Id::new(("side_strip", self))
    }
}

/// The widest a side area gets, whatever the window size.
pub const MAX_WIDTH: f32 = 800.0;
/// The viewport keeps at least this much beside the side area.
pub const VIEWPORT_MIN_WIDTH: f32 = 320.0;
/// The strip a hidden area leaves at its edge: a chevron and the title set on end.
pub const STRIP_WIDTH: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidePanel {
    pub shown: bool,
    /// The outer width in logical pixels while shown; kept while hidden.
    pub width: f32,
}

/// Visibility and width of the side area. A file from a build that still had the
/// assistant's side area keeps its `assistant` key, which this build ignores.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub tree: SidePanel,
}

impl Default for Layout {
    fn default() -> Self {
        Self { tree: SidePanel { shown: true, width: Side::Tree.default_width() } }
    }
}

impl Layout {
    pub fn panel(&self, side: Side) -> &SidePanel {
        match side {
            Side::Tree => &self.tree,
        }
    }

    pub fn panel_mut(&mut self, side: Side) -> &mut SidePanel {
        match side {
            Side::Tree => &mut self.tree,
        }
    }

    pub fn toggle(&mut self, side: Side) {
        let p = self.panel_mut(side);
        p.shown = !p.shown;
    }

    /// Brings a width that is not a number back to the default and the rest
    /// into range, so a hand-edited or stale file cannot hide a panel off-screen.
    pub fn clamped(mut self) -> Self {
        for side in Side::ALL {
            let p = self.panel_mut(side);
            if !p.width.is_finite() {
                p.width = side.default_width();
            }
            p.width = p.width.clamp(side.min_width(), MAX_WIDTH);
        }
        self
    }

    /// The widest this side may be in a window this wide: what leaves the viewport its
    /// minimum, capped at [`MAX_WIDTH`] and never below the side's own minimum.
    pub fn max_width(&self, side: Side, window_width: f32) -> f32 {
        (window_width - VIEWPORT_MIN_WIDTH).min(MAX_WIDTH).max(side.min_width())
    }

    /// The saved layout, or the default when there is none or it cannot be read.
    pub fn load() -> Self {
        crate::settings::layout()
    }

    pub fn save(&self) {
        if let Err(e) = crate::settings::save_layout(self) {
            log::warn!("cannot save the panel layout: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::settings::{layout_in, save_layout_in};
    use std::path::PathBuf;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soliddraft-layout-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn the_layout_round_trips_through_the_settings_file_beside_other_keys() {
        let file = temp_file("round-trip.json");
        let _ = std::fs::remove_file(&file);
        assert_eq!(layout_in(&file), Layout::default(), "no file means the default");
        std::fs::write(&file, r#"{"assistant":{"provider":"openrouter"}}"#).unwrap();
        let mut layout = Layout::default();
        layout.toggle(Side::Tree);
        layout.tree.width = 455.5;
        save_layout_in(&file, &layout).unwrap();
        let back = layout_in(&file);
        assert_eq!(back, layout);
        assert!(!back.tree.shown);
        assert_eq!(back.tree.width, 455.5);
        let all: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(all["assistant"]["provider"], "openrouter", "the other sections survive");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_bad_file_falls_back_and_widths_come_into_range() {
        let file = temp_file("bad.json");
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(layout_in(&file), Layout::default());
        std::fs::write(
            &file,
            r#"{"layout":{"tree":{"shown":false,"width":10.0},"assistant":{"shown":true,"width":99999.0}}}"#,
        )
        .unwrap();
        let back = layout_in(&file);
        assert!(!back.tree.shown, "the old assistant key is ignored");
        assert_eq!(back.tree.width, Side::Tree.min_width());
        std::fs::write(&file, r#"{"layout":{"tree":{"shown":true,"width":99999.0}}}"#).unwrap();
        assert_eq!(layout_in(&file).tree.width, MAX_WIDTH);
        let nan = Layout { tree: SidePanel { shown: true, width: f32::NAN } }.clamped();
        assert_eq!(nan.tree.width, Side::Tree.default_width());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn the_maximum_leaves_room_for_the_viewport() {
        let layout = Layout::default();
        // 1440 wide: 1440 - 320 (viewport) = 1120, over the cap.
        assert_eq!(layout.max_width(Side::Tree, 1440.0), MAX_WIDTH);
        assert_eq!(layout.max_width(Side::Tree, 1000.0), 680.0);
        // A narrow window never pushes the maximum under the minimum.
        assert_eq!(layout.max_width(Side::Tree, 400.0), Side::Tree.min_width());
    }
}
