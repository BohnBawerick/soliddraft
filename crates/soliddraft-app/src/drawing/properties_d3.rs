//! The phase D3 PropertyManagers: the panels of the section, detail, auxiliary, crop,
//! broken-out, dimension set and symbol tools while they run; the sections a derived
//! view adds to the view panel (letter, direction, hatch, magnification, crop,
//! broken-out sections, location labels, sheet); the editors of geometric tolerances,
//! datums, surface finishes, revision symbols and dimension sets; and the revision
//! table.

use super::properties_drawing::{choice, header, hint, primary_button, section_gap, small_button, value_row};
use super::{DrawingAction, DrawingSel, DrawingSession, DrawingTool, FinishDraft, GdtDraft, RevisionDraft};
use crate::icons::Icon;
use crate::theme;
use egui::{Rect, pos2, vec2};
use soliddraft_drawing::display::{self, scale_text};
use soliddraft_drawing::library;
use soliddraft_drawing::project::{effective_scale, view_label};
use soliddraft_drawing::{
    AnnotationKind, Command, DatumRef, GdtCharacteristic, Lay, LinearOrientation, MaterialCondition, RevisionRow,
    SetKind, SurfaceProcess, TableKind, View, ViewId, ViewKind, ViewScale, next_letter,
};

/// A labelled row: the label in the left column, the widget filling the rest.
fn labelled(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(68.0, 22.0), egui::Label::new(egui::RichText::new(label).size(12.0).color(theme::muted())));
        add(ui);
    });
}

/// A single-line text field that reports its text when the edit is done.
fn text_field(ui: &mut egui::Ui, label: &str, value: &mut String, placeholder: &str) -> bool {
    let mut done = false;
    labelled(ui, label, |ui| {
        let r =
            ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width() - 20.0).hint_text(placeholder));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, format!("{label} field")));
        done = r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    });
    done
}

fn cancel_button(ui: &mut egui::Ui, head: Rect, actions: &mut Vec<DrawingAction>) {
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
}

fn frame(ui: &mut egui::Ui, salt: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, add);
    });
}

/// The panel of the tool that is running, when it is one of this phase's.
pub fn tool_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    match s.tool.clone() {
        DrawingTool::Section { replace } => section_panel(ui, s, replace, actions),
        DrawingTool::Detail => detail_panel(ui, s, actions),
        DrawingTool::Auxiliary => auxiliary_panel(ui, s, actions),
        DrawingTool::Crop => simple_panel(
            ui,
            Icon::Rectangle,
            "Crop view",
            "Click two opposite corners on a view: only the line work inside the rectangle stays. The view's properties remove the crop.",
            actions,
        ),
        DrawingTool::BrokenOut => broken_out_panel(ui, s, actions),
        DrawingTool::DimensionSet(kind) => set_panel(ui, s, kind, actions),
        DrawingTool::Gdt => gdt_panel(ui, s, actions),
        DrawingTool::Datum => datum_panel(ui, s, actions),
        DrawingTool::SurfaceFinish => finish_panel(ui, s, actions),
        DrawingTool::RevisionSymbol => revision_symbol_panel(ui, s, actions),
        _ => {}
    }
}

fn simple_panel(ui: &mut egui::Ui, icon: Icon, title: &str, text: &str, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, icon, title);
    cancel_button(ui, head, actions);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| hint(ui, text));
}

fn view_name_of(s: &DrawingSession, view: Option<ViewId>) -> String {
    view.and_then(|v| s.doc.drawing.sheet_of(v).and_then(|sh| sh.view(v).map(|vw| view_label(sh, vw))))
        .unwrap_or_else(|| "click a view".into())
}

fn hatch_combo(ui: &mut egui::Ui, salt: &str, current: Option<&str>) -> Option<Option<String>> {
    let lib = library::library();
    let name = |id: Option<&str>| {
        lib.hatch.iter().find(|p| id.is_some_and(|i| i == p.id)).or(lib.hatch.first()).map(|p| p.name.clone())
    };
    let mut chosen = None;
    labelled(ui, "Hatch", |ui| {
        egui::ComboBox::from_id_salt(salt)
            .width(ui.available_width() - 20.0)
            .selected_text(name(current).unwrap_or_default())
            .show_ui(ui, |ui| {
                for (i, p) in lib.hatch.iter().enumerate() {
                    let on = match current {
                        Some(id) => id == p.id,
                        None => i == 0,
                    };
                    if ui.selectable_label(on, &p.name).clicked() && !on {
                        chosen = Some(if i == 0 { None } else { Some(p.id.clone()) });
                    }
                }
            });
    });
    chosen
}

