//! The sheet canvas: an egui panel with a 2D camera that paints the display list,
//! hit-tests it, and runs the tools (place view, smart dimension, note, hole callout,
//! centre mark, reattach). Also the update banner, the dimension palette and the pills.

use super::{
    Drag, DrawingAction, DrawingSel, DrawingSession, DrawingTool, NoteEdit, PendingView, PlaceKind, SHEET, sheets,
    tools,
};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use soliddraft_drawing::display::{self as layout, CAP_HEIGHT, DisplayList, Item, Owner, Role, VAlign};
use soliddraft_drawing::library::{Align, Weight};
use soliddraft_drawing::measure::{self as resolve, CurveShape, Pick, shape_of};
use soliddraft_drawing::project::{self, view_bounds};
use soliddraft_drawing::{
    AlignAxis, Alignment, Annotation, AnnotationId, AnnotationKind, AnnotationState, Attach, Command, Dimension,
    DimensionKind, FoldDirection, LinearOrientation, Orientation, StyleOverrides, Tolerance, ViewDisplay, ViewId,
    ViewKind, ViewScale,
};
use soliddraft_kernel_api::Point2;

/// Colours of the sheet.
const CANVAS_BG: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xee);
const PAPER: Color32 = Color32::WHITE;
const CHANGED_FILL: Color32 = Color32::from_rgb(0xff, 0xf1, 0xc2);
const HIT_PX: f32 = 6.0;

fn role_colour(role: Role) -> Color32 {
    match role {
        Role::Hidden => theme::ink().gamma_multiply(0.75),
        Role::Phantom => theme::ink().gamma_multiply(0.6),
        Role::Label => theme::muted(),
        Role::Error => theme::error(),
        Role::Changed => CHANGED_FILL,
        Role::Stale => theme::warn(),
        _ => theme::ink(),
    }
}

/// Paints one sheet item through the camera, optionally recoloured and widened.
pub(super) struct Sheet2<'a> {
    pub(super) painter: &'a egui::Painter,
    pub(super) cam: super::Camera2,
    pub(super) rect: Rect,
}

impl Sheet2<'_> {
    pub(super) fn s(&self, p: Point2) -> Pos2 {
        self.cam.to_screen(self.rect, p)
    }

    fn polylines(&self, path: &kurbo::BezPath) -> Vec<Vec<Pos2>> {
        let tol = (0.3 / self.cam.ppm as f64).max(0.002);
        layout::flatten(path, tol)
            .into_iter()
            .map(|poly| poly.into_iter().map(|q| self.s(Point2::new(q.x, q.y))).collect())
            .collect()
    }

    pub(super) fn paint(&self, item: &Item, colour: Option<Color32>, extra_width: f32) {
        match item {
            Item::Path(p) => {
                let colour = colour.unwrap_or_else(|| role_colour(p.role));
                let polys = self.polylines(&p.path);
                if p.fill {
                    for poly in polys {
                        if poly.len() >= 3 {
                            self.painter.add(egui::Shape::convex_polygon(poly, colour, Stroke::NONE));
                        }
                    }
                    return;
                }
                let width = ((p.width as f32) * self.cam.ppm).max(0.9) + extra_width;
                let stroke = Stroke::new(width, colour);
                match &p.dash {
                    Some(dash) if dash.len() >= 2 => {
                        let dashes: Vec<f32> =
                            dash.iter().step_by(2).map(|d| (*d as f32 * self.cam.ppm).max(1.0)).collect();
                        let gaps: Vec<f32> =
                            dash.iter().skip(1).step_by(2).map(|d| (*d as f32 * self.cam.ppm).max(1.0)).collect();
                        let mut shapes = Vec::new();
                        for poly in polys {
                            egui::Shape::dashed_line_many_with_offset(&poly, stroke, &dashes, &gaps, 0.0, &mut shapes);
                        }
                        self.painter.extend(shapes);
                    }
                    _ => {
                        for poly in polys {
                            if poly.len() >= 2 {
                                self.painter.add(egui::Shape::line(poly, stroke));
                            }
                        }
                    }
                }
            }
            Item::Text(t) => {
                let colour = colour.unwrap_or_else(|| role_colour(t.role));
                let (galley, pos, angle) = self.text_layout(t);
                self.painter.add(egui::epaint::TextShape::new(pos, galley, colour).with_angle(angle));
            }
        }
    }

    /// The galley, its top-left and its egui angle for a text item.
    fn text_layout(&self, t: &layout::TextItem) -> (std::sync::Arc<egui::Galley>, Pos2, f32) {
        let size = (t.height as f32 * self.cam.ppm / CAP_HEIGHT as f32).max(1.0);
        let font = match t.weight {
            Weight::Regular => theme::font(size),
            Weight::Medium => theme::font_medium(size),
            Weight::Semibold => theme::font_semibold(size),
        };
        let galley = self.painter.layout_no_wrap(t.text.clone(), font, Color32::PLACEHOLDER);
        let w = galley.size().x;
        let ox = match t.halign {
            Align::Left => 0.0,
            Align::Centre => w / 2.0,
            Align::Right => w,
        };
        // Inter: ascender about 0.97 em, cap height 0.727 em.
        let oy = match t.valign {
            VAlign::Bottom => 0.97 * size,
            VAlign::Centre => (0.97 - CAP_HEIGHT as f32 / 2.0) * size,
            VAlign::Top => (0.97 - CAP_HEIGHT as f32) * size,
        };
        let anchor = self.s(t.pos);
        let phi = -(t.angle as f32).to_radians();
        let (sin, cos) = phi.sin_cos();
        let pos = pos2(anchor.x - (ox * cos - oy * sin), anchor.y - (ox * sin + oy * cos));
        (galley, pos, phi)
    }

    /// The screen corners of a text item.
    fn text_corners(&self, t: &layout::TextItem) -> [Pos2; 4] {
        let (galley, pos, phi) = self.text_layout(t);
        let (w, h) = (galley.size().x, galley.size().y);
        let (sin, cos) = phi.sin_cos();
        let corner = |x: f32, y: f32| pos2(pos.x + x * cos - y * sin, pos.y + x * sin + y * cos);
        [corner(0.0, 0.0), corner(w, 0.0), corner(w, h), corner(0.0, h)]
    }
}

fn seg_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 < 1e-9 { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    (a + ab * t - p).length()
}

fn point_in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if sign == 0.0 {
            sign = cross;
        } else if (cross > 0.0) != (sign > 0.0) {
            return false;
        }
    }
    true
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Hit {
    pub(super) owner: Owner,
    /// For a curve: a snapped end.
    pub(super) end: Option<Attach>,
    pub(super) distance: f32,
    /// Screen length of the item, so the smaller of two overlapping curves wins.
    pub(super) length: f32,
}

