//! The File locations settings page: per kind of reference file, an ordered list of
//! folders and files (`soliddraft_document::locations`), with add, remove and reorder.
//! Built-in content is listed read-only above the user's entries. The page is the
//! SolidWorks one: the kinds on the left, the folders for the chosen kind on the right.
//!
//! Saving writes the `file_locations` section of the settings file. Search folders for
//! references apply at once; templates and styles are read when the app starts.

use crate::state::{AppState, NoteKind};
use crate::theme;
use egui::{Color32, CornerRadius, Sense, Stroke, vec2};
use soliddraft_document::{FileLocations, LocationKind};
use std::path::PathBuf;

/// The open File locations page: a working copy of the setting until OK.
#[derive(Clone, Debug)]
pub struct LocationsDialog {
    pub locations: FileLocations,
    pub kind: LocationKind,
    /// The selected entry of the shown kind.
    pub selected: Option<usize>,
    /// A path typed by hand.
    pub typed: String,
    pub error: Option<String>,
}

impl LocationsDialog {
    /// The page on the saved setting, showing `kind` (search folders by default).
    pub fn new(kind: Option<LocationKind>) -> LocationsDialog {
        LocationsDialog {
            locations: crate::settings::file_locations(),
            kind: kind.unwrap_or(LocationKind::ReferencedDocuments),
            selected: None,
            typed: String::new(),
            error: None,
        }
    }

    /// Adds `path` to the shown kind. Returns false when it is already listed.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let added = self.locations.add(self.kind, path);
        if added {
            self.selected = Some(self.locations.get(self.kind).len() - 1);
            self.error = None;
        } else {
            self.error = Some("Already listed".into());
        }
        added
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected.and_then(|i| self.locations.get(self.kind).get(i).cloned())
    }

    pub fn remove(&mut self) {
        if let Some(p) = self.selected_path() {
            self.locations.remove(self.kind, &p);
            let n = self.locations.get(self.kind).len();
            self.selected = if n == 0 { None } else { Some(self.selected.unwrap_or(0).min(n - 1)) };
        }
    }

    pub fn shift(&mut self, up: bool) {
        if let Some(p) = self.selected_path()
            && self.locations.shift(self.kind, &p, up)
        {
            self.selected = self.locations.get(self.kind).iter().position(|x| *x == p);
        }
    }
}

/// What a kind is for, under its name.
fn describe(kind: LocationKind) -> &'static str {
    match kind {
        LocationKind::ReferencedDocuments => {
            "Folders searched, with their subfolders, for a referenced part or assembly that is not where the \
             reference says. The referencing file's own folder is always searched first."
        }
        LocationKind::MaterialLibraries => "Material libraries (.toml): density and the other physical properties.",
        LocationKind::AppearanceLibraries => "Appearance libraries (.toml): colour, finish and texture.",
        LocationKind::PartTemplates => "Templates for new parts (.dwprt).",
        LocationKind::AssemblyTemplates => "Templates for new assemblies (.dwasm).",
        LocationKind::DrawingTemplates => "Sheet templates for new drawings (.toml): paper, margins, zones, scales.",
        LocationKind::SheetFormats => "Sheet formats and title blocks (.toml).",
        LocationKind::DraftingStyles => "Dimension styles (.toml): text, arrows, symbols.",
        LocationKind::ConfigurationTables => "Configuration tables (.csv, .toml).",
    }
}

/// The compiled-in content of a kind, read-only.
fn built_in(kind: LocationKind) -> Vec<String> {
    use soliddraft_drawing::library;
    match kind {
        LocationKind::ReferencedDocuments => vec!["The drawing's own folder and its subfolders".to_owned()],
        LocationKind::MaterialLibraries => vec!["Built-in material library".to_owned()],
        LocationKind::AppearanceLibraries => vec!["Built-in appearance library".to_owned()],
        LocationKind::PartTemplates => vec!["Default part (millimetres, no features)".to_owned()],
        LocationKind::AssemblyTemplates => vec!["Default assembly".to_owned()],
        LocationKind::DrawingTemplates | LocationKind::SheetFormats => {
            library::library().templates.iter().map(|t| format!("{} ({})", t.name, t.id)).collect()
        }
        LocationKind::DraftingStyles => {
            library::library().styles.iter().map(|s| format!("{} ({})", s.name, s.id)).collect()
        }
        LocationKind::ConfigurationTables => Vec::new(),
    }
}

/// `text` cut from the start with an ellipsis until it fits in `width`.
fn fit_end(ui: &egui::Ui, text: &str, width: f32) -> String {
    let measure = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), theme::font(12.0), theme::INK).size().x);
    if measure(text) <= width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut skip = 1;
    while skip < chars.len() {
        let candidate: String = std::iter::once('…').chain(chars[skip..].iter().copied()).collect();
        if measure(&candidate) <= width {
            return candidate;
        }
        skip += 1;
    }
    "…".to_owned()
}

