//! The drawing's model reference as the interface sees it: where the part is, how it
//! was found, whether it moved on from the revision the drawing is pinned at, and the
//! repair dialog for a reference whose target cannot be found.
//!
//! The resolver's answer is kept on the session and refreshed when anything it was made
//! from changed: the open part and its revision, either file's path, the stored
//! reference, or the File locations setting. Nothing here touches the file system on a
//! frame where none of those moved.

use super::{DrawingAction, DrawingSession};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, vec2};
use soliddraft_document::resolve::{Candidate, Missing, StoredPathProblem};
use soliddraft_document::{ContentHash, DocumentId, Resolution, Resolver, Target, Via};
use std::path::{Path, PathBuf};

/// What the link was resolved from. The link is refreshed when any of it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LinkKey {
    part: DocumentId,
    mesh_version: u64,
    part_path: Option<PathBuf>,
    drawing_path: Option<PathBuf>,
    model_id: Option<DocumentId>,
    model_path: String,
    locations: u64,
}

/// The resolver's last answer for the drawing's model.
#[derive(Clone, Debug)]
pub struct ModelLink {
    pub resolution: Resolution,
    key: LinkKey,
}

/// The model's state, in the words every panel shares.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    /// Not resolved yet.
    Unknown,
    /// The target is at the revision the drawing is pinned at.
    Current,
    /// The target moved on: the drawing shows the pinned revision until a Rebuild.
    Newer {
        /// The target's revision now.
        now: ContentHash,
    },
    /// The target has no revision to compare: it is not a SolidDraft document.
    Unverified,
    /// The target was not found; the reference needs a repair.
    Missing,
}

impl DrawingSession {
    /// The model's state against the pinned revision.
    pub fn link_state(&self) -> LinkState {
        match self.link.as_ref().map(|l| &l.resolution) {
            None => LinkState::Unknown,
            Some(Resolution::Missing(_)) => LinkState::Missing,
            Some(Resolution::Found(t)) => match t.status(&self.doc.drawing.model) {
                soliddraft_document::RefStatus::Current => LinkState::Current,
                soliddraft_document::RefStatus::NewerAvailable { now } => LinkState::Newer { now },
                soliddraft_document::RefStatus::Unverified => LinkState::Unverified,
            },
        }
    }

    /// The model's target, when it was found.
    pub fn model_target(&self) -> Option<&Target> {
        self.link.as_ref().and_then(|l| l.resolution.target())
    }

    /// The model's file, when it was found and has one.
    pub fn model_file(&self) -> Option<PathBuf> {
        self.model_target().and_then(|t| t.path()).map(Path::to_path_buf)
    }

    /// True when the open part (`part`) is the document the drawing shows: the reference
    /// names it, or an unbound reference resolved to its file.
    pub fn model_is(&self, part: DocumentId) -> bool {
        self.doc.drawing.model.id == Some(part) || self.model_target().and_then(Target::id) == Some(part)
    }

    /// The missing reference's problem and candidates, when it is missing.
    pub fn missing(&self) -> Option<&Missing> {
        match self.link.as_ref().map(|l| &l.resolution) {
            Some(Resolution::Missing(m)) => Some(m),
            _ => None,
        }
    }

    /// The stored path of the model, absolute when the drawing has a file.
    pub fn model_stored_path(&self) -> PathBuf {
        match &self.doc.path {
            Some(p) => self.doc.drawing.model.stored_path(p),
            None => PathBuf::from(&self.doc.drawing.model.path),
        }
    }

