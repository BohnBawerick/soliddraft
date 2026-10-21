//! The drawing's PropertyManagers: New drawing (on the part), the selected view, the
//! selected dimension or note, the sheet, and the Needs attention list with Reattach,
//! Delete and Keep as note.

use super::{DrawingAction, DrawingSel, DrawingSession, DrawingTool, NewDrawing, SHEET, properties_d3, sheets};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use soliddraft_drawing::display::annotation_name;
use soliddraft_drawing::library as templates;
use soliddraft_drawing::project::{standard_label, view_label};
use soliddraft_drawing::{
    AnnotationKind, AnnotationState, Command, DimensionKind, HiddenLines, LinearOrientation, Projection, Scale,
    TangentEdges, Tolerance, ViewScale,
};
use soliddraft_kernel_api::drawing::StandardView;

pub(super) fn header(ui: &mut egui::Ui, icon: Icon, title: &str) -> Rect {
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    icons::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(pos2(head.left() + 20.0, head.center().y), vec2(16.0, 16.0)),
        theme::INK,
    );
    ui.painter().text(
        pos2(head.left() + 34.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::font_semibold(12.5),
        theme::INK,
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::LINE));
    head
}

pub(super) fn small_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, tip: &str, primary: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("dpm_btn", tip)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    let (bg, fg) = if primary {
        (if resp.hovered() { theme::ACCENT.gamma_multiply(0.9) } else { theme::ACCENT }, Color32::WHITE)
    } else {
        (if resp.hovered() { theme::LINE } else { theme::HOVER }, theme::INK)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    icons::paint(ui.painter(), icon, rect.shrink(4.0), fg);
    resp.on_hover_text(tip)
}

pub(super) fn section_gap(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    theme::hline(ui);
    ui.add_space(6.0);
}

pub(super) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(11.0).color(theme::MUTED));
}

pub(super) fn value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::MUTED)));
        egui::Frame::new()
            .fill(theme::FIELD_BG)
            .stroke(Stroke::new(1.0, theme::LINE))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 16.0);
                ui.label(egui::RichText::new(value).size(12.0));
            });
    });
}

/// A full-width choice button, lit when chosen.
pub(super) fn choice(ui: &mut egui::Ui, on: bool, label: &str, tag: Option<&str>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    let (bg, stroke, ink) = if on {
        (theme::SELECTION_BG, theme::ACCENT, theme::SELECTION_INK)
    } else if resp.hovered() {
        (theme::HOVER, theme::LINE, theme::INK)
    } else {
        (theme::PANEL, theme::LINE, theme::INK)
    };
    ui.painter().rect(rect, CornerRadius::same(6), bg, Stroke::new(1.0, stroke), StrokeKind::Inside);
    ui.painter().text(pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, label, theme::font(12.0), ink);
    if let Some(t) = tag {
        ui.painter().text(
            pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            t,
            theme::font(10.5),
            theme::MUTED,
        );
    }
    resp.clicked()
}

pub(super) fn primary_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized(
        vec2(ui.available_width() - 4.0, 26.0),
        egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).family(theme::medium())).fill(theme::ACCENT),
    )
    .clicked()
}

/// The New drawing PropertyManager, shown in the part's property panel.
pub fn new_drawing(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(new) = state.drawing.new.as_mut() else { return };
    let head = header(ui, Icon::Rectangle, "New drawing");
    let ok = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 42.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Check,
        "Create drawing (Enter)",
        true,
    );
    let cancel = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        "Cancel (Esc)",
        false,
    );
    let mut actions: Vec<DrawingAction> = Vec::new();
    if ok.clicked() {
        actions.push(DrawingAction::CreateDrawing);
    }
    if cancel.clicked() {
        actions.push(DrawingAction::CancelNewDrawing);
    }
    let lib = templates::library();
    egui::ScrollArea::vertical().id_salt("new_drawing").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            theme::section_label(ui, "Template");
            ui.add_space(2.0);
            for t in &lib.templates {
                let tag = if t.id == "iso-a3" { Some("default") } else { None };
                if choice(ui, new.template == t.id, &t.name, tag) {
                    new.set_template(&t.id);
                }
            }
            section_gap(ui);
            theme::section_label(ui, "Sheet");
            let template = templates::template(&new.template);
            ui.horizontal(|ui| {
                ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new("Scale").size(12.0).color(theme::MUTED)));
                let label = new.scale.map(|s| s.to_string()).unwrap_or("Auto (fit the part)".into());
                egui::ComboBox::from_id_salt("new_drawing_scale").width(ui.available_width() - 20.0).selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut new.scale, None, "Auto (fit the part)");
                    for s in template.scales() {
                        ui.selectable_value(&mut new.scale, Some(s), s.to_string());
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new("Projection").size(12.0).color(theme::MUTED)));
                let label = match new.projection {
                    Projection::FirstAngle => "First angle",
                    Projection::ThirdAngle => "Third angle",
                };
                egui::ComboBox::from_id_salt("new_drawing_projection").width(ui.available_width() - 20.0).selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut new.projection, Projection::FirstAngle, "First angle (ISO)");
                    ui.selectable_value(&mut new.projection, Projection::ThirdAngle, "Third angle (ASME)");
                });
            });
            value_row(ui, "Units", "mm · 2 decimals");
            section_gap(ui);
            theme::section_label(ui, "Views to place");
            hint(ui, "Front is the base; the others fold off it per the projection angle.");
            ui.add_space(4.0);
            let cols = 4;
            let w = (ui.available_width() - 4.0 - 6.0 * (cols as f32 - 1.0)) / cols as f32;
            egui::Grid::new("new_drawing_views").spacing([6.0, 6.0]).show(ui, |ui| {
                for (i, (view, on)) in new.views.iter_mut().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 46.0), Sense::click());
                    let name = standard_label(*view).trim_end_matches(" view").to_owned();
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, format!("Place {name}")));
                    let (bg, stroke) = if *on { (theme::SELECTION_BG, theme::ACCENT) } else { (theme::PANEL, theme::LINE) };
                    ui.painter().rect(rect, CornerRadius::same(6), bg, Stroke::new(1.0, stroke), StrokeKind::Inside);
                    let glyph = Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(18.0, 18.0));
                    view_glyph(ui.painter(), *view, glyph, if *on { theme::SELECTION_INK } else { theme::MUTED });
                    ui.painter().text(pos2(rect.center().x, rect.bottom() - 9.0), Align2::CENTER_CENTER, &name, theme::font(10.5), if *on { theme::SELECTION_INK } else { theme::MUTED });
                    if resp.clicked() {
                        *on = !*on;
                    }
                    if (i + 1) % cols == 0 {
                        ui.end_row();
                    }
                }
            });
            if let Some(e) = &new.error {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(e).size(11.5).color(theme::ERROR));
            }
            ui.add_space(10.0);
            if primary_button(ui, "Create drawing") {
                actions.push(DrawingAction::CreateDrawing);
            }
            ui.add_space(4.0);
            hint(ui, "Makes a drawing document that references this part by id and recipe digest, places the ticked views at the sheet scale and opens it as a tab beside the part.");
        });
    });
    for a in actions {
        state.push(AppAction::Drawing(a));
    }
}

