//! FeatureManager-style tree: origin group, then one row per feature with its
//! absorbed sketch nested under it, and the rollback bar. Rows select,
//! double-click edits, F2 or a slow second click renames, drag reorders,
//! right-click offers the rest.

use crate::document::{Command, FeatureId, FeatureKind, FeatureStatus, PlaneKind, Selection, SketchPlane, fmt_mm};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState, EditorParams, Rename};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

/// A row the tree shows, flattened from the feature list.
struct RowItem {
    id: FeatureId,
    /// Index in the document's feature list.
    index: usize,
    indent: u8,
    chevron: Option<bool>,
    icon: Icon,
    name: String,
    sub: String,
    status: FeatureStatus,
    suppressed: bool,
    via_assistant: bool,
    is_sketch: bool,
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    // Header: part name and rebuild state.
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let title = if state.doc.dirty { format!("{} •", state.doc.name) } else { state.doc.name.clone() };
    ui.painter().text(
        pos2(head.left() + 12.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::font_semibold(12.5),
        theme::INK,
    );
    let errors = state.doc.error_count();
    let repairs = state.doc.repair_count();
    let (status, color) = if errors == 0 && repairs == 0 {
        ("Rebuilt · OK".to_owned(), theme::OK)
    } else if errors > 0 {
        (format!("{errors} error{}", if errors == 1 { "" } else { "s" }), theme::ERROR)
    } else {
        (format!("{repairs} to repair"), theme::WARN)
    };
    ui.painter().text(
        pos2(head.right() - 12.0, head.center().y),
        Align2::RIGHT_CENTER,
        status,
        theme::font_medium(11.0),
        color,
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::LINE));

    let mut actions = Vec::new();
    egui::ScrollArea::vertical().id_salt("tree").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        let selected_feature = state.selected_feature();
        let picking_plane = matches!(state.editor.as_ref().map(|e| &e.params), Some(EditorParams::PickPlane));

        // Origin group.
        let open = state.origin_open || picking_plane;
        if row(
            ui,
            RowSpec {
                indent: 0,
                chevron: Some(open),
                icon: Icon::Origin,
                name: "Origin and planes",
                ..RowSpec::plain()
            },
        )
        .clicked()
        {
            state.origin_open = !state.origin_open;
        }
        if open {
            for plane in [PlaneKind::Front, PlaneKind::Top, PlaneKind::Right] {
                let name = state.doc.model().datum_name(plane.to_model());
                let r = row(
                    ui,
                    RowSpec {
                        indent: 1,
                        icon: Icon::Plane,
                        name,
                        dim: !picking_plane,
                        selected: false,
                        ..RowSpec::plain()
                    },
                );
                let r = r.on_hover_text(if picking_plane {
                    "Click to sketch on this plane"
                } else {
                    "Double-click to sketch on this plane"
                });
                if (picking_plane && r.clicked()) || r.double_clicked() {
                    actions.push(AppAction::StartSketchOn(SketchPlane::Reference { plane }));
                }
            }
            row(ui, RowSpec { indent: 1, icon: Icon::Point, name: "Origin", dim: true, ..RowSpec::plain() });
        }
        row(ui, RowSpec { indent: 0, icon: Icon::Material, name: "Material", sub: "Not set", ..RowSpec::plain() });