    /// Where the model was found, in words: "open in this session", "at its stored
    /// path", "found by id in parts".
    pub fn found_words(&self) -> String {
        let Some(t) = self.model_target() else {
            return match self.link_state() {
                LinkState::Missing => "not found".to_owned(),
                _ => "not resolved yet".to_owned(),
            };
        };
        match t.via {
            Via::OpenDocument => "open in this session".to_owned(),
            Via::StoredPath => "at its stored path".to_owned(),
            Via::SavedSibling => "beside its stored path, saved in the current format".to_owned(),
            Via::Index => {
                let folder = t
                    .path()
                    .and_then(Path::parent)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "a search folder".to_owned());
                format!("found by id in {folder}")
            }
        }
    }

    /// The one-line marker for the tree header and the status bar, with its colour.
    pub fn marker(&self) -> Option<(String, Color32)> {
        match self.link_state() {
            LinkState::Missing => Some(("Part not found".to_owned(), theme::ERROR)),
            LinkState::Newer { .. } => Some(("Newer version available".to_owned(), theme::WARN)),
            _ if self.doc.is_stale() => Some(("Newer version available".to_owned(), theme::WARN)),
            _ => None,
        }
    }
}

fn key(state: &AppState) -> Option<LinkKey> {
    let s = state.drawing.session.as_ref()?;
    Some(LinkKey {
        part: state.doc.model().id(),
        mesh_version: state.doc.mesh_version,
        part_path: state.doc.path.clone(),
        drawing_path: s.doc.path.clone(),
        model_id: s.doc.drawing.model.id,
        model_path: s.doc.drawing.model.path.clone(),
        locations: state.locations_epoch,
    })
}

/// The resolver for the open drawing: the open part first, then the drawing's stored
/// path, then the drawing's folder and the File locations search folders.
pub fn resolver(state: &AppState) -> Option<Resolver> {
    let session = state.drawing.session.as_ref()?;
    let part = soliddraft_document::OpenDocument {
        id: state.doc.model().id(),
        kind: soliddraft_document::DocKind::Part,
        path: state.doc.path.clone(),
        revision: super::part::digest(&state.doc),
    };
    Some(
        Resolver::new(session.doc.path.as_deref())
            .with_open([part])
            .with_search_folders(crate::settings::file_locations().search_folders()),
    )
}

/// Re-resolves the drawing's model when anything it depends on changed, and keeps the
/// session's view of the part's current revision in step, so the views read as older
/// than the part when the part's file moved on while the part was not open. Runs once
/// per frame; it does nothing while a feature editor has the part rolled back.
pub fn refresh(state: &mut AppState) {
    if state.editor.is_some() || state.doc.rollback.is_some() {
        return;
    }
    let Some(key) = key(state) else { return };
    if state.drawing.session.as_ref().and_then(|s| s.link.as_ref()).is_some_and(|l| l.key == key) {
        return;
    }
    let Some(resolution) = super::resolve_model(state) else { return };
    let Some(session) = state.drawing.session.as_mut() else { return };
    let before = session.link_state();
    match &resolution {
        Resolution::Found(t) => {
            if let Some(now) = t.revision() {
                session.doc.seen_digest = now.clone();
            }
        }
        // The current revision is unknown; the drawing shows what it has.
        Resolution::Missing(_) => session.doc.seen_digest = session.doc.drawing.model.pinned.clone(),
    }
    session.link = Some(ModelLink { resolution, key });
    let now = session.link_state();
    if now != before {
        session.marker_dismissed = false;
    }
    // A repair dialog whose reference resolved (a search folder was added) is done.
    let words = session.found_words();
    if now != LinkState::Missing && state.drawing.repair.as_ref().is_some_and(|d| !d.replace) {
        state.drawing.repair = None;
        state.note(format!("Reference found: {words}"), crate::state::NoteKind::Ok);
    }
}

/// What was wrong at the stored path, in words.
pub fn problem_words(m: &Missing) -> String {
    match &m.stored {
        StoredPathProblem::NotFound => "There is no file at its stored path.".to_owned(),
        StoredPathProblem::DifferentDocument(id) => {
            format!("The file at its stored path is a different part (id {}).", short_id(*id))
        }
        StoredPathProblem::WrongKind(kind) => format!("The file at its stored path is a {kind}, not a part."),
        StoredPathProblem::Unreadable(e) => format!("The file at its stored path cannot be read: {e}"),
    }
}