fn section_panel(ui: &mut egui::Ui, s: &mut DrawingSession, replace: Option<ViewId>, actions: &mut Vec<DrawingAction>) {
    let title = match replace {
        Some(_) => "Redraw cutting line".to_owned(),
        None => format!("Section view {0}-{0}", s.doc.drawing.next_view_label),
    };
    let head = header(ui, Icon::Section, &title);
    cancel_button(ui, head, actions);
    let pending = s.draft.pending.is_some();
    frame(ui, "section_tool", |ui| {
        theme::section_label(ui, "Cutting line");
        let on = replace.and_then(|r| s.doc.drawing.view(r).and_then(|v| v.parent())).or(s.draft.view);
        value_row(ui, "On", &view_name_of(s, on));
        let drawn = !s.draft.points.is_empty() || pending;
        if choice(ui, s.draft.horizontal, "Horizontal", drawn.then_some("drawn")) {
            s.draft.horizontal = true;
        }
        if choice(ui, !s.draft.horizontal, "Vertical", drawn.then_some("drawn")) {
            s.draft.horizontal = false;
        }
        hint(
            ui,
            "The first click starts the line; moving the pointer picks horizontal or vertical; the second click finishes it. Circle centres snap.",
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let flip = egui::Button::new(if s.draft.flip { "Flip direction (flipped)" } else { "Flip direction" });
            if ui.add(flip).on_hover_text("Look the other way along the cut (F)").clicked() {
                actions.push(DrawingAction::FlipPending);
            }
        });
        if replace.is_none() {
            value_row(ui, "Label", &s.doc.drawing.next_view_label);
        }
        section_gap(ui);
        theme::section_label(ui, "Section");
        choice(ui, true, "Full section", None);
        choice(ui, false, "Slice (cut faces only)", Some("later"));
        if let Some(h) = hatch_combo(ui, "section_tool_hatch", s.draft.hatch.as_deref()) {
            s.draft.hatch = h;
        }
        choice(ui, true, "Emphasise outline", None);
        section_gap(ui);
        theme::section_label(ui, "Placement");
        if pending {
            value_row(ui, "Scale", "Use parent");
            choice(ui, true, "Keep aligned with parent", None);
            choice(ui, false, "Free (Ctrl while placing)", None);
            hint(
                ui,
                "Click to place. The view sits where the arrows point in first angle, where the eye is in third angle.",
            );
        } else {
            hint(
                ui,
                "The view is projected once the line is drawn, then follows the pointer along its alignment until you click.",
            );
        }
    });
}

fn detail_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let title = format!("Detail view {}", s.doc.drawing.next_view_label);
    let head = header(ui, Icon::Search, &title);
    cancel_button(ui, head, actions);
    let pending = s.draft.pending.is_some();
    frame(ui, "detail_tool", |ui| {
        theme::section_label(ui, "Boundary");
        value_row(ui, "On", &view_name_of(s, s.draft.view));
        let started = !s.draft.points.is_empty() || pending;
        if choice(ui, !s.draft.profile, "Circle", None) && s.draft.profile && !started {
            s.draft.profile = false;
        }
        if choice(ui, s.draft.profile, "Profile (closed polygon)", None) && !s.draft.profile && !started {
            s.draft.profile = true;
        }
        hint(
            ui,
            if s.draft.profile {
                "Click the corners; click the first corner again, double-click or press Enter to close."
            } else {
                "Click the centre, then a point on the circle. Circle centres snap."
            },
        );
        section_gap(ui);
        theme::section_label(ui, "Scale");
        let parent_scale = s
            .draft
            .view
            .and_then(|v| s.doc.drawing.sheet_of(v).and_then(|sh| sh.view(v).map(|vw| effective_scale(sh, vw))))
            .unwrap_or(s.sheet().scale.factor());
        for f in [2.0, 3.0, 4.0, 5.0] {
            let tag = scale_text(parent_scale * f);
            if choice(ui, (s.draft.factor - f).abs() < 1e-9, &format!("{f}× the parent"), Some(&tag)) {
                s.draft.factor = f;
            }
        }
        labelled(ui, "Custom", |ui| {
            let r = ui.add(egui::DragValue::new(&mut s.draft.factor).speed(0.1).range(1.0..=20.0).suffix("×"));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Detail factor"));
        });
        value_row(ui, "Label", &s.doc.drawing.next_view_label);
        section_gap(ui);
        theme::section_label(ui, "Place on");
        let names: Vec<String> = s.doc.drawing.sheets.iter().map(|sh| sh.name.clone()).collect();
        let current = s.draft.target_sheet;
        let label = match current {
            None => format!("{} (this sheet)", names[s.sheet]),
            Some(t) if t < names.len() => names[t].clone(),
            Some(_) => "New sheet".to_owned(),
        };
        labelled(ui, "Sheet", |ui| {
            egui::ComboBox::from_id_salt("detail_target_sheet")
                .width(ui.available_width() - 20.0)
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        let on =
                            if i == s.sheet { current.is_none() || current == Some(i) } else { current == Some(i) };
                        let text = if i == s.sheet { format!("{n} (this sheet)") } else { n.clone() };
                        if ui.selectable_label(on, text).clicked() {
                            s.draft.target_sheet = (i != s.sheet).then_some(i);
                        }
                    }
                    let new = current.is_some_and(|t| t >= names.len());
                    if ui.selectable_label(new, "New sheet").clicked() {
                        s.draft.target_sheet = Some(names.len());
                    }
                });
        });
        hint(
            ui,
            "A detail on another sheet is placed there at once; location labels on both sheets point to each other.",
        );
        if pending {
            section_gap(ui);
            hint(ui, "Click to place the magnified view.");
        }
    });
}

