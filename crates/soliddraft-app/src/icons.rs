//! Procedural icon set. Every glyph is drawn with the egui painter on a 16 by
//! 16 grid so it scales crisply, follows the text colour and carries no
//! third-party artwork.

use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Sketch,
    SmartDim,
    OnPlane,
    Extrude,
    Revolve,
    Sweep,
    Loft,
    Cut,
    Hole,
    Slot,
    Fillet,
    Chamfer,
    Shell,
    Draft,
    Import,
    LinearPattern,
    CircularPattern,
    Mirror,
    Plane,
    Axis,
    Point,
    Spark,
    Check,
    Chat,
    Search,
    Tree,
    Props,
    Config,
    Display,
    Origin,
    Material,
    Views,
    Fit,
    Section,
    Eye,
    Measure,
    Close,
    Send,
    ChevronRight,
    ChevronDown,
    ChevronLeft,
    Warning,
    Mass,
    Home,
    Wireframe,
    Undo,
    Redo,
    Line,
    Circle,
    Arc,
    Rectangle,
    Construction,
    Save,
    Open,
    Repair,
    Rollback,
    Relation,
    Suppress,
}

struct Grid {
    rect: Rect,
    stroke: Stroke,
}

impl Grid {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        let s = self.rect.width() / 16.0;
        pos2(self.rect.left() + x * s, self.rect.top() + y * s)
    }

    fn len(&self, v: f32) -> f32 {
        v * self.rect.width() / 16.0
    }

    fn line(&self, painter: &Painter, pts: &[(f32, f32)]) {
        painter.add(egui::Shape::line(pts.iter().map(|(x, y)| self.p(*x, *y)).collect(), self.stroke));
    }

    fn closed(&self, painter: &Painter, pts: &[(f32, f32)]) {
        painter.add(egui::Shape::closed_line(pts.iter().map(|(x, y)| self.p(*x, *y)).collect(), self.stroke));
    }

    fn circle(&self, painter: &Painter, cx: f32, cy: f32, r: f32) {
        painter.circle_stroke(self.p(cx, cy), self.len(r), self.stroke);
    }

    fn dot(&self, painter: &Painter, cx: f32, cy: f32, r: f32) {
        painter.circle_filled(self.p(cx, cy), self.len(r), self.stroke.color);
    }

    fn arc(&self, painter: &Painter, cx: f32, cy: f32, r: f32, from_deg: f32, to_deg: f32) {
        let n = 18;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = (from_deg + (to_deg - from_deg) * i as f32 / n as f32).to_radians();
                self.p(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        painter.add(egui::Shape::line(pts, self.stroke));
    }

    fn arrow_head(&self, painter: &Painter, tip: (f32, f32), dir: Vec2, size: f32) {
        let d = dir.normalized();
        let side = vec2(-d.y, d.x);
        let t = self.p(tip.0, tip.1);
        let s = self.len(size);
        painter.add(egui::Shape::line(vec![t - d * s + side * s * 0.7, t, t - d * s - side * s * 0.7], self.stroke));
    }

    fn rect(&self, painter: &Painter, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.closed(painter, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1)]);
    }
}

