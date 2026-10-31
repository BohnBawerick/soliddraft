//! The Preferences window: a searchable list of categories on the left and the chosen
//! page on the right. This first delivery fills Navigation > Mouse, Navigation > Orbit
//! and zoom and Navigation > View keys; the other categories are reserved here as data
//! and shown as coming later, never as finished pages.
//!
//! [`Preferences`] holds the window's state and a draft of the navigation settings.
//! Every accepted change is handed back as a [`Change`]; the app saves it
//! (`AppState::set_navigation`) and the tests can do the same without a GPU.

mod ui;

use crate::navigation::view_keys::{self, ViewKeys};
use crate::navigation::{Chord, NavigationSettings, Refusal, Router, presets};

/// A page of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// Stable: `navigation.mouse`.
    pub id: &'static str,
    pub category: &'static str,
    pub title: &'static str,
}

impl Page {
    /// As the window writes where it is: "Navigation > Mouse".
    pub fn path(&self) -> String {
        let category = categories().iter().find(|c| c.id == self.category).map_or(self.category, |c| c.label);
        format!("{category} > {}", self.title)
    }
}

pub const MOUSE: &str = "navigation.mouse";
pub const ORBIT_ZOOM: &str = "navigation.orbit_zoom";
pub const VIEW_KEYS: &str = "navigation.view_keys";

const PAGES: [Page; 3] = [
    Page { id: MOUSE, category: "navigation", title: "Mouse" },
    Page { id: ORBIT_ZOOM, category: "navigation", title: "Orbit and zoom" },
    Page { id: VIEW_KEYS, category: "navigation", title: "View keys" },
];

/// The pages with controls, in the order the list shows them.
pub fn available_pages() -> &'static [Page] {
    &PAGES
}

pub fn page(id: &str) -> Option<&'static Page> {
    PAGES.iter().find(|p| p.id == id)
}

/// Whether a category has pages yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Available,
    /// Its id is kept for the pages to come; it shows as coming later.
    Reserved,
}

/// A category of the left list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Category {
    pub id: &'static str,
    pub label: &'static str,
    pub status: Status,
}

const CATEGORIES: [Category; 7] = [
    Category { id: "navigation", label: "Navigation", status: Status::Available },
    Category { id: "viewport", label: "Viewport", status: Status::Reserved },
    Category { id: "selection_editing", label: "Selection and editing", status: Status::Reserved },
    Category { id: "interface", label: "Interface", status: Status::Reserved },
    Category { id: "appearance_graphics", label: "Appearance and graphics", status: Status::Reserved },
    Category { id: "general_documents", label: "General and documents", status: Status::Reserved },
    Category { id: "assistant", label: "Assistant", status: Status::Reserved },
];

pub fn categories() -> &'static [Category] {
    &CATEGORIES
}

/// The categories kept for later pages.
pub fn reserved_categories() -> Vec<&'static Category> {
    CATEGORIES.iter().filter(|c| c.status == Status::Reserved).collect()
}

/// A search result: a page, and the control on it when the match was one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub page: &'static str,
    /// The control the window highlights: `wheel_direction`, `preset.fusion`.
    pub control: Option<String>,
    pub label: String,
}

/// Everything search can find: the pages, their controls, the presets, the view-key
/// profiles and every binding.
fn index() -> Vec<Hit> {
    let hit = |page, control: Option<String>, label: String| Hit { page, control, label };
    let mut all: Vec<Hit> = PAGES.iter().map(|p| hit(p.id, None, p.path())).collect();
    for (control, label) in ui::ORBIT_ZOOM_CONTROLS {
        all.push(hit(ORBIT_ZOOM, Some(control.to_owned()), label.to_owned()));
    }
    for (control, label) in ui::MOUSE_CONTROLS {
        all.push(hit(MOUSE, Some(control.to_owned()), label.to_owned()));
    }
    for p in presets() {
        all.push(hit(MOUSE, Some(format!("preset.{}", p.id)), format!("{} navigation preset", p.label)));
        for b in &p.bindings {
            all.push(hit(
                MOUSE,
                Some(format!("row.{}", b.id)),
                format!("{}: {} {}", p.label, b.action.label(), b.text(false)),
            ));
        }
    }
    for v in view_keys::profiles() {
        all.push(hit(VIEW_KEYS, Some(format!("profile.{}", v.id)), format!("{} view keys", v.label)));
        for b in &v.rows {
            all.push(hit(
                VIEW_KEYS,
                Some(format!("key.{}", b.id)),
                format!("{}: {} {}", v.label, b.action.label(), b.text(false)),
            ));
        }
    }
    for (control, label) in ui::VIEW_KEY_CONTROLS {
        all.push(hit(VIEW_KEYS, Some(control.to_owned()), label.to_owned()));
    }
    all
}

/// What matches `query`, ignoring case and the spaces around it. Every word must appear.
pub fn search(query: &str) -> Vec<Hit> {
    let words: Vec<String> = query.trim().to_lowercase().split_whitespace().map(str::to_owned).collect();
    if words.is_empty() {
        return Vec::new();
    }
    index().into_iter().filter(|h| words.iter().all(|w| h.label.to_lowercase().contains(w.as_str()))).collect()
}

/// A change the window asks the app to make.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Save and use these navigation settings.
    Navigation(NavigationSettings),
}

/// A line the window shows under its page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub error: bool,
}