fn auxiliary_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Plane, "Auxiliary view");
    cancel_button(ui, head, actions);
    frame(ui, "aux_tool", |ui| {
        theme::section_label(ui, "Reference edge");
        let edge = s.draft.anchors.first().map(|a| a.entity.role.clone()).unwrap_or_else(|| "click an edge".into());
        value_row(ui, "Edge", &edge);
        value_row(ui, "On", &view_name_of(s, s.draft.view));
        hint(
            ui,
            "The view looks square to the edge, from the side away from the parent's centre, so the face the parent sees edge on shows at true size.",
        );
        ui.add_space(4.0);
        let flip = egui::Button::new(if s.draft.flip {
            "Look from the other side (flipped)"
        } else {
            "Look from the other side"
        });
        if ui.add(flip).on_hover_text("F").clicked() {
            actions.push(DrawingAction::FlipPending);
        }
        value_row(ui, "Label", &s.doc.drawing.next_view_label);
        if s.draft.pending.is_some() {
            section_gap(ui);
            hint(ui, "Click to place the view.");
        }
    });
}

fn broken_out_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Cut, "Broken-out section");
    cancel_button(ui, head, actions);
    frame(ui, "broken_tool", |ui| {
        theme::section_label(ui, "Region");
        value_row(ui, "On", &view_name_of(s, s.draft.view));
        hint(
            ui,
            "Click the centre of the region, then a point on its circle. Inside it the material is cut away to the depth and the cut is hatched.",
        );
        section_gap(ui);
        theme::section_label(ui, "Depth");
        labelled(ui, "Depth", |ui| {
            let r = ui.add(egui::DragValue::new(&mut s.draft.depth).speed(0.5).range(0.1..=10_000.0).suffix(" mm"));
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Broken-out depth"));
        });
        hint(ui, "Model millimetres from the point of the part nearest the eye.");
    });
}

fn set_panel(ui: &mut egui::Ui, s: &mut DrawingSession, kind: SetKind, actions: &mut Vec<DrawingAction>) {
    let title = match kind {
        SetKind::Ordinate => "Ordinate dimensions",
        SetKind::Baseline => "Baseline dimensions",
        SetKind::Chain => "Chain dimensions",
    };
    let head = header(ui, Icon::SmartDim, title);
    cancel_button(ui, head, actions);
    frame(ui, "set_tool", |ui| {
        theme::section_label(ui, "Kind");
        for (k, label, words) in [
            (SetKind::Ordinate, "Ordinate", "distances from the origin, written at leaders"),
            (SetKind::Baseline, "Baseline", "stacked dimensions from the origin"),
            (SetKind::Chain, "Chain", "dimensions end to end"),
        ] {
            if choice(ui, kind == k, label, Some(words)) && kind != k {
                actions.push(DrawingAction::SetTool(DrawingTool::DimensionSet(k)));
            }
        }
        section_gap(ui);
        theme::section_label(ui, "Direction");
        for (o, label) in [(LinearOrientation::Horizontal, "Horizontal"), (LinearOrientation::Vertical, "Vertical")] {
            if choice(ui, s.draft.orientation == o, label, None) {
                s.draft.orientation = o;
            }
        }
        hint(ui, "Points on one line take the direction that fits them.");
        section_gap(ui);
        theme::section_label(ui, "Points");
        let n = s.draft.anchors.len();
        value_row(
            ui,
            "Picked",
            &match n {
                0 => "none · pick the origin".to_owned(),
                1 => "origin · pick the points".to_owned(),
                n => format!("origin and {} point{}", n - 1, if n == 2 { "" } else { "s" }),
            },
        );
        for (i, a) in s.draft.anchors.iter().enumerate() {
            let role = &a.entity.role;
            let attach = format!("{:?}", a.attach).to_lowercase();
            let attach = attach.split([' ', '{']).next().unwrap_or("").to_owned();
            let number = if i == 0 { "0".to_owned() } else { i.to_string() };
            ui.label(egui::RichText::new(format!("{number}  {role} · {attach}")).size(11.5));
        }
        if n > 0 && ui.small_button("Clear picks").clicked() {
            s.draft.anchors.clear();
            s.draft.view = None;
        }
        hint(
            ui,
            "Edge ends and circle centres snap; an edge's middle otherwise. The click that places the set sets how far from the points it sits.",
        );
    });
}