/// Saves the page's setting and tells the app what changed.
fn save(state: &mut AppState, locations: &FileLocations) -> Result<(), String> {
    crate::settings::save_file_locations(locations)?;
    state.locations_epoch += 1;
    Ok(())
}

/// The File locations window. Shown while `state.locations_dialog` is set.
pub fn window(ctx: &egui::Context, state: &mut AppState) {
    let Some(mut d) = state.locations_dialog.clone() else { return };
    let mut open = true;
    enum Done {
        Ok,
        Cancel,
    }
    let mut done: Option<Done> = None;
    egui::Window::new("File locations")
        .collapsible(false)
        .resizable(false)
        .default_width(720.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -20.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(700.0);
            ui.label(
                egui::RichText::new(
                    "Where SolidDraft looks for reference files, in order. Built-in content stays compiled in; \
                     these add to it.",
                )
                .size(11.5)
                .color(theme::MUTED),
            );
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                // The kinds.
                ui.vertical(|ui| {
                    ui.set_width(220.0);
                    theme::section_label(ui, "Show folders for");
                    ui.add_space(2.0);
                    for kind in LocationKind::ALL {
                        let n = d.locations.get(kind).len();
                        let on = d.kind == kind;
                        let (rect, resp) = ui.allocate_exact_size(vec2(214.0, 24.0), Sense::click());
                        resp.widget_info(|| {
                            egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, kind.label())
                        });
                        let (bg, ink) = if on {
                            (theme::SELECTION_BG, theme::SELECTION_INK)
                        } else if resp.hovered() {
                            (theme::HOVER, theme::INK)
                        } else {
                            (Color32::TRANSPARENT, theme::INK)
                        };
                        ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
                        ui.painter().text(
                            egui::pos2(rect.left() + 10.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            kind.label(),
                            theme::font(12.0),
                            ink,
                        );
                        if n > 0 {
                            ui.painter().text(
                                egui::pos2(rect.right() - 10.0, rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                n.to_string(),
                                theme::font(11.0),
                                theme::MUTED,
                            );
                        }
                        if resp.clicked() && !on {
                            d.kind = kind;
                            d.selected = None;
                            d.error = None;
                        }
                    }
                });
                ui.add_space(10.0);
                // The entries of the chosen kind.
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(d.kind.label()).family(theme::semibold()).size(13.0));
                    ui.label(egui::RichText::new(describe(d.kind)).size(11.0).color(theme::MUTED));
                    ui.add_space(6.0);
                    let built = built_in(d.kind);
                    if !built.is_empty() {
                        theme::section_label(ui, "Built in");
                        for b in &built {
                            ui.horizontal(|ui| {
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new(b).size(12.0).color(theme::MUTED));
                                ui.label(egui::RichText::new("· read-only").size(10.5).color(theme::MUTED));
                            });
                        }
                        ui.add_space(6.0);
                    }
                    theme::section_label(ui, "Added");
                    ui.horizontal_top(|ui| {
                        let list_w = ui.available_width() - 110.0;
                        egui::Frame::new()
                            .fill(theme::FIELD_BG)
                            .stroke(Stroke::new(1.0, theme::LINE))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.set_width(list_w - 8.0);
                                ui.set_min_height(150.0);
                                let entries: Vec<PathBuf> = d.locations.get(d.kind).to_vec();
                                if entries.is_empty() {
                                    ui.add_space(4.0);
                                    ui.label(egui::RichText::new("Nothing added").size(11.5).color(theme::MUTED));
                                }
                                for (i, p) in entries.iter().enumerate() {
                                    let on = d.selected == Some(i);
                                    let text = p.display().to_string();
                                    let (rect, resp) =
                                        ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                                    resp.widget_info(|| {
                                        egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, text.clone())
                                    });
                                    if on {
                                        ui.painter().rect_filled(rect, CornerRadius::same(4), theme::SELECTION_BG);
                                    } else if resp.hovered() {
                                        ui.painter().rect_filled(rect, CornerRadius::same(4), theme::HOVER);
                                    }
                                    let what = if p.is_dir() {
                                        "folder"
                                    } else if p.is_file() {
                                        "file"
                                    } else {
                                        "missing"
                                    };
                                    // A long path keeps its end, which is the part that tells.
                                    let shown = fit_end(ui, &text, rect.width() - 66.0);
                                    ui.painter().text(
                                        egui::pos2(rect.left() + 6.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        shown,
                                        theme::font(12.0),
                                        if on { theme::SELECTION_INK } else { theme::INK },
                                    );
                                    ui.painter().text(
                                        egui::pos2(rect.right() - 6.0, rect.center().y),
                                        egui::Align2::RIGHT_CENTER,
                                        what,
                                        theme::font(10.5),
                                        if what == "missing" { theme::WARN } else { theme::MUTED },
                                    );
                                    if resp.clicked() {
                                        d.selected = Some(i);
                                    }
                                }
                            });
                        ui.vertical(|ui| {
                            ui.set_width(100.0);
                            let w = vec2(100.0, 24.0);
                            if ui.add_sized(w, egui::Button::new("Add folder…")).clicked()
                                && let Some(p) = crate::platform::pick_folder("Add folder")
                            {
                                d.add(p);
                            }
                            if ui.add_sized(w, egui::Button::new("Add file…")).clicked()
                                && let Some(p) = crate::platform::open_path(
                                    "Add file",
                                    d.kind.label(),
                                    if d.kind.extensions().is_empty() {
                                        &["dwprt", "dwasm"][..]
                                    } else {
                                        d.kind.extensions()
                                    },
                                )
                            {
                                d.add(p);
                            }
                            let has = d.selected.is_some();
                            if ui.add_enabled(has, egui::Button::new("Remove").min_size(w)).clicked() {
                                d.remove();
                            }
                            if ui.add_enabled(has, egui::Button::new("Move up").min_size(w)).clicked() {
                                d.shift(true);
                            }
                            if ui.add_enabled(has, egui::Button::new("Move down").min_size(w)).clicked() {
                                d.shift(false);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let field = egui::TextEdit::singleline(&mut d.typed)
                            .hint_text("Type a folder or file path")
                            .desired_width(ui.available_width() - 100.0);
                        let r = ui.add(field);
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Location path"));
                        let typed = d.typed.trim().to_owned();
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.add_enabled(!typed.is_empty(), egui::Button::new("Add path")).clicked() || enter)
                            && !typed.is_empty()
                            && d.add(PathBuf::from(&typed))
                        {
                            d.typed.clear();
                        }
                    });
                    if let Some(e) = &d.error {
                        ui.label(egui::RichText::new(e).size(11.5).color(theme::ERROR));
                    }
                });
            });
            ui.add_space(8.0);
            let frame = egui::Frame::new()
                .fill(theme::ACCENT_SOFT)
                .corner_radius(CornerRadius::same(6))
                .stroke(Stroke::new(1.0, theme::LINE))
                .inner_margin(egui::Margin::symmetric(8, 5));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(
                        "Search folders apply as soon as you press OK. Templates, sheet formats and styles added \
                         here are read when SolidDraft starts.",
                    )
                    .size(11.0)
                    .color(theme::SELECTION_INK),
                );
            });
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(egui::RichText::new("OK").color(Color32::WHITE)).fill(theme::ACCENT);
                if ui.add(ok).clicked() {
                    done = Some(Done::Ok);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(Done::Cancel);
                }
            });
        });
    if !open {
        done = Some(Done::Cancel);
    }
    match done {
        Some(Done::Ok) => match save(state, &d.locations) {
            Ok(()) => {
                state.locations_dialog = None;
                state.note("File locations saved", NoteKind::Ok);
            }
            Err(e) => {
                d.error = Some(e);
                state.locations_dialog = Some(d);
            }
        },
        Some(Done::Cancel) => state.locations_dialog = None,
        None => state.locations_dialog = Some(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_adds_removes_and_reorders() {
        let mut d = LocationsDialog {
            locations: FileLocations::new(),
            kind: LocationKind::MaterialLibraries,
            selected: None,
            typed: String::new(),
            error: None,
        };
        assert!(d.add(PathBuf::from("/a")));
        assert!(d.add(PathBuf::from("/b")));
        assert!(!d.add(PathBuf::from("/b")), "listed once");
        assert_eq!(d.error.as_deref(), Some("Already listed"));
        assert_eq!(d.selected, Some(1));
        d.shift(true);
        assert_eq!(d.locations.get(LocationKind::MaterialLibraries), [PathBuf::from("/b"), PathBuf::from("/a")]);
        assert_eq!(d.selected, Some(0));
        d.remove();
        assert_eq!(d.locations.get(LocationKind::MaterialLibraries), [PathBuf::from("/a")]);
        assert_eq!(d.selected, Some(0));
        d.remove();
        assert!(d.selected.is_none());
        for kind in LocationKind::ALL {
            assert!(!describe(kind).is_empty());
        }
        assert!(built_in(LocationKind::DrawingTemplates).iter().any(|t| t.contains("iso-a3")));
    }
}