/// Paints `icon` inside `rect` (use a square) in `color`.
pub fn paint(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    let side = rect.width().min(rect.height());
    let rect = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let width = (side / 16.0 * 1.35).max(1.0);
    let g = Grid { rect, stroke: Stroke::new(width, color) };
    let p = painter;
    match icon {
        Icon::Sketch => {
            g.line(p, &[(3.0, 13.0), (11.0, 5.0), (13.0, 7.0), (5.0, 15.0), (2.5, 15.5)]);
            g.line(p, &[(2.5, 15.5), (3.0, 13.0)]);
            g.line(p, &[(9.5, 6.5), (11.5, 8.5)]);
            g.line(p, &[(3.0, 2.5), (13.0, 2.5)]);
        }
        Icon::SmartDim => {
            g.line(p, &[(2.0, 11.0), (14.0, 11.0)]);
            g.arrow_head(p, (2.0, 11.0), vec2(-1.0, 0.0), 2.4);
            g.arrow_head(p, (14.0, 11.0), vec2(1.0, 0.0), 2.4);
            g.line(p, &[(2.0, 4.0), (2.0, 13.0)]);
            g.line(p, &[(14.0, 4.0), (14.0, 13.0)]);
        }
        Icon::OnPlane => {
            g.closed(p, &[(2.0, 12.0), (6.0, 5.0), (14.0, 5.0), (10.0, 12.0)]);
            g.line(p, &[(6.0, 10.0), (10.0, 7.0)]);
        }
        Icon::Extrude => {
            g.closed(p, &[(2.5, 11.0), (6.5, 8.5), (13.5, 8.5), (9.5, 11.0)]);
            g.line(p, &[(2.5, 11.0), (2.5, 13.5), (9.5, 13.5), (9.5, 11.0)]);
            g.line(p, &[(13.5, 8.5), (13.5, 11.0), (9.5, 13.5)]);
            g.line(p, &[(8.0, 7.0), (8.0, 1.5)]);
            g.arrow_head(p, (8.0, 1.5), vec2(0.0, -1.0), 2.6);
        }
        Icon::Revolve => {
            g.line(p, &[(8.0, 1.5), (8.0, 14.5)]);
            g.line(p, &[(10.0, 4.0), (13.0, 4.0), (13.0, 12.0), (10.0, 12.0)]);
            g.arc(p, 8.0, 12.5, 5.0, 20.0, 160.0);
            g.arrow_head(p, (3.3, 14.2), vec2(-0.8, -0.6), 2.2);
        }
        Icon::Sweep => {
            let pts: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let t = i as f32 / 20.0;
                    (2.5 + 11.0 * t, 8.0 + 4.5 * (t * TAU).sin())
                })
                .collect();
            g.line(p, &pts);
            g.closed(p, &[(1.5, 6.0), (3.5, 6.0), (3.5, 10.0), (1.5, 10.0)]);
        }
        Icon::Loft => {
            g.closed(p, &[(5.0, 3.0), (11.0, 3.0), (14.0, 13.0), (2.0, 13.0)]);
            g.line(p, &[(6.5, 3.0), (5.0, 13.0)]);
            g.line(p, &[(9.5, 3.0), (11.0, 13.0)]);
        }
        Icon::Cut => {
            g.closed(
                p,
                &[(2.5, 13.5), (2.5, 4.5), (6.0, 4.5), (6.0, 8.5), (10.0, 8.5), (10.0, 4.5), (13.5, 4.5), (13.5, 13.5)],
            );
        }
        Icon::Hole => {
            g.rect(p, 2.5, 2.5, 13.5, 13.5);
            g.circle(p, 8.0, 8.0, 3.2);
        }
        Icon::Slot => {
            g.arc(p, 5.0, 8.0, 3.0, 90.0, 270.0);
            g.arc(p, 11.0, 8.0, 3.0, -90.0, 90.0);
            g.line(p, &[(5.0, 5.0), (11.0, 5.0)]);
            g.line(p, &[(5.0, 11.0), (11.0, 11.0)]);
        }
        Icon::Fillet => {
            g.line(p, &[(3.0, 2.5), (3.0, 13.0), (13.5, 13.0)]);
            g.line(p, &[(6.5, 2.5), (6.5, 6.0)]);
            g.arc(p, 13.5, 6.0, 7.0, 90.0, 180.0);
            g.line(p, &[(13.5, 13.0), (13.5, 9.5)]);
        }
        Icon::Chamfer => {
            g.line(p, &[(3.0, 2.5), (3.0, 13.0), (13.5, 13.0)]);
            g.line(p, &[(6.5, 2.5), (6.5, 6.0), (13.5, 9.5), (13.5, 13.0)]);
        }
        Icon::Shell => {
            g.line(p, &[(2.5, 3.0), (2.5, 13.5), (13.5, 13.5), (13.5, 3.0)]);
            g.line(p, &[(5.5, 3.0), (5.5, 10.5), (10.5, 10.5), (10.5, 3.0)]);
        }
        Icon::Draft => {
            g.closed(p, &[(5.0, 3.0), (11.0, 3.0), (13.5, 13.0), (2.5, 13.0)]);
            g.line(p, &[(8.0, 1.5), (8.0, 14.5)]);
        }
        Icon::Import => {
            g.line(p, &[(2.5, 9.0), (2.5, 13.5), (13.5, 13.5), (13.5, 9.0)]);
            g.line(p, &[(8.0, 2.0), (8.0, 10.0)]);
            g.line(p, &[(5.0, 7.0), (8.0, 10.0), (11.0, 7.0)]);
        }
        Icon::LinearPattern => {
            for (x, y) in [(2.5, 2.5), (9.0, 2.5), (2.5, 9.0), (9.0, 9.0)] {
                g.rect(p, x, y, x + 4.5, y + 4.5);
            }
        }
        Icon::CircularPattern => {
            for i in 0..6 {
                let a = i as f32 / 6.0 * TAU;
                g.dot(p, 8.0 + 5.2 * a.cos(), 8.0 + 5.2 * a.sin(), 1.3);
            }
            g.dot(p, 8.0, 8.0, 0.9);
        }
        Icon::Mirror => {
            g.closed(p, &[(2.0, 4.0), (6.5, 8.0), (2.0, 12.0)]);
            g.closed(p, &[(14.0, 4.0), (9.5, 8.0), (14.0, 12.0)]);
            for i in 0..4 {
                let y = 2.0 + i as f32 * 3.5;
                g.line(p, &[(8.0, y), (8.0, y + 1.8)]);
            }
        }
        Icon::Plane => {
            g.closed(p, &[(1.5, 12.0), (5.5, 4.0), (14.5, 4.0), (10.5, 12.0)]);
        }
        Icon::Axis => {
            g.line(p, &[(8.0, 1.5), (8.0, 14.5)]);
            g.arrow_head(p, (8.0, 1.5), vec2(0.0, -1.0), 2.4);
            g.arrow_head(p, (8.0, 14.5), vec2(0.0, 1.0), 2.4);
        }
        Icon::Point => {
            g.dot(p, 8.0, 8.0, 1.8);
            g.line(p, &[(8.0, 2.0), (8.0, 5.0)]);
            g.line(p, &[(8.0, 11.0), (8.0, 14.0)]);
            g.line(p, &[(2.0, 8.0), (5.0, 8.0)]);
            g.line(p, &[(11.0, 8.0), (14.0, 8.0)]);
        }
        Icon::Spark => {
            let pts: Vec<(f32, f32)> = (0..8)
                .map(|i| {
                    let a = i as f32 / 8.0 * TAU - TAU / 4.0;
                    let r = if i % 2 == 0 { 6.5 } else { 2.2 };
                    (8.0 + r * a.cos(), 8.0 + r * a.sin())
                })
                .collect();
            g.closed(p, &pts);
            g.dot(p, 13.0, 3.0, 1.0);
        }
        Icon::Check => g.line(p, &[(3.0, 8.5), (6.5, 12.0), (13.0, 4.5)]),
        Icon::Chat => {
            g.line(p, &[(3.0, 12.0), (3.0, 3.0), (13.5, 3.0), (13.5, 10.5), (7.0, 10.5), (3.0, 13.5)]);
            g.line(p, &[(3.0, 13.5), (3.0, 12.0)]);
        }
        Icon::Search => {
            g.circle(p, 7.0, 7.0, 4.2);
            g.line(p, &[(10.2, 10.2), (14.0, 14.0)]);
        }
        Icon::Tree => {
            g.line(p, &[(2.5, 3.5), (6.5, 3.5)]);
            g.line(p, &[(5.5, 8.0), (13.5, 8.0)]);
            g.line(p, &[(5.5, 12.5), (13.5, 12.5)]);
            g.line(p, &[(3.5, 5.0), (3.5, 12.5), (5.5, 12.5)]);
            g.line(p, &[(3.5, 8.0), (5.5, 8.0)]);
        }
        Icon::Props => {
            for (i, knob) in [(3.5, 10.5), (8.0, 5.5), (12.5, 9.0)] {
                g.line(p, &[(2.0, i), (14.0, i)]);
                g.dot(p, knob, i, 1.6);
            }
        }
        Icon::Config => {
            g.circle(p, 4.0, 4.0, 1.8);
            g.circle(p, 4.0, 12.0, 1.8);
            g.circle(p, 12.0, 8.0, 1.8);
            g.line(p, &[(5.6, 4.8), (10.4, 7.4)]);
            g.line(p, &[(5.6, 11.2), (10.4, 8.6)]);
        }
        Icon::Display => {
            g.circle(p, 8.0, 8.0, 5.5);
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = (-90.0 + 180.0 * i as f32 / 16.0_f32).to_radians();
                    g.p(8.0 + 5.5 * a.cos(), 8.0 + 5.5 * a.sin())
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Origin => {
            g.line(p, &[(3.0, 13.0), (13.0, 13.0)]);
            g.line(p, &[(3.0, 13.0), (3.0, 3.0)]);
            g.line(p, &[(3.0, 13.0), (9.5, 6.5)]);
            g.arrow_head(p, (13.0, 13.0), vec2(1.0, 0.0), 2.2);
            g.arrow_head(p, (3.0, 3.0), vec2(0.0, -1.0), 2.2);
            g.arrow_head(p, (9.5, 6.5), vec2(1.0, -1.0), 2.2);
        }
        Icon::Material => {
            let pts: Vec<(f32, f32)> = (0..6)
                .map(|i| {
                    let a = i as f32 / 6.0 * TAU;
                    (8.0 + 6.0 * a.cos(), 8.0 + 6.0 * a.sin())
                })
                .collect();
            g.closed(p, &pts);
            g.line(p, &[(8.0, 8.0), (8.0, 2.0)]);
            g.line(p, &[(8.0, 8.0), (13.2, 11.0)]);
            g.line(p, &[(8.0, 8.0), (2.8, 11.0)]);
        }
        Icon::Views | Icon::Home => {
            g.closed(p, &[(8.0, 2.0), (14.0, 5.0), (14.0, 11.0), (8.0, 14.0), (2.0, 11.0), (2.0, 5.0)]);
            g.line(p, &[(2.0, 5.0), (8.0, 8.0), (14.0, 5.0)]);
            g.line(p, &[(8.0, 8.0), (8.0, 14.0)]);
        }
        Icon::Fit => {
            g.line(p, &[(2.0, 6.0), (2.0, 2.0), (6.0, 2.0)]);
            g.line(p, &[(10.0, 2.0), (14.0, 2.0), (14.0, 6.0)]);
            g.line(p, &[(14.0, 10.0), (14.0, 14.0), (10.0, 14.0)]);
            g.line(p, &[(6.0, 14.0), (2.0, 14.0), (2.0, 10.0)]);
        }
        Icon::Section => {
            g.rect(p, 2.5, 2.5, 13.5, 13.5);
            g.line(p, &[(2.5, 13.5), (13.5, 2.5)]);
            g.line(p, &[(6.0, 13.5), (13.5, 6.0)]);
            g.line(p, &[(9.5, 13.5), (13.5, 9.5)]);
        }
        Icon::Eye => {
            g.arc(p, 8.0, 12.5, 8.0, 215.0, 325.0);
            g.arc(p, 8.0, 3.5, 8.0, 35.0, 145.0);
            g.circle(p, 8.0, 8.0, 2.2);
        }
        Icon::Measure => {
            g.closed(p, &[(2.0, 11.0), (11.0, 2.0), (14.0, 5.0), (5.0, 14.0)]);
            for i in 0..3 {
                let t = 4.5 + i as f32 * 2.5;
                g.line(p, &[(t, 13.0 - t + 2.0 - 2.0), (t + 1.6, 13.0 - t + 2.0 - 0.4)]);
            }
        }
        Icon::Close => {
            g.line(p, &[(4.0, 4.0), (12.0, 12.0)]);
            g.line(p, &[(12.0, 4.0), (4.0, 12.0)]);
        }
        Icon::Send => {
            g.line(p, &[(2.5, 8.0), (13.0, 8.0)]);
            g.arrow_head(p, (13.5, 8.0), vec2(1.0, 0.0), 3.2);
        }
        Icon::ChevronRight => g.line(p, &[(6.0, 3.5), (10.5, 8.0), (6.0, 12.5)]),
        Icon::ChevronDown => g.line(p, &[(3.5, 6.0), (8.0, 10.5), (12.5, 6.0)]),
        Icon::ChevronLeft => g.line(p, &[(10.0, 3.5), (5.5, 8.0), (10.0, 12.5)]),
        Icon::Undo => {
            g.arc(p, 8.5, 8.5, 5.0, -150.0, 60.0);
            g.arrow_head(p, (3.8, 6.3), vec2(-0.6, 1.0), 2.4);
        }
        Icon::Redo => {
            g.arc(p, 7.5, 8.5, 5.0, 120.0, 330.0);
            g.arrow_head(p, (12.2, 6.3), vec2(0.6, 1.0), 2.4);
        }
        Icon::Line => {
            g.line(p, &[(3.0, 13.0), (13.0, 3.0)]);
            g.dot(p, 3.0, 13.0, 1.4);
            g.dot(p, 13.0, 3.0, 1.4);
        }
        Icon::Circle => {
            g.circle(p, 8.0, 8.0, 5.5);
            g.dot(p, 8.0, 8.0, 1.0);
        }
        Icon::Arc => {
            g.arc(p, 8.0, 10.0, 6.0, 200.0, 340.0);
            g.dot(p, 2.4, 8.0, 1.3);
            g.dot(p, 13.6, 8.0, 1.3);
            g.dot(p, 8.0, 10.0, 1.0);
        }
        Icon::Rectangle => {
            g.rect(p, 2.5, 4.0, 13.5, 12.0);
            g.dot(p, 2.5, 4.0, 1.3);
            g.dot(p, 13.5, 12.0, 1.3);
        }
        Icon::Construction => {
            for i in 0..4 {
                let t = i as f32 * 3.0;
                g.line(p, &[(2.5 + t, 13.5 - t), (4.0 + t, 12.0 - t)]);
            }
        }
        Icon::Save => {
            g.closed(p, &[(2.5, 2.5), (11.5, 2.5), (13.5, 4.5), (13.5, 13.5), (2.5, 13.5)]);
            g.rect(p, 5.0, 2.5, 10.5, 6.0);
            g.rect(p, 4.5, 9.0, 11.5, 13.5);
        }
        Icon::Open => {
            g.line(p, &[(2.0, 13.0), (2.0, 3.5), (6.5, 3.5), (8.0, 5.0), (13.0, 5.0), (13.0, 7.0)]);
            g.closed(p, &[(2.0, 13.0), (4.0, 7.5), (14.5, 7.5), (12.5, 13.0)]);
        }
        Icon::Repair => {
            g.line(p, &[(3.0, 13.0), (9.0, 7.0)]);
            g.arc(p, 11.0, 5.0, 3.2, 20.0, 300.0);
            g.line(p, &[(13.5, 3.5), (12.0, 5.0), (13.0, 6.5)]);
        }
        Icon::Rollback => {
            g.line(p, &[(2.5, 8.0), (13.5, 8.0)]);
            g.line(p, &[(2.5, 5.0), (2.5, 11.0)]);
            g.line(p, &[(13.5, 5.0), (13.5, 11.0)]);
            g.line(p, &[(8.0, 3.0), (8.0, 13.0)]);
        }
        Icon::Relation => {
            g.line(p, &[(3.0, 12.0), (3.0, 4.0)]);
            g.line(p, &[(3.0, 12.0), (12.0, 12.0)]);
            g.arc(p, 3.0, 12.0, 5.0, 270.0, 360.0);
        }
        Icon::Suppress => {
            g.line(p, &[(2.0, 8.0), (14.0, 8.0)]);
            g.arc(p, 8.0, 8.0, 5.5, 200.0, 340.0);
            g.line(p, &[(3.5, 13.0), (12.5, 3.0)]);
        }
        Icon::Warning => {
            g.closed(p, &[(8.0, 2.0), (14.5, 14.0), (1.5, 14.0)]);
            g.line(p, &[(8.0, 6.0), (8.0, 9.8)]);
            g.dot(p, 8.0, 12.0, 0.9);
        }
        Icon::Mass => {
            g.line(p, &[(8.0, 2.0), (8.0, 4.5)]);
            g.line(p, &[(2.0, 5.0), (14.0, 5.0)]);
            g.line(p, &[(2.0, 5.0), (3.5, 10.0)]);
            g.line(p, &[(14.0, 5.0), (12.5, 10.0)]);
            g.arc(p, 4.0, 10.0, 2.2, 0.0, 180.0);
            g.arc(p, 12.0, 10.0, 2.2, 0.0, 180.0);
            g.line(p, &[(8.0, 5.0), (8.0, 14.0)]);
            g.line(p, &[(5.0, 14.0), (11.0, 14.0)]);
        }
        Icon::Wireframe => {
            g.closed(p, &[(8.0, 2.0), (14.0, 5.0), (14.0, 11.0), (8.0, 14.0), (2.0, 11.0), (2.0, 5.0)]);
            g.line(p, &[(2.0, 5.0), (8.0, 8.0), (14.0, 5.0)]);
            g.line(p, &[(8.0, 8.0), (8.0, 14.0)]);
            g.line(p, &[(2.0, 11.0), (8.0, 8.0)]);
            g.line(p, &[(8.0, 2.0), (8.0, 8.0)]);
            g.line(p, &[(14.0, 11.0), (8.0, 8.0)]);
        }
    }
}
