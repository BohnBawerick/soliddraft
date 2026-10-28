//! PropertyManager: the open sketch, the feature editor while a feature is
//! being created or edited, otherwise the selected entity's properties.

use crate::document::{
    Command, EndKind, FeatureId, FeatureKind, FeatureStatus, PlaneKind, ProfileKind, Selection, SketchPlane, fmt_mm,
    plane_ref_label,
};
use crate::drag::Dimension;
use crate::icons::{self, Icon};
use crate::sketch::{ConstraintKind, EntityKind, fmt_value, solve};
use crate::sketcher;
use crate::state::{AppAction, AppState, DragStep, EditorParams, ExitInto, SketchSel, Tool};
use crate::theme;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, pos2, vec2};
use soliddraft_kernel_api::{HoleDepth, HoleHead, HoleTip, Vec3};
use soliddraft_model::{DatumPlane, PatternSpec, PlaneRef};

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    if state.sketch.is_some() {
        sketch_panel(ui, state);
    } else if state.editor.is_some() {
        editor(ui, state);
    } else {
        selection(ui, state);
    }
}

fn header(ui: &mut egui::Ui, icon: Icon, title: &str) -> Rect {
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    icons::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(pos2(head.left() + 20.0, head.center().y), vec2(16.0, 16.0)),
        theme::ink(),
    );
    ui.painter().text(
        pos2(head.left() + 34.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::font_semibold(12.5),
        theme::ink(),
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::line()));
    head
}

fn small_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, tip: &str, primary: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("pm_btn", tip)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    let (bg, fg) = if primary {
        (if resp.hovered() { theme::accent().gamma_multiply(0.9) } else { theme::accent() }, theme::on_accent())
    } else {
        (if resp.hovered() { theme::line() } else { theme::hover() }, theme::ink())
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    icons::paint(ui.painter(), icon, rect.shrink(4.0), fg);
    resp.on_hover_text(tip)
}

/// OK and Cancel in the header; returns (ok, cancel).
fn ok_cancel(ui: &mut egui::Ui, head: Rect, ok_tip: &str, cancel_tip: &str) -> (bool, bool) {
    let ok = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 42.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Check,
        ok_tip,
        true,
    );
    let cancel = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        cancel_tip,
        false,
    );
    (ok.clicked(), cancel.clicked())
}

fn field_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::field_bg())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 3))
}

pub fn mm_field(ui: &mut egui::Ui, label: &str, value: &mut f64) -> bool {
    unit_field(ui, label, value, "mm")
}

fn unit_field(ui: &mut egui::Ui, label: &str, value: &mut f64, unit: &str) -> bool {
    unit_value(ui, label, value, unit, true).changed()
}

/// A labelled number field with its unit. Without `update_while_editing`, a typed value
/// counts only when Enter or a click elsewhere ends the typing.
fn unit_value(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    unit: &str,
    update_while_editing: bool,
) -> egui::Response {
    let mut response = None;
    ui.horizontal(|ui| {
        ui.add_sized(vec2(58.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0)));
        field_frame().show(ui, |ui| {
            ui.set_width(ui.available_width() - 16.0);
            ui.horizontal(|ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.hovered.bg_fill = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
                ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                let r = ui.add(
                    egui::DragValue::new(value)
                        .speed(0.5)
                        .range(0.01..=100_000.0)
                        .fixed_decimals(2)
                        .update_while_editing(update_while_editing),
                );
                // Named apart from the caption so automation can find the field itself.
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, format!("{label} value"))
                });
                response = Some(r);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(unit).size(11.0).italics().color(theme::muted()));
                });
            });
        });
    });
    response.expect("the field is always added")
}

/// The selected feature's dimension. Dragging it previews the part off the interface
/// thread and applies the value where the drag ends as one undo step; a typed value
/// applies on Enter.
fn dimension_field(ui: &mut egui::Ui, state: &mut AppState, id: FeatureId, kind: &FeatureKind) {
    let Some(dimension) = Dimension::of(kind) else { return };
    let Some(applied) = dimension.get(kind) else { return };
    let mut value = match &state.drag_preview {
        Some(d) if d.feature() == id => d.value(),
        _ => applied,
    };
    ui.add_space(8.0);
    let r = unit_value(ui, dimension.label(), &mut value, "mm", false);
    if r.drag_started() {
        state.push(AppAction::DragDimension(DragStep::Start { feature: id, dimension }));
    }
    if r.changed() {
        if r.dragged() || r.drag_started() {
            state.push(AppAction::DragDimension(DragStep::Move(value)));
        } else if value != applied
            && let Some(command) = crate::drag::edit_command(id, dimension.with(kind, value))
        {
            state.push(AppAction::Command(command));
        }
    }
    if r.drag_stopped() {
        state.push(AppAction::DragDimension(DragStep::Finish));
    }
    if let Some(drag) = &state.drag_preview
        && drag.feature() == id
    {
        let (message, failed) = drag.label();
        ui.label(egui::RichText::new(message).size(11.0).color(if failed { theme::error() } else { theme::muted() }));
    } else if let Some((message, failed)) = state.doc.proof_message() {
        ui.label(egui::RichText::new(message).size(11.0).color(if failed { theme::error() } else { theme::muted() }));
    }
}