/// A small glyph for a standard orientation: a box seen from that side.
fn view_glyph(painter: &egui::Painter, view: StandardView, rect: Rect, colour: Color32) {
    let stroke = Stroke::new(1.3, colour);
    let r = rect.shrink(2.0);
    match view {
        StandardView::Isometric => {
            let c = r.center();
            let (w, h) = (r.width() / 2.0, r.height() / 2.0);
            let top = pos2(c.x, r.top());
            let left = pos2(r.left(), c.y - h * 0.4);
            let right = pos2(r.right(), c.y - h * 0.4);
            let bottom = pos2(c.x, r.bottom());
            let mid = pos2(c.x, c.y + h * 0.2);
            painter.add(egui::Shape::closed_line(vec![top, right, bottom, left], stroke));
            painter.line_segment([left, mid], stroke);
            painter.line_segment([right, mid], stroke);
            painter.line_segment([mid, bottom], stroke);
            let _ = w;
        }
        StandardView::Top | StandardView::Bottom => {
            painter.rect_stroke(r, CornerRadius::ZERO, stroke, StrokeKind::Inside);
            painter.circle_stroke(r.center(), r.width() * 0.22, stroke);
        }
        StandardView::Left | StandardView::Right => {
            painter.rect_stroke(
                Rect::from_min_max(pos2(r.left(), r.top()), pos2(r.right(), r.bottom())),
                CornerRadius::ZERO,
                stroke,
                StrokeKind::Inside,
            );
            let x = if view == StandardView::Left { r.left() + r.width() * 0.3 } else { r.right() - r.width() * 0.3 };
            painter.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], stroke);
        }
        StandardView::Front | StandardView::Back => {
            painter.rect_stroke(r, CornerRadius::ZERO, stroke, StrokeKind::Inside);
            let y = r.top() + r.height() * 0.35;
            painter.line_segment([pos2(r.left(), y), pos2(r.right(), y)], stroke);
        }
    }
}

/// The drawing's property panel: Needs attention first when there is something to fix,
/// then the selection.
pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let doc_name = state.doc.name.clone();
    let Some(s) = state.drawing.session.as_mut() else { return };
    let mut actions: Vec<DrawingAction> = Vec::new();
    let attention = s.doc.attention_count();
    if attention > 0 {
        needs_attention(ui, s, &mut actions);
    }
    let tool_has_panel = s.tool.is_derived() || s.tool.is_symbol() || matches!(s.tool, DrawingTool::DimensionSet(_));
    let part_open = s.model_is(state.doc.model().id());
    match s.selection {
        _ if tool_has_panel => properties_d3::tool_panel(ui, s, &mut actions),
        Some(DrawingSel::View(id)) => view_panel(ui, s, id, &doc_name, &mut actions),
        Some(DrawingSel::Annotation(id)) => annotation_panel(ui, s, id, &mut actions),
        Some(DrawingSel::Format) => format_panel(ui, s, &mut actions),
        Some(DrawingSel::Revisions) => properties_d3::revisions_panel(ui, s, &mut actions),
        Some(DrawingSel::Model) => {
            header(ui, Icon::Extrude, "Model reference");
            super::reference::panel(ui, s, part_open, &mut actions);
        }
        None => match s.tool.clone() {
            DrawingTool::PlaceView(kind) => placing_panel(ui, s, kind, &mut actions),
            _ => sheet_panel(ui, s, &mut actions),
        },
    }
    for a in actions {
        state.push(AppAction::Drawing(a));
    }
}