fn short_id(id: DocumentId) -> String {
    id.to_string().chars().take(8).collect()
}

/// The repair dialog: a missing reference's problem and candidates, a path typed or
/// browsed by hand, and whether a different part may take the reference. Also the
/// Replace model command, which is the same dialog with any part accepted.
#[derive(Clone, Debug)]
pub struct RepairDialog {
    /// True for Replace model: a different document is what is wanted.
    pub replace: bool,
    /// A path typed by hand.
    pub typed: String,
    /// Allow a different document for the typed or browsed path.
    pub allow_different: bool,
    /// Parts in reach of the drawing's folder and the search folders.
    pub candidates: Vec<Candidate>,
    pub error: Option<String>,
}

impl RepairDialog {
    /// The dialog for the open drawing, with the candidates in reach.
    pub fn new(state: &AppState, replace: bool) -> Option<RepairDialog> {
        let session = state.drawing.session.as_ref()?;
        let candidates = match (replace, session.missing()) {
            (false, Some(m)) => m.candidates.clone(),
            _ => resolver(state).map(|r| r.candidates(&session.doc.drawing.model)).unwrap_or_default(),
        };
        Some(RepairDialog { replace, typed: String::new(), allow_different: replace, candidates, error: None })
    }
}

/// The repair dialog window. Shown while `state.drawing.repair` is set.
pub fn window(ctx: &egui::Context, state: &mut AppState) {
    let Some(dialog) = state.drawing.repair.clone() else { return };
    let Some(session) = state.drawing.session.as_ref() else {
        state.drawing.repair = None;
        return;
    };
    let mut open = true;
    let mut actions: Vec<DrawingAction> = Vec::new();
    let mut next = dialog.clone();
    let title = if dialog.replace { "Replace model" } else { "Repair reference" };
    let model = &session.doc.drawing.model;
    let part_name = session.doc.drawing.meta.title.clone();
    let drawing_name = session.title();
    let drawing_dir = session.doc.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
    let stored = session.model_stored_path();
    let missing = session.missing().cloned();
    let model_id = model.id;
    let model_path = model.path.clone();
    let pinned = model.pinned.short().to_owned();
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0))
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(540.0);
            // What is wrong, in the drawing's words.
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                let colour = if dialog.replace { theme::ACCENT } else { theme::ERROR };
                icons::paint(ui.painter(), if dialog.replace { Icon::Open } else { Icon::Warning }, r, colour);
                let head = if dialog.replace {
                    format!("{drawing_name} shows {part_name} ({model_path})")
                } else {
                    format!("{drawing_name} references a part at {model_path}")
                };
                ui.label(egui::RichText::new(head).family(theme::semibold()).size(12.5).color(theme::INK));
            });
            ui.add_space(2.0);
            if dialog.replace {
                ui.label(
                    egui::RichText::new(
                        "Choose another part. The views keep what they show and re-project onto the new \
                         part at the next Rebuild; dimensions that find no edge there are listed for repair.",
                    )
                    .size(11.5)
                    .color(theme::MUTED),
                );
            } else if let Some(m) = &missing {
                ui.label(egui::RichText::new(problem_words(m)).size(11.5).color(theme::INK));
                let searched = match &drawing_dir {
                    Some(d) => format!(
                        "Not found by id in {} or the search folders. The drawing keeps showing rev {pinned}.",
                        d.display()
                    ),
                    None => {
                        format!("The drawing is not saved, so there is no folder to search. It keeps rev {pinned}.")
                    }
                };
                ui.label(egui::RichText::new(searched).size(11.0).color(theme::MUTED));
            }
            ui.add_space(8.0);

            // Candidates: parts in reach, the same name first.
            theme::section_label(
                ui,
                &if dialog.candidates.is_empty() {
                    "No parts in reach".to_owned()
                } else {
                    format!("Parts in reach ({})", dialog.candidates.len())
                },
            );
            ui.add_space(2.0);
            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                for c in &dialog.candidates {
                    candidate_card(ui, c, model_id, drawing_dir.as_deref(), &mut actions);
                    ui.add_space(4.0);
                }
            });
            if dialog.candidates.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Add the folder it moved to under Referenced documents in File locations, or pick the file.",
                    )
                    .size(11.0)
                    .color(theme::MUTED),
                );
            }
            ui.add_space(8.0);

            // By hand.
            theme::section_label(ui, "Pick the file by hand");
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let field = egui::TextEdit::singleline(&mut next.typed)
                    .hint_text(stored.display().to_string())
                    .desired_width(ui.available_width() - 190.0);
                let r = ui.add(field);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Repair path"));
                if ui.button("Browse…").on_hover_text("Choose the part file").clicked()
                    && let Some(p) = crate::platform::open_path(
                        "Choose the part",
                        "SolidDraft part",
                        soliddraft_document::DocKind::Part.read_extensions(),
                    )
                {
                    next.typed = p.display().to_string();
                }
                let typed = next.typed.trim().to_owned();
                if ui.add_enabled(!typed.is_empty(), egui::Button::new("Use this file")).clicked() {
                    actions
                        .push(DrawingAction::RepairModel { path: PathBuf::from(typed), replace: next.allow_different });
                }
            });
            if !dialog.replace {
                ui.checkbox(&mut next.allow_different, "Allow a different part to take the reference")
                    .on_hover_text("A same-named file with another id is never taken by itself");
            }
            if let Some(e) = &dialog.error {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(e).size(11.5).color(theme::ERROR));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("File locations…").on_hover_text("The folders searched for moved references").clicked() {
                    actions.push(DrawingAction::ShowLocations);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let later = if dialog.replace { "Cancel" } else { "Later" };
                    if ui.button(later).on_hover_text("The drawing keeps its views; the marker stays").clicked() {
                        actions.push(DrawingAction::CloseRepair);
                    }
                });
            });
        });
    if !open {
        actions.push(DrawingAction::CloseRepair);
    }
    if next.typed != dialog.typed || next.allow_different != dialog.allow_different {
        state.drawing.repair = Some(next);
    }
    for a in actions {
        state.push(AppAction::Drawing(a));
    }
}