/// The feature control frame editor. Returns true when something changed.
pub fn gdt_editor(ui: &mut egui::Ui, g: &mut GdtDraft, salt: &str) -> bool {
    let mut changed = false;
    theme::section_label(ui, "Characteristic");
    labelled(ui, "Symbol", |ui| {
        egui::ComboBox::from_id_salt((salt, "gdt_char"))
            .width(ui.available_width() - 20.0)
            .selected_text(g.characteristic.name())
            .show_ui(ui, |ui| {
                for c in GdtCharacteristic::ALL {
                    if ui.selectable_label(g.characteristic == c, c.name()).clicked() && g.characteristic != c {
                        g.characteristic = c;
                        changed = true;
                    }
                }
            });
    });
    section_gap(ui);
    theme::section_label(ui, "Tolerance");
    labelled(ui, "Value", |ui| {
        let r = ui.add(egui::DragValue::new(&mut g.tolerance).speed(0.01).range(0.0001..=100.0).max_decimals(4));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Tolerance value"));
        changed |= r.changed();
        if ui.selectable_label(g.diameter, "⌀").on_hover_text("Cylindrical zone").clicked() {
            g.diameter = !g.diameter;
            changed = true;
        }
    });
    labelled(ui, "Material", |ui| {
        for (m, label, tip) in [
            (MaterialCondition::Regardless, "RFS", "Regardless of feature size"),
            (MaterialCondition::Maximum, "Ⓜ", "Maximum material condition"),
            (MaterialCondition::Least, "Ⓛ", "Least material condition"),
        ] {
            if ui.selectable_label(g.material == m, label).on_hover_text(tip).clicked() && g.material != m {
                g.material = m;
                changed = true;
            }
        }
    });
    section_gap(ui);
    theme::section_label(ui, "Datums");
    match g.characteristic.takes_datums() {
        Some(false) => hint(ui, "Form tolerances take no datums."),
        takes => {
            let mut remove = None;
            for (i, d) in g.datums.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(68.0, 22.0),
                        egui::Label::new(
                            egui::RichText::new(["Primary", "Secondary", "Tertiary"][i.min(2)])
                                .size(12.0)
                                .color(theme::muted()),
                        ),
                    );
                    let r = ui.add(egui::TextEdit::singleline(&mut d.letter).desired_width(36.0).char_limit(4));
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, format!("Datum {} letter", i + 1))
                    });
                    if r.changed() {
                        d.letter = d.letter.to_uppercase();
                        changed = true;
                    }
                    for (m, label) in [
                        (MaterialCondition::Regardless, "–"),
                        (MaterialCondition::Maximum, "Ⓜ"),
                        (MaterialCondition::Least, "Ⓛ"),
                    ] {
                        if ui.selectable_label(d.material == m, label).clicked() && d.material != m {
                            d.material = m;
                            changed = true;
                        }
                    }
                    if ui.small_button("×").on_hover_text("Remove this datum").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                g.datums.remove(i);
                changed = true;
            }
            if g.datums.len() < 3 && ui.small_button("+ datum").clicked() {
                let next =
                    ["A", "B", "C"].into_iter().find(|l| !g.datums.iter().any(|d| d.letter == *l)).unwrap_or("A");
                g.datums.push(DatumRef::new(next));
                changed = true;
            }
            if takes == Some(true) && g.datums.is_empty() {
                hint(ui, "Orientation and runout tolerances need a datum.");
            }
        }
    }
    section_gap(ui);
    value_row(ui, "Frame", &gdt_text(g));
    changed
}

/// A tolerance value as written in a frame: no trailing zeros, at most four decimals.
fn tolerance_text(value: f64) -> String {
    let s = format!("{value:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_owned() }
}

/// The frame as text: "⊥ ⌀0.1 Ⓜ | A | B Ⓜ".
pub fn gdt_text(g: &GdtDraft) -> String {
    let mut cells = vec![
        g.characteristic.name().to_owned(),
        format!(
            "{}{}{}",
            if g.diameter { "⌀" } else { "" },
            tolerance_text(g.tolerance),
            match g.material {
                MaterialCondition::Regardless => "",
                MaterialCondition::Maximum => " Ⓜ",
                MaterialCondition::Least => " Ⓛ",
            }
        ),
    ];
    if g.characteristic.takes_datums() != Some(false) {
        for d in &g.datums {
            cells.push(format!(
                "{}{}",
                d.letter,
                match d.material {
                    MaterialCondition::Regardless => "",
                    MaterialCondition::Maximum => " Ⓜ",
                    MaterialCondition::Least => " Ⓛ",
                }
            ));
        }
    }
    cells.join(" | ")
}

fn gdt_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Rectangle, "Geometric tolerance");
    cancel_button(ui, head, actions);
    frame(ui, "gdt_tool", |ui| {
        gdt_editor(ui, &mut s.draft.gdt, "gdt_tool");
        section_gap(ui);
        theme::section_label(ui, "Attach");
        let attached = s.draft.anchors.first().map(|a| a.entity.role.clone());
        value_row(ui, "To", attached.as_deref().unwrap_or("click an edge, or anywhere for a free frame"));
        hint(ui, "A frame on an edge keeps a leader to it and follows the part. The next click places the frame.");
    });
}

fn datum_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Plane, "Datum feature");
    cancel_button(ui, head, actions);
    frame(ui, "datum_tool", |ui| {
        theme::section_label(ui, "Datum");
        let next = s.doc.drawing.next_datum_label.clone();
        text_field(ui, "Letter", &mut s.draft.datum, &next);
        hint(ui, "Empty takes the drawing's next letter; I, O and Q are skipped as ASME Y14.3 asks.");
        section_gap(ui);
        theme::section_label(ui, "Attach");
        let attached = s.draft.anchors.first().map(|a| a.entity.role.clone());
        value_row(ui, "To", attached.as_deref().unwrap_or("click the feature's edge"));
        hint(ui, "The filled triangle stands on the edge; the next click puts the letter's frame.");
    });
}