fn needs_attention(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let n = s.doc.attention_count();
    header(ui, Icon::Warning, &format!("Needs attention ({n})"));
    let style = s.doc.style();
    let sheet = s.sheet().clone();
    let dangling: Vec<_> =
        sheet.annotations.iter().filter(|a| matches!(a.state, AnnotationState::Dangling { .. })).cloned().collect();
    let changed: Vec<_> =
        sheet.annotations.iter().filter(|a| matches!(a.state, AnnotationState::Changed { .. })).cloned().collect();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        for a in &dangling {
            let AnnotationState::Dangling { lost, cause } = &a.state else { continue };
            let name = annotation_name(a, style);
            let view_name = sheet.view(a.view).map(|v| view_label(&sheet, v)).unwrap_or_else(|| "sheet".into());
            let focused = s.attention_focus == Some(a.id) || s.selection == Some(DrawingSel::Annotation(a.id));
            let card = egui::Frame::new()
                .fill(if focused { theme::FIELD_BG } else { theme::PANEL })
                .stroke(Stroke::new(1.0, if focused { theme::ERROR.gamma_multiply(0.6) } else { theme::LINE }))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(9, 7));
            card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                let bar = Rect::from_min_size(
                    ui.max_rect().left_top() - vec2(9.0, 7.0),
                    vec2(3.0, ui.max_rect().height() + 14.0),
                );
                ui.painter().rect_filled(bar, CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, theme::ERROR);
                ui.horizontal(|ui| {
                    ui.painter().circle_filled(ui.cursor().left_top() + vec2(4.0, 10.0), 3.5, theme::ERROR);
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(&name).family(theme::semibold()).size(12.5).color(theme::ERROR));
                    ui.label(egui::RichText::new(format!("· {}", kind_words(a))).size(11.5).color(theme::MUTED));
                    ui.label(egui::RichText::new(format!("· {view_name}")).size(11.5).color(theme::MUTED));
                });
                ui.add_space(2.0);
                ui.label(egui::RichText::new(cause).size(11.0).color(theme::MUTED).italics());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let end = lost.first().copied().unwrap_or(0);
                    let reattach = egui::Button::new(egui::RichText::new("Reattach…").color(Color32::WHITE).size(11.5))
                        .fill(theme::ACCENT);
                    if ui
                        .add(reattach)
                        .on_hover_text("One pick per lost end: click the new edge or point in the view")
                        .clicked()
                    {
                        actions.push(DrawingAction::StartReattach { annotation: a.id, end });
                    }
                    if ui.add(egui::Button::new(egui::RichText::new("Delete").size(11.5))).clicked() {
                        actions.push(DrawingAction::Select(Some(DrawingSel::Annotation(a.id))));
                        actions.push(DrawingAction::DeleteSelection);
                    }
                    if ui
                        .add(egui::Button::new(egui::RichText::new("Keep as note").size(11.5)))
                        .on_hover_text("Freeze the text as a note with a reference mark")
                        .clicked()
                    {
                        actions.push(DrawingAction::Command(Command::FreezeAsNote { annotation: a.id }));
                    }
                });
            });
            ui.add_space(6.0);
        }
        if !changed.is_empty() {
            theme::section_label(ui, &format!("Changed values ({})", changed.len()));
            ui.add_space(2.0);
            for a in &changed {
                let AnnotationState::Changed { previous } = &a.state else { continue };
                let name = annotation_name(a, style);
                let view_name = sheet.view(a.view).map(|v| view_label(&sheet, v)).unwrap_or_else(|| "sheet".into());
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(1.0, theme::LINE))
                    .corner_radius(CornerRadius::same(7))
                    .inner_margin(egui::Margin::symmetric(9, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let bar = Rect::from_min_size(
                            ui.max_rect().left_top() - vec2(9.0, 6.0),
                            vec2(3.0, ui.max_rect().height() + 12.0),
                        );
                        ui.painter().rect_filled(bar, CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, theme::WARN);
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!("{previous} → {name}"))
                                    .family(theme::semibold())
                                    .size(12.0)
                                    .color(theme::WARN),
                            );
                            ui.label(
                                egui::RichText::new(format!("· {} · {view_name}", kind_words(a)))
                                    .size(11.0)
                                    .color(theme::MUTED),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(egui::RichText::new("Accept").size(11.0))).clicked() {
                                    actions.push(DrawingAction::AcceptChange(a.id));
                                }
                            });
                        });
                    });
                ui.add_space(4.0);
            }
        }
    });
    section_gap(ui);
}

fn kind_words(a: &soliddraft_drawing::Annotation) -> String {
    match &a.body {
        AnnotationKind::Dimension(d) => match d.kind {
            DimensionKind::Linear { orientation: LinearOrientation::Horizontal } => "horizontal".into(),
            DimensionKind::Linear { orientation: LinearOrientation::Vertical } => "vertical".into(),
            DimensionKind::Linear { .. } => "aligned".into(),
            DimensionKind::Diameter => "diameter".into(),
            DimensionKind::Radius => "radius".into(),
            DimensionKind::Angular => "angle".into(),
            DimensionKind::Ordinate => "ordinate".into(),
            DimensionKind::Chamfer => "chamfer".into(),
        },
        AnnotationKind::Note { .. } => "note".into(),
        AnnotationKind::HoleCallout { .. } => "hole callout".into(),
        AnnotationKind::CentreMark => "centre mark".into(),
        AnnotationKind::Centreline => "centreline".into(),
        AnnotationKind::Gdt { .. } => "geometric tolerance".into(),
        AnnotationKind::Datum { .. } => "datum".into(),
        AnnotationKind::SurfaceFinish { .. } => "surface finish".into(),
        AnnotationKind::RevisionSymbol { .. } => "revision symbol".into(),
        AnnotationKind::DimensionSet(set) => match set.kind {
            soliddraft_drawing::SetKind::Ordinate => "ordinate".into(),
            soliddraft_drawing::SetKind::Baseline => "baseline".into(),
            soliddraft_drawing::SetKind::Chain => "chain".into(),
        },
    }
}