/// Annotations win over curves, visible curves over hidden ones, and of two curves at
/// the same distance the shorter one, so a hole's edge wins over the long edge it
/// lies on.
fn hit_test(sheet: &Sheet2<'_>, list: &DisplayList, pos: Pos2, curves_only: bool) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    let mut consider = |owner: Owner, distance: f32, end: Option<Attach>, penalty: f32, length: f32| {
        let priority = match owner {
            Owner::Annotation(_) | Owner::AnnotationEnd(..) => -1.5,
            _ => penalty,
        };
        let score = distance + priority;
        if distance <= HIT_PX
            && best.is_none_or(|b| score < b.distance - 0.5 || ((score - b.distance).abs() <= 0.5 && length < b.length))
        {
            best = Some(Hit { owner, end, distance: score, length });
        }
    };
    let skip = |owner: Owner| matches!(owner, Owner::Sheet) || (curves_only && !matches!(owner, Owner::Curve(..)));
    for item in &list.items {
        match item {
            Item::Path(p) => {
                if skip(p.owner) || p.role == Role::Changed || p.role == Role::Stale {
                    continue;
                }
                let penalty = if p.role == Role::Hidden { 1.0 } else { 0.0 };
                let polys = sheet.polylines(&p.path);
                let length: f32 =
                    polys.iter().map(|poly| poly.windows(2).map(|w| (w[1] - w[0]).length()).sum::<f32>()).sum();
                for poly in &polys {
                    for w in poly.windows(2) {
                        let d = seg_distance(pos, w[0], w[1]);
                        if d <= HIT_PX {
                            let end = if let Owner::Curve(..) = p.owner {
                                let (a, b) = (poly[0], poly[poly.len() - 1]);
                                if (pos - a).length() <= HIT_PX + 1.0 {
                                    Some(Attach::Start)
                                } else if (pos - b).length() <= HIT_PX + 1.0 {
                                    Some(Attach::End)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            consider(p.owner, d, end, penalty, length);
                        }
                    }
                }
            }
            Item::Text(t) => {
                if skip(t.owner) {
                    continue;
                }
                let q = sheet.text_corners(t);
                if point_in_quad(pos, &q) {
                    consider(t.owner, 0.0, None, 0.0, 0.0);
                } else {
                    for i in 0..4 {
                        let d = seg_distance(pos, q[i], q[(i + 1) % 4]);
                        if d <= HIT_PX {
                            consider(t.owner, d, None, 0.0, 0.0);
                        }
                    }
                }
            }
        }
    }
    best
}

/// The view a sheet point falls in, by bounds with a margin.
pub(super) fn view_at(session: &DrawingSession, p: Point2) -> Option<ViewId> {
    let margin = 12.0;
    session
        .sheet()
        .views
        .iter()
        .filter(|v| {
            let [x0, y0, x1, y1] = view_bounds(v);
            p.u >= x0 - margin && p.u <= x1 + margin && p.v >= y0 - margin && p.v <= y1 + margin
        })
        .map(|v| v.id)
        .next()
}

/// Sheet point to view millimetres.
pub(super) fn to_view(session: &DrawingSession, view: ViewId, p: Point2) -> Point2 {
    if view == SHEET {
        return p;
    }
    match session.doc.drawing.view(view) {
        Some(v) => Point2::new(p.u - v.position.u, p.v - v.position.v),
        None => p,
    }
}

/// The fold direction of a pointer relative to a parent view.
fn fold_direction(parent_centre: Point2, p: Point2) -> FoldDirection {
    let (dx, dy) = (p.u - parent_centre.u, p.v - parent_centre.v);
    let angle = dy.atan2(dx).to_degrees();
    let a = if angle < 0.0 { angle + 360.0 } else { angle };
    // Eight sectors of 45 degrees centred on the axes and diagonals.
    let sector = ((a + 22.5) / 45.0).floor() as i32 % 8;
    match sector {
        0 => FoldDirection::Right,
        1 => FoldDirection::UpRight,
        2 => FoldDirection::Up,
        3 => FoldDirection::UpLeft,
        4 => FoldDirection::Left,
        5 => FoldDirection::DownLeft,
        6 => FoldDirection::Down,
        _ => FoldDirection::DownRight,
    }
}

fn direction_name(d: FoldDirection) -> &'static str {
    match d {
        FoldDirection::Left => "Left of parent",
        FoldDirection::Right => "Right of parent",
        FoldDirection::Up => "Above parent",
        FoldDirection::Down => "Below parent",
        FoldDirection::UpLeft => "Above left",
        FoldDirection::UpRight => "Above right",
        FoldDirection::DownLeft => "Below left",
        FoldDirection::DownRight => "Below right",
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let doc = &state.doc;
    let Some(s) = state.drawing.session.as_mut() else {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No drawing open").color(theme::muted()));
        });
        return;
    };
    // The sheet tabs take a strip along the bottom; the canvas is the rest.
    let whole = ui.available_rect_before_wrap();
    let strip = Rect::from_min_max(pos2(whole.left(), whole.bottom() - sheets::STRIP_HEIGHT), whole.max);
    let canvas = Rect::from_min_max(whole.min, pos2(whole.right(), strip.top()));
    let (rect, resp) = ui.allocate_exact_size(canvas.size(), Sense::click_and_drag());
    s.canvas_rect = rect;
    let paper = (s.doc.template().paper.width, s.doc.template().paper.height);
    if s.needs_fit {
        s.camera.fit(rect, paper);
        s.needs_fit = false;
    }
    let painter = ui.painter_at(rect);
    let mut actions: Vec<AppAction> = Vec::new();
    let mut push = |a: DrawingAction| actions.push(AppAction::Drawing(a));

    // Navigation: wheel zooms at the cursor, middle or right drag pans, Shift + left too.
    let hover_pos = resp.hover_pos();
    if let Some(pos) = hover_pos {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        let zoom = ui.input(|i| i.zoom_delta());
        if scroll.abs() > 0.0 {
            s.camera.zoom_at(rect, pos, (1.0 + scroll * 0.0025).clamp(0.5, 2.0));
        }
        if (zoom - 1.0).abs() > 1e-4 {
            s.camera.zoom_at(rect, pos, zoom);
        }
    }
    let shift = ui.input(|i| i.modifiers.shift);
    let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
    let panning = resp.dragged_by(egui::PointerButton::Middle)
        || resp.dragged_by(egui::PointerButton::Secondary)
        || (shift && resp.dragged_by(egui::PointerButton::Primary));
    if panning {
        let d = resp.drag_delta();
        s.camera.centre = Point2::new(
            s.camera.centre.u - f64::from(d.x / s.camera.ppm),
            s.camera.centre.v + f64::from(d.y / s.camera.ppm),
        );
    }
    s.cursor = hover_pos.map(|p| s.camera.to_sheet(rect, p));

    // The display list for this frame.
    let stale = s.doc.stale_views();
    let format = s.doc.template();
    let style = format.style_file();
    let lctx =
        layout::Context { format, style, stale: &stale, sheet_index: s.sheet, sheet_count: s.doc.drawing.sheets.len() };
    let list = layout::layout(&s.doc.drawing, s.sheet, &lctx);
    let sheet2 = Sheet2 { painter: &painter, cam: s.camera, rect };

    // Paper.
    painter.rect_filled(rect, CornerRadius::ZERO, CANVAS_BG);
    let p0 = sheet2.s(Point2::new(0.0, paper.1));
    let p1 = sheet2.s(Point2::new(paper.0, 0.0));
    let paper_rect = Rect::from_min_max(p0, p1);
    painter.rect_filled(paper_rect.translate(vec2(3.0, 4.0)), CornerRadius::ZERO, Color32::from_black_alpha(28));
    painter.rect_filled(paper_rect, CornerRadius::ZERO, PAPER);
    if s.show_grid {
        let step = 10.0;
        let stroke = Stroke::new(0.5, theme::line().gamma_multiply(0.8));
        let mut x = 0.0;
        while x <= paper.0 {
            let a = sheet2.s(Point2::new(x, 0.0));
            painter.vline(a.x, paper_rect.y_range(), stroke);
            x += step;
        }
        let mut y = 0.0;
        while y <= paper.1 {
            let a = sheet2.s(Point2::new(0.0, y));
            painter.hline(paper_rect.x_range(), a.y, stroke);
            y += step;
        }
    }

    // Hover: the tools that pick model entities see curves only.
    let curves_only = s.tool != DrawingTool::Select && !s.tool.is_derived();
    let hit = hover_pos.filter(|_| !panning).and_then(|p| hit_test(&sheet2, &list, p, curves_only));
    s.hover = hit.map(|h| h.owner);
    s.hover_label = hit.and_then(|h| hover_label(s, h.owner));

    // Items, with the hovered and selected ones recoloured.
    let selected_owner = |o: Owner| match (s.selection, o) {
        (Some(DrawingSel::View(v)), Owner::Curve(w, _)) | (Some(DrawingSel::View(v)), Owner::View(w)) => v == w,
        (Some(DrawingSel::Annotation(a)), Owner::Annotation(b))
        | (Some(DrawingSel::Annotation(a)), Owner::AnnotationEnd(b, _)) => a == b,
        (Some(DrawingSel::Format), Owner::Sheet) => true,
        (Some(DrawingSel::Revisions), Owner::Table(..)) => true,
        _ => false,
    };
    let hovered_owner = |o: Owner| match (hit.map(|h| h.owner), o) {
        (Some(Owner::Curve(v, i)), Owner::Curve(w, j)) => v == w && i == j,
        (Some(Owner::View(v)), Owner::View(w)) => v == w,
        (Some(Owner::Annotation(a)), Owner::Annotation(b))
        | (Some(Owner::AnnotationEnd(a, _)), Owner::Annotation(b))
        | (Some(Owner::Annotation(a)), Owner::AnnotationEnd(b, _)) => a == b,
        _ => false,
    };
    for item in &list.items {
        let o = item.owner();
        let (colour, extra) = if hovered_owner(o) {
            (Some(theme::accent()), 1.0)
        } else if selected_owner(o) && !matches!(o, Owner::Curve(..)) {
            (Some(theme::accent()), 0.5)
        } else {
            (None, 0.0)
        };
        sheet2.paint(item, colour, extra);
    }

    // Selected view: a dashed boundary; while a projected view is being placed, the
    // eight direction dots around its parent.
    if let Some(DrawingSel::View(id)) = s.selection
        && let Some(v) = s.doc.drawing.view(id)
    {
        let [x0, y0, x1, y1] = view_bounds(v);
        let m = 4.0;
        let a = sheet2.s(Point2::new(x0 - m, y1 + m));
        let b = sheet2.s(Point2::new(x1 + m, y0 - m));
        let r = Rect::from_min_max(a, b);
        let stroke = Stroke::new(1.0, theme::accent());
        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(egui::Shape::dashed_line(&corners, stroke, 6.0, 4.0));
    }

    // Tool overlays and clicks.
    let clicked = resp.clicked_by(egui::PointerButton::Primary) && !shift;
    let pointer = hover_pos.map(|p| s.camera.to_sheet(rect, p));
    match s.tool.clone() {
        DrawingTool::Select => {
            select_tool(s, &resp, hit, pointer, &mut push);
        }
        DrawingTool::PlaceView(kind) => {
            place_tool(s, doc, &sheet2, &lctx, kind, pointer, ctrl, clicked, &mut push);
        }
        DrawingTool::SmartDimension => {
            smart_dimension_tool(s, &sheet2, &lctx, hit, pointer, clicked, &mut push);
        }
        DrawingTool::Note => {
            if clicked && let Some(p) = pointer {
                note_tool(s, hit, p, &mut push);
            }
        }
        DrawingTool::HoleCallout | DrawingTool::CentreMark => {
            if clicked
                && let Some(p) = pointer
                && let Some(Hit { owner: Owner::Curve(view, index), .. }) = hit
            {
                circle_tool(s, doc, view, index, p, &mut push);
            } else if clicked {
                s.say("Click a circle seen face-on");
            }
        }
        DrawingTool::Reattach { annotation, end } => {
            if clicked {
                if let Some(Hit { owner: Owner::Curve(view, index), end: picked_end, .. }) = hit {
                    reattach_tool(s, annotation, end, view, index, picked_end, &mut push);
                } else {
                    s.say("Reattach: click an edge or circle of the view");
                }
            }
        }
        DrawingTool::Section { .. }
        | DrawingTool::Detail
        | DrawingTool::Auxiliary
        | DrawingTool::Crop
        | DrawingTool::BrokenOut => {
            tools::derived_tool(s, doc, &sheet2, &lctx, hit, pointer, ctrl, clicked, resp.double_clicked(), &mut push);
        }
        DrawingTool::DimensionSet(kind) => {
            tools::set_tool(s, kind, &sheet2, &lctx, hit, pointer, clicked, &mut push);
        }
        DrawingTool::Gdt | DrawingTool::Datum | DrawingTool::SurfaceFinish | DrawingTool::RevisionSymbol => {
            tools::symbol_tool(s, &sheet2, &lctx, hit, pointer, clicked, &mut push);
        }
    }

    // Pills: fit and zoom at the top, sheet and zoom percentage at the bottom right,
    // the tool prompt at the bottom centre.
    let zoom_pct = (s.camera.ppm / (rect.width() - 56.0) * paper.0 as f32 * 100.0).round();
    let _ = zoom_pct;
    let scale_text = s.sheet().scale.to_string();
    let hud = format!(
        "Fit · {scale_text} · Grid {} · Snap {}",
        if s.show_grid { "on" } else { "off" },
        if s.snap { "on" } else { "off" }
    );
    let hud_rect = pill(&painter, pos2(rect.center().x, rect.top() + 16.0), &hud, Align2::CENTER_CENTER);
    let hud_resp = ui.interact(hud_rect, ui.id().with("drawing_hud"), Sense::click());
    if hud_resp.clicked() {
        push(DrawingAction::ZoomFit);
    }
    hud_resp.on_hover_text("Click to fit the sheet (Z). G toggles the grid.");
    let pct = (s.camera.ppm * 25.4 / 96.0 * 100.0).round();
    pill(
        &painter,
        pos2(rect.right() - 12.0, rect.bottom() - 14.0),
        &format!("Sheet {} of {} · {} · {pct:.0} %", s.sheet + 1, s.doc.drawing.sheets.len(), s.sheet().size.name),
        Align2::RIGHT_CENTER,
    );
    let prompt = s.tool_prompt();
    if !prompt.is_empty() {
        pill(&painter, pos2(rect.center().x, rect.bottom() - 14.0), &prompt, Align2::CENTER_CENTER);
    } else if let Some(label) = &s.hover_label {
        pill(&painter, pos2(rect.center().x, rect.bottom() - 14.0), label, Align2::CENTER_CENTER);
    }

    // The update banner, or the reference bar while the part moved on or is missing.
    let update_shown = s.doc.last_update.as_ref().is_some_and(|r| {
        !r.dismissed && (!r.changed.is_empty() || !r.dangling.is_empty() || r.at.elapsed().as_secs() < 12)
    });
    if update_shown {
        if let Some(r) = s.doc.last_update.clone() {
            banner(ui, s, rect, &r, &mut push);
        }
    } else if !s.marker_dismissed {
        super::reference::bar(ui, s, rect, &mut push);
    }

    // The dimension palette on the selected dimension.
    if let Some(DrawingSel::Annotation(id)) = s.selection
        && s.tool == DrawingTool::Select
        && let Some(a) = s.doc.drawing.annotation(id).cloned()
        && let AnnotationKind::Dimension(d) = &a.body
    {
        let text_pos = list.items.iter().find_map(|i| match i {
            Item::Text(t)
                if t.owner == Owner::Annotation(id) && t.role != Role::Error
                    || matches!(i, Item::Text(t) if t.owner == Owner::Annotation(id)) =>
            {
                Some(sheet2.s(t.pos))
            }
            _ => None,
        });
        if let Some(p) = text_pos {
            palette(ui, s, id, d, p, &mut push);
        }
    }

    // The sheet tabs under the canvas.
    sheets::strip(ui, s, strip, &mut push);

    for a in actions {
        state.push(a);
    }
}

