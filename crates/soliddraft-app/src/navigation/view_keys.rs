//! View-key profiles: the keys another application uses to fit the view, turn normal to
//! a face and show the standard views, chosen apart from the mouse preset.
//!
//! A profile adds rows beside the command registry's shortcuts and never edits them.
//! Applying one checks every row against the registry: a row that does what the
//! registry's binding of the key already does is dropped as a duplicate, one that
//! extends it (Onshape's N, which also reverses) takes the key over, and one that would
//! take a key from another command is blocked and listed, so every command keeps its key
//! and its button. A key the registry uses only in a sketch stays the sketch's there.

use super::{
    Action, Binding, Capability, KeyLocation, LimitKind, Limitation, Mods, NavKey, NavigationSettings, Preset, Refusal,
    Supports, WheelPolicy, modifiers_match, presets, validate,
};
use crate::commands::{self, Scope};
use crate::document::Selection;
use crate::state::{AppAction, AppState, NoteKind};
use crate::viewport::camera::StandardView;
use egui::Key;
use glam::Vec3;
use std::sync::LazyLock;

/// The profile that keeps SolidDraft's own keys.
pub const SOLIDDRAFT: &str = "soliddraft";

/// What the platforms can deliver now. egui reports the keypad's digits and point as the
/// number row's, so no build can tell them apart and keypad rows are unavailable.
pub const PLATFORM: Supports = Supports { camera_roll: false, numpad_location: false, ordered_buttons: false };

/// A keyboard profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewKeyProfile {
    pub id: &'static str,
    pub label: &'static str,
    pub rows: Vec<Binding>,
    pub limitations: Vec<Limitation>,
}

/// The profiles: SolidDraft's own, then one per mouse preset.
pub fn profiles() -> &'static [ViewKeyProfile] {
    &PROFILES
}

pub fn profile(id: &str) -> Option<&'static ViewKeyProfile> {
    profiles().iter().find(|p| p.id == id)
}

static PROFILES: LazyLock<Vec<ViewKeyProfile>> = LazyLock::new(|| {
    let own = ViewKeyProfile {
        id: SOLIDDRAFT,
        label: "SolidDraft",
        rows: Vec::new(),
        limitations: vec![Limitation {
            kind: LimitKind::Note,
            text: "SolidDraft's own keys, as the shortcuts list shows them.",
        }],
    };
    std::iter::once(own)
        .chain(presets().iter().map(|p| ViewKeyProfile {
            id: p.id,
            label: p.label,
            rows: p.bindings.iter().filter(|b| b.key.is_some()).cloned().chain(views(p.id)).collect(),
            limitations: p.limitations.clone(),
        }))
        .collect()
});

/// The standard-view keys a profile adds to its preset's fit and normal-to keys.
fn views(id: &str) -> Vec<Binding> {
    let row = |id, key, mods, action, priority| Binding::key(id, key, mods, action, priority);
    let numbered = |mods: Mods| {
        [
            ("front", Key::Num1, Action::Front),
            ("back", Key::Num2, Action::Back),
            ("left", Key::Num3, Action::Left),
            ("right", Key::Num4, Action::Right),
            ("top", Key::Num5, Action::Top),
            ("bottom", Key::Num6, Action::Bottom),
            ("isometric", Key::Num7, Action::Isometric),
        ]
        .into_iter()
        .zip(40..)
        .map(move |((id, key, action), priority)| row(id, key, mods, action, priority))
        .collect()
    };
    match id {
        "solidworks" => numbered(Mods::CTRL),
        "onshape" => numbered(Mods::SHIFT),
        "solid_edge" => vec![
            row("front", Key::F, Mods::NONE, Action::Front, 40),
            row("right", Key::R, Mods::NONE, Action::Right, 41),
            row("isometric", Key::I, Mods::NONE, Action::Isometric, 42),
            row("top", Key::T, Mods::CTRL, Action::Top, 43),
            row("bottom", Key::B, Mods::NONE, Action::Bottom, 44),
            row("back", Key::K, Mods::NONE, Action::Back, 45),
            row("left", Key::L, Mods::NONE, Action::Left, 46),
            row("dimetric", Key::J, Mods::NONE, Action::Dimetric, 47),
            row("trimetric", Key::M, Mods::NONE, Action::Trimetric, 48),
            row("home", Key::Home, Mods::NONE, Action::Isometric, 49),
        ],
        "blender" => [
            ("front", Key::Num1, Mods::NONE, Action::Front),
            ("back", Key::Num1, Mods::CTRL, Action::Back),
            ("right", Key::Num3, Mods::NONE, Action::Right),
            ("left", Key::Num3, Mods::CTRL, Action::Left),
            ("top", Key::Num7, Mods::NONE, Action::Top),
            ("bottom", Key::Num7, Mods::CTRL, Action::Bottom),
            ("projection", Key::Num5, Mods::NONE, Action::ProjectionToggle),
            ("align_top", Key::Num7, Mods::SHIFT, Action::AlignTopToFace),
        ]
        .into_iter()
        .zip(40..)
        .map(|((id, key, mods, action), priority)| {
            row(id, key, mods, action, priority).on_numpad().needs(Capability::NumpadLocation)
        })
        .collect(),
        _ => Vec::new(),
    }
}