fn view_panel(
    ui: &mut egui::Ui,
    s: &mut DrawingSession,
    id: soliddraft_drawing::ViewId,
    part: &str,
    actions: &mut Vec<DrawingAction>,
) {
    let sheet = s.sheet().clone();
    let Some(v) = sheet.view(id).cloned() else { return };
    let name = view_label(&sheet, &v);
    let head = header(ui, Icon::Display, &name);
    let close = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        "Deselect (Esc)",
        false,
    );
    if close.clicked() {
        actions.push(DrawingAction::Select(None));
    }
    let stale = s.doc.stale_views().contains(&id);
    egui::ScrollArea::vertical().id_salt("view_pm").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            theme::section_label(ui, "Reference");
            let state_text = if stale { "out of date" } else { "up to date" };
            value_row(ui, "Model", &format!("{part} (rev {}, {state_text})", s.doc.drawing.model.pinned.short()));
            let orientation = match &v.kind {
                soliddraft_drawing::ViewKind::Base { .. } => name.trim_end_matches(" view").to_owned(),
                soliddraft_drawing::ViewKind::Projected { parent, direction } => {
                    let parent_name = sheet.view(*parent).map(|p| view_label(&sheet, p)).unwrap_or_default();
                    format!("{:?} of {parent_name}", direction).to_lowercase()
                }
                soliddraft_drawing::ViewKind::Section { parent, .. } => {
                    let parent_name = s.doc.drawing.sheet_of(*parent).and_then(|sh| sh.view(*parent).map(|p| view_label(sh, p))).unwrap_or_default();
                    format!("section of {parent_name}")
                }
                soliddraft_drawing::ViewKind::Detail { parent, .. } => {
                    let parent_name = s.doc.drawing.sheet_of(*parent).and_then(|sh| sh.view(*parent).map(|p| view_label(sh, p))).unwrap_or_default();
                    format!("detail of {parent_name}")
                }
                soliddraft_drawing::ViewKind::Auxiliary { parent, .. } => {
                    let parent_name = s.doc.drawing.sheet_of(*parent).and_then(|sh| sh.view(*parent).map(|p| view_label(sh, p))).unwrap_or_default();
                    format!("auxiliary of {parent_name}")
                }
            };
            value_row(ui, "Orientation", &orientation);
            section_gap(ui);
            theme::section_label(ui, "Scale");
            let sheet_scale = sheet.scale;
            let is_sheet = v.scale == ViewScale::Sheet;
            let is_parent = v.scale == ViewScale::Parent;
            if choice(ui, is_sheet, &format!("Use sheet scale {sheet_scale}"), None) && !is_sheet {
                actions.push(DrawingAction::Command(Command::SetViewScale { view: id, scale: ViewScale::Sheet }));
            }
            if v.parent().is_some() && choice(ui, is_parent, "Use parent scale", None) && !is_parent {
                actions.push(DrawingAction::Command(Command::SetViewScale { view: id, scale: ViewScale::Parent }));
            }
            ui.horizontal(|ui| {
                ui.add_sized(
                    vec2(68.0, 22.0),
                    egui::Label::new(egui::RichText::new("Custom").size(12.0).color(theme::MUTED)),
                );
                let current = match v.scale {
                    ViewScale::Custom { factor } => soliddraft_drawing::display::scale_text(factor),
                    _ => "…".into(),
                };
                egui::ComboBox::from_id_salt(("view_scale", id.0))
                    .width(ui.available_width() - 20.0)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for sc in s.doc.template().scales() {
                            let on =
                                matches!(v.scale, ViewScale::Custom { factor } if (factor - sc.factor()).abs() < 1e-9);
                            if ui.selectable_label(on, sc.to_string()).clicked() {
                                actions.push(DrawingAction::Command(Command::SetViewScale {
                                    view: id,
                                    scale: ViewScale::Custom { factor: sc.factor() },
                                }));
                            }
                        }
                    });
            });
            section_gap(ui);
            theme::section_label(ui, "Display style");
            let hidden_removed = v.display.hidden == HiddenLines::Removed;
            if choice(ui, hidden_removed, "Hidden lines removed", None) && !hidden_removed {
                actions.push(DrawingAction::Command(Command::SetViewDisplay {
                    view: id,
                    display: soliddraft_drawing::ViewDisplay { hidden: HiddenLines::Removed, ..v.display },
                }));
            }
            if choice(ui, !hidden_removed, "Hidden lines visible", None) && hidden_removed {
                actions.push(DrawingAction::Command(Command::SetViewDisplay {
                    view: id,
                    display: soliddraft_drawing::ViewDisplay { hidden: HiddenLines::Visible, ..v.display },
                }));
            }
            choice(ui, false, "Wireframe", Some("later"));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    vec2(68.0, 22.0),
                    egui::Label::new(egui::RichText::new("Tangent edges").size(12.0).color(theme::MUTED)),
                );
                let label = match v.display.tangent {
                    TangentEdges::Removed => "Removed",
                    TangentEdges::Phantom => "Phantom",
                    TangentEdges::Visible => "Visible",
                };
                egui::ComboBox::from_id_salt(("tangent", id.0))
                    .width(ui.available_width() - 20.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (t, l) in [
                            (TangentEdges::Removed, "Removed"),
                            (TangentEdges::Phantom, "Phantom"),
                            (TangentEdges::Visible, "Visible"),
                        ] {
                            if ui.selectable_label(v.display.tangent == t, l).clicked() && v.display.tangent != t {
                                actions.push(DrawingAction::Command(Command::SetViewDisplay {
                                    view: id,
                                    display: soliddraft_drawing::ViewDisplay { tangent: t, ..v.display },
                                }));
                            }
                        }
                    });
            });
            hint(ui, "Tangent edges and seams remake the view's line work through the kernel; hidden lines only change what is drawn.");
            section_gap(ui);
            properties_d3::view_extras(ui, s, &v, actions);
            theme::section_label(ui, "Alignment and children");
            match v.alignment {
                Some(al) => {
                    let parent_name = sheet.view(al.parent).map(|p| view_label(&sheet, p)).unwrap_or_default();
                    let axis = match al.axis {
                        soliddraft_drawing::AlignAxis::Horizontal => "side by side",
                        soliddraft_drawing::AlignAxis::Vertical => "one above the other",
                    };
                    value_row(ui, "Aligned to", &format!("{parent_name} · {axis}"));
                    if ui.button("Break alignment").clicked() {
                        actions.push(DrawingAction::Command(Command::BreakAlignment { view: id }));
                    }
                }
                None => {
                    value_row(ui, "Aligned to", "nothing");
                    if v.parent().is_some() && ui.button("Align with parent").clicked() {
                        actions.push(DrawingAction::Command(Command::Align { view: id }));
                    }
                }
            }
            let children: Vec<String> =
                sheet.views.iter().filter(|c| c.parent() == Some(id)).map(|c| view_label(&sheet, c)).collect();
            if !children.is_empty() {
                value_row(ui, "Children", &children.join(", "));
            }
            section_gap(ui);
            theme::section_label(ui, "Automatic update");
            let mut excluded = v.excluded_from_update;
            if ui.checkbox(&mut excluded, "Exclude this view from updates").changed() {
                actions.push(DrawingAction::Command(Command::SetViewExcluded { view: id, excluded }));
            }
            if stale {
                ui.add_space(4.0);
                if ui.button("Rebuild").on_hover_text("Bring the views to the current part (Ctrl+B)").clicked() {
                    actions.push(DrawingAction::Rebuild);
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Projected view…").on_hover_text("Fold a view off this one").clicked() {
                    actions.push(DrawingAction::SetTool(DrawingTool::PlaceView(super::PlaceKind::Projected {
                        parent: id,
                    })));
                }
                if ui.button("Delete view").clicked() {
                    actions.push(DrawingAction::DeleteSelection);
                }
            });
        });
    });
}

