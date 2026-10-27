//! The sheet tabs under the canvas, as SolidWorks shows them: one tab per sheet, a plus
//! to add one, a double-click or the context menu to rename, and the context menu to
//! move a sheet left or right or delete it.

use super::{DrawingAction, DrawingSession};
use crate::theme;
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use soliddraft_drawing::{Command, Drawing};

/// Height of the strip in points.
pub const STRIP_HEIGHT: f32 = 26.0;

/// "Sheet3": the first sheet name of that form the drawing does not have yet.
pub fn fresh_name(drawing: &Drawing) -> String {
    let mut n = drawing.sheets.len() + 1;
    loop {
        let name = format!("Sheet{n}");
        if !drawing.sheets.iter().any(|s| s.name == name) {
            return name;
        }
        n += 1;
    }
}

/// Why a sheet cannot be deleted, or `None` when it can.
pub fn delete_blocker(drawing: &Drawing, sheet: usize) -> Option<String> {
    if drawing.sheets.len() <= 1 {
        return Some("A drawing keeps at least one sheet".into());
    }
    let s = drawing.sheets.get(sheet)?;
    let ids: Vec<_> = s.views.iter().map(|v| v.id).collect();
    let elsewhere = drawing
        .sheets
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != sheet)
        .flat_map(|(_, s)| &s.views)
        .find(|v| v.parent().is_some_and(|p| ids.contains(&p)));
    elsewhere.map(|v| format!("{} on another sheet is made from a view on this one", v.id))
}

pub(super) fn strip(ui: &mut egui::Ui, s: &mut DrawingSession, rect: Rect, push: &mut impl FnMut(DrawingAction)) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, theme::panel());
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, theme::line()));
    let count = s.doc.drawing.sheets.len();
    let names: Vec<String> = s.doc.drawing.sheets.iter().map(|sh| sh.name.clone()).collect();
    let mut x = rect.left() + 8.0;
    let mut rename_done: Option<(usize, String)> = None;
    let mut rename_cancel = false;
    for (i, name) in names.iter().enumerate() {
        let editing = s.sheet_rename.as_ref().is_some_and(|(j, _)| *j == i);
        let galley = painter.layout_no_wrap(name.clone(), theme::font_medium(11.5), theme::ink());
        let w = galley.size().x.max(48.0) + 22.0;
        let r = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, rect.height() - 6.0));
        x += w + 2.0;
        let on = i == s.sheet;
        let resp = ui.interact(r, ui.id().with(("sheet_tab", i)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, format!("{name} tab")));
        if on {
            painter.rect(
                r,
                CornerRadius { nw: 0, ne: 0, sw: theme::RADIUS, se: theme::RADIUS },
                theme::canvas_bg(),
                Stroke::new(1.0, theme::line()),
                StrokeKind::Inside,
            );
            painter.hline((r.left() + 1.0)..=(r.right() - 1.0), rect.top() + 0.5, Stroke::new(1.0, theme::canvas_bg()));
        } else if resp.hovered() {
            painter.rect_filled(r, CornerRadius { nw: 0, ne: 0, sw: theme::RADIUS, se: theme::RADIUS }, theme::hover());
        }
        if editing {
            let mut text = s.sheet_rename.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
            let edit_rect = r.shrink2(vec2(4.0, 2.0));
            let mut submitted = false;
            ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                let e = ui.add(
                    egui::TextEdit::singleline(&mut text).desired_width(edit_rect.width()).font(theme::font(11.5)),
                );
                e.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Sheet name"));
                if !e.has_focus() && !e.lost_focus() {
                    e.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submitted = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    rename_cancel = true;
                }
                if e.lost_focus() && !rename_cancel {
                    submitted = true;
                }
            });
            if submitted {
                rename_done = Some((i, text.clone()));
            } else if let Some(r) = s.sheet_rename.as_mut() {
                r.1 = text;
            }
            continue;
        }
        painter.galley(r.center() - galley.size() / 2.0, galley, if on { theme::ink() } else { theme::muted() });
        if resp.double_clicked() {
            s.sheet_rename = Some((i, name.clone()));
        } else if resp.clicked() {
            push(DrawingAction::SetSheet(i));
        }
        let blocker = delete_blocker(&s.doc.drawing, i);
        resp.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                s.sheet_rename = Some((i, name.clone()));
                ui.close();
            }
            if i > 0 && ui.button("Move left").clicked() {
                push(DrawingAction::MoveSheet { sheet: i, to: i - 1 });
                ui.close();
            }
            if i + 1 < count && ui.button("Move right").clicked() {
                push(DrawingAction::MoveSheet { sheet: i, to: i + 1 });
                ui.close();
            }
            let delete = ui.add_enabled(blocker.is_none(), egui::Button::new("Delete sheet"));
            let delete = match &blocker {
                Some(why) => delete.on_disabled_hover_text(why.clone()),
                None => delete,
            };
            if delete.clicked() {
                push(DrawingAction::DeleteSheet(i));
                ui.close();
            }
        });
    }
    if let Some((i, text)) = rename_done {
        let text = text.trim().to_owned();
        if !text.is_empty() && names.get(i).is_some_and(|n| *n != text) {
            push(DrawingAction::Command(Command::RenameSheet { sheet: i, name: text }));
        }
        s.sheet_rename = None;
    }
    if rename_cancel {
        s.sheet_rename = None;
    }
    // The plus adds a sheet from the first sheet's template.
    let r = Rect::from_min_size(pos2(x + 2.0, rect.top() + 4.0), vec2(22.0, rect.height() - 8.0));
    let resp = ui.interact(r, ui.id().with("sheet_add"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add sheet"));
    if resp.hovered() {
        painter.rect_filled(r, CornerRadius::same(5), theme::hover());
    }
    painter.text(r.center(), Align2::CENTER_CENTER, "+", theme::font_medium(14.0), theme::muted());
    if resp.on_hover_text("Add a sheet after the last one").clicked() {
        push(DrawingAction::AddSheet);
    }
    painter.text(
        pos2(rect.right() - 10.0, rect.center().y),
        Align2::RIGHT_CENTER,
        format!("Sheet {} of {count} · double-click a tab to rename", s.sheet + 1),
        theme::font(10.5),
        theme::muted(),
    );
}