/// Paints a view that is not on the sheet yet, in the accent colour: its line work, its
/// hatch and caption, and the mark it leaves on its parent.
pub(super) fn ghost_view(
    s: &DrawingSession,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    view: &soliddraft_drawing::View,
    sheet_index: usize,
) {
    let mut d = s.doc.drawing.clone();
    if let Some(sheet) = d.sheets.get_mut(sheet_index) {
        sheet.views.push(view.clone());
    }
    let ctx = layout::Context { stale: &[], ..*lctx };
    let ghost = layout::layout(&d, sheet_index, &ctx);
    let mine = |o: Owner| matches!(o, Owner::Curve(v, _) | Owner::View(v) if v == view.id);
    for item in ghost.items.iter().filter(|i| mine(i.owner())) {
        sheet2.paint(item, Some(theme::accent().gamma_multiply(0.85)), 0.0);
    }
}

/// Paints an annotation that is not on the sheet yet, in the accent colour.
pub(super) fn ghost_annotation(
    s: &DrawingSession,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    annotation: &Annotation,
) {
    let mut d = s.doc.drawing.clone();
    d.sheets[s.sheet].annotations.push(annotation.clone());
    let ctx = layout::Context { stale: &[], ..*lctx };
    let ghost = layout::layout(&d, s.sheet, &ctx);
    for item in ghost.items.iter().filter(|i| i.owner() == Owner::Annotation(annotation.id)) {
        sheet2.paint(item, Some(theme::accent()), 0.0);
    }
}