fn annotation_panel(
    ui: &mut egui::Ui,
    s: &mut DrawingSession,
    id: soliddraft_drawing::AnnotationId,
    actions: &mut Vec<DrawingAction>,
) {
    let sheet = s.sheet().clone();
    let Some(a) = s.doc.drawing.annotation(id).cloned() else { return };
    let style = s.doc.style();
    let name = annotation_name(&a, style);
    let (icon, title) = match &a.body {
        AnnotationKind::Dimension(_) => (Icon::SmartDim, format!("Dimension {name}")),
        AnnotationKind::Note { .. } => (Icon::Chat, "Note".to_owned()),
        AnnotationKind::HoleCallout { .. } => (Icon::Hole, "Hole callout".to_owned()),
        AnnotationKind::CentreMark => (Icon::Point, "Centre mark".to_owned()),
        AnnotationKind::Gdt { .. } => (Icon::Rectangle, "Geometric tolerance".to_owned()),
        AnnotationKind::Datum { .. } => (Icon::Plane, name.clone()),
        AnnotationKind::SurfaceFinish { .. } => (Icon::Material, "Surface finish".to_owned()),
        AnnotationKind::RevisionSymbol { .. } => (Icon::Rollback, format!("Revision symbol {name}")),
        AnnotationKind::DimensionSet(_) => (Icon::SmartDim, name.clone()),
        _ => (Icon::Material, name.clone()),
    };
    let head = header(ui, icon, &title);
    let close = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        "Deselect (Esc)",
        false,
    );
    if close.clicked() {
        actions.push(DrawingAction::Select(None));
    }
    let view_name = if a.view == SHEET {
        "Sheet".to_owned()
    } else {
        sheet.view(a.view).map(|v| view_label(&sheet, v)).unwrap_or_default()
    };
    let scale = sheet.view(a.view).map(|v| soliddraft_drawing::project::effective_scale(&sheet, v)).unwrap_or(1.0);
    let _ = scale;
    egui::ScrollArea::vertical().id_salt("annotation_pm").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            if !a.anchors.is_empty() {
                theme::section_label(ui, "References");
                for (i, an) in a.anchors.iter().enumerate() {
                    let lost = matches!(&a.state, AnnotationState::Dangling { lost, .. } if lost.contains(&i));
                    let kind = match an.entity.kind {
                        soliddraft_drawing::EntityKind::Edge => "edge",
                        soliddraft_drawing::EntityKind::Face => "face",
                        soliddraft_drawing::EntityKind::Vertex => "vertex",
                    };
                    let attach = format!("{:?}", an.attach).to_lowercase();
                    let attach = attach.split(' ').next().unwrap_or("").trim_matches(|c| c == '{' || c == '}').to_owned();
                    let text = format!("{} · {kind} · {attach}", an.entity.role);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                        icons::paint(ui.painter(), Icon::Axis, r, if lost { theme::ERROR } else { theme::INK });
                        let rich = egui::RichText::new(text).size(11.5);
                        ui.label(if lost { rich.strikethrough().color(theme::ERROR) } else { rich });
                    });
                    if lost && ui.small_button("Reattach this end…").clicked() {
                        actions.push(DrawingAction::StartReattach { annotation: id, end: i });
                    }
                }
                value_row(ui, "View", &view_name);
                section_gap(ui);
            }
            match &a.body {
                AnnotationKind::Dimension(d) => {
                    theme::section_label(ui, "Value");
                    let value = match d.value {
                        soliddraft_drawing::DimValue::Measured { value } => value,
                        _ => 0.0,
                    };
                    let unit = if d.kind == DimensionKind::Angular { "°" } else { " mm" };
                    value_row(ui, "Measured", &format!("{value:.2}{unit} (reference)"));
                    let tol = match d.tolerance {
                        Tolerance::None => "None".to_owned(),
                        Tolerance::Symmetric { value } => format!("Bilateral ± {value}"),
                        Tolerance::Deviation { upper, lower } => format!("Deviation +{upper} / {lower}"),
                        Tolerance::Limits { upper, lower } => format!("Limits +{upper} / {lower}"),
                    };
                    value_row(ui, "Tolerance", &tol);
                    value_row(ui, "Precision", &format!("{} decimal{}", d.precision, if d.precision == 1 { "" } else { "s" }));
                    section_gap(ui);
                    theme::section_label(ui, "Display");
                    let mut edited = d.clone();
                    let mut changed = false;
                    let round = matches!(d.kind, DimensionKind::Diameter | DimensionKind::Radius);
                    if round {
                        if choice(ui, d.kind == DimensionKind::Diameter, "Diameter", None) && d.kind != DimensionKind::Diameter {
                            edited.kind = DimensionKind::Diameter;
                            edited.value = soliddraft_drawing::DimValue::Measured { value: value * 2.0 };
                            changed = true;
                        }
                        if choice(ui, d.kind == DimensionKind::Radius, "Radius", None) && d.kind != DimensionKind::Radius {
                            edited.kind = DimensionKind::Radius;
                            edited.value = soliddraft_drawing::DimValue::Measured { value: value / 2.0 };
                            changed = true;
                        }
                    } else if let DimensionKind::Linear { orientation } = d.kind {
                        for (o, l) in [(LinearOrientation::Horizontal, "Horizontal"), (LinearOrientation::Vertical, "Vertical"), (LinearOrientation::Aligned, "Aligned")] {
                            if choice(ui, orientation == o, l, None) && orientation != o {
                                edited.kind = DimensionKind::Linear { orientation: o };
                                if a.anchors.len() >= 2 {
                                    let v = soliddraft_drawing::measure::linear_value(o, a.anchors[0].cached, a.anchors[1].cached, scale);
                                    edited.value = soliddraft_drawing::DimValue::Measured { value: v };
                                }
                                changed = true;
                            }
                        }
                    }
                    ui.add_space(4.0);
                    let mut text = d.text.clone();
                    ui.horizontal(|ui| {
                        ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new("Text").size(12.0).color(theme::MUTED)));
                        let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width() - 20.0));
                        if r.lost_focus() && text != d.text {
                            edited.text = if text.trim().is_empty() { soliddraft_drawing::Dimension::VALUE_TEXT.into() } else { text.clone() };
                            changed = true;
                        }
                    });
                    hint(ui, "<DIM> stands for the value. The palette on the sheet edits tolerance and precision.");
                    if changed {
                        actions.push(DrawingAction::Command(Command::EditDimension { annotation: id, dimension: edited }));
                    }
                }
                AnnotationKind::Note { text, leader } => {
                    theme::section_label(ui, "Note text");
                    let edit = s.note_edit.get_or_insert_with(|| super::NoteEdit { annotation: id, text: text.clone(), focus: false });
                    if edit.annotation != id {
                        *edit = super::NoteEdit { annotation: id, text: text.clone(), focus: false };
                    }
                    let r = ui.add(egui::TextEdit::multiline(&mut edit.text).desired_rows(4).desired_width(ui.available_width() - 4.0).font(theme::font(12.0)));
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Note text"));
                    if edit.focus {
                        r.request_focus();
                        edit.focus = false;
                    }
                    let apply_now = r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command));
                    if apply_now && edit.text != *text {
                        actions.push(DrawingAction::Command(Command::EditNote { annotation: id, text: edit.text.clone() }));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Apply text").clicked() && edit.text != *text {
                            actions.push(DrawingAction::Command(Command::EditNote { annotation: id, text: edit.text.clone() }));
                        }
                        hint(ui, "Ctrl+Enter applies");
                    });
                    value_row(ui, "Leader", if a.anchors.is_empty() { "none" } else if leader.is_empty() { "straight" } else { "bent" });
                }
                AnnotationKind::HoleCallout { text } => {
                    theme::section_label(ui, "Callout");
                    value_row(ui, "Text", text);
                    hint(ui, "Read from the hole feature: count, diameter, THRU or depth. It follows the part.");
                }
                AnnotationKind::CentreMark => {
                    hint(ui, "Centre marks are inserted automatically for circles seen face-on; the arms reach past the circle.");
                }
                _ => {
                    properties_d3::annotation_body(ui, s, &a, actions);
                }
            }
            match &a.state {
                AnnotationState::Changed { previous } => {
                    section_gap(ui);
                    ui.label(egui::RichText::new(format!("Changed from {previous} in the last update")).size(11.5).color(theme::WARN));
                    if ui.button("Accept the new value").clicked() {
                        actions.push(DrawingAction::AcceptChange(id));
                    }
                }
                AnnotationState::Dangling { cause, .. } => {
                    section_gap(ui);
                    ui.label(egui::RichText::new(cause).size(11.5).color(theme::ERROR));
                }
                AnnotationState::Ok => {}
            }
            ui.add_space(8.0);
            if ui.button("Delete").clicked() {
                actions.push(DrawingAction::DeleteSelection);
            }
        });
    });
}

