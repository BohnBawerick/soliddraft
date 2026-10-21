//! The drawing tree: document, sheets with their size, scale and projection, the sheet
//! format, each view with its model reference and its children indented under it,
//! annotations under their views, and tables. Status dots: green up to date, amber
//! changed, red dangling.

use super::{DrawingAction, DrawingSel, DrawingTool, LinkState, PlaceKind, SHEET};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, pos2, vec2};
use soliddraft_drawing::display::annotation_name;
use soliddraft_drawing::display::scale_text;
use soliddraft_drawing::project::effective_scale;
use soliddraft_drawing::project::view_label;
use soliddraft_drawing::{AnnotationKind, AnnotationState, Command, Projection, ViewId, ViewKind, ViewScale};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dot {
    None,
    Ok,
    Warn,
    Error,
}

struct RowSpec<'a> {
    indent: u8,
    chevron: Option<bool>,
    icon: Icon,
    name: &'a str,
    sub: &'a str,
    selected: bool,
    dim: bool,
    dot: Dot,
    tip: Option<&'a str>,
}

fn row(ui: &mut egui::Ui, spec: RowSpec<'_>) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_HEIGHT), Sense::click());
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
    let x0 = rect.left() + 10.0 + spec.indent as f32 * 18.0;
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
    icons::paint(painter, spec.icon, Rect::from_center_size(pos2(x0 + 20.0, rect.center().y), vec2(15.0, 15.0)), ink);
    let name_x = x0 + 32.0;
    let galley = painter.layout_no_wrap(spec.name.to_owned(), theme::font(12.5), ink);
    painter.galley(pos2(name_x, rect.center().y - galley.size().y / 2.0), galley.clone(), ink);
    if !spec.sub.is_empty() {
        painter.text(
            pos2(name_x + galley.size().x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            spec.sub,
            theme::font(11.0),
            theme::MUTED,
        );
    }
    let dot_colour = match spec.dot {
        Dot::None => None,
        Dot::Ok => Some(theme::OK),
        Dot::Warn => Some(theme::WARN),
        Dot::Error => Some(theme::ERROR),
    };
    if let Some(c) = dot_colour {
        painter.circle_filled(pos2(rect.right() - 14.0, rect.center().y), 3.5, c);
    }
    match spec.tip {
        Some(t) => resp.on_hover_text(t),
        None => resp,
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(s) = state.drawing.session.as_mut() else { return };
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let title = if s.doc.dirty { format!("{} •", s.title()) } else { s.title() };
    ui.painter().text(
        pos2(head.left() + 12.0, head.center().y),
        Align2::LEFT_CENTER,
        title,
        theme::font_semibold(12.5),
        theme::INK,
    );
    let dangling = s.doc.dangling_count();
    let changed = s.doc.attention_count() - dangling;
    let (status, colour) = if let Some(marker) = s.marker() {
        marker
    } else if dangling > 0 {
        (format!("{dangling} dangling"), theme::ERROR)
    } else if changed > 0 {
        (format!("{changed} changed"), theme::WARN)
    } else {
        (format!("Up to date · rev {}", s.doc.drawing.model.pinned.short()), theme::OK)
    };
    ui.painter().text(
        pos2(head.right() - 12.0, head.center().y),
        Align2::RIGHT_CENTER,
        status,
        theme::font_medium(11.0),
        colour,
    );
    ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, theme::LINE));

    let mut actions: Vec<DrawingAction> = Vec::new();
    let selection = s.selection;
    egui::ScrollArea::vertical().id_salt("drawing_tree").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(4.0);
        let doc_name = s.title();
        if row(
            ui,
            RowSpec {
                indent: 0,
                chevron: None,
                icon: Icon::Open,
                name: &doc_name,
                sub: "",
                selected: false,
                dim: false,
                dot: Dot::None,
                tip: Some("The drawing document"),
            },
        )
        .clicked()
        {
            actions.push(DrawingAction::Select(None));
        }
        // The model reference: the part the drawing shows, with the SolidWorks
        // out-of-date glyph when a newer version is available or the part is missing.
        {
            let model = &s.doc.drawing.model;
            let file = model.path.rsplit('/').next().unwrap_or(&model.path).to_owned();
            let sub = format!("rev {}", model.pinned.short());
            let state = s.link_state();
            let (dot, tip) = match &state {
                LinkState::Missing => (Dot::Error, "Not found: select for the repair".to_owned()),
                LinkState::Newer { now } => (
                    Dot::Warn,
                    format!("Newer version available (rev {}): Rebuild brings the views to it", now.short()),
                ),
                _ if s.doc.is_stale() => (Dot::Warn, "The open part moved on: Rebuild brings the views to it".into()),
                LinkState::Current => (Dot::Ok, format!("Up to date · {}", s.found_words())),
                LinkState::Unverified => (Dot::None, "Not a SolidDraft document; no revision to compare".into()),
                LinkState::Unknown => (Dot::None, "Not resolved yet".into()),
            };
            let r = row(
                ui,
                RowSpec {
                    indent: 1,
                    chevron: None,
                    icon: Icon::Extrude,
                    name: &file,
                    sub: &sub,
                    selected: selection == Some(DrawingSel::Model),
                    dim: false,
                    dot,
                    tip: Some(&tip),
                },
            );
            if let Some(colour) = match &state {
                LinkState::Missing => Some(theme::ERROR),
                LinkState::Newer { .. } => Some(theme::WARN),
                _ if s.doc.is_stale() => Some(theme::WARN),
                _ => None,
            } {
                let glyph = Rect::from_center_size(pos2(r.rect.right() - 34.0, r.rect.center().y), vec2(14.0, 14.0));
                super::reference::glyph(ui.painter(), glyph, colour);
            }
            if r.clicked() {
                actions.push(DrawingAction::Select(Some(DrawingSel::Model)));
            }
            r.context_menu(|ui| {
                if ui.button("Rebuild").clicked() {
                    actions.push(DrawingAction::Rebuild);
                    ui.close();
                }
                if state == LinkState::Missing && ui.button("Repair reference…").clicked() {
                    actions.push(DrawingAction::ShowRepair { replace: false });
                    ui.close();
                }
                if ui.button("Replace model…").clicked() {
                    actions.push(DrawingAction::ShowRepair { replace: true });
                    ui.close();
                }
            });
        }
        for (si, sheet) in s.doc.drawing.sheets.iter().enumerate() {
            let projection = match sheet.projection {
                Projection::FirstAngle => "first angle",
                Projection::ThirdAngle => "third angle",
            };
            let sub = format!("({} · {} · {projection})", sheet.size.name, sheet.scale);
            let sheet_name = sheet.name.replace("Sheet", "Sheet ");
            let r = row(
                ui,
                RowSpec {
                    indent: 1,
                    chevron: Some(true),
                    icon: Icon::Rectangle,
                    name: &sheet_name,
                    sub: &sub,
                    selected: selection.is_none() && si == s.sheet,
                    dim: false,
                    dot: Dot::None,
                    tip: Some("Sheet properties are in the property panel"),
                },
            );
            if r.clicked() {
                actions.push(DrawingAction::SetSheet(si));
                actions.push(DrawingAction::Select(None));
            }
            let sheet_count = s.doc.drawing.sheets.len();
            let blocker = super::sheets::delete_blocker(&s.doc.drawing, si);
            r.context_menu(|ui| {
                if ui.button("Rename…").clicked() {
                    actions.push(DrawingAction::SetSheet(si));
                    actions.push(DrawingAction::Select(None));
                    s.sheet_rename = Some((si, sheet.name.clone()));
                    ui.close();
                }
                if ui.button("Add sheet").clicked() {
                    actions.push(DrawingAction::AddSheet);
                    ui.close();
                }
                if si > 0 && ui.button("Move up").clicked() {
                    actions.push(DrawingAction::MoveSheet { sheet: si, to: si - 1 });
                    ui.close();
                }
                if si + 1 < sheet_count && ui.button("Move down").clicked() {
                    actions.push(DrawingAction::MoveSheet { sheet: si, to: si + 1 });
                    ui.close();
                }
                let delete = ui.add_enabled(blocker.is_none(), egui::Button::new("Delete sheet"));
                let delete = match &blocker {
                    Some(why) => delete.on_disabled_hover_text(why.clone()),
                    None => delete,
                };
                if delete.clicked() {
                    actions.push(DrawingAction::DeleteSheet(si));
                    ui.close();
                }
            });
            let format_name = format!("Sheet format {}", s.doc.template().name.split(" ·").next().unwrap_or("").trim());
            let r = row(
                ui,
                RowSpec {
                    indent: 2,
                    chevron: None,
                    icon: Icon::Props,
                    name: &format_name,
                    sub: "",
                    selected: selection == Some(DrawingSel::Format),
                    dim: false,
                    dot: Dot::None,
                    tip: Some("Border, zones and title block, from the template"),
                },
            );
            if r.clicked() {
                actions.push(DrawingAction::Select(Some(DrawingSel::Format)));
            }
            // Views: parents first, children indented under them.
            let model = &s.doc.drawing.meta.title;
            let rev = s.doc.drawing.model.pinned.short().to_owned();
            let stale = s.doc.stale_views();
            // Each view right after its parent, children in sheet order.
            fn descend(sheet: &soliddraft_drawing::Sheet, parent: ViewId, depth: u8, order: &mut Vec<(ViewId, u8)>) {
                for c in sheet.views.iter().filter(|c| c.parent() == Some(parent)) {
                    order.push((c.id, depth));
                    if depth < 12 {
                        descend(sheet, c.id, depth + 1, order);
                    }
                }
            }
            let mut order: Vec<(ViewId, u8)> = Vec::new();
            for v in sheet.views.iter().filter(|v| v.parent().is_none() || sheet.view(v.parent().unwrap()).is_none()) {
                order.push((v.id, 2));
                descend(sheet, v.id, 3, &mut order);
            }
            for (id, depth) in order {
                let v = sheet.view(id).unwrap();
                let name = view_label(sheet, v);
                // A derived view's scale and its location on another sheet.
                let mut sub = String::new();
                let own = effective_scale(sheet, v);
                if matches!(v.kind, ViewKind::Detail { .. })
                    || (v.scale != ViewScale::Sheet && (own - sheet.scale.factor()).abs() > 1e-9)
                {
                    sub = format!("({})", scale_text(own));
                }
                if let Some(p) = v.parent()
                    && sheet.view(p).is_none()
                    && let Some(ps) = s.doc.drawing.sheets.iter().position(|sh| sh.view(p).is_some())
                {
                    sub = format!("{sub} from Sheet {}", ps + 1).trim().to_owned();
                }
                let icon = match v.kind {
                    ViewKind::Section { .. } => Icon::Section,
                    ViewKind::Detail { .. } => Icon::Search,
                    ViewKind::Auxiliary { .. } => Icon::Plane,
                    _ => Icon::Display,
                };
                let dot = if stale.contains(&id) {
                    Dot::Warn
                } else {
                    let worst = sheet
                        .annotations_of(id)
                        .map(|a| match a.state {
                            AnnotationState::Ok => 0,
                            AnnotationState::Changed { .. } => 1,
                            AnnotationState::Dangling { .. } => 2,
                        })
                        .max()
                        .unwrap_or(0);
                    match worst {
                        2 => Dot::Error,
                        1 => Dot::Warn,
                        _ => Dot::Ok,
                    }
                };
                let open = s.expanded.contains(&id) || depth == 2;
                let r = row(
                    ui,
                    RowSpec {
                        indent: depth,
                        chevron: Some(open),
                        icon,
                        name: &name,
                        sub: &sub,
                        selected: selection == Some(DrawingSel::View(id)),
                        dim: v.excluded_from_update,
                        dot,
                        tip: Some(if stale.contains(&id) {
                            "Out of date: update views"
                        } else {
                            "Click to select; double-click to fold a view off it"
                        }),
                    },
                );
                if r.clicked() {
                    if r.interact_pointer_pos().is_some_and(|p| p.x < r.rect.left() + 14.0 + depth as f32 * 18.0) {
                        if s.expanded.contains(&id) {
                            s.expanded.remove(&id);
                        } else {
                            s.expanded.insert(id);
                        }
                    } else {
                        actions.push(DrawingAction::Select(Some(DrawingSel::View(id))));
                    }
                }
                if r.double_clicked() {
                    actions.push(DrawingAction::SetTool(DrawingTool::PlaceView(PlaceKind::Projected { parent: id })));
                }
                let id_copy = id;
                let excluded = v.excluded_from_update;
                let aligned = v.alignment.is_some();
                r.context_menu(|ui| {
                    if ui.button("Projected view from here").clicked() {
                        actions.push(DrawingAction::SetTool(DrawingTool::PlaceView(PlaceKind::Projected {
                            parent: id_copy,
                        })));
                        ui.close();
                    }
                    if aligned && ui.button("Break alignment").clicked() {
                        actions.push(DrawingAction::Command(soliddraft_drawing::Command::BreakAlignment {
                            view: id_copy,
                        }));
                        ui.close();
                    }
                    if ui
                        .button(if excluded { "Include in automatic update" } else { "Exclude from automatic update" })
                        .clicked()
                    {
                        actions.push(DrawingAction::Command(Command::SetViewExcluded {
                            view: id_copy,
                            excluded: !excluded,
                        }));
                        ui.close();
                    }
                    if ui.button("Delete view").clicked() {
                        actions.push(DrawingAction::Select(Some(DrawingSel::View(id_copy))));
                        actions.push(DrawingAction::DeleteSelection);
                        ui.close();
                    }
                });
                if open && depth == 2 {
                    let reference = format!("{model} rev {rev}");
                    let tip = if stale.contains(&id) {
                        "Newer version available; this view shows the pinned revision until a Rebuild"
                    } else {
                        "The model this view shows, at the revision of the last update"
                    };
                    row(
                        ui,
                        RowSpec {
                            indent: depth + 1,
                            chevron: None,
                            icon: Icon::Extrude,
                            name: &reference,
                            sub: "",
                            selected: false,
                            dim: true,
                            dot: Dot::None,
                            tip: Some(tip),
                        },
                    );
                }
            }
            // Annotations.
            let n = sheet.annotations.len();
            let attention = sheet.annotations.iter().filter(|a| a.needs_attention()).count();
            let dangling_here =
                sheet.annotations.iter().filter(|a| matches!(a.state, AnnotationState::Dangling { .. })).count();
            let label = if dangling_here > 0 {
                format!("Annotations ({n} · {dangling_here} dangling)")
            } else if attention > 0 {
                format!("Annotations ({n} · {attention} changed)")
            } else {
                format!("Annotations ({n})")
            };
            let r = row(
                ui,
                RowSpec {
                    indent: 2,
                    chevron: Some(s.annotations_open),
                    icon: Icon::SmartDim,
                    name: &label,
                    sub: "",
                    selected: false,
                    dim: false,
                    dot: Dot::None,
                    tip: None,
                },
            );
            if r.clicked() {
                s.annotations_open = !s.annotations_open;
            }
            if s.annotations_open {
                let style = s.doc.style();
                for a in &sheet.annotations {
                    let name = annotation_name(a, style);
                    let view_name = if a.view == SHEET {
                        "sheet".to_owned()
                    } else {
                        sheet.view(a.view).map(|v| view_label(sheet, v)).unwrap_or_default()
                    };
                    let (dot, sub) = match &a.state {
                        AnnotationState::Ok => (Dot::Ok, view_name.clone()),
                        AnnotationState::Changed { previous } => (Dot::Warn, format!("was {previous} · {view_name}")),
                        AnnotationState::Dangling { .. } => (Dot::Error, format!("dangling · {view_name}")),
                    };
                    let icon = match &a.body {
                        AnnotationKind::Dimension(_) | AnnotationKind::DimensionSet(_) => Icon::SmartDim,
                        AnnotationKind::Note { .. } => Icon::Chat,
                        AnnotationKind::HoleCallout { .. } => Icon::Hole,
                        AnnotationKind::CentreMark | AnnotationKind::Centreline => Icon::Point,
                        AnnotationKind::Gdt { .. } => Icon::Rectangle,
                        AnnotationKind::Datum { .. } => Icon::Plane,
                        AnnotationKind::RevisionSymbol { .. } => Icon::Rollback,
                        _ => Icon::Material,
                    };
                    let r = row(
                        ui,
                        RowSpec {
                            indent: 3,
                            chevron: None,
                            icon,
                            name: &name,
                            sub: &sub,
                            selected: selection == Some(DrawingSel::Annotation(a.id)),
                            dim: false,
                            dot,
                            tip: None,
                        },
                    );
                    if r.clicked() {
                        actions.push(DrawingAction::Select(Some(DrawingSel::Annotation(a.id))));
                    }
                    let id = a.id;
                    r.context_menu(|ui| {
                        if ui.button("Delete").clicked() {
                            actions.push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
                            actions.push(DrawingAction::DeleteSelection);
                            ui.close();
                        }
                    });
                }
            }
            let revisions = sheet.revision_table().map(|t| t.revisions.len());
            let rev_sub = match revisions {
                Some(n) => format!("({n} row{})", if n == 1 { "" } else { "s" }),
                None => "(none yet)".to_owned(),
            };
            let r = row(
                ui,
                RowSpec {
                    indent: 2,
                    chevron: None,
                    icon: Icon::Tree,
                    name: "Revision table",
                    sub: &rev_sub,
                    selected: selection == Some(DrawingSel::Revisions) && si == s.sheet,
                    dim: revisions.is_none(),
                    dot: Dot::None,
                    tip: Some("The sheet's revisions; the first row makes the table"),
                },
            );
            if r.clicked() {
                actions.push(DrawingAction::SetSheet(si));
                actions.push(DrawingAction::Select(Some(DrawingSel::Revisions)));
            }
        }
        ui.add_space(8.0);
        let _ = Color32::WHITE;
    });
    for a in actions {
        state.push(AppAction::Drawing(a));
    }
}