/// A hover card beside the pointer: a title line and a detail line.
pub(super) fn card(painter: &egui::Painter, at: Pos2, line1: &str, line2: &str) {
    hover_card(painter, at, line1, line2);
}

/// The words for what is under the pointer: "Front view · Boss-Extrude3 · Edge 4".
fn hover_label(s: &DrawingSession, owner: Owner) -> Option<String> {
    let sheet = s.sheet();
    match owner {
        Owner::Curve(view, index) => {
            let v = sheet.view(view)?;
            let c = v.geometry.curves.get(index)?;
            let kind = match c.kind {
                soliddraft_drawing::CurveKind::Edge => "",
                soliddraft_drawing::CurveKind::Silhouette => " · silhouette",
                soliddraft_drawing::CurveKind::Seam => " · seam",
                soliddraft_drawing::CurveKind::Section => " · section",
            };
            Some(format!(
                "{} · {}{kind}{}",
                project::view_label(sheet, v),
                c.source.role,
                if c.visible { "" } else { " · hidden" }
            ))
        }
        Owner::View(view) => sheet.view(view).map(|v| match &v.kind {
            ViewKind::Section { .. } => format!("{} · cutting line", project::view_label(sheet, v)),
            ViewKind::Detail { .. } => format!("{} · boundary", project::view_label(sheet, v)),
            _ => project::view_label(sheet, v),
        }),
        Owner::Annotation(id) | Owner::AnnotationEnd(id, _) => {
            let a = s.doc.drawing.annotation(id)?;
            Some(layout::annotation_name(a, s.doc.style()))
        }
        Owner::Table(_) => Some("Revision table".into()),
        Owner::Sheet => None,
    }
}

fn pill(painter: &egui::Painter, at: Pos2, text: &str, anchor: Align2) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), theme::font(11.0), theme::ink());
    let size = galley.size() + vec2(18.0, 10.0);
    let r = anchor.anchor_size(at, size);
    painter.rect(r, CornerRadius::same(99), theme::hud_bg(), Stroke::new(1.0, theme::line()), StrokeKind::Inside);
    painter.galley(r.center() - galley.size() / 2.0, galley, theme::ink());
    r
}

fn select_tool(
    s: &mut DrawingSession,
    resp: &egui::Response,
    hit: Option<Hit>,
    pointer: Option<Point2>,
    push: &mut impl FnMut(DrawingAction),
) {
    let shift = resp.ctx.input(|i| i.modifiers.shift);
    if resp.drag_started_by(egui::PointerButton::Primary) && !shift {
        let target = match hit.map(|h| h.owner) {
            Some(Owner::Annotation(id)) | Some(Owner::AnnotationEnd(id, _)) => Some(DrawingSel::Annotation(id)),
            Some(Owner::Curve(v, _)) | Some(Owner::View(v)) => Some(DrawingSel::View(v)),
            _ => None,
        };
        if let (Some(t), Some(p)) = (target, pointer) {
            let origin = match t {
                DrawingSel::View(v) => s.doc.drawing.view(v).map(|v| v.position),
                DrawingSel::Annotation(a) => s.doc.drawing.annotation(a).map(|a| a.placement),
                DrawingSel::Format | DrawingSel::Revisions | DrawingSel::Model => None,
            };
            if let Some(origin) = origin {
                s.drag = Some(Drag { target: t, origin, start_pointer: p, moved: false });
                s.selection = Some(t);
            }
        }
    }
    if let (Some(d), Some(p)) = (s.drag.clone(), pointer)
        && resp.dragged_by(egui::PointerButton::Primary)
    {
        let delta = (p.u - d.start_pointer.u, p.v - d.start_pointer.v);
        if delta.0.abs() + delta.1.abs() > 0.5 || d.moved {
            let to = Point2::new(d.origin.u + delta.0, d.origin.v + delta.1);
            match d.target {
                DrawingSel::View(v) => {
                    // Live move without an undo entry; the command lands on release.
                    live_move_view(s, v, to);
                    draw_alignment_guides(s, v, resp);
                }
                DrawingSel::Annotation(a) => {
                    let placement = to_view_pos(s_view_of(&s.doc.drawing, a), &s.doc.drawing, to);
                    if let Some(an) =
                        s.doc.drawing.sheets.iter_mut().flat_map(|sh| sh.annotations.iter_mut()).find(|x| x.id == a)
                    {
                        an.placement = placement;
                    }
                }
                DrawingSel::Format | DrawingSel::Revisions | DrawingSel::Model => {}
            }
            if let Some(d) = s.drag.as_mut() {
                d.moved = true;
            }
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary)
        && let Some(d) = s.drag.take()
        && d.moved
    {
        match d.target {
            DrawingSel::View(v) => {
                let to = s.doc.drawing.view(v).map(|x| x.position).unwrap_or(d.origin);
                live_move_view(s, v, d.origin);
                push(DrawingAction::Command(Command::MoveView { view: v, to }));
            }
            DrawingSel::Annotation(a) => {
                let to = s.doc.drawing.annotation(a).map(|x| x.placement).unwrap_or(d.origin);
                if let Some(an) =
                    s.doc.drawing.sheets.iter_mut().flat_map(|sh| sh.annotations.iter_mut()).find(|x| x.id == a)
                {
                    an.placement = d.origin;
                }
                push(DrawingAction::Command(Command::MoveAnnotation { annotation: a, to }));
            }
            DrawingSel::Format | DrawingSel::Revisions | DrawingSel::Model => {}
        }
        return;
    }
    if resp.clicked_by(egui::PointerButton::Primary) && !shift {
        let sel = match hit.map(|h| h.owner) {
            Some(Owner::Annotation(id)) | Some(Owner::AnnotationEnd(id, _)) => Some(DrawingSel::Annotation(id)),
            Some(Owner::Curve(v, _)) | Some(Owner::View(v)) => Some(DrawingSel::View(v)),
            Some(Owner::Sheet) => Some(DrawingSel::Format),
            Some(Owner::Table(_)) => Some(DrawingSel::Revisions),
            None => None,
        };
        s.drag = None;
        push(DrawingAction::Select(sel));
    }
    if resp.double_clicked()
        && let Some(Owner::Annotation(id)) = hit.map(|h| h.owner)
        && let Some(AnnotationKind::Note { text, .. }) = s.doc.drawing.annotation(id).map(|a| &a.body)
    {
        s.note_edit = Some(NoteEdit { annotation: id, text: text.clone(), focus: true });
    }
}

fn s_view_of(drawing: &soliddraft_drawing::Drawing, a: AnnotationId) -> ViewId {
    drawing.annotation(a).map(|a| a.view).unwrap_or(SHEET)
}

fn to_view_pos(view: ViewId, drawing: &soliddraft_drawing::Drawing, p: Point2) -> Point2 {
    if view == SHEET {
        return p;
    }
    match drawing.view(view) {
        Some(v) => Point2::new(p.u - v.position.u, p.v - v.position.v),
        None => p,
    }
}

/// Moves a view and its aligned children without touching the undo stack.
fn live_move_view(s: &mut DrawingSession, view: ViewId, to: Point2) {
    let sheet_i = s.sheet;
    let sheet = &s.doc.drawing.sheets[sheet_i];
    let Some(v) = sheet.view(view) else { return };
    let mut target = to;
    if let Some(al) = v.alignment
        && let Some(p) = sheet.view(al.parent)
    {
        match al.axis {
            AlignAxis::Horizontal => target.v = p.position.v,
            AlignAxis::Vertical => target.u = p.position.u,
        }
    }
    let delta = (target.u - v.position.u, target.v - v.position.v);
    let children = project::descendants(sheet, view);
    for v in &mut s.doc.drawing.sheets[sheet_i].views {
        if v.id == view {
            v.position = target;
        } else if children.contains(&v.id) {
            v.position = Point2::new(v.position.u + delta.0, v.position.v + delta.1);
        }
    }
}

/// Dashed accent guides from an aligned view to its parent while it is dragged.
fn draw_alignment_guides(s: &DrawingSession, view: ViewId, resp: &egui::Response) {
    let sheet = s.sheet();
    let Some(v) = sheet.view(view) else { return };
    let Some(al) = v.alignment else { return };
    let Some(p) = sheet.view(al.parent) else { return };
    guides(s, resp, p, v.position, al);
}

fn guides(
    s: &DrawingSession,
    resp: &egui::Response,
    parent: &soliddraft_drawing::View,
    child_pos: Point2,
    al: Alignment,
) {
    let painter = resp.ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, resp.id.with("guides")));
    let sheet2 = Sheet2 { painter: &painter, cam: s.camera, rect: s.canvas_rect };
    let [px0, py0, px1, py1] = view_bounds(parent);
    let stroke = Stroke::new(1.0, theme::accent().gamma_multiply(0.8));
    let lines: Vec<(Point2, Point2)> = match al.axis {
        AlignAxis::Horizontal => {
            let (a, b) =
                if child_pos.u > parent.position.u { (px1, child_pos.u + 40.0) } else { (child_pos.u - 40.0, px0) };
            vec![(Point2::new(a, py0), Point2::new(b, py0)), (Point2::new(a, py1), Point2::new(b, py1))]
        }
        AlignAxis::Vertical => {
            let (a, b) =
                if child_pos.v > parent.position.v { (py1, child_pos.v + 40.0) } else { (child_pos.v - 40.0, py0) };
            vec![(Point2::new(px0, a), Point2::new(px0, b)), (Point2::new(px1, a), Point2::new(px1, b))]
        }
    };
    for (a, b) in lines {
        painter.extend(egui::Shape::dashed_line(&[sheet2.s(a), sheet2.s(b)], stroke, 6.0, 4.0));
    }
}

