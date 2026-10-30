//! The dimensions that build the selected body, drawn where they act, the way CAD
//! applications show a feature's dimensions on selection: every dimension of the
//! sketches the body's features consume, on their planes, and each feature's own values
//! (depth, angle, radius, count and so on) at the feature. Double-clicking one edits it:
//! a sketch dimension in its sketch's value editor, a feature value in the feature's
//! editor.

use super::camera::Camera;
use crate::document::{Document, EndKind, Feature, FeatureId, FeatureKind, FeatureStatus, ProfileKind, fmt_mm};
use crate::sketch::{ConstraintId, P2, Sketch};
use crate::sketcher::{arrow, paint_closed_dimensions, value_pill};
use crate::theme;
use egui::{Color32, Pos2, Rect, Stroke, vec2};
use glam::Vec3;
use soliddraft_kernel_api::{HoleDepth, HoleHead, HoleTip, Plane, Point2};
use soliddraft_model as model;
use std::collections::HashMap;

/// What double-clicking a shown dimension edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimensionTarget {
    /// A dimension of a sketch, edited in the sketch.
    Sketch { sketch: FeatureId, constraint: ConstraintId },
    /// A value of a feature, edited in the feature's editor.
    Feature(FeatureId),
}

/// One dimension on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownDimension {
    pub target: DimensionTarget,
    /// The value's rectangle, which takes the pointer.
    pub rect: Rect,
    /// What the hover tip says: the feature and the value's name.
    pub tip: String,
}