fn placing_panel(ui: &mut egui::Ui, s: &DrawingSession, kind: super::PlaceKind, actions: &mut Vec<DrawingAction>) {
    let sheet = s.sheet();
    let head = header(
        ui,
        Icon::OnPlane,
        match &kind {
            super::PlaceKind::Standard(_) => "Model view",
            super::PlaceKind::Projected { .. } => "Projected view",
        },
    );
    let cancel = small_button(
        ui,
        Rect::from_center_size(pos2(head.right() - 16.0, head.center().y), vec2(22.0, 22.0)),
        Icon::Close,
        "Cancel (Esc)",
        false,
    );
    if cancel.clicked() {
        actions.push(DrawingAction::SetTool(DrawingTool::Select));
    }
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| match &kind {
        super::PlaceKind::Standard(v) => {
            theme::section_label(ui, "Orientation");
            for sv in StandardView::ALL {
                if choice(ui, *v == sv, standard_label(sv), None) && *v != sv {
                    actions.push(DrawingAction::SetTool(DrawingTool::PlaceView(super::PlaceKind::Standard(sv))));
                }
            }
            section_gap(ui);
            hint(ui, "Click on the sheet to place. Esc cancels.");
        }
        super::PlaceKind::Projected { parent } => {
            theme::section_label(ui, "Parent");
            let parent_name = sheet.view(*parent).map(|p| view_label(sheet, p)).unwrap_or_default();
            value_row(ui, "From", &parent_name);
            let direction = s.pending.as_ref().and_then(|p| p.direction);
            let angle = match sheet.projection {
                Projection::FirstAngle => "first angle",
                Projection::ThirdAngle => "third angle",
            };
            let result = s.pending.as_ref().map(|p| {
                let mut tmp = sheet.clone();
                tmp.views.push(p.view.clone());
                view_label(&tmp, &p.view)
            });
            let text = match (direction, result) {
                (Some(d), Some(r)) => format!("{} → {r} ({angle})", format!("{d:?}").to_lowercase()),
                _ => "move the pointer around the parent".into(),
            };
            value_row(ui, "Direction", &text);
            section_gap(ui);
            theme::section_label(ui, "Placement");
            let aligned = s.pending.as_ref().is_some_and(|p| p.aligned);
            choice(ui, aligned, "Keep aligned with parent", None);
            choice(ui, !aligned, "Free (Ctrl while placing)", None);
            value_row(ui, "Scale", "Use parent");
            value_row(ui, "Display style", "Use parent");
            section_gap(ui);
            hint(
                ui,
                "Click to place. Esc cancels. Orthographic directions give the folded view; diagonals give pictorials.",
            );
        }
    });
}