        // Flatten features: absorbed sketches sit under their consumer.
        let absorbed: Vec<FeatureId> = state.doc.features.iter().filter_map(|f| f.kind.sketch()).collect();
        let mut items: Vec<RowItem> = Vec::new();
        for (i, f) in state.doc.features.iter().enumerate() {
            if f.is_sketch() && absorbed.contains(&f.id) {
                continue;
            }
            let icon = match &f.kind {
                FeatureKind::Sketch(_) => Icon::Sketch,
                FeatureKind::Extrude(_) => Icon::Extrude,
                FeatureKind::Revolve(_) => Icon::Revolve,
                FeatureKind::Cut(_) => Icon::Cut,
                FeatureKind::Fillet(_) => Icon::Fillet,
                FeatureKind::Chamfer(_) => Icon::Chamfer,
                FeatureKind::Hole(_) => Icon::Hole,
                FeatureKind::Pattern(p) => match p.layout {
                    soliddraft_model::PatternSpec::Linear { .. } => Icon::LinearPattern,
                    soliddraft_model::PatternSpec::Circular { .. } => Icon::CircularPattern,
                },
                FeatureKind::Mirror(_) => Icon::Mirror,
                FeatureKind::Draft(_) => Icon::Draft,
            };
            let child = f.kind.sketch().and_then(|s| state.doc.feature(s));
            items.push(RowItem {
                id: f.id,
                index: i,
                indent: 0,
                chevron: child.map(|_| state.expanded.contains(&f.id)),
                icon,
                name: f.name.clone(),
                // A dragged dimension shows as a preview until the drag applies it.
                sub: match &state.drag_preview {
                    Some(d) if d.feature() == f.id => format!("Preview {} mm", fmt_mm(d.value())),
                    _ => f.summary(),
                },
                status: f.status.clone(),
                suppressed: f.suppressed,
                via_assistant: f.via_assistant,
                is_sketch: f.is_sketch(),
            });
            if let Some(c) = child
                && state.expanded.contains(&f.id)
            {
                items.push(RowItem {
                    id: c.id,
                    index: i,
                    indent: 1,
                    chevron: None,
                    icon: Icon::Sketch,
                    name: c.name.clone(),
                    sub: c.summary(),
                    status: c.status.clone(),
                    suppressed: c.suppressed,
                    via_assistant: false,
                    is_sketch: true,
                });
            }
        }
        if selected_feature.is_none() {
            state.tree_followed = None;
        }
        if items.is_empty() {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("No features yet").family(theme::medium()).color(theme::MUTED));
                ui.label(
                    egui::RichText::new("Sketch on a plane, or press E to extrude a built-in profile.")
                        .size(11.0)
                        .color(theme::MUTED),
                );
            });
        }

        let rollback = state.doc.rollback;
        let feature_count = state.doc.features.len();
        let pointer_released = ui.input(|i| i.pointer.primary_released());
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());
        let dragging = state.drag_feature;
        let mut drop_target: Option<usize> = None;
        let mut bar_target: Option<usize> = None;
        let mut rename_commit: Option<(FeatureId, String)> = None;
        let mut rename_cancel = false;
        let mut last_bottom = ui.cursor().top();
        let mut row_centres: Vec<(usize, f32)> = Vec::new();
        let mut bar_drawn = false;
        let bar_at = rollback.unwrap_or(feature_count);

        for item in &items {
            // The rollback bar sits before the first feature past it.
            if !bar_drawn && item.indent == 0 && item.index >= bar_at {
                rollback_bar(ui, state, bar_at, feature_count, &mut actions, &mut bar_target, pointer_released);
                bar_drawn = true;
            }
            let inactive = item.suppressed || matches!(item.status, FeatureStatus::Inactive);
            let error = match &item.status {
                FeatureStatus::Error(e) => Some(e.clone()),
                _ => None,
            };
            let repair = match &item.status {
                FeatureStatus::NeedsRepair(items) => Some(format!(
                    "{} reference{} missing: {}. Select the feature and choose Repair.",
                    items.len(),
                    if items.len() == 1 { "" } else { "s" },
                    items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>().join(", ")
                )),
                _ => None,
            };
            let renaming = state.rename.as_ref().filter(|r| r.feature == item.id).cloned();
            let resp = if let Some(r) = renaming {
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_HEIGHT), Sense::hover());
                let mut text = r.text.clone();
                let edit = ui.put(
                    Rect::from_min_max(
                        pos2(rect.left() + 44.0 + item.indent as f32 * 20.0, rect.top() + 1.0),
                        pos2(rect.right() - 6.0, rect.bottom() - 1.0),
                    ),
                    egui::TextEdit::singleline(&mut text).font(theme::font(12.5)),
                );
                edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Feature name"));
                if r.focus {
                    edit.request_focus();
                }
                if let Some(rn) = state.rename.as_mut() {
                    rn.text = text.clone();
                    rn.focus = false;
                }
                icons::paint(
                    ui.painter(),
                    item.icon,
                    Rect::from_center_size(
                        pos2(rect.left() + 32.0 + item.indent as f32 * 20.0, rect.center().y),
                        vec2(15.0, 15.0),
                    ),
                    theme::INK,
                );
                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    || (edit.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape)))
                {
                    rename_commit = Some((item.id, text));
                } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    rename_cancel = true;
                }
                resp
            } else {
                row(
                    ui,
                    RowSpec {
                        indent: item.indent,
                        chevron: item.chevron,
                        icon: item.icon,
                        name: &item.name,
                        sub: &item.sub,
                        selected: selected_feature == Some(item.id),
                        dim: inactive,
                        error: error.as_deref(),
                        warn: repair.as_deref(),
                        via_assistant: item.via_assistant,
                        suppressed: item.suppressed,
                        drop_above: dragging.is_some_and(|d| d != item.id)
                            && pointer_pos.is_some_and(|p| {
                                let r = Rect::from_min_size(pos2(0.0, last_bottom), vec2(10_000.0, theme::ROW_HEIGHT));
                                r.contains(pos2(r.center().x, p.y))
                            }),
                    },
                )
            };
            last_bottom = resp.rect.bottom();
            if item.indent == 0 {
                row_centres.push((item.index, resp.rect.center().y));
            }
            // A newly selected feature scrolls into view, wherever the selection came from.
            if selected_feature == Some(item.id) && state.tree_followed != Some(item.id) {
                state.tree_followed = Some(item.id);
                resp.scroll_to_me(None);
            }
            if dragging.is_some_and(|d| d != item.id) && resp.hovered() && pointer_released {
                drop_target = Some(item.index);
            }
            if resp.drag_started_by(egui::PointerButton::Primary) && item.indent == 0 {
                state.drag_feature = Some(item.id);
            }
            if resp.clicked() {
                let chevron_zone = resp.rect.left() + 20.0;
                if item.chevron.is_some() && resp.interact_pointer_pos().is_some_and(|p| p.x < chevron_zone) {
                    if state.expanded.contains(&item.id) {
                        state.expanded.remove(&item.id);
                    } else {
                        state.expanded.insert(item.id);
                    }
                } else if state.doc.selection == Some(Selection::Feature(item.id))
                    && state.rename.is_none()
                    && !ui.input(|i| i.modifiers.any())
                {
                    // A second click on the selected row starts a rename, like a file manager.
                    state.rename = Some(Rename { feature: item.id, text: item.name.clone(), focus: true });
                } else {
                    actions.push(AppAction::Select(Some(Selection::Feature(item.id))));
                }
            }
            if resp.double_clicked() {
                state.rename = None;
                actions.push(if item.is_sketch {
                    AppAction::EditSketch(item.id)
                } else {
                    AppAction::EditFeature(item.id)
                });
            }
            let id = item.id;
            let (is_sketch, suppressed, index) = (item.is_sketch, item.suppressed, item.index);
            let needs_repair = matches!(item.status, FeatureStatus::NeedsRepair(_));
            let name = item.name.clone();
            resp.context_menu(|ui| {
                if ui.button(if is_sketch { "Edit sketch" } else { "Edit feature" }).clicked() {
                    actions.push(if is_sketch { AppAction::EditSketch(id) } else { AppAction::EditFeature(id) });
                    ui.close();
                }
                if needs_repair && ui.button("Repair references…").clicked() {
                    actions.push(AppAction::StartRepair(id));
                    ui.close();
                }
                if ui.add(egui::Button::new("Rename").shortcut_text("F2")).clicked() {
                    state.rename = Some(Rename { feature: id, text: name.clone(), focus: true });
                    ui.close();
                }
                if ui.button(if suppressed { "Unsuppress" } else { "Suppress" }).clicked() {
                    actions.push(AppAction::Command(Command::Suppress { id, suppressed: !suppressed }));
                    ui.close();
                }
                if ui.button("Roll back to here").clicked() {
                    actions.push(AppAction::Command(Command::Rollback(Some(index + 1))));
                    ui.close();
                }
                if ui.button("Zoom to fit").clicked() {
                    actions.push(AppAction::ZoomFit);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add(egui::Button::new(egui::RichText::new("Delete").color(theme::ERROR)).shortcut_text("Del"))
                    .clicked()
                {
                    actions.push(AppAction::Command(Command::DeleteFeature(id)));
                    ui.close();
                }
            });
        }
        if !bar_drawn && !items.is_empty() {
            rollback_bar(ui, state, bar_at, feature_count, &mut actions, &mut bar_target, pointer_released);
        }

        // Finish a drag.
        if let Some((id, text)) = rename_commit {
            state.rename = None;
            if state.doc.feature(id).is_some_and(|f| f.name != text) && !text.trim().is_empty() {
                actions.push(AppAction::Command(Command::Rename { id, name: text }));
            }
        }
        if rename_cancel {
            state.rename = None;
        }
        if let Some(d) = dragging {
            if let Some(p) = pointer_pos {
                let painter =
                    ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("tree_drag")));
                let name = state.doc.feature(d).map(|f| f.name.clone()).unwrap_or_default();
                let galley = painter.layout_no_wrap(name, theme::font_medium(12.0), theme::INK);
                let r = Rect::from_min_size(p + vec2(12.0, -10.0), galley.size() + vec2(16.0, 8.0));
                painter.rect(
                    r,
                    CornerRadius::same(6),
                    theme::PANEL,
                    Stroke::new(1.0, theme::ACCENT),
                    StrokeKind::Inside,
                );
                painter.galley(r.min + vec2(8.0, 4.0), galley, theme::INK);
            }
            if pointer_released {
                state.drag_feature = None;
                if let Some(target) = drop_target {
                    actions.push(AppAction::Command(Command::Reorder { id: d, index: target }));
                }
            }
        }
        if state.drag_rollback {
            if let Some(p) = pointer_pos {
                let painter = ui.painter();
                let y = p.y.clamp(ui.min_rect().top(), ui.min_rect().bottom());
                painter.hline(ui.min_rect().x_range().shrink(10.0), y, Stroke::new(2.0, theme::ACCENT));
            }
            if pointer_released {
                state.drag_rollback = false;
                if let Some(p) = pointer_pos {
                    // Build every feature whose row centre is above the pointer.
                    bar_target =
                        Some(row_centres.iter().filter(|(_, cy)| *cy < p.y).map(|(i, _)| i + 1).max().unwrap_or(0));
                }
            }
        }
        if let Some(t) = bar_target {
            actions.push(AppAction::Command(Command::Rollback(if t >= feature_count { None } else { Some(t) })));
        }
        ui.add_space(6.0);
    });
    for a in actions {
        state.push(a);
    }
}