fn gv(v: soliddraft_kernel_api::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

fn line_color(hover: bool) -> Color32 {
    if hover { theme::accent() } else { theme::ink().gamma_multiply(0.7) }
}

/// Paints every dimension of the body at the rollback bar and returns where each one is.
/// A value under `pointer` shows as hovered.
pub fn paint(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    doc: &Document,
    pointer: Option<Pos2>,
) -> Vec<ShownDimension> {
    let end = doc.rollback.unwrap_or(doc.features.len()).min(doc.features.len());
    let built: Vec<&Feature> =
        doc.features[..end].iter().filter(|f| !f.suppressed && f.status != FeatureStatus::Inactive).collect();
    let anchors = anchors(doc);
    let mut shown = Vec::new();
    let sketch_dims =
        |shown: &mut Vec<ShownDimension>, f: &Feature, plane: Plane, sketch: &Sketch, edit_sketch: bool| {
            let taken: Vec<Rect> = shown.iter().map(|d| d.rect).collect();
            for (constraint, r) in paint_closed_dimensions(painter, camera, rect, plane, sketch, &taken, pointer) {
                let name = sketch.constraint(constraint).map(|c| c.name()).unwrap_or("Dimension");
                let target = if edit_sketch {
                    DimensionTarget::Sketch { sketch: f.id, constraint }
                } else {
                    DimensionTarget::Feature(f.id)
                };
                shown.push(ShownDimension { target, rect: r, tip: format!("{} · {name}", f.name) });
            }
        };
    for f in &built {
        let mut depth: Option<(Plane, &Sketch, f64)> = None;
        let builtin;
        match &f.kind {
            FeatureKind::Sketch(s) => {
                // Only the sketches the body consumes build it.
                if let Some(plane) = f.plane
                    && built.iter().any(|c| c.kind.sketch() == Some(f.id))
                {
                    sketch_dims(&mut shown, f, plane, &s.sketch, true);
                }
            }
            FeatureKind::Extrude(p) => {
                let profile = match p.sketch {
                    None => {
                        let shape = match p.profile {
                            ProfileKind::Rectangle { width, height } => {
                                model::BuiltinShape::Rectangle { width, height }
                            }
                            ProfileKind::Circle { diameter } => model::BuiltinShape::Circle { diameter },
                        };
                        builtin = shape.sketch().ok();
                        // A built-in profile's sizes are the extrude's own values.
                        builtin.as_ref().map(|sk| {
                            sketch_dims(&mut shown, f, p.plane.to_kernel(), sk, false);
                            (p.plane.to_kernel(), sk)
                        })
                    }
                    Some(id) => doc.feature(id).and_then(|s| Some((s.plane?, &s.sketch()?.sketch))),
                };
                if p.end == EndKind::Blind
                    && let Some((plane, sk)) = profile
                {
                    depth = Some((plane, sk, if p.flip { -p.depth } else { p.depth }));
                }
            }
            FeatureKind::Cut(p) => {
                if p.end() == EndKind::Blind
                    && let Some(s) = doc.feature(p.sketch)
                    && let (Some(plane), Some(sk)) = (s.plane, s.sketch())
                {
                    // Cuts go against the sketch normal unless flipped.
                    depth = Some((plane, &sk.sketch, if p.flip { p.depth } else { -p.depth }));
                }
            }
            kind => {
                let values = values(kind);
                if values.is_empty() {
                    continue;
                }
                let Some(at) = anchors.get(&(f.id as u32)).copied().or_else(|| body_centre(doc)) else { continue };
                for r in paint_leader(painter, camera, rect, at, values, pointer) {
                    shown.push(ShownDimension { target: DimensionTarget::Feature(f.id), rect: r, tip: f.name.clone() });
                }
            }
        }
        let taken: Vec<Rect> = shown.iter().map(|d| d.rect).collect();
        if let Some((plane, sk, value)) = depth
            && let Some(r) = paint_depth(painter, camera, rect, plane, sk, value, &taken, pointer)
        {
            shown.push(ShownDimension {
                target: DimensionTarget::Feature(f.id),
                rect: r,
                tip: format!("{} · Depth", f.name),
            });
        }
    }
    shown
}

/// The values a feature shows beside its faces.
fn values(kind: &FeatureKind) -> Vec<String> {
    match kind {
        FeatureKind::Revolve(r) => vec![format!("{}°", fmt_mm(r.angle_deg))],
        FeatureKind::Fillet(p) => vec![format!("R{}", fmt_mm(p.radius))],
        FeatureKind::Chamfer(p) => vec![fmt_mm(p.distance)],
        FeatureKind::Draft(d) => vec![format!("{}°", fmt_mm(d.angle))],
        FeatureKind::Shell(s) => vec![fmt_mm(s.thickness)],
        FeatureKind::Hole(h) => {
            let mut v = vec![format!("Ø{}", fmt_mm(h.diameter))];
            if let HoleDepth::Blind { depth } = h.depth {
                v.push(format!("{} deep", fmt_mm(depth)));
                if let HoleTip::Cone { angle_deg } = h.tip {
                    v.push(format!("{}° tip", fmt_mm(angle_deg)));
                }
            }
            match h.head {
                Some(HoleHead::Counterbore { diameter, depth }) => {
                    v.push(format!("Ø{} × {} deep", fmt_mm(diameter), fmt_mm(depth)))
                }
                Some(HoleHead::Countersink { diameter, angle_deg }) => {
                    v.push(format!("Ø{} × {}°", fmt_mm(diameter), fmt_mm(angle_deg)))
                }
                None => {}
            }
            v
        }
        FeatureKind::Pattern(p) => match p.layout {
            model::PatternSpec::Linear { spacing, count, .. } => vec![format!("{count} ×"), fmt_mm(spacing)],
            model::PatternSpec::Circular { count, angle, .. } => {
                vec![format!("{count} ×"), format!("{}°", fmt_mm(angle))]
            }
        },
        FeatureKind::Sketch(_)
        | FeatureKind::Extrude(_)
        | FeatureKind::Cut(_)
        | FeatureKind::Mirror(_)
        | FeatureKind::Lattice(_)
        | FeatureKind::Gyroid(_)
        | FeatureKind::ImportStep(_) => Vec::new(),
    }
}

/// A point on the faces each feature made: the mesh vertex nearest their centre, by
/// feature id.
fn anchors(doc: &Document) -> HashMap<u32, Vec3> {
    let Some(mesh) = doc.display_body().and_then(|b| doc.meshes.get(&b)) else { return HashMap::new() };
    let mut sums: HashMap<u32, (Vec3, f32)> = HashMap::new();
    for (p, f) in mesh.positions.iter().zip(&mesh.face_features).filter(|(_, f)| **f != 0) {
        let s = sums.entry(*f).or_insert((Vec3::ZERO, 0.0));
        s.0 += *p;
        s.1 += 1.0;
    }
    let mut best: HashMap<u32, (Vec3, f32)> = HashMap::new();
    for (p, f) in mesh.positions.iter().zip(&mesh.face_features) {
        let Some((sum, n)) = sums.get(f) else { continue };
        let d = p.distance_squared(*sum / *n);
        if best.get(f).is_none_or(|(_, bd)| d < *bd) {
            best.insert(*f, (*p, d));
        }
    }
    best.into_iter().map(|(f, (p, _))| (f, p)).collect()
}

fn body_centre(doc: &Document) -> Option<Vec3> {
    let (lo, hi) = doc.display_body().and_then(|b| doc.meshes.get(&b))?.bounds()?;
    Some((lo + hi) / 2.0)
}

/// A value pill that may be hovered, with its rectangle.
fn pill(painter: &egui::Painter, at: Pos2, text: String, pointer: Option<Pos2>) -> (Rect, bool) {
    let size = painter.layout_no_wrap(text.clone(), theme::font_medium(11.5), theme::ink()).size();
    let hover = pointer.is_some_and(|p| Rect::from_center_size(at, size + vec2(12.0, 6.0)).contains(p));
    let (bg, stroke) = if hover { (theme::hover(), theme::accent()) } else { (theme::hud_bg(), theme::line()) };
    (value_pill(painter, at, text, theme::ink(), bg, Stroke::new(1.0, stroke)), hover)
}

/// Values beside a point on the feature, joined to it by a leader.
fn paint_leader(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    at: Vec3,
    values: Vec<String>,
    pointer: Option<Pos2>,
) -> Vec<Rect> {
    let Some(a) = camera.project(at, rect) else { return Vec::new() };
    let mut centre = a + vec2(48.0, -32.0);
    let mut rects = Vec::new();
    for text in values {
        let (r, _) = pill(painter, centre, text, pointer);
        centre.y += r.height() + 3.0;
        rects.push(r);
    }
    let hover = rects.iter().any(|r| pointer.is_some_and(|p| r.contains(p)));
    if let Some(first) = rects.first() {
        painter.line_segment([a, first.left_center()], Stroke::new(1.0, line_color(hover)));
    }
    painter.circle_filled(a, 2.5, line_color(hover));
    rects
}

/// A linear dimension of an extrude's or cut's depth, along the sketch normal from the
/// profile's upper-left extreme, which stands clear of the sketch's own dimensions. The
/// value sits mid-line, or past an end when that would cover one of the `taken` values.
#[allow(clippy::too_many_arguments)]
fn paint_depth(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    plane: Plane,
    sketch: &Sketch,
    depth: f64,
    taken: &[Rect],
    pointer: Option<Pos2>,
) -> Option<Rect> {
    let dir = P2::new(-1.0, 1.0).normalized();
    let corner = sketch
        .drawn()
        .filter(|e| !e.construction)
        .flat_map(|e| e.polyline(32))
        .max_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)))?;
    let base = gv(plane.to_world(Point2::new(corner.u, corner.v)));
    let out = (gv(plane.x_dir) * dir.u as f32 + gv(plane.y_dir()) * dir.v as f32) * (0.1 * depth.abs() as f32 + 4.0);
    let top = base + gv(plane.normal) * depth as f32;
    let [b, t, bo, to] = [base, top, base + out, top + out].map(|p| camera.project(p, rect));
    let (b, t, bo, to) = (b?, t?, bo?, to?);
    let text = fmt_mm(depth.abs());
    let size = painter.layout_no_wrap(text.clone(), theme::font_medium(11.5), theme::ink()).size();
    let along = (to - bo).normalized();
    let reach = along.x.abs() * size.x / 2.0 + along.y.abs() * size.y / 2.0 + 14.0;
    let mid = bo + (to - bo) / 2.0;
    let at = [mid, to + along * reach, bo - along * reach]
        .into_iter()
        .find(|&p| {
            let r = Rect::from_center_size(p, size + vec2(16.0, 10.0));
            !taken.iter().any(|t| t.intersects(r))
        })
        .unwrap_or(mid);
    let color = line_color(pointer.is_some_and(|p| Rect::from_center_size(at, size + vec2(12.0, 6.0)).contains(p)));
    let ext = |from: Pos2, to: Pos2| {
        let d = (to - from).normalized();
        painter.line_segment([from + d * 4.0, to + d * 5.0], Stroke::new(0.8, color.gamma_multiply(0.8)));
    };
    ext(b, bo);
    ext(t, to);
    painter.line_segment([bo, to], Stroke::new(1.0, color));
    // A value moved past an end hangs off that end of the line.
    if at != mid {
        let end = if (at - to).length() < (at - bo).length() { to } else { bo };
        painter.line_segment([end, at], Stroke::new(1.0, color));
    }
    let d = to - bo;
    if d.length() > 20.0 {
        arrow(painter, bo, -d, color);
        arrow(painter, to, d, color);
    }
    Some(pill(painter, at, text, pointer).0)
}
