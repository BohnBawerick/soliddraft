//! A PNG of a sheet's line work, for file browsers and recent-file lists. Text is left
//! out: at thumbnail size it is noise.

use kurbo::PathEl;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

use crate::DrawingError;
use crate::display::{DisplayList, Item};

/// The sheet drawn `width_px` pixels wide on white, as PNG bytes.
pub fn png(list: &DisplayList, width_px: u32) -> Result<Vec<u8>, DrawingError> {
    let (w, h) = list.paper;
    let k = f64::from(width_px) / w;
    let height_px = (h * k).round().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width_px.max(1), height_px).ok_or(DrawingError::Io("thumbnail size".into()))?;
    pixmap.fill(Color::WHITE);
    // Millimetres, Y up, to pixels, Y down.
    let to_px = Transform::from_row(k as f32, 0.0, 0.0, -k as f32, 0.0, height_px as f32);
    for item in list.printable() {
        let Item::Path(p) = item else { continue };
        let Some(path) = skia_path(&p.path) else { continue };
        let [r, g, b] = super::colour(p.role);
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, 255);
        paint.anti_alias = true;
        if p.fill {
            pixmap.fill_path(&path, &paint, FillRule::Winding, to_px, None);
            continue;
        }
        let mut st = Stroke { width: (p.width * k).max(0.6) as f32, ..Stroke::default() };
        if let Some(d) = p.dash.as_ref().filter(|d| !d.is_empty()) {
            st.dash = StrokeDash::new(d.iter().map(|d| (d * k).max(0.5) as f32).collect(), 0.0);
        }
        // The stroke width is in pixels, so stroke the path after moving it.
        if let Some(moved) = path.transform(to_px) {
            pixmap.stroke_path(&moved, &paint, &st, Transform::identity(), None);
        }
    }
    pixmap.encode_png().map_err(|e| DrawingError::Io(format!("thumbnail: {e}")))
}

fn skia_path(path: &kurbo::BezPath) -> Option<tiny_skia::Path> {
    let mut b = PathBuilder::new();
    let f = |v: f64| v as f32;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => b.move_to(f(p.x), f(p.y)),
            PathEl::LineTo(p) => b.line_to(f(p.x), f(p.y)),
            PathEl::QuadTo(a, p) => b.quad_to(f(a.x), f(a.y), f(p.x), f(p.y)),
            PathEl::CurveTo(a, c, p) => b.cubic_to(f(a.x), f(a.y), f(c.x), f(c.y), f(p.x), f(p.y)),
            PathEl::ClosePath => b.close(),
        }
    }
    b.finish()
}