/// The surface finish editor. Returns true when something changed.
pub fn finish_editor(ui: &mut egui::Ui, f: &mut FinishDraft, salt: &str) -> bool {
    let mut changed = false;
    theme::section_label(ui, "Requirement");
    changed |= text_field(ui, "Roughness", &mut f.text, "Ra 1.6");
    for (p, label, words) in [
        (SurfaceProcess::Any, "Any process", "basic symbol"),
        (SurfaceProcess::Removal, "Material removed", "with a bar"),
        (SurfaceProcess::NoRemoval, "No removal", "with a circle"),
    ] {
        if choice(ui, f.process == p, label, Some(words)) && f.process != p {
            f.process = p;
            changed = true;
        }
    }
    section_gap(ui);
    theme::section_label(ui, "Details");
    changed |= text_field(ui, "Method", &mut f.method, "Ground");
    labelled(ui, "Lay", |ui| {
        let lays = [
            (None, "None"),
            (Some(Lay::Parallel), "= parallel"),
            (Some(Lay::Perpendicular), "⊥ perpendicular"),
            (Some(Lay::Crossed), "X crossed"),
            (Some(Lay::Multidirectional), "M multidirectional"),
            (Some(Lay::Circular), "C circular"),
            (Some(Lay::Radial), "R radial"),
            (Some(Lay::Particulate), "P particulate"),
        ];
        let current = lays.iter().find(|(l, _)| *l == f.lay).map(|(_, n)| *n).unwrap_or("None");
        egui::ComboBox::from_id_salt((salt, "lay")).width(ui.available_width() - 20.0).selected_text(current).show_ui(
            ui,
            |ui| {
                for (l, n) in lays {
                    if ui.selectable_label(f.lay == l, n).clicked() && f.lay != l {
                        f.lay = l;
                        changed = true;
                    }
                }
            },
        );
    });
    changed |= text_field(ui, "Allowance", &mut f.allowance, "0.5");
    let mut all = f.all_around;
    if ui.checkbox(&mut all, "All around the outline").changed() {
        f.all_around = all;
        changed = true;
    }
    changed
}

fn finish_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Material, "Surface finish");
    cancel_button(ui, head, actions);
    frame(ui, "finish_tool", |ui| {
        finish_editor(ui, &mut s.draft.finish, "finish_tool");
        section_gap(ui);
        theme::section_label(ui, "Attach");
        let attached = s.draft.anchors.first().map(|a| a.entity.role.clone());
        value_row(ui, "To", attached.as_deref().unwrap_or("click the surface's edge"));
        hint(ui, "The symbol stands on the edge; the next click puts its text.");
    });
}

fn revision_symbol_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Rollback, "Revision symbol");
    cancel_button(ui, head, actions);
    frame(ui, "revsym_tool", |ui| {
        theme::section_label(ui, "Revision");
        let current = s.doc.drawing.meta.revision.clone();
        text_field(ui, "Revision", &mut s.draft.revision, &current);
        hint(
            ui,
            "Empty marks the drawing's current revision. The symbol's zone fills the revision table's ZONE column.",
        );
        section_gap(ui);
        if ui.button("Revision table…").clicked() {
            actions.push(DrawingAction::Select(Some(DrawingSel::Revisions)));
        }
    });
}