#[allow(clippy::too_many_arguments)]
fn place_tool(
    s: &mut DrawingSession,
    doc: &crate::document::Document,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    kind: PlaceKind,
    pointer: Option<Point2>,
    ctrl: bool,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(p) = pointer else { return };
    let sheet_i = s.sheet;
    // Work out the view at the pointer, projecting (and caching) as needed.
    let pending: Option<PendingView> = match &kind {
        PlaceKind::Standard(v) => {
            let cached = s.pending.take().filter(|pv| pv.direction.is_none());
            match cached {
                Some(mut pv) => {
                    pv.view.position = p;
                    Some(pv)
                }
                None => {
                    let Some(ctx) = super::part::PartContext::new(doc) else {
                        s.say("The part has no body");
                        s.set_tool(DrawingTool::Select);
                        return;
                    };
                    let kind = ViewKind::Base { orientation: Orientation::Standard { view: *v } };
                    match s.doc.project_new(&ctx, sheet_i, &kind, project::default_display(*v), ViewScale::Sheet) {
                        Ok((mut view, projected)) => {
                            view.position = p;
                            Some(PendingView { view, projected, direction: None, aligned: false })
                        }
                        Err(e) => {
                            s.say(e);
                            s.set_tool(DrawingTool::Select);
                            return;
                        }
                    }
                }
            }
        }
        PlaceKind::Projected { parent } => {
            let Some(parent_view) = s.doc.drawing.view(*parent).cloned() else {
                s.set_tool(DrawingTool::Select);
                return;
            };
            let direction = fold_direction(parent_view.position, p);
            let key = (*parent, direction);
            if !s.fold_cache.contains_key(&key) {
                let Some(ctx) = super::part::PartContext::new(doc) else {
                    s.say("The part has no body");
                    s.set_tool(DrawingTool::Select);
                    return;
                };
                let kind = ViewKind::Projected { parent: *parent, direction };
                let display = if direction.is_diagonal() { ViewDisplay::PICTORIAL } else { parent_view.display };
                match s.doc.project_new(&ctx, sheet_i, &kind, display, ViewScale::Parent) {
                    Ok(pair) => {
                        s.fold_cache.insert(key, pair);
                    }
                    Err(e) => {
                        s.say(e);
                        s.set_tool(DrawingTool::Select);
                        return;
                    }
                }
            }
            let (mut view, projected) = s.fold_cache[&key].clone();
            let aligned = !ctrl && !direction.is_diagonal();
            view.position = p;
            if aligned {
                match direction {
                    FoldDirection::Left | FoldDirection::Right => view.position.v = parent_view.position.v,
                    FoldDirection::Up | FoldDirection::Down => view.position.u = parent_view.position.u,
                    _ => {}
                }
                let axis = match direction {
                    FoldDirection::Left | FoldDirection::Right => AlignAxis::Horizontal,
                    _ => AlignAxis::Vertical,
                };
                view.alignment = Some(Alignment { parent: *parent, axis });
            } else {
                view.alignment = None;
            }
            Some(PendingView { view, projected, direction: Some(direction), aligned })
        }
    };
    let Some(pending) = pending else { return };

    // Ghost of the view at the pointer, in the accent colour.
    let ghost_list = {
        let mut sheet = s.sheet().clone();
        sheet.views.push(pending.view.clone());
        let mut d = s.doc.drawing.clone();
        d.sheets[sheet_i] = sheet;
        let ctx = layout::Context { stale: &[], ..*lctx };
        layout::layout(&d, sheet_i, &ctx)
    };
    for item in ghost_list.items.iter().filter(|i| matches!(i.owner(), Owner::Curve(v, _) if v == pending.view.id)) {
        sheet2.paint(item, Some(theme::accent().gamma_multiply(0.85)), 0.0);
    }
    // Direction dots around the parent and the alignment guides.
    if let PlaceKind::Projected { parent } = &kind
        && let Some(pv) = s.doc.drawing.view(*parent)
    {
        let [x0, y0, x1, y1] = view_bounds(pv);
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (rx, ry) = ((x1 - x0) / 2.0 + 14.0, (y1 - y0) / 2.0 + 14.0);
        for (d, dx, dy) in [
            (FoldDirection::Right, 1.0, 0.0),
            (FoldDirection::UpRight, 1.0, 1.0),
            (FoldDirection::Up, 0.0, 1.0),
            (FoldDirection::UpLeft, -1.0, 1.0),
            (FoldDirection::Left, -1.0, 0.0),
            (FoldDirection::DownLeft, -1.0, -1.0),
            (FoldDirection::Down, 0.0, -1.0),
            (FoldDirection::DownRight, 1.0, -1.0),
        ] {
            let at = sheet2.s(Point2::new(cx + dx * rx, cy + dy * ry));
            let active = pending.direction == Some(d);
            if active {
                sheet2.painter.circle_filled(at, 4.5, theme::accent());
            } else {
                sheet2.painter.circle(at, 3.5, theme::panel(), Stroke::new(1.0, theme::accent().gamma_multiply(0.7)));
            }
        }
        if let (true, Some(al)) = (pending.aligned, pending.view.alignment) {
            let painter = sheet2.painter;
            let stroke = Stroke::new(1.0, theme::accent().gamma_multiply(0.8));
            let child = pending.view.position;
            let lines: Vec<(Point2, Point2)> = match al.axis {
                AlignAxis::Horizontal => {
                    let (a, b) = if child.u > pv.position.u { (x1, child.u + 40.0) } else { (child.u - 40.0, x0) };
                    vec![(Point2::new(a, y0), Point2::new(b, y0)), (Point2::new(a, y1), Point2::new(b, y1))]
                }
                AlignAxis::Vertical => {
                    let (a, b) = if child.v > pv.position.v { (y1, child.v + 40.0) } else { (child.v - 40.0, y0) };
                    vec![(Point2::new(x0, a), Point2::new(x0, b)), (Point2::new(x1, a), Point2::new(x1, b))]
                }
            };
            for (a, b) in lines {
                painter.extend(egui::Shape::dashed_line(&[sheet2.s(a), sheet2.s(b)], stroke, 6.0, 4.0));
            }
        }
        // Hover card naming the resulting orientation and the modifier.
        if let Some(d) = pending.direction {
            let sheet = s.sheet();
            let mut tmp = sheet.clone();
            tmp.views.push(pending.view.clone());
            let name = project::view_label(&tmp, &pending.view);
            let line1 = format!("Projected view · {}", name.trim_end_matches(" view"));
            let line2 = if d.is_diagonal() {
                "Pictorial, not aligned".to_owned()
            } else if pending.aligned {
                format!("Aligned to {} · hold Ctrl to break alignment", project::view_label(sheet, pv))
            } else {
                "Free (Ctrl held)".to_owned()
            };
            let at = sheet2.s(pending.view.position);
            let [bx0, _, bx1, by1] = view_bounds(&pending.view);
            let _ = (bx0, bx1);
            let top = sheet2.s(Point2::new(pending.view.position.u, by1));
            let _ = top;
            hover_card(sheet2.painter, at + vec2(30.0, -30.0), &line1, &line2);
            let _ = direction_name(d);
        }
    }
    if clicked {
        let view = pending.view.clone();
        let id = view.id;
        s.doc.remember(id, &pending.projected);
        push(DrawingAction::Command(Command::PlaceView { sheet: sheet_i, view: view.clone() }));
        if !matches!(pending.direction, Some(d) if d.is_diagonal())
            && !matches!(
                view.kind,
                ViewKind::Base {
                    orientation: Orientation::Standard {
                        view: soliddraft_kernel_api::drawing::StandardView::Isometric
                    }
                }
            )
        {
            let marks = project::centre_marks(&pending.projected, id, s.doc.next_annotation_id().0);
            for m in marks {
                push(DrawingAction::Command(Command::AddAnnotation { annotation: m }));
            }
        }
        push(DrawingAction::SetTool(DrawingTool::Select));
        push(DrawingAction::Select(Some(DrawingSel::View(id))));
        s.pending = None;
    } else {
        s.pending = Some(pending);
    }
}