fn sheet_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let si = s.sheet;
    let sheet = s.sheet().clone();
    let name = sheet.name.replace("Sheet", "Sheet ");
    header(ui, Icon::Rectangle, &name);
    let template = s.doc.template();
    let auto = s.doc.drawing.settings.automatic_update;
    let meta = s.doc.drawing.meta.clone();
    let sheet_count = s.doc.drawing.sheets.len();
    let blocker = sheets::delete_blocker(&s.doc.drawing, si);
    egui::ScrollArea::vertical().id_salt("sheet_pm").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            theme::section_label(ui, "Sheet");
            ui.horizontal(|ui| {
                ui.add_sized(
                    vec2(68.0, 22.0),
                    egui::Label::new(egui::RichText::new("Name").size(12.0).color(theme::MUTED)),
                );
                let mut text = s
                    .sheet_rename
                    .as_ref()
                    .filter(|(i, _)| *i == si)
                    .map(|(_, t)| t.clone())
                    .unwrap_or_else(|| sheet.name.clone());
                let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width() - 20.0));
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Sheet name field"));
                if r.has_focus() || r.changed() {
                    s.sheet_rename = Some((si, text.clone()));
                }
                if r.lost_focus() {
                    let t = text.trim().to_owned();
                    if !t.is_empty() && t != sheet.name {
                        actions.push(DrawingAction::Command(Command::RenameSheet { sheet: si, name: t }));
                    }
                    s.sheet_rename = None;
                }
            });
            value_row(ui, "Order", &format!("sheet {} of {sheet_count}", si + 1));
            ui.horizontal(|ui| {
                if ui.button("Add sheet").on_hover_text("A sheet after the last one, from the same template").clicked()
                {
                    actions.push(DrawingAction::AddSheet);
                }
                if si > 0 && ui.button("Move left").clicked() {
                    actions.push(DrawingAction::MoveSheet { sheet: si, to: si - 1 });
                }
                if si + 1 < sheet_count && ui.button("Move right").clicked() {
                    actions.push(DrawingAction::MoveSheet { sheet: si, to: si + 1 });
                }
                let delete = ui.add_enabled(blocker.is_none(), egui::Button::new("Delete sheet"));
                let delete = match &blocker {
                    Some(why) => delete.on_disabled_hover_text(why.clone()),
                    None => delete,
                };
                if delete.clicked() {
                    actions.push(DrawingAction::DeleteSheet(si));
                }
            });
            value_row(ui, "Template", &template.name);
            value_row(
                ui,
                "Size",
                &format!("{} · {} × {} mm", sheet.size.name, sheet.size.width_mm, sheet.size.height_mm),
            );
            ui.horizontal(|ui| {
                ui.add_sized(
                    vec2(68.0, 22.0),
                    egui::Label::new(egui::RichText::new("Scale").size(12.0).color(theme::MUTED)),
                );
                egui::ComboBox::from_id_salt("sheet_scale")
                    .width(ui.available_width() - 20.0)
                    .selected_text(sheet.scale.to_string())
                    .show_ui(ui, |ui| {
                        for sc in template.scales() {
                            if ui.selectable_label(sheet.scale == sc, sc.to_string()).clicked() && sheet.scale != sc {
                                actions.push(DrawingAction::Command(Command::SetSheetScale { sheet: si, scale: sc }));
                            }
                        }
                        let _ = Scale::FULL;
                    });
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    vec2(68.0, 22.0),
                    egui::Label::new(egui::RichText::new("Projection").size(12.0).color(theme::MUTED)),
                );
                let label = match sheet.projection {
                    Projection::FirstAngle => "First angle",
                    Projection::ThirdAngle => "Third angle",
                };
                egui::ComboBox::from_id_salt("sheet_projection")
                    .width(ui.available_width() - 20.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        for (p, l) in [
                            (Projection::FirstAngle, "First angle (ISO)"),
                            (Projection::ThirdAngle, "Third angle (ASME)"),
                        ] {
                            if ui.selectable_label(sheet.projection == p, l).clicked() && sheet.projection != p {
                                actions.push(DrawingAction::Command(Command::SetSheetProjection {
                                    sheet: si,
                                    projection: p,
                                }));
                            }
                        }
                    });
            });
            hint(ui, "Changing the projection angle applies to views placed from now on.");
            section_gap(ui);
            theme::section_label(ui, "Automatic update");
            let mut on = auto;
            if ui.checkbox(&mut on, "Rebuild after every change of the open part").changed() {
                actions.push(DrawingAction::ToggleAutomaticUpdate);
            }
            if s.doc.is_stale() && ui.button("Rebuild now").clicked() {
                actions.push(DrawingAction::Rebuild);
            }
            section_gap(ui);
            theme::section_label(ui, "Title block");
            let mut m = meta.clone();
            let mut changed = false;
            for (label, value) in [
                ("Title", &mut m.title),
                ("Number", &mut m.number),
                ("Revision", &mut m.revision),
                ("Designer", &mut m.designer),
                ("Date", &mut m.date),
            ] {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(68.0, 22.0),
                        egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::MUTED)),
                    );
                    let r = ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width() - 20.0));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, format!("{label} field"))
                    });
                    changed |= r.lost_focus() && r.changed()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if r.lost_focus() {
                        changed = true;
                    }
                });
            }
            if changed && m != meta {
                actions.push(DrawingAction::Command(Command::SetMeta { meta: m }));
            }
            for key in ["owner", "status", "checked", "approved"] {
                let current = sheet.format_overrides.get(key).cloned().unwrap_or_default();
                let mut text = current.clone();
                ui.horizontal(|ui| {
                    let label = key[..1].to_uppercase() + &key[1..];
                    ui.add_sized(
                        vec2(68.0, 22.0),
                        egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::MUTED)),
                    );
                    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width() - 20.0));
                    if r.lost_focus() && text != current {
                        actions.push(DrawingAction::Command(Command::SetTitleField {
                            sheet: si,
                            key: key.to_owned(),
                            value: text.clone(),
                        }));
                    }
                });
            }
            section_gap(ui);
            theme::section_label(ui, "Tables");
            let revisions = sheet.revision_table().map(|t| t.revisions.len());
            value_row(
                ui,
                "Revisions",
                &match revisions {
                    Some(n) => format!("{n} row{}", if n == 1 { "" } else { "s" }),
                    None => "no table yet".to_owned(),
                },
            );
            if ui.button("Revision table…").clicked() {
                actions.push(DrawingAction::Select(Some(DrawingSel::Revisions)));
            }
            section_gap(ui);
            hint(ui, "Select a view or an annotation on the sheet or in the tree to see its properties.");
        });
    });
}

fn format_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    header(ui, Icon::Props, "Sheet format");
    let t = s.doc.template();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        theme::section_label(ui, "From the template");
        value_row(ui, "Template", &t.name);
        value_row(ui, "Margins", &format!("{} / {} / {} / {} mm", t.margins.left, t.margins.top, t.margins.right, t.margins.bottom));
        value_row(ui, "Zones", &format!("{} × {}", t.zones.columns, t.zones.rows));
        value_row(ui, "Title block", &format!("{} × {} mm, {} fields", t.title_block.width, t.title_block.height, t.title_block.fields.len()));
        value_row(ui, "Lines", &format!("{} visible · {} hidden · {} border", t.lines.visible, t.lines.hidden, t.lines.border));
        section_gap(ui);
        hint(ui, "The format is a data file: crates/soliddraft-drawing/templates. Field values are edited in the sheet properties.");
        if ui.button("Sheet properties").clicked() {
            actions.push(DrawingAction::Select(None));
        }
    });
    let _ = NewDrawing::new;
}