/// One candidate, in the card language of the Needs attention list.
fn candidate_card(
    ui: &mut egui::Ui,
    c: &Candidate,
    model_id: Option<DocumentId>,
    drawing_dir: Option<&Path>,
    actions: &mut Vec<DrawingAction>,
) {
    let same_id = model_id == Some(c.identity.id);
    let name = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let shown = match drawing_dir {
        Some(d) => soliddraft_document::reference::relative_path(d, &c.path),
        None => c.path.display().to_string(),
    };
    let bar_colour = if same_id {
        theme::OK
    } else if c.same_name {
        theme::WARN
    } else {
        theme::LINE
    };
    let card = egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::LINE))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(9, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&name).family(theme::semibold()).size(12.5).color(theme::INK));
                let tag = if same_id {
                    "same part"
                } else if c.same_name {
                    "same name · different part"
                } else {
                    "different part"
                };
                ui.label(egui::RichText::new(format!("· {tag}")).size(11.5).color(theme::MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let use_it =
                        egui::Button::new(egui::RichText::new("Use this part").color(Color32::WHITE).size(11.5))
                            .fill(theme::ACCENT);
                    if ui.add(use_it).clicked() {
                        actions.push(DrawingAction::RepairModel { path: c.path.clone(), replace: !same_id });
                    }
                });
            });
            ui.label(
                egui::RichText::new(format!("{shown} · rev {}", c.identity.revision.short()))
                    .size(11.0)
                    .color(theme::MUTED),
            );
        })
        .response
        .rect;
    // The colour bar on the card's left edge, over its final height.
    let bar = Rect::from_min_size(card.left_top(), vec2(3.0, card.height()));
    ui.painter().rect_filled(bar, CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, bar_colour);
}