fn hover_card(painter: &egui::Painter, at: Pos2, line1: &str, line2: &str) {
    let g1 = painter.layout_no_wrap(line1.to_owned(), theme::font_medium(12.0), theme::ink());
    let g2 = painter.layout_no_wrap(line2.to_owned(), theme::font(11.0), theme::muted());
    let w = g1.size().x.max(g2.size().x) + 20.0;
    let h = g1.size().y + g2.size().y + 16.0;
    let r = Rect::from_min_size(at, vec2(w, h));
    painter.rect(r, CornerRadius::same(7), theme::panel(), Stroke::new(1.0, theme::line()), StrokeKind::Inside);
    painter.galley(r.left_top() + vec2(10.0, 7.0), g1, theme::ink());
    painter.galley(r.left_top() + vec2(10.0, 9.0 + g2.size().y), g2, theme::muted());
}

#[allow(clippy::too_many_arguments)]
fn smart_dimension_tool(
    s: &mut DrawingSession,
    sheet2: &Sheet2<'_>,
    lctx: &layout::Context<'_>,
    hit: Option<Hit>,
    pointer: Option<Point2>,
    clicked: bool,
    push: &mut impl FnMut(DrawingAction),
) {
    // Picked curves glow.
    let picks = s.picks.clone();
    for p in &picks {
        if let Some(v) = s.sheet().view(p.view)
            && let Some(c) = v.geometry.curves.get(p.curve)
        {
            let mut path = c.path.clone();
            path.apply_affine(kurbo::Affine::translate((v.position.u, v.position.v)));
            let item = Item::Path(layout::PathItem {
                path,
                width: 0.5,
                dash: None,
                role: Role::Visible,
                owner: Owner::Curve(p.view, p.curve),
                fill: false,
                accuracy: Some(c.accuracy),
            });
            sheet2.paint(&item, Some(theme::accent()), 1.5);
            if let Some(end) = p.end
                && let Some(c) = v.geometry.curves.get(p.curve)
            {
                let shape = shape_of(&c.path);
                let q = if end == Attach::Start { shape.start() } else { shape.end() };
                let at = sheet2.s(Point2::new(q.u + v.position.u, q.v + v.position.v));
                sheet2.painter.circle_filled(at, 4.0, theme::accent());
            }
        }
    }
    // A complete proposal follows the pointer as a ghost dimension.
    if let (Some(prop), Some(p)) = (s.proposal.clone(), pointer)
        && prop.complete
    {
        let a = build_dimension(s, &prop, p);
        let mut d = s.doc.drawing.clone();
        d.sheets[s.sheet].annotations.push(a);
        let ctx = layout::Context { stale: &[], ..*lctx };
        let ghost = layout::layout(&d, s.sheet, &ctx);
        let id = ghost.items.iter().rev().find_map(|i| match i.owner() {
            Owner::Annotation(id) => Some(id),
            _ => None,
        });
        for item in ghost.items.iter().filter(|i| Some(i.owner()) == id.map(Owner::Annotation)) {
            sheet2.paint(item, Some(theme::accent()), 0.0);
        }
    }
    if !clicked {
        return;
    }
    let Some(p) = pointer else { return };
    // A second curve picked while one is held turns the proposal into a two-entity
    // dimension; any other click with a complete proposal places it.
    let second_pick = matches!(hit, Some(Hit { owner: Owner::Curve(..), .. })) && s.picks.len() < 2;
    match (&s.proposal, hit) {
        (Some(prop), _) if prop.complete && !second_pick => {
            let a = build_dimension(s, prop, p);
            let id = a.id;
            push(DrawingAction::Command(Command::AddDimension { annotation: a }));
            push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
            s.picks.clear();
            s.proposal = None;
            s.say("Dimension placed · pick the next item, or Esc");
        }
        (_, Some(Hit { owner: Owner::Curve(view, index), end, .. })) => {
            if s.picks.first().is_some_and(|f| f.view != view) {
                s.say("Both picks must be in the same view");
                return;
            }
            let pick = Pick { view, curve: index, end };
            if s.picks.contains(&pick) {
                return;
            }
            s.picks.push(pick);
            let view_ref = s.sheet().view(view).cloned();
            s.proposal = view_ref.and_then(|v| {
                let circles = s.doc.circles.get(&v.id).map(Vec::as_slice).unwrap_or_default();
                resolve::propose(&v, &s.picks, circles)
            });
            if let Some(pr) = &s.proposal {
                let what = match pr.kind {
                    DimensionKind::Diameter => "diameter",
                    DimensionKind::Radius => "radius",
                    DimensionKind::Angular => "angle",
                    _ => "distance",
                };
                s.say(if pr.complete {
                    format!("Click to place the {what}")
                } else {
                    "Pick the second end".to_owned()
                });
            }
        }
        _ => {
            if !s.picks.is_empty() {
                s.picks.clear();
                s.proposal = None;
                s.say("Picks cleared");
            }
        }
    }
}

