//! The Preferences window's widgets: the search field, the category list, the three
//! Navigation pages and Viewport > Grid and planes.

use super::{
    GRID_AND_PLANES, Hit, MOUSE, Message, ORBIT_ZOOM, Preferences, Status, VIEW_KEYS, categories, page, search,
};
use crate::navigation::view_keys::{self, RowState};
use crate::navigation::{Action, Binding, Capability, EventKind, Outcome, Pivot, ZoomAnchor, gather, presets};
use crate::theme;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, vec2};

/// The Orbit and zoom page's controls, for search: id and label.
pub(super) const ORBIT_ZOOM_CONTROLS: [(&str, &str); 9] = [
    ("orbit_sensitivity", "Orbit sensitivity"),
    ("pan_sensitivity", "Pan sensitivity"),
    ("wheel_speed", "Wheel speed"),
    ("wheel_direction", "Wheel direction: invert"),
    ("zoom_anchor", "Zoom toward the cursor or the centre"),
    ("orbit_pivot", "Orbit pivot"),
    ("orbit_mode", "Orbit mode: turntable or free trackball"),
    ("transition", "Transition duration"),
    ("reset_page", "Reset page"),
];

/// The Mouse page's controls, for search.
pub(super) const MOUSE_CONTROLS: [(&str, &str); 5] = [
    ("preset", "Navigation preset"),
    ("bindings", "Mouse bindings: orbit, pan, zoom, fit, normal to"),
    ("practice", "Practice area"),
    ("reset_preset", "Reset preset"),
    ("limitations", "Preset limitations"),
];

/// The View keys page's controls, for search.
pub(super) const VIEW_KEY_CONTROLS: [(&str, &str); 5] = [
    ("profile", "Keyboard profile"),
    ("fit", "Fit all and fit selected keys"),
    ("normal", "Normal to and reverse normal keys"),
    ("conflicts", "Shortcut conflicts"),
    ("reset_view_keys", "Reset view keys"),
];

/// The Grid and planes page's controls, for search.
pub(super) const GRID_AND_PLANES_CONTROLS: [(&str, &str); 3] = [
    ("show_grid", "Show grid"),
    ("show_origin_planes", "Show origin planes"),
    ("always_show_origin_axes", "Always show origin axes"),
];

pub(super) fn show(p: &mut Preferences, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label("Search");
        ui.add(egui::TextEdit::singleline(&mut p.search).hint_text("Search settings").desired_width(320.0))
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search settings"));
    });
    ui.separator();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(190.0);
            categories_list(p, ui);
        });
        ui.separator();
        ui.vertical(|ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if p.search.trim().is_empty() {
                    match p.page {
                        MOUSE => mouse_page(p, ui),
                        ORBIT_ZOOM => orbit_zoom_page(p, ui),
                        VIEW_KEYS => view_keys_page(p, ui),
                        GRID_AND_PLANES => grid_and_planes_page(p, ui),
                        _ => {}
                    }
                } else {
                    results(p, ui);
                }
                if let Some(Message { text, error }) = &p.message {
                    ui.add_space(8.0);
                    let color = if *error { theme::error() } else { theme::muted() };
                    ui.label(egui::RichText::new(text).color(color));
                }
            });
        });
    });
}

fn categories_list(p: &mut Preferences, ui: &mut egui::Ui) {
    for c in categories() {
        match c.status {
            Status::Available => {
                ui.label(egui::RichText::new(c.label).strong());
                for page in super::available_pages().iter().filter(|x| x.category == c.id) {
                    if ui.selectable_label(p.page == page.id && p.search.trim().is_empty(), page.title).clicked() {
                        p.page = page.id;
                        p.search.clear();
                        p.highlight = None;
                    }
                }
            }
            Status::Reserved => {
                ui.add_enabled(false, egui::Button::new(format!("{} · coming later", c.label)).frame(false));
            }
        }
    }
}