/// The reference bar over the sheet while the part moved on or cannot be found: the
/// SolidWorks message bar for an out-of-date drawing, with Rebuild or Repair.
pub fn bar(ui: &mut egui::Ui, s: &DrawingSession, rect: Rect, push: &mut impl FnMut(DrawingAction)) {
    let part = &s.doc.drawing.meta.title;
    let pinned = s.doc.drawing.model.pinned.short();
    let (head, detail, colour, action, label) = match s.link_state() {
        LinkState::Missing => (
            format!("{part} not found"),
            format!("Not at {}, not found by id. The sheet shows rev {pinned}.", s.doc.drawing.model.path),
            theme::ERROR,
            DrawingAction::ShowRepair { replace: false },
            "Repair…",
        ),
        LinkState::Newer { now } => (
            format!("Newer version of {part} available"),
            format!("The sheet shows rev {pinned}; the part is at rev {} ({}).", now.short(), s.found_words()),
            theme::WARN,
            DrawingAction::Rebuild,
            "Rebuild",
        ),
        _ if s.doc.is_stale() => (
            format!("Newer version of {part} available"),
            format!("The sheet shows rev {pinned}; the open part moved on."),
            theme::WARN,
            DrawingAction::Rebuild,
            "Rebuild",
        ),
        _ => return,
    };
    let width = (rect.width() - 40.0).min(820.0);
    let area = Rect::from_min_size(egui::pos2(rect.center().x - width / 2.0, rect.top() + 38.0), vec2(width, 44.0));
    let bg =
        if colour == theme::ERROR { Color32::from_rgb(0xfd, 0xee, 0xe8) } else { Color32::from_rgb(0xfd, 0xf6, 0xe3) };
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            let painter = ui.painter();
            painter.rect(
                area,
                CornerRadius::same(8),
                bg,
                Stroke::new(1.0, colour.gamma_multiply(0.5)),
                egui::StrokeKind::Inside,
            );
            let icon = Rect::from_center_size(egui::pos2(area.left() + 18.0, area.center().y), vec2(14.0, 14.0));
            icons::paint(painter, Icon::Warning, icon, colour);
            ui.add_space(30.0);
            ui.vertical(|ui| {
                ui.set_max_width(area.width() - 150.0);
                ui.add_space(5.0);
                let head = egui::RichText::new(head).family(theme::semibold()).size(12.0).color(theme::INK);
                ui.add(egui::Label::new(head).truncate());
                ui.add(egui::Label::new(egui::RichText::new(detail).size(11.5).color(theme::INK)).truncate());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui
                    .add(egui::Button::new(egui::RichText::new("×").size(13.0)).frame(false))
                    .on_hover_text("Hide until the state changes")
                    .clicked()
                {
                    push(DrawingAction::DismissMarker);
                }
                let button = egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(colour);
                if ui.add(button).clicked() {
                    push(action);
                }
            });
        },
    );
}