/// The sections a view's kind, crop, broken-out sections, location and sheet add to
/// the view panel.
pub fn view_extras(ui: &mut egui::Ui, s: &DrawingSession, v: &View, actions: &mut Vec<DrawingAction>) {
    let id = v.id;
    let sheet = s.sheet();
    match &v.kind {
        ViewKind::Section { parent, line, flip, label, model_line, hatch } => {
            theme::section_label(ui, "Section");
            let mut letter = label.clone();
            if text_field(ui, "Letter", &mut letter, "A") && letter.trim() != label {
                let kind = ViewKind::Section {
                    parent: *parent,
                    line: line.clone(),
                    flip: *flip,
                    label: letter.trim().to_uppercase(),
                    model_line: model_line.clone(),
                    hatch: hatch.clone(),
                };
                actions.push(DrawingAction::Command(Command::EditView { view: id, kind }));
            }
            let cut = match (line.first(), line.last()) {
                (Some(a), Some(b)) if (a.v - b.v).abs() < 1e-6 => format!("horizontal at y {:.1}", a.v),
                (Some(a), _) => format!("vertical at x {:.1}", a.u),
                _ => "none".into(),
            };
            value_row(ui, "Cut", &cut);
            ui.horizontal(|ui| {
                if ui.button("Flip direction").on_hover_text("Look the other way along the cut").clicked() {
                    let kind = ViewKind::Section {
                        parent: *parent,
                        line: line.clone(),
                        flip: !*flip,
                        label: label.clone(),
                        model_line: model_line.clone(),
                        hatch: hatch.clone(),
                    };
                    actions.push(DrawingAction::Command(Command::EditView { view: id, kind }));
                }
                if ui.button("Redraw cutting line…").clicked() {
                    actions.push(DrawingAction::SetTool(DrawingTool::Section { replace: Some(id) }));
                }
            });
            if let Some(h) = hatch_combo(ui, "view_hatch", hatch.as_deref()) {
                let kind = ViewKind::Section {
                    parent: *parent,
                    line: line.clone(),
                    flip: *flip,
                    label: label.clone(),
                    model_line: model_line.clone(),
                    hatch: h,
                };
                actions.push(DrawingAction::Command(Command::EditView { view: id, kind }));
            }
            let regions = v.geometry.hatch.len();
            value_row(ui, "Cut faces", &format!("{regions} hatched region{}", if regions == 1 { "" } else { "s" }));
            section_gap(ui);
        }
        ViewKind::Detail { parent, center, radius, factor, label, profile, model_center } => {
            theme::section_label(ui, "Detail");
            let remake = |label: String, radius: f64| ViewKind::Detail {
                parent: *parent,
                center: *center,
                radius,
                factor: *factor,
                label,
                profile: profile.clone(),
                model_center: *model_center,
            };
            let mut letter = label.clone();
            if text_field(ui, "Letter", &mut letter, "B") && letter.trim() != label {
                actions.push(DrawingAction::Command(Command::EditView {
                    view: id,
                    kind: remake(letter.trim().to_uppercase(), *radius),
                }));
            }
            value_row(
                ui,
                "Boundary",
                &if profile.len() >= 3 {
                    format!("profile with {} corners", profile.len())
                } else {
                    format!("circle, radius {radius:.1} mm")
                },
            );
            if profile.len() < 3 {
                labelled(ui, "Radius", |ui| {
                    let mut r = *radius;
                    let resp = ui.add(egui::DragValue::new(&mut r).speed(0.5).range(1.0..=1000.0).suffix(" mm"));
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Detail radius"));
                    if (resp.drag_stopped() || (resp.lost_focus() && resp.changed())) && (r - radius).abs() > 1e-6 {
                        actions.push(DrawingAction::Command(Command::EditView {
                            view: id,
                            kind: remake(label.clone(), r),
                        }));
                    }
                });
            }
            let parent_scale = sheet.view(*parent).map(|p| effective_scale(sheet, p)).unwrap_or(sheet.scale.factor());
            let own = effective_scale(sheet, v);
            let mag = own / parent_scale;
            labelled(ui, "Magnify", |ui| {
                for f in [2.0, 3.0, 4.0, 5.0] {
                    if ui
                        .selectable_label((mag - f).abs() < 1e-6, format!("{f}×"))
                        .on_hover_text(scale_text(parent_scale * f))
                        .clicked()
                        && (mag - f).abs() > 1e-6
                    {
                        actions.push(DrawingAction::Command(Command::SetViewScale {
                            view: id,
                            scale: ViewScale::Custom { factor: parent_scale * f },
                        }));
                    }
                }
            });
            value_row(ui, "Scale", &format!("{} (parent {})", scale_text(own), scale_text(parent_scale)));
            section_gap(ui);
        }
        ViewKind::Auxiliary { parent, edge, flip, label } => {
            theme::section_label(ui, "Auxiliary");
            let mut letter = label.clone();
            if text_field(ui, "Letter", &mut letter, "C") && letter.trim() != label {
                let kind = ViewKind::Auxiliary {
                    parent: *parent,
                    edge: edge.clone(),
                    flip: *flip,
                    label: letter.trim().to_uppercase(),
                };
                actions.push(DrawingAction::Command(Command::EditView { view: id, kind }));
            }
            value_row(ui, "Edge", &edge.role);
            if ui.button("Look from the other side").clicked() {
                let kind =
                    ViewKind::Auxiliary { parent: *parent, edge: edge.clone(), flip: !*flip, label: label.clone() };
                actions.push(DrawingAction::Command(Command::EditView { view: id, kind }));
            }
            section_gap(ui);
        }
        _ => {}
    }
    // Crop and broken-out sections, on any view but a detail (which shows its parent's).
    theme::section_label(ui, "Crop and break-out");
    ui.horizontal(|ui| {
        match &v.crop {
            Some(_) => {
                if ui.button("Remove crop").clicked() {
                    actions.push(DrawingAction::Command(Command::SetViewCrop { view: id, crop: None }));
                }
            }
            None => {
                if ui.button("Crop…").on_hover_text("Keep only a rectangle of the view").clicked() {
                    actions.push(DrawingAction::SetTool(DrawingTool::Crop));
                }
            }
        }
        if ui.button("Broken-out…").on_hover_text("Cut away a circle to a depth").clicked() {
            actions.push(DrawingAction::SetTool(DrawingTool::BrokenOut));
        }
    });
    for (i, b) in v.broken_out.iter().enumerate() {
        ui.horizontal(|ui| {
            let what = match &b.boundary {
                soliddraft_drawing::Boundary::Circle { radius, .. } => format!("circle r {radius:.1} mm"),
                soliddraft_drawing::Boundary::Polygon { points } => format!("polygon, {} corners", points.len()),
            };
            ui.label(egui::RichText::new(format!("Broken out {}: {what}, {:.1} mm deep", i + 1, b.depth)).size(11.5));
            if ui.small_button("Remove").clicked() {
                actions.push(DrawingAction::Command(Command::RemoveBrokenOut { view: id, index: i }));
            }
        });
    }
    section_gap(ui);
    // Location labels and the sheet the view is on.
    theme::section_label(ui, "Location");
    let labels = display::location_labels(&s.doc.drawing);
    let sheet_name = |i: usize| s.doc.drawing.sheets.get(i).map(|sh| sh.name.clone()).unwrap_or_default();
    if let Some(l) = labels.iter().find(|l| l.view == id) {
        value_row(ui, "This view", &format!("{}, zone {}", sheet_name(l.view_sheet), l.view_zone));
        value_row(ui, "Its mark", &format!("{}, zone {}", sheet_name(l.parent_sheet), l.parent_zone));
        if l.view_sheet != l.parent_sheet {
            hint(ui, "Labels on both sheets point to each other: the mark says SEE SHEET, the view says FROM SHEET.");
        }
    }
    for l in labels.iter().filter(|l| l.parent == id && l.view_sheet != l.parent_sheet) {
        let name = s
            .doc
            .drawing
            .sheet_of(l.view)
            .and_then(|sh| sh.view(l.view).map(|c| view_label(sh, c)))
            .unwrap_or_default();
        value_row(ui, "Child", &format!("{name} on {}, zone {}", sheet_name(l.view_sheet), l.view_zone));
    }
    let names: Vec<String> = s.doc.drawing.sheets.iter().map(|sh| sh.name.clone()).collect();
    labelled(ui, "Sheet", |ui| {
        egui::ComboBox::from_id_salt(("view_sheet", id.0))
            .width(ui.available_width() - 20.0)
            .selected_text(sheet_name(s.sheet))
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(i == s.sheet, n).clicked() && i != s.sheet {
                        actions.push(DrawingAction::MoveViewToSheet { view: id, sheet: i });
                    }
                }
            });
    });
    hint(ui, "Moving a view to another sheet frees it from its alignment and keeps its children pointing at it.");
    section_gap(ui);
    theme::section_label(ui, "Derive from this view");
    ui.horizontal(|ui| {
        if ui.button("Section…").clicked() {
            actions.push(DrawingAction::SetTool(DrawingTool::Section { replace: None }));
        }
        if ui.button("Detail…").clicked() {
            actions.push(DrawingAction::SetTool(DrawingTool::Detail));
        }
        if ui.button("Auxiliary…").clicked() {
            actions.push(DrawingAction::SetTool(DrawingTool::Auxiliary));
        }
    });
    section_gap(ui);
}