fn results(p: &mut Preferences, ui: &mut egui::Ui) {
    let hits = search(&p.search);
    if hits.is_empty() {
        ui.label("No settings found");
        return;
    }
    let mut chosen: Option<Hit> = None;
    for hit in hits {
        let path = page(hit.page).map(|x| x.path()).unwrap_or_default();
        let text = if hit.control.is_none() { path } else { format!("{path} — {}", hit.label) };
        if ui.button(text).clicked() {
            chosen = Some(hit);
        }
    }
    if let Some(hit) = chosen {
        p.open_hit(&hit);
    }
}

/// Whether a search pointed at `control`.
fn lit(p: &Preferences, control: &str) -> bool {
    p.highlight.as_deref() == Some(control)
}

/// Draws `add`, inside an accent outline when `lit`.
fn control<R>(lit: bool, ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let stroke = if lit { Stroke::new(1.5, theme::accent()) } else { Stroke::NONE };
    egui::Frame::new().stroke(stroke).inner_margin(2.0).corner_radius(3).show(ui, add).inner
}

fn heading(ui: &mut egui::Ui, id: &str) {
    if let Some(p) = page(id) {
        ui.heading(p.path());
    }
}

/// Why a row cannot work yet, when it cannot.
fn missing(b: &Binding) -> Option<&'static str> {
    match b.requires {
        Capability::None => None,
        Capability::CameraRoll => Some("needs camera roll, which SolidDraft's camera does not have yet"),
        Capability::NumpadLocation => Some("needs the numeric keypad told apart from the number row"),
        Capability::OrderedButtons => Some("needs ordered button presses"),
    }
}

fn mouse_page(p: &mut Preferences, ui: &mut egui::Ui) {
    heading(ui, MOUSE);
    let active = p.draft.active_preset().id;
    let mut chosen = None;
    control(lit(p, "preset"), ui, |ui| {
        ui.label("Navigation preset");
        ui.horizontal_wrapped(|ui| {
            for preset in presets() {
                let lit = p.highlight.as_deref() == Some(&format!("preset.{}", preset.id)[..]);
                let text =
                    if lit { egui::RichText::new(preset.label).color(theme::accent()) } else { preset.label.into() };
                if ui.radio(active == preset.id, text).clicked() {
                    chosen = Some(preset.id);
                }
            }
        });
    });
    if let Some(id) = chosen {
        let _ = p.choose_preset(id);
    }
    let preset = p.draft.effective(p.draft.active_preset());
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(330.0);
            let mut change = None;
            control(lit(p, "bindings"), ui, |ui| {
                ui.label(egui::RichText::new("Mouse gestures").strong());
                for b in preset.bindings.iter().filter(|b| b.event != EventKind::Key) {
                    let lit = p.highlight.as_deref() == Some(&format!("row.{}", b.id)[..]);
                    ui.horizontal(|ui| {
                        let mut text = format!("{}: {}", b.action.label(), b.text(p.mac));
                        if let Some(why) = missing(b) {
                            text.push_str(&format!(" — not available: {why}"));
                        }
                        let rich = egui::RichText::new(text);
                        ui.label(if lit { rich.color(theme::accent()) } else { rich });
                        let editable = missing(b).is_none() && !matches!(b.event, EventKind::Wheel | EventKind::Pinch);
                        if editable && ui.small_button(format!("Change {}", b.id.replace('_', " "))).clicked() {
                            change = Some(b.id);
                        }
                    });
                }
            });
            if let Some(id) = change {
                p.capture = Some(id);
                p.message =
                    Some(Message { text: format!("Do the new gesture for {id} in the practice area."), error: false });
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Keyboard, in this preset's view-key profile").strong());
            if let Ok(keys) = view_keys::analyse(preset.id, Some(&p.draft), view_keys::PLATFORM) {
                for (b, state) in keys.rows.iter().filter(|(b, _)| {
                    matches!(
                        b.action,
                        Action::Fit
                            | Action::FitSelected
                            | Action::NormalTo
                            | Action::NormalToggle
                            | Action::NormalOrNearest
                    )
                }) {
                    ui.label(format!("{}: {}{}", b.action.label(), b.text(p.mac), state_text(&keys, b, *state)));
                }
            }
            ui.add_space(4.0);
            control(lit(p, "limitations"), ui, |ui| {
                ui.label(egui::RichText::new("Limitations").strong());
                for l in &preset.limitations {
                    ui.label(egui::RichText::new(l.text).color(theme::muted()));
                }
            });
            if control(lit(p, "reset_preset"), ui, |ui| ui.button("Reset preset")).clicked() {
                p.reset_preset();
            }
        });
        ui.vertical(|ui| practice(p, ui));
    });
}