/// Where a key code from the platform is: the codes winit and the browser use,
/// `"Numpad1"` on the keypad and `"Digit1"` on the number row. The adapter a build uses
/// to place keys; egui itself does not pass the code on.
pub fn location_from_code(code: &str) -> KeyLocation {
    if code.starts_with("Numpad") {
        KeyLocation::Numpad
    } else if code.is_empty() || code == "Unidentified" {
        KeyLocation::Unknown
    } else {
        KeyLocation::Main
    }
}

/// How a row stands against the command registry and the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Active,
    /// The registry already binds the key to the same thing; it keeps running it.
    Duplicate(&'static str),
    /// The row does what the registry's command does and more, and takes the key.
    Supersedes(&'static str),
    /// The key is another command's; the row is off and listed as a conflict.
    Blocked,
    /// Active outside a sketch; in a sketch the key stays the sketch command's.
    OutsideSketch,
    /// The platform cannot deliver what the row needs, for the reason given.
    Unavailable(&'static str),
}

/// A row that would take a key from a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    pub row: &'static str,
    /// As the interface writes the key: "F", "Ctrl+8".
    pub key: String,
    pub action: Action,
    pub command: &'static str,
    /// The command's label, which names its button and menu entry.
    pub label: &'static str,
    pub scope: Scope,
}

/// A profile checked against the registry and the platform.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewKeys {
    pub profile: &'static ViewKeyProfile,
    pub rows: Vec<(Binding, RowState)>,
    pub conflicts: Vec<Conflict>,
}

/// The registry command that does exactly what an action does.
fn same_command(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::Fit => "zoom_fit",
        Action::NormalTo => "normal_to",
        Action::Front => "view_front",
        Action::Back => "view_back",
        Action::Left => "view_left",
        Action::Right => "view_right",
        Action::Top => "view_top",
        Action::Bottom => "view_bottom",
        Action::Isometric => "view_isometric",
        _ => return None,
    })
}

/// The registry command an action does more than.
fn extends(action: Action) -> Option<&'static str> {
    match action {
        Action::NormalToggle | Action::NormalOrNearest => Some("normal_to"),
        _ => None,
    }
}

/// The scopes of the part's 3D view.
fn in_part_view(scope: Scope) -> bool {
    matches!(scope, Scope::App | Scope::Part | Scope::Model | Scope::Edge | Scope::Sketch)
}

/// The profile with the user's changes to its rows.
fn effective(p: &'static ViewKeyProfile, settings: Option<&NavigationSettings>) -> Vec<Binding> {
    let changes = settings.and_then(|s| s.view_key_overrides.get(p.id));
    p.rows
        .iter()
        .map(|b| match changes.and_then(|c| c.get(b.id)) {
            Some(chord) => b.with_chord(chord),
            None => b.clone(),
        })
        .collect()
}

