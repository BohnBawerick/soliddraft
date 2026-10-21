//! SVG from a display list: one group per DXF layer, coordinates in millimetres with the
//! paper's top-left corner at the origin, numbers rounded to a micrometre so the output
//! is stable enough for golden files.

use std::fmt::Write as _;

use kurbo::{BezPath, PathEl};

use super::{LAYERS, baseline, colour, layer};
use crate::display::{DisplayList, Item};
use crate::library::Weight;
use crate::text::FONT_FAMILY;

/// Writes the display list as an SVG document sized to the paper.
pub fn svg(list: &DisplayList) -> String {
    let (w, h) = list.paper;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="0 0 {} {}">"#,
        num(w),
        num(h),
        num(w),
        num(h)
    );
    let _ = writeln!(out, r##"<rect width="{}" height="{}" fill="#fff"/>"##, num(w), num(h));
    for name in LAYERS {
        let items: Vec<&Item> = list.printable().filter(|i| layer(i.role()) == name).collect();
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            r#"<g id="{name}" fill="none" stroke-linecap="round" stroke-linejoin="round" font-family="{FONT_FAMILY}, sans-serif">"#
        );
        for item in items {
            match item {
                Item::Path(p) if p.fill => {
                    let _ = writeln!(
                        out,
                        r#"<path d="{}" fill="{}" stroke="none"/>"#,
                        data(&p.path, h),
                        hex(colour(p.role))
                    );
                }
                Item::Path(p) => {
                    let dash = match &p.dash {
                        Some(d) if !d.is_empty() => {
                            let d: Vec<String> = d.iter().map(|v| num(*v)).collect();
                            format!(r#" stroke-dasharray="{}""#, d.join(" "))
                        }
                        _ => String::new(),
                    };
                    let _ = writeln!(
                        out,
                        r#"<path d="{}" stroke="{}" stroke-width="{}"{dash}/>"#,
                        data(&p.path, h),
                        hex(colour(p.role)),
                        num(p.width)
                    );
                }
                Item::Text(t) => {
                    let (at, size) = baseline(t);
                    let (x, y) = (num(at.u), num(h - at.v));
                    let rotate = if t.angle.abs() > 1e-9 {
                        format!(r#" transform="rotate({} {x} {y})""#, num(-t.angle))
                    } else {
                        String::new()
                    };
                    let weight = match t.weight {
                        Weight::Regular => "",
                        Weight::Medium => r#" font-weight="500""#,
                        Weight::Semibold => r#" font-weight="600""#,
                    };
                    let _ = writeln!(
                        out,
                        r#"<text x="{x}" y="{y}" font-size="{}"{weight}{rotate} fill="{}" stroke="none">{}</text>"#,
                        num(size),
                        hex(colour(t.role)),
                        escape(&t.text)
                    );
                }
            }
        }
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// A number with at most three decimals and no trailing zeros or negative zero.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".to_owned() } else { s.to_owned() }
}

/// Path data with Y flipped from the sheet's Y up to SVG's Y down.
fn data(path: &BezPath, h: f64) -> String {
    let mut d = String::new();
    let p = |pt: kurbo::Point| format!("{} {}", num(pt.x), num(h - pt.y));
    for el in path.elements() {
        if !d.is_empty() {
            d.push(' ');
        }
        match *el {
            PathEl::MoveTo(a) => d.push_str(&format!("M{}", p(a))),
            PathEl::LineTo(a) => d.push_str(&format!("L{}", p(a))),
            PathEl::QuadTo(a, b) => d.push_str(&format!("Q{} {}", p(a), p(b))),
            PathEl::CurveTo(a, b, c) => d.push_str(&format!("C{} {} {}", p(a), p(b), p(c))),
            PathEl::ClosePath => d.push('Z'),
        }
    }
    d
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::layout_sheet;
    use crate::fixture::bracket_a3;

    #[test]
    fn numbers_are_short_and_stable() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.0004), "0");
        assert_eq!(num(12.3456), "12.346");
        assert_eq!(num(-2.5), "-2.5");
    }

    #[test]
    fn the_fixture_writes_its_layers_with_text() {
        let s = svg(&layout_sheet(&bracket_a3(), 0));
        assert!(s.starts_with("<?xml") && s.trim_end().ends_with("</svg>"));
        assert!(s.contains(r#"width="420mm" height="297mm""#));
        for layer in ["FORMAT", "HIDDEN", "VISIBLE", "CENTER", "DIMENSION", "TEXT"] {
            assert!(s.contains(&format!(r#"<g id="{layer}""#)), "{layer}");
        }
        assert!(s.contains(">Bracket</text>") && s.contains("stroke-dasharray"));
        assert!(s.contains("rotate(-90"), "vertical dimension values are rotated");
    }
}