/// The dimension a proposal makes once placed at a sheet point.
fn build_dimension(s: &DrawingSession, prop: &resolve::Proposal, placement_sheet: Point2) -> Annotation {
    let sheet = s.sheet();
    let view = sheet.view(prop.view);
    let scale = view.map(|v| project::effective_scale(sheet, v)).unwrap_or(1.0);
    let placement = to_view(s, prop.view, placement_sheet);
    let (kind, value, precision) = match prop.kind {
        DimensionKind::Linear { .. } if prop.points.len() >= 2 => {
            let orientation = resolve::orientation_for(&prop.points, placement);
            (
                DimensionKind::Linear { orientation },
                resolve::linear_value(orientation, prop.points[0], prop.points[1], scale),
                0,
            )
        }
        DimensionKind::Diameter => (DimensionKind::Diameter, prop.radius * 2.0 / scale, 0),
        DimensionKind::Radius => (DimensionKind::Radius, prop.radius / scale, 0),
        DimensionKind::Angular => (DimensionKind::Angular, prop.angle, 0),
        other => (other, 0.0, 0),
    };
    let precision = if (value - value.round()).abs() > 1e-6 { 2 } else { precision };
    let mut anchors = prop.anchors.clone();
    // Linear dimensions between two rounds attach at the centres; the picks decided.
    if let DimensionKind::Linear { orientation } = kind {
        let axis = match orientation {
            LinearOrientation::Horizontal => (1.0, 0.0),
            LinearOrientation::Vertical => (0.0, 1.0),
            LinearOrientation::Aligned => (0.0, 0.0),
        };
        let _ = axis;
        for a in &mut anchors {
            if a.attach == Attach::Silhouette {
                a.attach = Attach::Mid;
            }
        }
    }
    Annotation {
        id: s.doc.next_annotation_id(),
        view: prop.view,
        body: AnnotationKind::Dimension(Dimension::measured(kind, value, precision)),
        anchors,
        placement,
        style: StyleOverrides::default(),
        state: AnnotationState::Ok,
    }
}

fn note_tool(s: &mut DrawingSession, hit: Option<Hit>, p: Point2, push: &mut impl FnMut(DrawingAction)) {
    let id = s.doc.next_annotation_id();
    let (view, anchors, placement, leader) = match hit {
        Some(Hit { owner: Owner::Curve(view, index), .. }) => {
            let v = s.sheet().view(view).cloned();
            match v.and_then(|v| v.geometry.curves.get(index).cloned().map(|c| (v, c))) {
                Some((v, c)) => {
                    let shape = shape_of(&c.path);
                    let at = match shape {
                        CurveShape::Circle { centre, radius } => Point2::new(
                            centre.u + radius * std::f64::consts::FRAC_1_SQRT_2,
                            centre.v + radius * std::f64::consts::FRAC_1_SQRT_2,
                        ),
                        other => other.mid(),
                    };
                    let anchor =
                        soliddraft_drawing::Anchor { entity: c.source.clone(), attach: Attach::Mid, cached: at };
                    let placement = Point2::new(at.u + 14.0, at.v + 10.0);
                    let _ = v;
                    (view, vec![anchor], placement, vec![Point2::new(at.u + 10.0, at.v + 10.0 - 1.75)])
                }
                None => (SHEET, Vec::new(), p, Vec::new()),
            }
        }
        _ => {
            let view = view_at(s, p).unwrap_or(SHEET);
            (view, Vec::new(), to_view(s, view, p), Vec::new())
        }
    };
    let a = Annotation {
        id,
        view,
        body: AnnotationKind::Note { text: "NOTE".into(), leader },
        anchors,
        placement,
        style: StyleOverrides::default(),
        state: AnnotationState::Ok,
    };
    push(DrawingAction::Command(Command::AddNote { annotation: a }));
    push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
    push(DrawingAction::SetTool(DrawingTool::Select));
    s.note_edit = Some(NoteEdit { annotation: id, text: "NOTE".into(), focus: true });
    s.say("Note placed · type its text in the property panel");
}

fn circle_tool(
    s: &mut DrawingSession,
    doc: &crate::document::Document,
    view: ViewId,
    index: usize,
    p: Point2,
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(v) = s.sheet().view(view).cloned() else { return };
    let Some(c) = v.geometry.curves.get(index) else { return };
    let circle = s
        .doc
        .circles
        .get(&view)
        .and_then(|cs| {
            cs.iter().find(|k| k.entity.kernel_hint.is_some() && k.entity.kernel_hint == c.source.kernel_hint)
        })
        .cloned();
    let Some(circle) = circle else {
        s.say("That is not a circle seen face-on");
        return;
    };
    let id = s.doc.next_annotation_id();
    let anchor =
        soliddraft_drawing::Anchor { entity: circle.entity.clone(), attach: Attach::Centre, cached: circle.centre };
    let a = match s.tool {
        DrawingTool::HoleCallout => {
            let Some(ctx) = super::part::PartContext::new(doc) else {
                s.say("The part has no body");
                return;
            };
            let Some(projected) = s.doc.projected_of(view) else { return };
            let text = super::part::hole_callout(&ctx, &circle, &projected, s.sheet().template.as_str());
            let pv = to_view(s, view, p);
            let dir = ((pv.u - circle.centre.u).signum(), (pv.v - circle.centre.v).signum());
            let placement = Point2::new(
                circle.centre.u + dir.0 * (circle.radius + 16.0),
                circle.centre.v + dir.1 * (circle.radius + 12.0),
            );
            Annotation {
                id,
                view,
                body: AnnotationKind::HoleCallout { text },
                anchors: vec![anchor],
                placement,
                style: StyleOverrides::default(),
                state: AnnotationState::Ok,
            }
        }
        _ => Annotation {
            id,
            view,
            body: AnnotationKind::CentreMark,
            anchors: vec![anchor],
            placement: circle.centre,
            style: StyleOverrides::default(),
            state: AnnotationState::Ok,
        },
    };
    push(DrawingAction::Command(Command::AddAnnotation { annotation: a }));
    push(DrawingAction::Select(Some(DrawingSel::Annotation(id))));
    push(DrawingAction::SetTool(DrawingTool::Select));
}

fn reattach_tool(
    s: &mut DrawingSession,
    annotation: AnnotationId,
    end: usize,
    view: ViewId,
    index: usize,
    picked_end: Option<Attach>,
    push: &mut impl FnMut(DrawingAction),
) {
    let Some(a) = s.doc.drawing.annotation(annotation) else {
        s.set_tool(DrawingTool::Select);
        return;
    };
    if a.view != view {
        s.say("Pick in the view the dimension belongs to");
        return;
    }
    let Some(c) = s.sheet().view(view).and_then(|v| v.geometry.curves.get(index)).cloned() else { return };
    let mut entity = c.source.clone();
    entity.kernel_hint = c.source.kernel_hint;
    // Which end of the picked edge: an end pick keeps it, otherwise the anchor's own kind.
    let next_lost: Vec<usize> = match &a.state {
        AnnotationState::Dangling { lost, .. } => lost.iter().copied().filter(|i| *i != end).collect(),
        _ => Vec::new(),
    };
    if let Some(e) = picked_end
        && let Some(an) =
            s.doc.drawing.sheets.iter_mut().flat_map(|sh| sh.annotations.iter_mut()).find(|x| x.id == annotation)
        && let Some(anchor) = an.anchors.get_mut(end)
    {
        anchor.attach = e;
    }
    push(DrawingAction::Command(Command::Reattach { annotation, end, entity }));
    match next_lost.first() {
        Some(next) => {
            let tool = DrawingTool::Reattach { annotation, end: *next };
            s.say(format!("Reattached end {} · now pick for end {}", end + 1, next + 1));
            push(DrawingAction::SetTool(tool));
        }
        None => {
            s.say("Reattached; the dimension is measured again");
            push(DrawingAction::SetTool(DrawingTool::Select));
            push(DrawingAction::Select(Some(DrawingSel::Annotation(annotation))));
        }
    }
}