/// The end condition of an extrude or cut. Through all is offered to cuts only, and
/// the up-to conditions need a body to stop against.
fn end_combo(ui: &mut egui::Ui, id: &str, end: &mut EndKind, cut: bool, up_to: bool) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(58.0, 22.0), egui::Label::new(egui::RichText::new("End").size(12.0)));
        egui::ComboBox::from_id_salt(id).width(ui.available_width() - 20.0).selected_text(end.label()).show_ui(
            ui,
            |ui| {
                let mut options = vec![EndKind::Blind];
                if cut {
                    options.push(EndKind::ThroughAll);
                }
                if up_to {
                    options.push(EndKind::UpToNext);
                    options.push(EndKind::UpToFace(None));
                }
                for option in options {
                    let same = std::mem::discriminant(&option) == std::mem::discriminant(end);
                    if ui.selectable_label(same, option.label()).clicked() && !same {
                        *end = option;
                    }
                }
            },
        );
    });
}

fn pick_prompt(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::field_bg())
        .stroke(Stroke::new(1.0, theme::accent().gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).size(11.5).color(theme::accent()));
        });
}

fn picked_face(ui: &mut egui::Ui, face: Option<&crate::document::FaceRef>) {
    match face {
        Some(f) => {
            ui.label(egui::RichText::new(&f.label).size(12.0));
        }
        None => pick_prompt(ui, "Click the face to stop at"),
    }
}

fn coord(ui: &mut egui::Ui, id: &str, label: &str, value: &mut f64) {
    ui.label(egui::RichText::new(label).size(11.0).color(theme::muted()));
    let r = ui.add(egui::DragValue::new(value).speed(0.5).range(-100_000.0..=100_000.0).fixed_decimals(2));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, format!("{id} value")));
}

fn count_field(ui: &mut egui::Ui, count: &mut u32) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(58.0, 22.0), egui::Label::new(egui::RichText::new("Copies").size(12.0)));
        ui.add(egui::DragValue::new(count).range(2..=1000));
    });
}

/// An angle with standard choices and a free value.
fn standard_angle(ui: &mut egui::Ui, id: &str, label: &str, angle: &mut f64, choices: &[(f64, &str)]) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(58.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0)));
        let text =
            choices.iter().find(|(v, _)| v == angle).map_or(format!("{}°", fmt_mm(*angle)), |(_, t)| (*t).into());
        egui::ComboBox::from_id_salt(id).selected_text(text).show_ui(ui, |ui| {
            for (value, text) in choices {
                ui.selectable_value(angle, *value, *text);
            }
        });
        if *angle != 0.0 {
            ui.add(egui::DragValue::new(angle).speed(0.5).range(1.0..=179.0).suffix("°"));
        }
    });
}

/// One of the world axes as a direction.
fn axis_combo(ui: &mut egui::Ui, id: &str, label: &str, direction: &mut Vec3) {
    let axes = [(Vec3::X, "X"), (Vec3::Y, "Y"), (Vec3::Z, "Z"), (-Vec3::X, "−X"), (-Vec3::Y, "−Y"), (-Vec3::Z, "−Z")];
    ui.horizontal(|ui| {
        ui.add_sized(vec2(58.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0)));
        let text = axes.iter().find(|(v, _)| v == direction).map_or("Custom", |(_, t)| t);
        egui::ComboBox::from_id_salt(id).selected_text(text).show_ui(ui, |ui| {
            for (value, text) in axes {
                ui.selectable_value(direction, value, text);
            }
        });
    });
}

/// A reference plane choice; a picked face shows by its label.
fn plane_combo(ui: &mut egui::Ui, id: &str, plane: &mut PlaneRef) {
    egui::ComboBox::from_id_salt(id).width(ui.available_width() - 4.0).selected_text(plane_ref_label(plane)).show_ui(
        ui,
        |ui| {
            for datum in [DatumPlane::Top, DatumPlane::Front, DatumPlane::Right] {
                let chosen = matches!(plane, PlaneRef::Datum { datum: d, .. } if *d == datum);
                if ui.selectable_label(chosen, datum.label()).clicked() {
                    *plane = PlaneRef::Datum { datum, offset: 0.0 };
                }
            }
        },
    );
}

fn section_gap(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    theme::hline(ui);
    ui.add_space(6.0);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(theme::muted()));
}

fn error_row(ui: &mut egui::Ui, err: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        icons::paint(ui.painter(), Icon::Warning, r, theme::error());
        ui.label(egui::RichText::new(err).size(11.5).color(theme::error()));
    });
}