fn state_text(keys: &view_keys::ViewKeys, b: &Binding, state: RowState) -> String {
    match state {
        RowState::Active => String::new(),
        RowState::Duplicate(c) => format!(" — SolidDraft's {c} already has it"),
        RowState::Supersedes(c) => format!(" — takes the key over from {c}"),
        RowState::OutsideSketch => " — outside a sketch; in a sketch the key stays the sketch's".into(),
        RowState::Unavailable(why) => format!(" — not available: {why}"),
        RowState::Blocked => {
            let owners: Vec<String> = keys
                .conflicts
                .iter()
                .filter(|c| c.row == b.id)
                .map(|c| format!("{} ({}, {})", c.label, c.command, c.scope.label()))
                .collect();
            format!(" — blocked: the key is {}", owners.join(" and "))
        }
    }
}

/// A small area to try the preset in. It reports what a gesture did and never moves the
/// part's camera. While a row waits for a new gesture, the next drag here is it.
fn practice(p: &mut Preferences, ui: &mut egui::Ui) {
    control(lit(p, "practice"), ui, |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(300.0, 170.0), Sense::click_and_drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Practice area"));
        paint_schematic(ui, rect, response.hovered());
        p.practice.configure(&p.draft, p.mac);
        let hovered = response.hovered();
        let events = ui.input(|i| gather(&i.events, i.time, |pos| hovered && rect.contains(pos), hovered));
        for e in events {
            let r = p.practice.handle(e);
            if let Some(id) = p.capture
                && p.practice.moved_past_threshold()
            {
                let chord = p.practice.gesture();
                p.capture = None;
                let _ = p.customize(id, chord);
            }
            p.practice_label = match r.outcome {
                Outcome::Action(a) => a.label().to_owned(),
                Outcome::Unsupported(a) => format!("{} (not available)", a.label()),
                Outcome::None => continue,
            };
        }
        let released: Vec<_> = p.practice.buttons().to_vec();
        for b in released {
            if !ui.input(|i| i.pointer.button_down(crate::navigation::egui_button(b))) {
                let time = ui.input(|i| i.time);
                p.practice.handle(crate::navigation::NavEvent::Release { button: b, pos: rect.center(), time });
            }
        }
        ui.label(format!("Practice: {}", p.practice_label));
    });
}

/// An original drawing of a three-button mouse, for the practice area.
fn paint_schematic(ui: &egui::Ui, rect: Rect, hovered: bool) {
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(6), theme::panel(), Stroke::new(1.0, theme::line()), StrokeKind::Inside);
    let body = Rect::from_center_size(rect.center(), vec2(54.0, 84.0));
    let ink = if hovered { theme::accent() } else { theme::muted() };
    painter.rect_stroke(body, CornerRadius::same(26), Stroke::new(1.5, ink), StrokeKind::Inside);
    let split = body.top() + 34.0;
    painter.line_segment(
        [egui::pos2(body.left() + 4.0, split), egui::pos2(body.right() - 4.0, split)],
        Stroke::new(1.0, ink),
    );
    painter.line_segment(
        [egui::pos2(body.center().x, body.top()), egui::pos2(body.center().x, split)],
        Stroke::new(1.0, ink),
    );
    let wheel = Rect::from_center_size(egui::pos2(body.center().x, body.top() + 17.0), vec2(8.0, 16.0));
    painter.rect_filled(wheel, CornerRadius::same(4), ink);
    painter.text(
        rect.left_bottom() + vec2(8.0, -8.0),
        Align2::LEFT_BOTTOM,
        "Drag or scroll here",
        theme::font(11.0),
        theme::muted(),
    );
}