/// The editor of a phase D3 annotation's body. Returns false for other kinds.
pub fn annotation_body(
    ui: &mut egui::Ui,
    s: &DrawingSession,
    a: &soliddraft_drawing::Annotation,
    actions: &mut Vec<DrawingAction>,
) -> bool {
    let id = a.id;
    match &a.body {
        AnnotationKind::Gdt { .. } => {
            let Some(mut g) = GdtDraft::from_body(&a.body) else { return false };
            if gdt_editor(ui, &mut g, "gdt_edit") {
                actions.push(DrawingAction::Command(Command::EditAnnotation { annotation: id, body: g.body() }));
            }
            hint(ui, "ISO 1101 and ASME Y14.5: the characteristic, the zone and the datums, primary first.");
        }
        AnnotationKind::SurfaceFinish { .. } => {
            let Some(mut f) = FinishDraft::from_body(&a.body) else { return false };
            if finish_editor(ui, &mut f, "finish_edit") {
                actions.push(DrawingAction::Command(Command::EditAnnotation { annotation: id, body: f.body() }));
            }
        }
        AnnotationKind::Datum { letter } => {
            theme::section_label(ui, "Datum");
            let mut l = letter.clone();
            if text_field(ui, "Letter", &mut l, "A") && l.trim().to_uppercase() != *letter && !l.trim().is_empty() {
                actions.push(DrawingAction::Command(Command::EditAnnotation {
                    annotation: id,
                    body: AnnotationKind::Datum { letter: l.trim().to_uppercase() },
                }));
            }
            hint(ui, "The letter geometric tolerances refer to. The triangle stands on the feature's edge.");
        }
        AnnotationKind::RevisionSymbol { revision } => {
            theme::section_label(ui, "Revision");
            let mut r = revision.clone();
            if text_field(ui, "Revision", &mut r, "B") && r.trim() != revision && !r.trim().is_empty() {
                actions.push(DrawingAction::Command(Command::EditAnnotation {
                    annotation: id,
                    body: AnnotationKind::RevisionSymbol { revision: r.trim().to_owned() },
                }));
            }
            let zone = display::zone_at(s.doc.template(), display_point(s, a));
            value_row(ui, "Zone", &zone);
            if ui.button("Revision table…").clicked() {
                actions.push(DrawingAction::Select(Some(DrawingSel::Revisions)));
            }
        }
        AnnotationKind::DimensionSet(set) => {
            theme::section_label(ui, "Dimension set");
            value_row(ui, "Values", &set.values_text());
            let kind_words = |k: SetKind| match k {
                SetKind::Ordinate => "Ordinate",
                SetKind::Baseline => "Baseline",
                SetKind::Chain => "Chain",
            };
            for k in [SetKind::Ordinate, SetKind::Baseline, SetKind::Chain] {
                if choice(ui, set.kind == k, kind_words(k), None) && set.kind != k {
                    let mut edited = set.clone();
                    // Ordinate and baseline values are from the origin; a chain's are
                    // from point to point.
                    if set.kind == SetKind::Chain && k != SetKind::Chain {
                        let mut sum = 0.0;
                        edited.values = set
                            .values
                            .iter()
                            .map(|v| {
                                sum += v;
                                sum
                            })
                            .collect();
                    } else if set.kind != SetKind::Chain && k == SetKind::Chain {
                        let mut prev = 0.0;
                        edited.values = set
                            .values
                            .iter()
                            .map(|v| {
                                let d = v - prev;
                                prev = *v;
                                d
                            })
                            .collect();
                    }
                    edited.kind = k;
                    actions.push(DrawingAction::Command(Command::EditAnnotation {
                        annotation: id,
                        body: AnnotationKind::DimensionSet(edited),
                    }));
                }
            }
            labelled(ui, "Precision", |ui| {
                for (p, label) in [(0u8, "0"), (1, ".1"), (2, ".12"), (3, ".123")] {
                    if ui.selectable_label(set.precision == p, label).clicked() && set.precision != p {
                        let mut edited = set.clone();
                        edited.precision = p;
                        actions.push(DrawingAction::Command(Command::EditAnnotation {
                            annotation: id,
                            body: AnnotationKind::DimensionSet(edited),
                        }));
                    }
                }
            });
            value_row(
                ui,
                "Direction",
                match set.orientation {
                    LinearOrientation::Horizontal => "horizontal",
                    LinearOrientation::Vertical => "vertical",
                    LinearOrientation::Aligned => "aligned",
                },
            );
            hint(ui, "The origin is the first pick, marked 0. Drag the set to move its leaders or dimension lines.");
        }
        _ => return false,
    }
    true
}