fn editor(ui: &mut egui::Ui, state: &mut AppState) {
    let (title, icon) = match state.editor.as_ref().map(|e| &e.params) {
        Some(EditorParams::Revolve(_)) => (state.editor.as_ref().unwrap().title(), Icon::Revolve),
        Some(EditorParams::Extrude(_)) => (state.editor.as_ref().unwrap().title(), Icon::Extrude),
        Some(EditorParams::Cut(_)) => (state.editor.as_ref().unwrap().title(), Icon::Cut),
        Some(EditorParams::Fillet(_)) => (state.editor.as_ref().unwrap().title(), Icon::Fillet),
        Some(EditorParams::Chamfer(_)) => (state.editor.as_ref().unwrap().title(), Icon::Chamfer),
        Some(EditorParams::Hole(_)) => (state.editor.as_ref().unwrap().title(), Icon::Hole),
        Some(EditorParams::Pattern(p)) => (
            state.editor.as_ref().unwrap().title(),
            if matches!(p.layout, PatternSpec::Linear { .. }) { Icon::LinearPattern } else { Icon::CircularPattern },
        ),
        Some(EditorParams::Mirror(_)) => (state.editor.as_ref().unwrap().title(), Icon::Mirror),
        Some(EditorParams::Draft(_)) => (state.editor.as_ref().unwrap().title(), Icon::Draft),
        Some(EditorParams::Shell(_)) => (state.editor.as_ref().unwrap().title(), Icon::Shell),
        Some(EditorParams::PickPlane) => ("Sketch".to_owned(), Icon::Sketch),
        Some(EditorParams::Repair { .. }) => ("Repair references".to_owned(), Icon::Repair),
        None => return,
    };
    let head = header(ui, icon, &title);
    let pick_plane = matches!(state.editor.as_ref().map(|e| &e.params), Some(EditorParams::PickPlane));
    let (ok, cancel) = if pick_plane {
        let cancel = small_button(
            ui,
            Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
            Icon::Close,
            "Cancel (Esc)",
            false,
        );
        (false, cancel.clicked())
    } else {
        ok_cancel(ui, head, "OK (Enter)", "Cancel (Esc)")
    };
    if ok {
        state.push(AppAction::CommitEditor);
    }
    if cancel {
        state.push(AppAction::CancelEditor);
    }

    let sketch_names: Vec<(crate::document::FeatureId, String)> =
        state.doc.features.iter().filter(|f| f.is_sketch()).map(|f| (f.id, f.name.clone())).collect();
    let mut actions = Vec::new();
    let Some(editor) = state.editor.as_mut() else { return };
    egui::ScrollArea::vertical().id_salt("pm").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| match &mut editor.params {
            EditorParams::Revolve(r) => {
                theme::section_label(ui, "Meridian sketch");
                ui.label(
                    sketch_names
                        .iter()
                        .find(|(id, _)| *id == r.sketch.0 as u64)
                        .map(|(_, name)| name.as_str())
                        .unwrap_or("Missing sketch"),
                );
                theme::section_label(ui, "Axis point");
                for (i, label) in ["X", "Y", "Z"].iter().enumerate() {
                    coord(ui, &format!("revolve origin {label}"), label, &mut r.axis_origin[i]);
                }
                theme::section_label(ui, "Axis direction");
                for (i, label) in ["X", "Y", "Z"].iter().enumerate() {
                    coord(ui, &format!("revolve direction {label}"), label, &mut r.axis_direction[i]);
                }
                theme::section_label(ui, "Angle");
                ui.add(egui::DragValue::new(&mut r.angle_deg).speed(1.0).range(0.0..=360.0).suffix("°"));
            }
            EditorParams::Extrude(params) => {
                theme::section_label(ui, "Profile");
                match params.sketch {
                    Some(id) => {
                        let name = sketch_names
                            .iter()
                            .find(|(i, _)| *i == id)
                            .map(|(_, n)| n.clone())
                            .unwrap_or("Sketch".into());
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            icons::paint(ui.painter(), Icon::Sketch, r, theme::ink());
                            ui.label(egui::RichText::new(name).size(12.0));
                        });
                    }
                    None => {
                        ui.horizontal(|ui| {
                            let is_rect = matches!(params.profile, ProfileKind::Rectangle { .. });
                            if ui.selectable_label(is_rect, "Rectangle").clicked() && !is_rect {
                                params.profile = ProfileKind::Rectangle { width: 60.0, height: 40.0 };
                            }
                            if ui.selectable_label(!is_rect, "Circle").clicked() && is_rect {
                                params.profile = ProfileKind::Circle { diameter: 24.0 };
                            }
                        });
                        ui.add_space(2.0);
                        match &mut params.profile {
                            ProfileKind::Rectangle { width, height } => {
                                mm_field(ui, "Width", width);
                                mm_field(ui, "Height", height);
                            }
                            ProfileKind::Circle { diameter } => {
                                mm_field(ui, "Diameter", diameter);
                            }
                        }
                        section_gap(ui);
                        theme::section_label(ui, "Sketch plane");
                        egui::ComboBox::from_id_salt("plane")
                            .width(ui.available_width() - 4.0)
                            .selected_text(state.doc.model().datum_name(params.plane.to_model()))
                            .show_ui(ui, |ui| {
                                for p in PlaneKind::ALL {
                                    ui.selectable_value(
                                        &mut params.plane,
                                        p,
                                        state.doc.model().datum_name(p.to_model()),
                                    );
                                }
                            });
                    }
                }
                section_gap(ui);
                theme::section_label(ui, "Direction 1");
                let mut end = params.end.clone();
                end_combo(ui, "extrude_end", &mut end, false, params.sketch.is_some());
                params.end = end;
                match &params.end {
                    EndKind::Blind => {
                        mm_field(ui, "Depth", &mut params.depth);
                    }
                    EndKind::UpToFace(face) => picked_face(ui, face.as_ref()),
                    _ => {}
                }
                ui.checkbox(&mut params.flip, "Flip direction");
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "The ghost in the viewport previews the added material. Enter applies, Esc cancels.");
            }
            EditorParams::Cut(params) => {
                theme::section_label(ui, "Profile");
                let name = sketch_names
                    .iter()
                    .find(|(i, _)| *i == params.sketch)
                    .map(|(_, n)| n.clone())
                    .unwrap_or("Sketch".into());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Sketch, r, theme::ink());
                    ui.label(egui::RichText::new(name).size(12.0));
                });
                section_gap(ui);
                theme::section_label(ui, "Direction 1");
                let mut end = params.end();
                end_combo(ui, "cut_end", &mut end, true, true);
                if end != params.end() {
                    params.set_end(end);
                }
                match params.end() {
                    EndKind::Blind => {
                        mm_field(ui, "Depth", &mut params.depth);
                    }
                    EndKind::UpToFace(face) => picked_face(ui, face.as_ref()),
                    _ => {}
                }
                ui.checkbox(&mut params.flip, "Flip direction");
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "The material to remove shows in red; the body previews the result.");
            }
            EditorParams::Fillet(_) | EditorParams::Chamfer(_) => {
                let chamfer = matches!(editor.params, EditorParams::Chamfer(_));
                theme::section_label(ui, "Items to round");
                let edges = editor.edges().to_vec();
                if edges.is_empty() {
                    egui::Frame::new()
                        .fill(theme::field_bg())
                        .stroke(Stroke::new(1.0, theme::accent().gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                egui::RichText::new("Click edges in the viewport").size(11.5).color(theme::accent()),
                            );
                        });
                }
                let mut remove = None;
                for (i, e) in edges.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                        icons::paint(ui.painter(), Icon::Axis, r, theme::ink());
                        ui.label(egui::RichText::new(&e.label).size(12.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new("×").small().frame(false)).on_hover_text("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if let (Some(i), Some(list)) = (remove, editor.edges_mut()) {
                    list.remove(i);
                }
                section_gap(ui);
                theme::section_label(ui, if chamfer { "Chamfer parameters" } else { "Fillet parameters" });
                match &mut editor.params {
                    EditorParams::Fillet(f) => {
                        mm_field(ui, "Radius", &mut f.radius);
                    }
                    EditorParams::Chamfer(c) => {
                        mm_field(ui, "Distance", &mut c.distance);
                    }
                    _ => {}
                }
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "Edges stay picked across a rebuild: each one is named by the feature that made it.");
            }
            EditorParams::Hole(h) => {
                theme::section_label(ui, "Placement");
                match &h.face {
                    Some(face) => {
                        ui.label(egui::RichText::new(&face.label).size(12.0));
                    }
                    None => pick_prompt(ui, "Click the planar face the holes start on"),
                }
                hint(ui, "Centres in the face's sketch frame, mm.");
                let mut remove = None;
                for (i, p) in h.points.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{}", i + 1)).size(12.0).color(theme::muted()));
                        coord(ui, &format!("hole_u{i}"), "u", &mut p.u);
                        coord(ui, &format!("hole_v{i}"), "v", &mut p.v);
                        if ui.add(egui::Button::new("×").small().frame(false)).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove.filter(|_| h.points.len() > 1) {
                    h.points.remove(i);
                }
                if ui.button("Add point").clicked() {
                    let next = h.points.last().copied().unwrap_or_default();
                    h.points.push(crate::sketch::P2::new(next.u + 10.0, next.v));
                }
                section_gap(ui);
                theme::section_label(ui, "Hole");
                mm_field(ui, "Diameter", &mut h.diameter);
                let through = matches!(h.depth, HoleDepth::Through);
                ui.horizontal(|ui| {
                    if ui.selectable_label(through, "Through").clicked() {
                        h.depth = HoleDepth::Through;
                    }
                    if ui.selectable_label(!through, "Blind").clicked() && through {
                        h.depth = HoleDepth::Blind { depth: 10.0 };
                    }
                });
                if let HoleDepth::Blind { depth } = &mut h.depth {
                    mm_field(ui, "Depth", depth);
                    let mut angle = match h.tip {
                        HoleTip::Flat => 0.0,
                        HoleTip::Cone { angle_deg } => angle_deg,
                    };
                    standard_angle(ui, "hole_tip", "Tip", &mut angle, &[(0.0, "Flat"), (118.0, "118°"), (90.0, "90°")]);
                    h.tip = if angle == 0.0 { HoleTip::Flat } else { HoleTip::Cone { angle_deg: angle } };
                }
                section_gap(ui);
                theme::section_label(ui, "Head");
                let kind = match h.head {
                    None => 0,
                    Some(HoleHead::Counterbore { .. }) => 1,
                    Some(HoleHead::Countersink { .. }) => 2,
                };
                ui.horizontal(|ui| {
                    if ui.selectable_label(kind == 0, "None").clicked() {
                        h.head = None;
                    }
                    if ui.selectable_label(kind == 1, "Counterbore").clicked() && kind != 1 {
                        h.head = Some(HoleHead::Counterbore { diameter: h.diameter * 1.8, depth: h.diameter });
                    }
                    if ui.selectable_label(kind == 2, "Countersink").clicked() && kind != 2 {
                        h.head = Some(HoleHead::Countersink { diameter: h.diameter * 2.0, angle_deg: 90.0 });
                    }
                });
                match &mut h.head {
                    Some(HoleHead::Counterbore { diameter, depth }) => {
                        mm_field(ui, "Diameter", diameter);
                        mm_field(ui, "Depth", depth);
                    }
                    Some(HoleHead::Countersink { diameter, angle_deg }) => {
                        mm_field(ui, "Diameter", diameter);
                        standard_angle(
                            ui,
                            "hole_sink",
                            "Angle",
                            angle_deg,
                            &[(82.0, "82°"), (90.0, "90°"), (100.0, "100°"), (120.0, "120°")],
                        );
                    }
                    None => {}
                }
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "Standard angles: 118° drill tips; 82°, 90° and 100° countersinks.");
            }
            EditorParams::Pattern(p) => {
                theme::section_label(ui, "Layout");
                match &mut p.layout {
                    PatternSpec::Linear { direction, spacing, count } => {
                        axis_combo(ui, "pattern_dir", "Direction", direction);
                        mm_field(ui, "Spacing", spacing);
                        count_field(ui, count);
                    }
                    PatternSpec::Circular { axis_direction, count, angle, .. } => {
                        axis_combo(ui, "pattern_axis", "Axis", axis_direction);
                        count_field(ui, count);
                        unit_field(ui, "Angle", angle, "°");
                    }
                }
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "Copies the part's body and unites the copies with it.");
            }
            EditorParams::Mirror(m) => {
                theme::section_label(ui, "Mirror plane");
                plane_combo(ui, "mirror_plane", &mut m.plane);
                hint(ui, "Or click a planar face in the viewport.");
                ui.checkbox(&mut m.keep_original, "Keep the original");
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
            }
            EditorParams::Shell(s) => {
                theme::section_label(ui, "Open faces");
                pick_prompt(ui, "Click faces to open; leave empty for a closed shell");
                let mut remove = None;
                for (i, face) in s.open_faces.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&face.label).size(12.0));
                        if ui.add(egui::Button::new("×").small().frame(false)).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    s.open_faces.remove(i);
                }
                section_gap(ui);
                // Keep the user's typed value: invalid walls must be reported, not
                // silently clamped to the generic positive dimension-field range.
                ui.horizontal(|ui| {
                    ui.label("Wall thickness");
                    let response = ui.add(egui::DragValue::new(&mut s.thickness).speed(0.1).suffix(" mm"));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Wall thickness value")
                    });
                });
                if let Some(error) = &editor.error {
                    error_row(ui, error);
                }
            }
            EditorParams::Draft(d) => {
                theme::section_label(ui, "Faces to draft");
                if d.faces.is_empty() {
                    pick_prompt(ui, "Click faces in the viewport");
                }
                let mut remove = None;
                for (i, f) in d.faces.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&f.label).size(12.0));
                        if ui.add(egui::Button::new("×").small().frame(false)).on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    d.faces.remove(i);
                }
                section_gap(ui);
                theme::section_label(ui, "Neutral plane");
                plane_combo(ui, "draft_neutral", &mut d.neutral);
                ui.checkbox(&mut d.picking_neutral, "Next face click sets the neutral plane");
                unit_field(ui, "Angle", &mut d.angle, "°");
                ui.checkbox(&mut d.reverse, "Pull against the neutral normal");
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
                ui.add_space(8.0);
                hint(ui, "Catallax drafts planar faces and cylinders parallel to the pull.");
            }
            EditorParams::PickPlane => {
                theme::section_label(ui, "Sketch plane");
                hint(ui, "Choose a reference plane, or click a planar face of the body.");
                ui.add_space(4.0);
                for p in PlaneKind::ALL {
                    if ui
                        .add_sized(
                            vec2(ui.available_width() - 4.0, 24.0),
                            egui::Button::new(state.doc.model().datum_name(p.to_model())),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::StartSketchOn(SketchPlane::Reference { plane: p, offset: 0.0 }));
                    }
                }
            }
            EditorParams::Repair { items, chosen, active } => {
                theme::section_label(ui, "Missing references");
                hint(ui, "Pick the replacement for each one in the viewport, then OK.");
                ui.add_space(4.0);
                for (i, item) in items.iter().enumerate() {
                    let is_active = *active == i;
                    let picked = chosen.get(i).and_then(|c| c.as_ref()).map(|c| c.label().to_owned());
                    let frame = egui::Frame::new()
                        .fill(if is_active { theme::accent_soft() } else { theme::panel() })
                        .stroke(Stroke::new(1.0, if is_active { theme::accent() } else { theme::line() }))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 5));
                    let resp = frame
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                                let icon = match item.kind {
                                    crate::document::RefKind::Edge => Icon::Axis,
                                    crate::document::RefKind::Face => Icon::Plane,
                                };
                                icons::paint(ui.painter(), icon, r, theme::muted());
                                ui.label(
                                    egui::RichText::new(&item.label).size(12.0).strikethrough().color(theme::muted()),
                                );
                            });
                            match &picked {
                                Some(l) => {
                                    ui.label(egui::RichText::new(format!("→ {l}")).size(12.0).color(theme::ok()));
                                }
                                None => {
                                    ui.label(
                                        egui::RichText::new(if is_active {
                                            match item.kind {
                                                crate::document::RefKind::Edge => "Click the replacement edge",
                                                crate::document::RefKind::Face => "Click the replacement face",
                                            }
                                        } else {
                                            "Waiting"
                                        })
                                        .size(11.5)
                                        .color(if is_active {
                                            theme::accent()
                                        } else {
                                            theme::muted()
                                        }),
                                    );
                                }
                            }
                        })
                        .response;
                    if ui.interact(resp.rect, ui.id().with(("repair_item", i)), Sense::click()).clicked() {
                        *active = i;
                    }
                    ui.add_space(4.0);
                }
                if let Some(err) = &editor.error {
                    error_row(ui, err);
                }
            }
        });
    });
    for a in actions {
        state.push(a);
    }
}