/// A slider whose change is handed on when the user lets go.
fn slider(
    p: &mut Preferences,
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    range: std::ops::RangeInclusive<f32>,
    value: impl Fn(&mut crate::navigation::NavigationSettings) -> &mut f32,
) {
    let mut v = *value(&mut p.draft.clone());
    let response = control(lit(p, id), ui, |ui| ui.add(egui::Slider::new(&mut v, range).text(label)));
    if response.changed() {
        *value(&mut p.draft) = v;
    }
    // A drag saves once, when the slider is let go; a typed or stepped value at once.
    if response.drag_stopped() || (response.changed() && !response.dragged()) {
        let settled = p.draft.clone();
        p.commit(settled, None);
    }
}

fn orbit_zoom_page(p: &mut Preferences, ui: &mut egui::Ui) {
    heading(ui, ORBIT_ZOOM);
    ui.label(egui::RichText::new("These change how the camera answers, not the part.").color(theme::muted()));
    slider(p, ui, "orbit_sensitivity", "Orbit sensitivity", 0.25..=4.0, |s| &mut s.camera.orbit_sensitivity);
    slider(p, ui, "pan_sensitivity", "Pan sensitivity", 0.25..=4.0, |s| &mut s.camera.pan_sensitivity);
    slider(p, ui, "wheel_speed", "Wheel speed", 0.25..=4.0, |s| &mut s.wheel.speed);
    let mut invert = p.draft.wheel.invert;
    if control(lit(p, "wheel_direction"), ui, |ui| ui.checkbox(&mut invert, "Wheel direction: invert")).changed() {
        p.edit(|s| s.wheel.invert = invert);
    }
    let anchor = p.draft.camera.zoom_anchor;
    let mut chosen = anchor;
    control(lit(p, "zoom_anchor"), ui, |ui| {
        ui.horizontal(|ui| {
            ui.radio_value(&mut chosen, ZoomAnchor::Cursor, "Zoom toward the cursor");
            ui.radio_value(&mut chosen, ZoomAnchor::Center, "Zoom toward the centre");
        });
    });
    if chosen != anchor {
        p.edit(|s| s.camera.zoom_anchor = chosen);
    }
    control(lit(p, "orbit_pivot"), ui, |ui| {
        ui.label("Orbit pivot");
        ui.horizontal_wrapped(|ui| {
            let _ = ui.radio(p.draft.camera.pivot == Pivot::Target, "View centre");
            for (label, why) in [
                ("Under the cursor", "needs a pivot under the cursor"),
                ("Selection", "needs a pivot at the selection"),
                ("Origin", "needs a pivot at the origin"),
            ] {
                ui.add_enabled(false, egui::RadioButton::new(false, label)).on_disabled_hover_text(why);
                ui.label(egui::RichText::new(format!("({why})")).color(theme::muted()).small());
            }
        });
    });
    control(lit(p, "orbit_mode"), ui, |ui| {
        ui.horizontal(|ui| {
            let _ = ui.radio(true, "Turntable, upright");
            ui.add_enabled(false, egui::RadioButton::new(false, "Free trackball"));
            ui.label(
                egui::RichText::new("(needs a free trackball camera; not available yet)").color(theme::muted()).small(),
            );
        });
    });
    slider(p, ui, "transition", "Transition duration (s)", 0.0..=1.0, |s| &mut s.camera.transition_seconds);
    ui.horizontal(|ui| {
        if control(lit(p, "reset_page"), ui, |ui| ui.button("Reset page")).clicked() {
            p.reset_orbit_and_zoom();
        }
        if ui.button("Reset preset").clicked() {
            p.reset_preset();
        }
    });
}