/// The rollback bar: everything below it is not built. Drag it between rows.
fn rollback_bar(
    ui: &mut egui::Ui,
    state: &mut AppState,
    at: usize,
    count: usize,
    actions: &mut Vec<AppAction>,
    target: &mut Option<usize>,
    released: bool,
) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Rollback bar"));
    let active = at < count;
    let color = if resp.hovered() || resp.dragged() {
        theme::ACCENT
    } else if active {
        theme::WARN
    } else {
        theme::LINE.gamma_multiply(1.3)
    };
    let y = rect.center().y;
    ui.painter().hline(
        (rect.left() + 10.0)..=(rect.right() - 10.0),
        y,
        Stroke::new(if active { 2.5 } else { 2.0 }, color),
    );
    ui.painter().rect_filled(
        Rect::from_center_size(pos2(rect.center().x, y), vec2(26.0, 6.0)),
        CornerRadius::same(3),
        color,
    );
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let resp = resp.on_hover_text(if active {
        "Rollback bar: features below are not built. Drag it down, or right-click a feature."
    } else {
        "Rollback bar: drag it up to build only the features above it."
    });
    if resp.drag_started() {
        state.drag_feature = None;
        state.drag_rollback = true;
    }
    if resp.double_clicked() && active {
        actions.push(AppAction::Command(Command::Rollback(None)));
    }
    let _ = (released, &target);
}