fn sketch_panel(ui: &mut egui::Ui, state: &mut AppState) {
    let name = state
        .sketch
        .as_ref()
        .and_then(|s| state.doc.feature(s.feature))
        .map(|f| f.name.clone())
        .unwrap_or("Sketch".into());
    let plane_label = state
        .sketch
        .as_ref()
        .and_then(|s| state.doc.feature(s.feature))
        .and_then(|f| f.sketch())
        .map(|s| s.plane.label())
        .unwrap_or_default();
    let head = header(ui, Icon::Sketch, &name);
    let (ok, cancel) = ok_cancel(ui, head, "Exit sketch (Enter)", "Exit sketch and discard nothing (Esc)");
    if ok || cancel {
        state.push(AppAction::ExitSketch(None));
    }
    let mut actions = Vec::new();
    let Some(s) = state.sketch.as_mut() else { return };
    let has_profile = crate::sketch::profile::profiles(&s.sketch, s.plane).is_ok();
    egui::ScrollArea::vertical().id_salt("pm").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            // Status.
            let (color, label) = match &s.sketch.status {
                crate::sketch::SolveStatus::Empty => (theme::muted(), "Empty".to_owned()),
                crate::sketch::SolveStatus::UnderDefined => (theme::accent(), "Under defined".to_owned()),
                crate::sketch::SolveStatus::FullyDefined => (theme::ok(), "Fully defined".to_owned()),
                crate::sketch::SolveStatus::OverDefined { conflicting } => {
                    (theme::error(), format!("Over defined ({} conflicting)", conflicting.len()))
                }
                crate::sketch::SolveStatus::Failed(e) => (theme::error(), format!("Solver failed: {e}")),
            };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, color);
                ui.label(egui::RichText::new(&label).family(theme::medium()).size(12.0).color(color));
            });
            let (dims, rels) = s.sketch.counts();
            hint(
                ui,
                &format!(
                    "On {plane_label} · {} entities, {rels} relations, {dims} dimensions",
                    s.sketch.drawn().count()
                ),
            );
            if let crate::sketch::SolveStatus::OverDefined { conflicting } = &s.sketch.status {
                ui.add_space(4.0);
                let names: Vec<String> =
                    conflicting.iter().filter_map(|id| s.sketch.constraint(*id)).map(|c| c.name().to_owned()).collect();
                hint(ui, &format!("Conflicting: {}. Delete one of them.", names.join(", ")));
            }
            section_gap(ui);

            // Exit into a feature.
            theme::section_label(ui, "Use the sketch");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                if ui
                    .add_enabled(has_profile, egui::Button::new("Extrude").min_size(vec2(w, 24.0)))
                    .on_hover_text("Exit the sketch into a boss extrude")
                    .clicked()
                {
                    actions.push(AppAction::ExitSketch(Some(ExitInto::Extrude)));
                }
                if ui
                    .add_enabled(has_profile, egui::Button::new("Cut").min_size(vec2(w, 24.0)))
                    .on_hover_text("Exit the sketch into a cut")
                    .clicked()
                {
                    actions.push(AppAction::ExitSketch(Some(ExitInto::Cut)));
                }
            });
            if !has_profile {
                hint(ui, "Close a loop to extrude or cut.");
            }
            section_gap(ui);

            // Tools.
            theme::section_label(ui, "Tools");
            let tools = [
                (Tool::Select, Icon::Views, "Select"),
                (Tool::Line, Icon::Line, "Line (L)"),
                (Tool::Rectangle, Icon::Rectangle, "Rectangle (R)"),
                (Tool::Circle, Icon::Circle, "Circle (C)"),
                (Tool::Arc, Icon::Arc, "Arc (A)"),
                (Tool::Slot, Icon::Slot, "Slot (S)"),
                (Tool::Point, Icon::Point, "Point (P)"),
                (Tool::Dimension, Icon::SmartDim, "Smart dimension (D)"),
            ];
            ui.horizontal_wrapped(|ui| {
                for (tool, icon, tip) in tools {
                    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::Button, true, s.tool == tool, tip)
                    });
                    let on = s.tool == tool;
                    if on {
                        ui.painter().rect_filled(r, CornerRadius::same(6), theme::selection_bg());
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(6), theme::hover());
                    }
                    icons::paint(
                        ui.painter(),
                        icon,
                        r.shrink(6.0),
                        if on { theme::selection_ink() } else { theme::ink() },
                    );
                    if resp.on_hover_text(tip).clicked() {
                        s.set_tool(tool);
                    }
                }
            });
            section_gap(ui);
            crate::sketch_tools::panel(ui, s, &mut actions);
            section_gap(ui);

            // Selection.
            let selection = s.selection.clone();
            if selection.is_empty() {
                theme::section_label(ui, "Selection");
                hint(ui, "Click an entity, a point or a relation. Ctrl-click adds to the selection.");
            } else {
                theme::section_label(ui, &format!("Selection ({})", selection.len()));
                let mut remove_constraint = None;
                let mut delete_entities = false;
                let mut toggle_construction = false;
                let mut value_change: Option<(crate::sketch::ConstraintId, f64)> = None;
                for sel in &selection {
                    match sel {
                        SketchSel::Entity(id) => {
                            if let Some(e) = s.sketch.entity(*id) {
                                let detail = match e.kind {
                                    EntityKind::Line { a, b } => format!("{} mm long", fmt_value(a.dist(b))),
                                    EntityKind::Circle { radius, .. } => format!("Ø{} mm", fmt_value(radius * 2.0)),
                                    EntityKind::Arc { .. } => format!("R{} mm", fmt_value(e.radius().unwrap_or(0.0))),
                                    EntityKind::Point { p } => format!("{}, {}", fmt_value(p.u), fmt_value(p.v)),
                                };
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{} {}", e.kind_name(), e.id)).size(12.0));
                                    ui.label(egui::RichText::new(detail).size(11.5).color(theme::muted()));
                                    if e.construction {
                                        ui.label(egui::RichText::new("construction").size(10.5).color(theme::muted()));
                                    }
                                });
                            }
                        }
                        SketchSel::Point(p) => {
                            if let Some(q) = s.sketch.point(*p) {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Point of {} {} at {}, {}",
                                        s.sketch.entity(p.entity).map(|e| e.kind_name()).unwrap_or("?"),
                                        p.entity,
                                        fmt_value(q.u),
                                        fmt_value(q.v)
                                    ))
                                    .size(12.0),
                                );
                            }
                        }
                        SketchSel::Constraint(c) => {
                            if let Some(con) = s.sketch.constraint(*c).copied() {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(con.name()).size(12.0));
                                    if con.is_dimension() {
                                        let mut v = con.value().unwrap_or(0.0);
                                        let unit =
                                            if matches!(con.kind, ConstraintKind::Angle { .. }) { "°" } else { "mm" };
                                        let r = ui.add(
                                            egui::DragValue::new(&mut v)
                                                .speed(0.5)
                                                .fixed_decimals(2)
                                                .suffix(format!(" {unit}")),
                                        );
                                        if r.changed() {
                                            value_change = Some((con.id, v));
                                        }
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui
                                            .add(egui::Button::new("×").small().frame(false))
                                            .on_hover_text("Delete relation")
                                            .clicked()
                                        {
                                            remove_constraint = Some(con.id);
                                        }
                                    });
                                });
                            }
                        }
                    }
                }
                let entities = s.selected_entities();
                if !entities.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let all_construction =
                            entities.iter().all(|e| s.sketch.entity(*e).is_some_and(|e| e.construction));
                        if ui
                            .selectable_label(all_construction, "For construction")
                            .on_hover_text(
                                "Construction geometry guides other entities and never becomes a profile (X)",
                            )
                            .clicked()
                        {
                            toggle_construction = true;
                        }
                        if ui.button(egui::RichText::new("Delete").color(theme::error())).clicked() {
                            delete_entities = true;
                        }
                    });
                    // Relations on the selected entities.
                    let rels: Vec<crate::sketch::Constraint> = entities
                        .iter()
                        .flat_map(|e| s.sketch.constraints_of(*e).copied().collect::<Vec<_>>())
                        .collect::<Vec<_>>();
                    let mut seen = std::collections::HashSet::new();
                    let rels: Vec<_> = rels.into_iter().filter(|c| seen.insert(c.id)).collect();
                    if !rels.is_empty() {
                        ui.add_space(6.0);
                        theme::section_label(ui, "Existing relations");
                        for c in rels {
                            ui.horizontal(|ui| {
                                let text = match c.value() {
                                    Some(v) => format!("{} {}", c.name(), fmt_value(v)),
                                    None => c.name().to_owned(),
                                };
                                if ui
                                    .selectable_label(s.selection.contains(&SketchSel::Constraint(c.id)), text)
                                    .clicked()
                                {
                                    s.selection = vec![SketchSel::Constraint(c.id)];
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui
                                        .add(egui::Button::new("×").small().frame(false))
                                        .on_hover_text("Delete relation")
                                        .clicked()
                                    {
                                        remove_constraint = Some(c.id);
                                    }
                                });
                            });
                        }
                    }
                }
                // Relations that can be added.
                let applicable = sketcher::applicable_relations(&s.sketch, &selection);
                if !applicable.is_empty() {
                    ui.add_space(6.0);
                    theme::section_label(ui, "Add relation");
                    let mut add = None;
                    ui.horizontal_wrapped(|ui| {
                        for (name, kind) in applicable {
                            if ui.button(name).clicked() {
                                add = Some(kind);
                            }
                        }
                    });
                    if let Some(kind) = add {
                        s.sketch.constrain(kind);
                        solve::solve(&mut s.sketch, None, true);
                        actions
                            .push(AppAction::Command(Command::EditSketch { id: s.feature, sketch: s.sketch.clone() }));
                    }
                }
                let mut changed = false;
                if let Some((id, v)) = value_change
                    && let Some(c) = s.sketch.constraint_mut(id)
                {
                    c.set_value(v);
                    changed = true;
                }
                if let Some(id) = remove_constraint {
                    s.sketch.remove_constraint(id);
                    s.selection.retain(|x| *x != SketchSel::Constraint(id));
                    changed = true;
                }
                if toggle_construction {
                    let all = entities.iter().all(|e| s.sketch.entity(*e).is_some_and(|e| e.construction));
                    for e in &entities {
                        if let Some(ent) = s.sketch.entity_mut(*e) {
                            ent.construction = !all;
                        }
                    }
                    changed = true;
                }
                if delete_entities {
                    for e in &entities {
                        s.sketch.remove_entity(*e);
                    }
                    s.selection.clear();
                    changed = true;
                }
                if changed {
                    solve::solve(&mut s.sketch, None, true);
                    actions.push(AppAction::Command(Command::EditSketch { id: s.feature, sketch: s.sketch.clone() }));
                }
            }
            ui.add_space(8.0);
            hint(ui, "Blue entities can still move; black ones are fully defined. Enter exits the sketch.");
        });
    });
    for a in actions {
        state.push(a);
    }
}