/// An annotation's placement in sheet millimetres.
fn display_point(s: &DrawingSession, a: &soliddraft_drawing::Annotation) -> soliddraft_kernel_api::Point2 {
    match s.doc.drawing.view(a.view) {
        Some(v) => soliddraft_kernel_api::Point2::new(a.placement.u + v.position.u, a.placement.v + v.position.v),
        None => a.placement,
    }
}

/// The revision table panel: the rows so far and a form for the next one.
pub fn revisions_panel(ui: &mut egui::Ui, s: &mut DrawingSession, actions: &mut Vec<DrawingAction>) {
    let head = header(ui, Icon::Tree, "Revision table");
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
    let si = s.sheet;
    let table =
        s.sheet().tables.iter().enumerate().find(|(_, t)| t.kind == TableKind::Revision).map(|(i, t)| (i, t.clone()));
    let current = s.doc.drawing.meta.revision.clone();
    if s.draft.revision_row.revision.is_empty() {
        s.draft.revision_row.revision = next_letter(&current);
    }
    if s.draft.revision_row.date.is_empty() {
        s.draft.revision_row.date = super::today();
    }
    frame(ui, "revisions_pm", |ui| {
        theme::section_label(ui, "Revisions");
        match &table {
            Some((_, t)) if !t.revisions.is_empty() => {
                egui::Grid::new("revision_rows").striped(true).spacing([8.0, 3.0]).show(ui, |ui| {
                    for h in ["Rev", "Description", "Date", "Approved", "Zone"] {
                        ui.label(egui::RichText::new(h).size(10.5).color(theme::muted()));
                    }
                    ui.end_row();
                    for r in &t.revisions {
                        ui.label(egui::RichText::new(&r.revision).size(11.5).family(theme::semibold()));
                        ui.label(egui::RichText::new(&r.description).size(11.5));
                        ui.label(egui::RichText::new(&r.date).size(11.5));
                        ui.label(egui::RichText::new(&r.approved).size(11.5));
                        ui.label(
                            egui::RichText::new(if r.zone.is_empty() { "from symbols" } else { &r.zone })
                                .size(11.0)
                                .color(theme::muted()),
                        );
                        ui.end_row();
                    }
                });
            }
            Some(_) => hint(ui, "The table is empty."),
            None => hint(ui, "No table yet: the first revision makes one at the top right of the drawing area."),
        }
        value_row(ui, "Current", &current);
        section_gap(ui);
        theme::section_label(ui, "Add revision");
        let row: &mut RevisionDraft = &mut s.draft.revision_row;
        text_field(ui, "Revision", &mut row.revision, "B");
        text_field(ui, "Description", &mut row.description, "What changed");
        text_field(ui, "Date", &mut row.date, "2026-10-14");
        text_field(ui, "Approved", &mut row.approved, "Initials");
        ui.add_space(6.0);
        let can = !row.revision.trim().is_empty() && !row.description.trim().is_empty();
        if primary_button(ui, "Add revision row") && can {
            let new = RevisionRow {
                revision: row.revision.trim().to_owned(),
                description: row.description.trim().to_owned(),
                date: row.date.trim().to_owned(),
                approved: row.approved.trim().to_owned(),
                zone: String::new(),
            };
            actions.push(DrawingAction::Command(Command::AddRevision { sheet: si, row: new.clone() }));
            *row = RevisionDraft { revision: next_letter(&new.revision), date: super::today(), ..Default::default() };
        }
        if !can {
            hint(ui, "A row needs a revision and a description.");
        }
        hint(
            ui,
            "The drawing's revision becomes the row's; the ZONE column lists the zones of the revision symbols that carry it.",
        );
        section_gap(ui);
        ui.horizontal(|ui| {
            if ui
                .button("Revision symbol…")
                .on_hover_text("Mark a changed place with the revision in a triangle")
                .clicked()
            {
                actions.push(DrawingAction::SetTool(DrawingTool::RevisionSymbol));
            }
            if let Some((i, _)) = &table
                && ui.button("Delete table").clicked()
            {
                actions.push(DrawingAction::Command(Command::DeleteTable { sheet: si, table: *i }));
                actions.push(DrawingAction::Select(None));
            }
        });
    });
}