/// The banner over the canvas after an update: what changed in the part and what it
/// did to the sheet, with Review and Undo update.
fn banner(
    ui: &mut egui::Ui,
    s: &DrawingSession,
    rect: Rect,
    r: &super::document::UpdateReport,
    push: &mut impl FnMut(DrawingAction),
) {
    let part = &s.doc.drawing.meta.title;
    let what = if r.change.is_empty() { String::new() } else { format!(": {}", r.change) };
    let head = format!("{part} changed (rev {}{what})", r.to_revision.short());
    let mut parts = vec![format!("{} view{} updated", r.views, if r.views == 1 { "" } else { "s" })];
    if !r.changed.is_empty() {
        parts.push(format!("{} value{} changed", r.changed.len(), if r.changed.len() == 1 { "" } else { "s" }));
    }
    if !r.dangling.is_empty() {
        parts.push(format!(
            "{} dimension{} lost their references",
            r.dangling.len(),
            if r.dangling.len() == 1 { "" } else { "s" }
        ));
    }
    let detail = parts.join(" · ");
    let width = (rect.width() - 40.0).min(820.0);
    let area = Rect::from_min_size(pos2(rect.center().x - width / 2.0, rect.top() + 38.0), vec2(width, 44.0));
    let bg =
        if r.dangling.is_empty() { Color32::from_rgb(0xfd, 0xf6, 0xe3) } else { Color32::from_rgb(0xfd, 0xee, 0xe8) };
    let stroke = if r.dangling.is_empty() { theme::warn() } else { theme::error() };
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            let painter = ui.painter();
            painter.rect(
                area,
                CornerRadius::same(8),
                bg,
                Stroke::new(1.0, stroke.gamma_multiply(0.5)),
                StrokeKind::Inside,
            );
            let icon = Rect::from_center_size(pos2(area.left() + 18.0, area.center().y), vec2(14.0, 14.0));
            icons::paint(painter, Icon::Warning, icon, stroke);
            ui.add_space(30.0);
            ui.vertical(|ui| {
                ui.add_space(5.0);
                ui.label(egui::RichText::new(head).family(theme::semibold()).size(12.0).color(theme::ink()));
                ui.label(egui::RichText::new(detail).size(11.5).color(theme::ink()));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0);
                if ui
                    .add(egui::Button::new(egui::RichText::new("×").size(13.0)).frame(false))
                    .on_hover_text("Dismiss")
                    .clicked()
                {
                    push(DrawingAction::DismissBanner);
                }
                if ui.button("Undo update").on_hover_text("One undo step: the views and references go back").clicked() {
                    push(DrawingAction::UndoUpdate);
                }
                let review =
                    egui::Button::new(egui::RichText::new("Review").color(Color32::WHITE)).fill(theme::accent());
                if ui.add(review).on_hover_text("Open the Needs attention list").clicked() {
                    push(DrawingAction::Review);
                }
            });
        },
    );
}

/// The dimension palette: tolerance, precision, parentheses and text on the selected
/// dimension.
fn palette(
    ui: &mut egui::Ui,
    s: &DrawingSession,
    id: AnnotationId,
    d: &Dimension,
    at: Pos2,
    push: &mut impl FnMut(DrawingAction),
) {
    let rect = s.canvas_rect;
    let pos = pos2((at.x - 150.0).clamp(rect.left() + 6.0, rect.right() - 306.0), (at.y - 58.0).max(rect.top() + 6.0));
    let mut edited = d.clone();
    let mut changed = false;
    egui::Area::new(ui.id().with(("dim_palette", id.0))).order(egui::Order::Foreground).fixed_pos(pos).show(
        ui.ctx(),
        |ui| {
            egui::Frame::new()
                .fill(theme::panel())
                .stroke(Stroke::new(1.0, theme::line()))
                .corner_radius(CornerRadius::same(8))
                .shadow(egui::Shadow { offset: [0, 3], blur: 10, spread: 0, color: Color32::from_black_alpha(22) })
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(s.doc.dimension_text(d))
                                .family(theme::semibold())
                                .size(12.5)
                                .color(theme::ink()),
                        );
                        ui.separator();
                        let sym = matches!(edited.tolerance, Tolerance::Symmetric { .. });
                        let dev = matches!(edited.tolerance, Tolerance::Deviation { .. });
                        if ui.selectable_label(sym, "±").on_hover_text("Symmetric tolerance").clicked() {
                            edited.tolerance = if sym { Tolerance::None } else { Tolerance::Symmetric { value: 0.05 } };
                            changed = true;
                        }
                        if ui.selectable_label(dev, "+/−").on_hover_text("Upper and lower deviations").clicked() {
                            edited.tolerance =
                                if dev { Tolerance::None } else { Tolerance::Deviation { upper: 0.1, lower: -0.05 } };
                            changed = true;
                        }
                        match &mut edited.tolerance {
                            Tolerance::Symmetric { value } => {
                                let r = ui
                                    .add(egui::DragValue::new(value).speed(0.01).range(0.001..=10.0).fixed_decimals(2));
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::WidgetType::DragValue, true, "Tolerance value")
                                });
                                changed |= r.changed();
                            }
                            Tolerance::Deviation { upper, lower } => {
                                changed |= ui.add(egui::DragValue::new(upper).speed(0.01).fixed_decimals(2)).changed();
                                changed |= ui.add(egui::DragValue::new(lower).speed(0.01).fixed_decimals(2)).changed();
                            }
                            _ => {}
                        }
                        ui.separator();
                        for (p, label) in [(0u8, "0"), (1, ".1"), (2, ".12"), (3, ".123")] {
                            if ui
                                .selectable_label(edited.precision == p, label)
                                .on_hover_text("Decimal places")
                                .clicked()
                            {
                                edited.precision = p;
                                changed = true;
                            }
                        }
                        ui.separator();
                        let round = matches!(edited.kind, DimensionKind::Diameter | DimensionKind::Radius);
                        if round {
                            let dia = edited.kind == DimensionKind::Diameter;
                            if ui.selectable_label(dia, "⌀").on_hover_text("Diameter").clicked() && !dia {
                                edited.kind = DimensionKind::Diameter;
                                if let soliddraft_drawing::DimValue::Measured { value } = &mut edited.value {
                                    *value *= 2.0;
                                }
                                changed = true;
                            }
                            if ui.selectable_label(!dia, "R").on_hover_text("Radius").clicked() && dia {
                                edited.kind = DimensionKind::Radius;
                                if let soliddraft_drawing::DimValue::Measured { value } = &mut edited.value {
                                    *value /= 2.0;
                                }
                                changed = true;
                            }
                        }
                        let parens = edited.text.starts_with('(') && edited.text.ends_with(')');
                        if ui.selectable_label(parens, "( )").on_hover_text("Parentheses: a reference value").clicked()
                        {
                            edited.text = if parens {
                                edited.text[1..edited.text.len() - 1].to_owned()
                            } else {
                                format!("({})", edited.text)
                            };
                            changed = true;
                        }
                        ui.separator();
                        let mut text = edited.text.clone();
                        let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(70.0).hint_text("<DIM>"));
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Dimension text"));
                        if r.lost_focus() && text != edited.text {
                            edited.text = if text.trim().is_empty() { Dimension::VALUE_TEXT.to_owned() } else { text };
                            changed = true;
                        }
                    });
                });
        },
    );
    if changed {
        push(DrawingAction::Command(Command::EditDimension { annotation: id, dimension: edited }));
    }
}
