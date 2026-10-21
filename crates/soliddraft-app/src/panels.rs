//! The side areas: the feature tree on the left and the assistant on the right.
//!
//! Each one can be hidden and resized. What the user chose is kept in the app's
//! settings file (see `settings.rs`) and restored at the next start.

use serde::{Deserialize, Serialize};

/// The two side areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// The feature tree with the property panel under it, left of the viewport.
    Tree,
    /// The assistant chat, right of the viewport.
    Assistant,
}

impl Side {
    pub const ALL: [Side; 2] = [Side::Tree, Side::Assistant];

    pub fn other(self) -> Side {
        match self {
            Side::Tree => Side::Assistant,
            Side::Assistant => Side::Tree,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Side::Tree => "Feature tree",
            Side::Assistant => "Assistant",
        }
    }

    /// The title as it reads inside a sentence.
    pub fn name(self) -> &'static str {
        match self {
            Side::Tree => "feature tree",
            Side::Assistant => "assistant",
        }
    }

    /// The command that shows or hides the side area.
    pub fn command(self) -> &'static str {
        match self {
            Side::Tree => "toggle_tree",
            Side::Assistant => "toggle_assistant",
        }
    }

    /// The show-or-hide shortcut, from the command registry. F9 hides the FeatureManager
    /// in SolidWorks; the assistant takes the shifted key.
    pub fn shortcut(self) -> String {
        crate::commands::get(self.command()).and_then(|c| c.shortcut_text()).unwrap_or_default()
    }

    pub fn min_width(self) -> f32 {
        match self {
            Side::Tree => 200.0,
            Side::Assistant => 240.0,
        }
    }

    pub fn default_width(self) -> f32 {
        match self {
            Side::Tree => 380.0,
            Side::Assistant => 360.0,
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
/// The viewport keeps at least this much between the two side areas.
pub const VIEWPORT_MIN_WIDTH: f32 = 320.0;
/// The strip a hidden area leaves at its edge: a chevron and the title set on end.
pub const STRIP_WIDTH: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidePanel {
    pub shown: bool,
    /// The outer width in logical pixels while shown; kept while hidden.
    pub width: f32,
}

/// Visibility and width of both side areas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub tree: SidePanel,
    pub assistant: SidePanel,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            tree: SidePanel { shown: true, width: Side::Tree.default_width() },
            assistant: SidePanel { shown: true, width: Side::Assistant.default_width() },
        }
    }
}

impl Layout {
    pub fn panel(&self, side: Side) -> &SidePanel {
        match side {
            Side::Tree => &self.tree,
            Side::Assistant => &self.assistant,
        }
    }

    pub fn panel_mut(&mut self, side: Side) -> &mut SidePanel {
        match side {
            Side::Tree => &mut self.tree,
            Side::Assistant => &mut self.assistant,
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

    /// The widest this side may be in a window this wide: what is left after the
    /// other side and the viewport's minimum, capped at [`MAX_WIDTH`] and never
    /// below the side's own minimum.
    pub fn max_width(&self, side: Side, window_width: f32) -> f32 {
        let other = self.panel(side.other());
        let other_width = if other.shown { other.width } else { STRIP_WIDTH };
        (window_width - other_width - VIEWPORT_MIN_WIDTH).min(MAX_WIDTH).max(side.min_width())
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
        layout.toggle(Side::Assistant);
        layout.tree.width = 455.5;
        save_layout_in(&file, &layout).unwrap();
        let back = layout_in(&file);
        assert_eq!(back, layout);
        assert!(!back.assistant.shown);
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
        assert!(!back.tree.shown);
        assert_eq!(back.tree.width, Side::Tree.min_width());
        assert_eq!(back.assistant.width, MAX_WIDTH);
        let nan = Layout { tree: SidePanel { shown: true, width: f32::NAN }, ..Default::default() }.clamped();
        assert_eq!(nan.tree.width, Side::Tree.default_width());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn the_maximum_leaves_room_for_the_other_side_and_the_viewport() {
        let layout = Layout::default();
        // 1440 wide: 1440 - 360 (assistant) - 320 (viewport) = 760.
        assert_eq!(layout.max_width(Side::Tree, 1440.0), 760.0);
        // A hidden assistant only costs its strip, so the cap applies.
        let mut hidden = layout;
        hidden.assistant.shown = false;
        assert_eq!(hidden.max_width(Side::Tree, 1440.0), MAX_WIDTH);
        // A narrow window never pushes the maximum under the minimum.
        assert_eq!(layout.max_width(Side::Tree, 600.0), Side::Tree.min_width());
    }
}