/// Checks profile `id` against the registry and what the platform supports.
pub fn analyse(id: &str, settings: Option<&NavigationSettings>, supports: Supports) -> Result<ViewKeys, Refusal> {
    let profile = profile(id).ok_or_else(|| Refusal::UnknownProfile(id.to_owned()))?;
    let registry: Vec<(&'static commands::CommandSpec, commands::Shortcut)> = commands::all()
        .iter()
        .flat_map(|c| c.keys.iter().map(move |k| (c, *k)))
        .filter(|(_, k)| in_part_view(k.scope))
        .collect();
    let mut rows = Vec::new();
    let mut conflicts = Vec::new();
    for row in effective(profile, settings) {
        let key = row.text(cfg!(target_os = "macos"));
        let state = if !supports_row(&row, supports) {
            let reason = profile
                .limitations
                .iter()
                .find(|l| l.kind == LimitKind::NeedsNumpadLocation)
                .map_or("this platform cannot deliver the key", |l| l.text);
            RowState::Unavailable(reason)
        } else {
            let taken: Vec<_> = registry.iter().filter(|(_, k)| chord_is(&row, k)).collect();
            if let Some((c, _)) = taken.iter().find(|(c, _)| same_command(row.action) == Some(c.id)) {
                RowState::Duplicate(c.id)
            } else if let Some((c, _)) = taken.iter().find(|(c, _)| extends(row.action) == Some(c.id)) {
                RowState::Supersedes(c.id)
            } else if taken.is_empty() {
                RowState::Active
            } else {
                for (c, k) in &taken {
                    conflicts.push(Conflict {
                        row: row.id,
                        key: key.clone(),
                        action: row.action,
                        command: c.id,
                        label: c.label,
                        scope: k.scope,
                    });
                }
                if taken.iter().all(|(_, k)| k.scope == Scope::Sketch) {
                    RowState::OutsideSketch
                } else {
                    RowState::Blocked
                }
            }
        };
        rows.push((row, state));
    }
    Ok(ViewKeys { profile, rows, conflicts })
}

fn supports_row(row: &Binding, supports: Supports) -> bool {
    match row.requires {
        Capability::None => true,
        Capability::NumpadLocation => supports.numpad_location,
        Capability::CameraRoll => supports.camera_roll,
        Capability::OrderedButtons => supports.ordered_buttons,
    }
}

/// Whether a row and a registry shortcut are the same key and modifiers.
fn chord_is(row: &Binding, key: &commands::Shortcut) -> bool {
    let m = key.modifiers;
    row.key.is_some_and(|k| k.key == key.key)
        && (row.required.ctrl || row.required.platform) == (m.ctrl || m.command)
        && row.required.shift == m.shift
        && row.required.alt == m.alt
}

impl ViewKeys {
    /// The row a key press runs, if any: active rows, rows that took a key over, and in
    /// a sketch none that leave the key to it. Modifiers match exactly; a keypad row needs
    /// a key known to be on the keypad, and a main-row key also matches one egui could not
    /// place.
    pub fn row_for(&self, key: NavKey, modifiers: egui::Modifiers, sketching: bool, mac: bool) -> Option<&Binding> {
        self.rows
            .iter()
            .filter(|(_, s)| match s {
                RowState::Active | RowState::Supersedes(_) => true,
                RowState::OutsideSketch => !sketching,
                _ => false,
            })
            .map(|(b, _)| b)
            .filter(|b| {
                b.key.is_some_and(|k| {
                    k.key == key.key
                        && match k.location {
                            KeyLocation::Numpad => key.location == KeyLocation::Numpad,
                            _ => key.location != KeyLocation::Numpad,
                        }
                })
            })
            .filter(|b| modifiers_match(b, modifiers, mac))
            .max_by_key(|b| b.priority)
    }

    /// Registry commands whose key a row took over: the key no longer runs them.
    pub fn superseded(&self) -> impl Iterator<Item = (&'static str, Key)> + '_ {
        self.rows.iter().filter_map(|(b, s)| match s {
            RowState::Supersedes(command) => Some((*command, b.key?.key)),
            _ => None,
        })
    }

    /// The rows the profile cannot use, with why.
    pub fn state_of(&self, row: &str) -> Option<RowState> {
        self.rows.iter().find(|(b, _)| b.id == row).map(|(_, s)| *s)
    }
}

/// Makes `id` the view-key profile in `settings`. Unknown profiles are refused and change
/// nothing.
pub fn apply_to(settings: &mut NavigationSettings, id: &str) -> Result<ViewKeys, Refusal> {
    let keys = analyse(id, Some(settings), PLATFORM)?;
    settings.keyboard_profile = keys.profile.id.to_owned();
    Ok(keys)
}

/// Back to SolidDraft's own keys, dropping changes to view-key rows and nothing else.
pub fn reset_in(settings: &mut NavigationSettings) {
    settings.keyboard_profile = SOLIDDRAFT.to_owned();
    settings.view_key_overrides.clear();
}

/// Applies profile `id` to the user's settings and saves them once.
pub fn apply(id: &str) -> Result<ViewKeys, Refusal> {
    let mut settings = crate::settings::navigation(crate::settings::install_kind());
    let keys = apply_to(&mut settings, id)?;
    crate::settings::save_navigation(&settings).map_err(Refusal::Save)?;
    Ok(keys)
}

/// Resets the user's view keys and saves the settings once.
pub fn reset() -> Result<(), Refusal> {
    let mut settings = crate::settings::navigation(crate::settings::install_kind());
    reset_in(&mut settings);
    crate::settings::save_navigation(&settings).map_err(Refusal::Save)
}

/// The view keys in use: the settings' profile, or SolidDraft's own when it is unknown.
pub fn in_use(settings: &NavigationSettings) -> ViewKeys {
    analyse(&settings.keyboard_profile, Some(settings), PLATFORM)
        .or_else(|_| analyse(SOLIDDRAFT, Some(settings), PLATFORM))
        .expect("SolidDraft's profile exists")
}

/// A profile's rows as a preset, for [`validate`].
pub fn as_preset(p: &ViewKeyProfile) -> Preset {
    Preset {
        id: p.id,
        label: p.label,
        release_scope: "View keys.",
        wheel: WheelPolicy::SoliddraftDefault,
        bindings: p.rows.clone(),
        limitations: p.limitations.clone(),
    }
}

/// Checks every profile's rows.
pub fn validate_all() -> Result<(), Vec<super::SchemaError>> {
    profiles().iter().try_for_each(|p| validate(&as_preset(p)))
}

/// The normal of the selected planar face, or the open sketch's plane.
fn selected_normal(state: &AppState) -> Option<Vec3> {
    let n = match (&state.sketch, state.doc.selection) {
        (Some(s), _) => s.plane.normal,
        (None, Some(Selection::Face(f))) => state.doc.face_plane(f)?.normal,
        _ => return None,
    };
    Some(Vec3::new(n.x as f32, n.y as f32, n.z as f32))
}

/// The standard view whose direction is nearest the camera's.
fn nearest_principal(heading: Vec3) -> StandardView {
    [
        (StandardView::Front, Vec3::NEG_Y),
        (StandardView::Back, Vec3::Y),
        (StandardView::Right, Vec3::X),
        (StandardView::Left, Vec3::NEG_X),
        (StandardView::Top, Vec3::Z),
        (StandardView::Bottom, Vec3::NEG_Z),
    ]
    .into_iter()
    .max_by(|a, b| heading.dot(a.1).total_cmp(&heading.dot(b.1)))
    .map(|(v, _)| v)
    .expect("six views")
}

/// Runs a view key's action.
pub fn run(state: &mut AppState, action: Action) {
    let view = AppAction::SetView;
    match action {
        Action::Fit => state.push(AppAction::ZoomFit),
        Action::FitSelected => match state.doc.selection {
            Some(_) => {
                let bounds = state.doc.bounds();
                state.viewport.camera.fit(bounds);
            }
            None => state.note("Fit selected: select something to fit", NoteKind::Info),
        },
        Action::NormalTo => state.push(AppAction::NormalTo),
        Action::NormalToggle => match selected_normal(state) {
            // Already normal to it: the same key looks from the other side.
            Some(n) => {
                let camera = &mut state.viewport.camera;
                if camera.heading().dot(n) > 0.999 { camera.look_along(-n) } else { camera.look_along(n) }
            }
            None => state.push(AppAction::NormalTo),
        },
        Action::NormalOrNearest => match selected_normal(state) {
            Some(n) => state.viewport.camera.look_along(n),
            None => {
                let v = nearest_principal(state.viewport.camera.heading());
                state.push(view(v));
            }
        },
        Action::AlignTopToFace => match selected_normal(state) {
            Some(n) => state.viewport.camera.look_along(n),
            None => state.note("Align view: select a planar face first", NoteKind::Info),
        },
        Action::Front => state.push(view(StandardView::Front)),
        Action::Back => state.push(view(StandardView::Back)),
        Action::Left => state.push(view(StandardView::Left)),
        Action::Right => state.push(view(StandardView::Right)),
        Action::Top => state.push(view(StandardView::Top)),
        Action::Bottom => state.push(view(StandardView::Bottom)),
        Action::Isometric | Action::DefaultView => state.push(view(StandardView::Isometric)),
        Action::Dimetric => state.viewport.camera.turn_to("Dimetric", -45.0, 20.705),
        Action::Trimetric => state.viewport.camera.turn_to("Trimetric", -30.0, 25.0),
        Action::ProjectionToggle => state.push(AppAction::ToggleProjection),
        Action::Orbit | Action::Pan | Action::DragZoom | Action::Roll | Action::WheelZoom | Action::Unknown => {}
    }
}
