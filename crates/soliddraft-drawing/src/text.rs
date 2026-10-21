//! The sheet font: Inter, the font the interface already uses, embedded so text is
//! measured and written the same way by every renderer.

use std::sync::OnceLock;

use ttf_parser::{Face, GlyphId};

/// Inter Regular (SIL Open Font License 1.1, `fonts/LICENSE-Inter.txt`).
pub static FONT: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");

/// The font family name renderers ask for.
pub const FONT_FAMILY: &str = "Inter";

fn face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| Face::parse(FONT, 0).expect("the bundled font parses"))
}

/// Characters the font lacks, drawn with a look-alike it has.
fn substitute(c: char) -> char {
    match c {
        '⌀' => 'Ø',
        '−' => '-',
        _ => c,
    }
}

/// The glyph for a character, with look-alikes for characters the font lacks and the
/// missing-glyph box for anything else.
pub fn glyph(c: char) -> GlyphId {
    let f = face();
    f.glyph_index(c).or_else(|| f.glyph_index(substitute(c))).unwrap_or(GlyphId(0))
}

/// Font units per em.
pub fn units_per_em() -> u16 {
    face().units_per_em()
}

/// The advance width of a glyph in font units.
pub fn advance(g: GlyphId) -> u16 {
    face().glyph_hor_advance(g).unwrap_or(0)
}

/// The width of one line of text at a given height (the font size, in millimetres).
pub fn text_width(text: &str, size: f64) -> f64 {
    let upm = f64::from(units_per_em());
    text.chars().map(|c| f64::from(advance(glyph(c)))).sum::<f64>() * size / upm
}

/// The cap height of the font as a fraction of its size, for placing text above lines.
pub fn cap_height() -> f64 {
    let f = face();
    f64::from(f.capital_height().unwrap_or(f.ascender())) / f64::from(f.units_per_em())
}

/// The font's PostScript name.
pub fn postscript_name() -> String {
    face()
        .names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
        .and_then(|n| n.to_string())
        .unwrap_or_else(|| "Inter-Regular".to_owned())
}

/// Font-wide metrics in font units: ascender, descender, cap height and bounding box.
pub fn metrics() -> (i16, i16, i16, [i16; 4]) {
    let f = face();
    let b = f.global_bounding_box();
    (f.ascender(), f.descender(), f.capital_height().unwrap_or(f.ascender()), [b.x_min, b.y_min, b.x_max, b.y_max])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_has_the_drawing_characters() {
        for c in "Bracket 0123456789 ±°·/X Ø".chars() {
            assert_ne!(glyph(c), GlyphId(0), "{c:?}");
        }
        assert_ne!(glyph('⌀'), GlyphId(0), "the diameter sign or its look-alike");
        let w = text_width("Bracket", 5.0);
        assert!(w > 15.0 && w < 25.0, "{w}");
        assert!(cap_height() > 0.6 && cap_height() < 0.8);
    }
}