/// The Model reference PropertyManager: the part, its file, how it was found, the
/// pinned and current revisions, and the commands that act on the reference.
pub fn panel(ui: &mut egui::Ui, s: &DrawingSession, part_open: bool, actions: &mut Vec<DrawingAction>) {
    let model = &s.doc.drawing.model;
    let value_row = |ui: &mut egui::Ui, label: &str, value: &str| {
        ui.horizontal(|ui| {
            ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::MUTED)));
            egui::Frame::new()
                .fill(theme::FIELD_BG)
                .stroke(Stroke::new(1.0, theme::LINE))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 16.0);
                    ui.add(egui::Label::new(egui::RichText::new(value).size(12.0)).truncate());
                });
        });
    };
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        let state = s.link_state();
        let (status, colour) = match &state {
            LinkState::Unknown => ("Not resolved yet".to_owned(), theme::MUTED),
            LinkState::Current => ("Up to date".to_owned(), theme::OK),
            LinkState::Newer { now } => (format!("Newer version available · rev {}", now.short()), theme::WARN),
            LinkState::Unverified => ("Not a SolidDraft document; no revision to compare".to_owned(), theme::MUTED),
            LinkState::Missing => ("Not found".to_owned(), theme::ERROR),
        };
        ui.label(egui::RichText::new(status).family(theme::semibold()).size(12.5).color(colour));
        ui.add_space(6.0);
        value_row(ui, "Part", &s.doc.drawing.meta.title);
        value_row(ui, "File", &model.path);
        value_row(ui, "Found", &s.found_words());
        value_row(ui, "Pinned", &format!("rev {}", model.pinned.short()));
        let current = match &state {
            LinkState::Newer { now } => format!("rev {}", now.short()),
            LinkState::Current => format!("rev {} (same)", model.pinned.short()),
            _ => "unknown".to_owned(),
        };
        value_row(ui, "Current", &current);
        if let Some(id) = model.id {
            value_row(ui, "Id", &id.to_string());
        } else {
            value_row(ui, "Id", "unbound: bound at the next Rebuild or save");
        }
        if let Some(t) = s.model_target()
            && let Some(p) = &t.new_path
        {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("The file moved; the drawing stores {p} when it is next saved."))
                    .size(11.0)
                    .color(theme::MUTED),
            );
        }
        ui.add_space(8.0);
        theme::hline(ui);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let newer = matches!(state, LinkState::Newer { .. }) || s.doc.is_stale();
            if newer {
                let rebuild = egui::Button::new(egui::RichText::new("Rebuild").color(Color32::WHITE)).fill(theme::WARN);
                if ui.add(rebuild).on_hover_text("Bring the views to the current revision (Ctrl+B)").clicked() {
                    actions.push(DrawingAction::Rebuild);
                }
            }
            if state == LinkState::Missing {
                let repair = egui::Button::new(egui::RichText::new("Repair…").color(Color32::WHITE)).fill(theme::ERROR);
                if ui.add(repair).clicked() {
                    actions.push(DrawingAction::ShowRepair { replace: false });
                }
            }
            if ui.button("Replace model…").on_hover_text("Point the drawing at a different part").clicked() {
                actions.push(DrawingAction::ShowRepair { replace: true });
            }
            if part_open {
                if ui.button("Show part").clicked() {
                    actions.push(DrawingAction::ShowPart);
                }
            } else if s.model_file().is_some() && ui.button("Open part").clicked() {
                actions.push(DrawingAction::OpenModel);
            }
        });
    });
}

/// Draws the small out-of-date glyph SolidWorks puts beside a reference that needs a
/// rebuild: a traffic light, amber for newer available and red for missing.
pub fn glyph(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let r = rect.height() * 0.5;
    painter.rect_filled(rect, CornerRadius::same(3), colour.gamma_multiply(0.18));
    painter.circle_filled(rect.center(), r * 0.55, colour);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_are_said_in_words() {
        let m = Missing { stored: StoredPathProblem::NotFound, candidates: Vec::new() };
        assert_eq!(problem_words(&m), "There is no file at its stored path.");
        let id = DocumentId::new();
        let m = Missing { stored: StoredPathProblem::DifferentDocument(id), candidates: Vec::new() };
        assert!(problem_words(&m).contains(&short_id(id)));
        let m = Missing {
            stored: StoredPathProblem::WrongKind(soliddraft_document::DocKind::Drawing),
            candidates: Vec::new(),
        };
        assert_eq!(problem_words(&m), "The file at its stored path is a drawing, not a part.");
    }
}