fn grid_and_planes_page(p: &mut Preferences, ui: &mut egui::Ui) {
    heading(ui, GRID_AND_PLANES);
    let mut d = p.display;
    control(lit(p, "show_grid"), ui, |ui| ui.checkbox(&mut d.show_grid, "Show grid"));
    control(lit(p, "show_origin_planes"), ui, |ui| ui.checkbox(&mut d.show_origin_planes, "Show origin planes"));
    control(lit(p, "always_show_origin_axes"), ui, |ui| {
        ui.checkbox(&mut d.always_show_origin_axes, "Always show origin axes")
    });
    ui.label(
        egui::RichText::new("The origin axes also show while the origin, a plane or an axis is selected.")
            .color(theme::muted()),
    );
    p.edit_display(|display| *display = d);
}

fn view_keys_page(p: &mut Preferences, ui: &mut egui::Ui) {
    heading(ui, VIEW_KEYS);
    let current = p.draft.keyboard_profile.clone();
    let mut chosen = None;
    control(lit(p, "profile"), ui, |ui| {
        ui.label("Keyboard profile");
        ui.horizontal_wrapped(|ui| {
            for v in view_keys::profiles() {
                if ui.radio(current == v.id, v.label).clicked() {
                    chosen = Some(v.id);
                }
            }
        });
    });
    if let Some(id) = chosen {
        let _ = p.choose_view_keys(id);
    }
    let keys = view_keys::in_use(&p.draft);
    ui.add_space(6.0);
    control(lit(p, "fit"), ui, |ui| {
        for (b, state) in keys.rows.iter().filter(|(b, _)| matches!(b.action, Action::Fit | Action::FitSelected)) {
            ui.label(format!("{}: {}{}", b.action.label(), b.text(p.mac), state_text(&keys, b, *state)));
        }
    });
    control(lit(p, "normal"), ui, |ui| {
        for (b, state) in keys.rows.iter().filter(|(b, _)| {
            matches!(
                b.action,
                Action::NormalTo | Action::NormalToggle | Action::NormalOrNearest | Action::AlignTopToFace
            )
        }) {
            ui.label(format!("{}: {}{}", b.action.label(), b.text(p.mac), state_text(&keys, b, *state)));
        }
    });
    for (b, state) in keys.rows.iter().filter(|(b, _)| {
        !matches!(
            b.action,
            Action::Fit
                | Action::FitSelected
                | Action::NormalTo
                | Action::NormalToggle
                | Action::NormalOrNearest
                | Action::AlignTopToFace
        )
    }) {
        ui.label(format!("{}: {}{}", b.action.label(), b.text(p.mac), state_text(&keys, b, *state)));
    }
    if keys.rows.is_empty() {
        ui.label("SolidDraft's own keys: Z fits, N turns normal to, F, T, R and I show the views.");
    }
    ui.add_space(6.0);
    control(lit(p, "conflicts"), ui, |ui| {
        ui.label(egui::RichText::new("Shortcut conflicts").strong());
        if keys.conflicts.is_empty() {
            ui.label("None: this profile takes no command's key.");
        }
        for c in &keys.conflicts {
            ui.label(format!(
                "{}: {} is off; the key stays {} ({}, {}), which keeps its button and menu entry",
                c.key,
                c.action.label(),
                c.label,
                c.command,
                c.scope.label()
            ));
        }
    });
    for l in &keys.profile.limitations {
        ui.label(egui::RichText::new(l.text).color(theme::muted()));
    }
    if control(lit(p, "reset_view_keys"), ui, |ui| ui.button("Reset view keys")).clicked() {
        p.reset_view_keys();
    }
}