fn selection(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(sel) = state.doc.selection else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
            icons::paint(ui.painter(), Icon::Views, r, theme::line().gamma_multiply(1.4));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Nothing selected").family(theme::medium()).color(theme::muted()));
            ui.label(egui::RichText::new("Click a face, edge, vertex or a tree row.").size(11.0).color(theme::muted()));
        });
        return;
    };
    let Some(info) = state.doc.describe(sel) else { return };
    let feature = info.feature.and_then(|id| state.doc.feature(id)).cloned();
    let feature = feature.as_ref();
    let icon = match (sel, feature.map(|f| &f.kind)) {
        (Selection::Feature(_), Some(crate::document::FeatureKind::Sketch(_))) => Icon::Sketch,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Cut(_))) => Icon::Cut,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Fillet(_))) => Icon::Fillet,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Chamfer(_))) => Icon::Chamfer,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Hole(_))) => Icon::Hole,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Pattern(_))) => Icon::LinearPattern,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Mirror(_))) => Icon::Mirror,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Draft(_))) => Icon::Draft,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Shell(_))) => Icon::Shell,
        (Selection::Feature(_), Some(crate::document::FeatureKind::ImportStep(_))) => Icon::Import,
        (Selection::Feature(_), Some(crate::document::FeatureKind::Lattice(_))) => Icon::LinearPattern,
        (Selection::Feature(_), _) => Icon::Extrude,
        (Selection::Body, _) => Icon::Extrude,
        (Selection::Face(_), _) => Icon::Plane,
        (Selection::Edge(_), _) => Icon::Axis,
        (Selection::Vertex(_), _) => Icon::Point,
    };
    let head = header(ui, icon, &info.title);
    let clear = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        "Clear selection (Esc)",
        false,
    );
    if clear.clicked() {
        state.push(AppAction::Select(None));
    }
    let status = feature.map(|f| f.status.clone());
    let suppressed = feature.is_some_and(|f| f.suppressed);
    let is_sketch = feature.is_some_and(|f| f.is_sketch());
    let is_planar_face = matches!(sel, Selection::Face(f) if state.doc.named_face(f).is_some_and(|nf| nf.surface == soliddraft_kernel_api::SurfaceKind::Plane));
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        let kind = match sel {
            Selection::Body => "Body",
            Selection::Feature(_) => "Feature",
            Selection::Face(_) => "Face",
            Selection::Edge(_) => "Edge",
            Selection::Vertex(_) => "Vertex",
        };
        theme::section_label(ui, kind);
        egui::Grid::new("props").num_columns(2).spacing([12.0, 5.0]).show(ui, |ui| {
            for (k, v) in &info.details {
                ui.label(egui::RichText::new(k).size(12.0).color(theme::muted()));
                let color = match k.as_str() {
                    "Error" => theme::error(),
                    "Needs repair" => theme::warn(),
                    _ => theme::ink(),
                };
                ui.add(egui::Label::new(egui::RichText::new(v).size(12.0).color(color)).wrap());
                ui.end_row();
            }
        });
        if let (Selection::Feature(id), Some(f)) = (sel, feature)
            && f.status != FeatureStatus::Inactive
        {
            dimension_field(ui, state, id, &f.kind);
        }
        if let Some(FeatureStatus::NeedsRepair(items)) = &status
            && let Some(id) = info.feature
        {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(theme::warn_soft())
                .stroke(Stroke::new(1.0, theme::warn().gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        icons::paint(ui.painter(), Icon::Warning, r, theme::warn());
                        ui.label(
                            egui::RichText::new(format!(
                                "{} reference{} could not be found after the rebuild. The feature was skipped.",
                                items.len(),
                                if items.len() == 1 { "" } else { "s" }
                            ))
                            .size(11.5)
                            .color(theme::ink()),
                        );
                    });
                    ui.add_space(4.0);
                    if ui.add(egui::Button::new("Repair…").fill(theme::warn()).stroke(Stroke::NONE)).clicked() {
                        state.push(AppAction::StartRepair(id));
                    }
                });
        }
        ui.add_space(10.0);
        theme::hline(ui);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| match sel {
            Selection::Feature(id) => {
                if is_sketch {
                    if ui.button("Edit sketch").clicked() {
                        state.push(AppAction::EditSketch(id));
                    }
                } else if ui.button("Edit feature").clicked() {
                    state.push(AppAction::EditFeature(id));
                }
                if ui.button(if suppressed { "Unsuppress" } else { "Suppress" }).clicked() {
                    state.push(AppAction::Command(Command::Suppress { id, suppressed: !suppressed }));
                }
                if ui.button(egui::RichText::new("Delete").color(theme::error())).clicked() {
                    state.push(AppAction::Command(Command::DeleteFeature(id)));
                }
            }
            Selection::Face(f) => {
                if is_planar_face
                    && ui.button("Sketch on face").clicked()
                    && let Some(r) = state.doc.face_ref(f)
                {
                    state.push(AppAction::StartSketchOn(SketchPlane::Face { face: r }));
                }
                if is_planar_face && ui.button("Normal to").clicked() {
                    state.push(AppAction::NormalTo);
                }
            }
            Selection::Edge(_) => {
                for id in ["fillet", "chamfer"] {
                    let command = crate::commands::get(id).expect("registered finishing command");
                    if ui
                        .add_enabled(command.is_enabled(state), egui::Button::new(command.label))
                        .on_disabled_hover_text(command.tip(state))
                        .clicked()
                        && let Some(action) = command.action_now(state)
                    {
                        state.push(action);
                    }
                }
            }
            Selection::Vertex(_) => {}
            Selection::Body => {
                ui.label(
                    egui::RichText::new(
                        "The viewport shows every dimension that builds the body. Double-click one to edit it.",
                    )
                    .size(11.0)
                    .color(theme::muted()),
                );
            }
        });
        let _ = fmt_mm(0.0);
    });
}