struct RowSpec<'a> {
    indent: u8,
    chevron: Option<bool>,
    icon: Icon,
    name: &'a str,
    sub: &'a str,
    selected: bool,
    dim: bool,
    error: Option<&'a str>,
    warn: Option<&'a str>,
    via_assistant: bool,
    suppressed: bool,
    drop_above: bool,
}

impl RowSpec<'static> {
    fn plain() -> Self {
        RowSpec {
            indent: 0,
            chevron: None,
            icon: Icon::Point,
            name: "",
            sub: "",
            selected: false,
            dim: false,
            error: None,
            warn: None,
            via_assistant: false,
            suppressed: false,
            drop_above: false,
        }
    }
}

fn row(ui: &mut egui::Ui, spec: RowSpec<'_>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_HEIGHT), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, spec.selected, spec.name));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    let inner = Rect::from_min_max(pos2(rect.left() + 4.0, rect.top()), pos2(rect.right() - 4.0, rect.bottom()));
    if spec.selected {
        painter.rect_filled(inner, CornerRadius::same(5), theme::SELECTION_BG);
    } else if resp.hovered() {
        painter.rect_filled(inner, CornerRadius::same(5), theme::HOVER);
    }
    if spec.drop_above {
        painter.hline(inner.x_range(), inner.top() + 0.5, Stroke::new(2.0, theme::ACCENT));
    }
    let x0 = rect.left() + 10.0 + spec.indent as f32 * 20.0;
    let ink = if spec.selected {
        theme::SELECTION_INK
    } else if spec.dim {
        theme::MUTED
    } else {
        theme::INK
    };
    if let Some(open) = spec.chevron {
        let icon = if open { Icon::ChevronDown } else { Icon::ChevronRight };
        icons::paint(
            painter,
            icon,
            Rect::from_center_size(pos2(x0 + 4.0, rect.center().y), vec2(11.0, 11.0)),
            theme::MUTED,
        );
    }
    let icon_color = if spec.error.is_some() {
        theme::ERROR
    } else if spec.warn.is_some() {
        theme::WARN
    } else {
        ink
    };
    icons::paint(
        painter,
        spec.icon,
        Rect::from_center_size(pos2(x0 + 22.0, rect.center().y), vec2(15.0, 15.0)),
        icon_color,
    );
    let font = if spec.dim { theme::font(12.0) } else { theme::font(12.5) };
    let name_rect = painter.text(pos2(x0 + 35.0, rect.center().y), Align2::LEFT_CENTER, spec.name, font, ink);
    let mut after = name_rect.right() + 6.0;
    if spec.suppressed {
        icons::paint(
            painter,
            Icon::Suppress,
            Rect::from_center_size(pos2(after + 6.0, rect.center().y), vec2(12.0, 12.0)),
            theme::MUTED,
        );
        after += 16.0;
    }
    if spec.via_assistant {
        icons::paint(
            painter,
            Icon::Spark,
            Rect::from_center_size(pos2(after + 6.0, rect.center().y), vec2(11.0, 11.0)),
            theme::ACCENT,
        );
    }
    if !spec.sub.is_empty() {
        painter.text(
            pos2(inner.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            spec.sub,
            theme::font(11.0),
            theme::MUTED,
        );
    }
    if let Some(err) = spec.error {
        painter.circle_filled(pos2(x0 + 29.0, rect.top() + 6.0), 3.0, theme::ERROR);
        return resp.on_hover_text(egui::RichText::new(err).color(Color32::WHITE));
    }
    if let Some(w) = spec.warn {
        icons::paint(
            painter,
            Icon::Warning,
            Rect::from_center_size(pos2(x0 + 30.0, rect.top() + 6.5), vec2(9.0, 9.0)),
            theme::WARN,
        );
        return resp.on_hover_text(egui::RichText::new(w).color(Color32::WHITE));
    }
    resp
}