/// The window's state.
pub struct Preferences {
    pub page: &'static str,
    pub search: String,
    /// The control a search result pointed at.
    pub highlight: Option<String>,
    /// The navigation settings as the window shows them.
    pub draft: NavigationSettings,
    pub message: Option<Message>,
    /// The row waiting for a new gesture from the practice area.
    pub capture: Option<&'static str>,
    /// Names what the last gesture in the practice area did.
    pub practice_label: String,
    practice: Router,
    mac: bool,
    changes: Vec<Change>,
}

impl Preferences {
    /// The window on Navigation > Mouse with `settings`.
    pub fn new(settings: &NavigationSettings) -> Self {
        let mac = cfg!(target_os = "macos");
        Self {
            page: MOUSE,
            search: String::new(),
            highlight: None,
            draft: settings.clone(),
            message: None,
            capture: None,
            practice_label: "Try a gesture here".to_owned(),
            practice: Router::new(settings, mac),
            mac,
            changes: Vec::new(),
        }
    }

    /// The changes made since the last call, for the app to save.
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// Puts the draft back to what was saved, after the app could not save a change.
    pub fn revert(&mut self, saved: &NavigationSettings, why: String) {
        self.draft = saved.clone();
        self.message = Some(Message { text: why, error: true });
    }

    pub fn open_page(&mut self, id: &str) {
        if let Some(p) = page(id) {
            self.page = p.id;
        }
    }

    /// Opens a search result's page and highlights its control.
    pub fn open_hit(&mut self, hit: &Hit) {
        self.open_page(hit.page);
        self.highlight = hit.control.clone();
        self.search.clear();
    }

    pub(super) fn commit(&mut self, settings: NavigationSettings, said: Option<String>) {
        self.draft = settings;
        self.message = said.map(|text| Message { text, error: false });
        self.changes.push(Change::Navigation(self.draft.clone()));
    }

    fn refuse(&mut self, refusal: Refusal) -> Result<(), Refusal> {
        self.message = Some(Message { text: refusal.to_string(), error: true });
        Err(refusal)
    }

    /// Changes the draft with `edit` and hands it to the app.
    pub fn edit(&mut self, edit: impl FnOnce(&mut NavigationSettings)) {
        let mut s = self.draft.clone();
        edit(&mut s);
        if s != self.draft {
            self.commit(s, None);
        }
    }

    pub fn choose_preset(&mut self, id: &str) -> Result<(), Refusal> {
        let mut s = self.draft.clone();
        match s.select_preset(id) {
            Ok(()) => {
                let label = s.active_preset().label;
                self.commit(s, Some(format!("Navigation now follows {label}.")));
                Ok(())
            }
            Err(r) => self.refuse(r),
        }
    }

    /// Changes a row of the active preset. Refused, with nothing written, when the gesture
    /// is another row's.
    pub fn customize(&mut self, binding: &str, chord: Chord) -> Result<(), Refusal> {
        let mut s = self.draft.clone();
        let preset = s.active_preset().id;
        match s.set_override(preset, binding, chord) {
            Ok(()) => {
                self.commit(s, Some(format!("{binding} changed.")));
                Ok(())
            }
            Err(r) => self.refuse(r),
        }
    }

    /// Drops the active preset's changed rows; the wheel, camera and view keys stay.
    pub fn reset_preset(&mut self) {
        let mut s = self.draft.clone();
        let id = s.active_preset().id;
        if s.reset_preset(id).is_ok() {
            self.commit(s, Some("The preset's rows are back to its defaults.".into()));
        }
    }

    /// Puts the Orbit and zoom page's controls back; the preset and its rows stay.
    pub fn reset_orbit_and_zoom(&mut self) {
        let mut s = self.draft.clone();
        s.reset_orbit_and_zoom();
        self.commit(s, Some("Orbit and zoom are back to their defaults.".into()));
    }

    pub fn choose_view_keys(&mut self, id: &str) -> Result<ViewKeys, Refusal> {
        let mut s = self.draft.clone();
        match view_keys::apply_to(&mut s, id) {
            Ok(keys) => {
                self.commit(s, Some(format!("View keys now follow {}.", keys.profile.label)));
                Ok(keys)
            }
            Err(r) => {
                self.message = Some(Message { text: r.to_string(), error: true });
                Err(r)
            }
        }
    }

    pub fn reset_view_keys(&mut self) {
        let mut s = self.draft.clone();
        view_keys::reset_in(&mut s);
        self.commit(s, Some("View keys are back to SolidDraft's own.".into()));
    }

    /// Draws the window's contents.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui::show(self, ui);
    }
}

/// The Preferences window, while `prefs` holds one: draws it, saves what it changed
/// and drops it when closed.
pub fn window(ctx: &egui::Context, prefs: &mut Option<Preferences>, state: &mut crate::state::AppState) {
    let Some(p) = prefs.as_mut() else { return };
    let mut open = true;
    egui::Window::new("Preferences")
        .open(&mut open)
        .default_size([760.0, 520.0])
        .collapsible(false)
        .show(ctx, |ui| p.ui(ui));
    for change in p.take_changes() {
        match change {
            Change::Navigation(s) => {
                if let Err(e) = state.set_navigation(s) {
                    p.revert(&state.navigation, format!("Not saved: {e}"));
                }
            }
        }
    }
    if !open {
        *prefs = None;
    }
}
