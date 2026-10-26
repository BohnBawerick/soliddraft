//! PDF from display lists: one page per sheet at the paper size, vector paths with their
//! widths and dash patterns, and text in an embedded subset of the sheet font with a
//! Unicode map, so the text can be searched and copied.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use kurbo::PathEl;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use subsetter::GlyphRemapper;

use crate::DrawingError;
use crate::display::{DisplayList, Item, Role};
use crate::text::{self, FONT};

/// PDF points per millimetre.
const PT_PER_MM: f64 = 72.0 / 25.4;

/// Document properties of the PDF.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfInfo {
    /// Title, such as "Bracket".
    pub title: String,
    /// Author.
    pub author: String,
}

/// Writes one page per display list.
pub fn pdf(sheets: &[DisplayList], info: &PdfInfo) -> Result<Vec<u8>, DrawingError> {
    super::proven(sheets)?;
    // Every glyph the text uses, for the font subset.
    let mut used: BTreeSet<u16> = BTreeSet::new();
    let mut chars: BTreeMap<u16, char> = BTreeMap::new();
    for list in sheets {
        for item in list.printable() {
            if let Item::Text(t) = item {
                for c in t.text.chars() {
                    let g = text::glyph(c).0;
                    used.insert(g);
                    chars.entry(g).or_insert(c);
                }
            }
        }
    }
    let mut remapper = GlyphRemapper::new();
    for g in &used {
        remapper.remap(*g);
    }
    let subset = subsetter::subset(FONT, 0, &remapper).map_err(|e| DrawingError::Io(format!("font subset: {e}")))?;

    let mut next = 1;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let (catalog, tree, info_id) = (alloc(), alloc(), alloc());
    let (font, cid_font, descriptor, font_file, cmap) = (alloc(), alloc(), alloc(), alloc(), alloc());
    let pages: Vec<(Ref, Ref)> = sheets.iter().map(|_| (alloc(), alloc())).collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids(pages.iter().map(|(p, _)| *p)).count(pages.len() as i32);
    let mut doc = pdf.document_info(info_id);
    if !info.title.is_empty() {
        doc.title(TextStr(&info.title));
    }
    if !info.author.is_empty() {
        doc.author(TextStr(&info.author));
    }
    let producer = format!("soliddraft {}", env!("CARGO_PKG_VERSION"));
    doc.producer(TextStr(&producer));
    doc.finish();

    let font_name = Name(b"F1");
    for (list, (page_id, content_id)) in sheets.iter().zip(&pages) {
        let mut page = pdf.page(*page_id);
        let (w, h) = list.paper;
        page.media_box(Rect::new(0.0, 0.0, (w * PT_PER_MM) as f32, (h * PT_PER_MM) as f32));
        page.parent(tree);
        page.contents(*content_id);
        page.resources().fonts().pair(font_name, font);
        page.finish();
        let content = page_content(list, &remapper, font_name);
        pdf.stream(*content_id, &deflate(&content)?).filter(Filter::FlateDecode);
    }

    // The font: a Type 0 font over a TrueType CID font whose CIDs are the subset's glyph
    // ids.
    let tag = subset_tag(&used);
    let base = format!("{tag}+{}", text::postscript_name());
    let base = Name(base.as_bytes());
    let system = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
    pdf.type0_font(font)
        .base_font(base)
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid_font)
        .to_unicode(cmap);
    let upm = f64::from(text::units_per_em());
    let em = |v: f64| (v * 1000.0 / upm) as f32;
    let mut cid = pdf.cid_font(cid_font);
    cid.subtype(CidFontType::Type2);
    cid.base_font(base);
    cid.system_info(system);
    cid.font_descriptor(descriptor);
    cid.cid_to_gid_map_predefined(Name(b"Identity"));
    cid.default_width(0.0);
    {
        let mut widths = cid.widths();
        for g in &used {
            let new = remapper.get(*g).unwrap_or(0);
            widths.consecutive(new, [em(f64::from(text::advance(ttf_parser::GlyphId(*g))))]);
        }
    }
    cid.finish();
    let (ascender, descender, cap, bbox) = text::metrics();
    pdf.font_descriptor(descriptor)
        .name(base)
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(em(f64::from(bbox[0])), em(f64::from(bbox[1])), em(f64::from(bbox[2])), em(f64::from(bbox[3]))))
        .italic_angle(0.0)
        .ascent(em(f64::from(ascender)))
        .descent(em(f64::from(descender)))
        .cap_height(em(f64::from(cap)))
        .stem_v(80.0)
        .font_file2(font_file);
    pdf.stream(font_file, &deflate(&subset)?).filter(Filter::FlateDecode);
    let mut unicode = UnicodeCmap::new(Name(b"Custom"), system);
    for (g, c) in &chars {
        unicode.pair(remapper.get(*g).unwrap_or(0), *c);
    }
    pdf.cmap(cmap, &unicode.finish());
    // A fixed file id keeps the output the same for the same drawing.
    pdf.set_file_id((tag.as_bytes().to_vec(), tag.as_bytes().to_vec()));
    Ok(pdf.finish())
}

fn deflate(bytes: &[u8]) -> Result<Vec<u8>, DrawingError> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(bytes).map_err(|e| DrawingError::Io(e.to_string()))?;
    e.finish().map_err(|e| DrawingError::Io(e.to_string()))
}

/// Six capital letters from the glyph set, as PDF asks of subset font names.
fn subset_tag(glyphs: &BTreeSet<u16>) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for g in glyphs {
        for b in g.to_be_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
    (0..6).map(|i| char::from(b'A' + ((h >> (i * 5)) % 26) as u8)).collect()
}

fn page_content(list: &DisplayList, remapper: &GlyphRemapper, font: Name) -> Vec<u8> {
    let mut c = Content::new();
    // Millimetres, Y up, as the display list is.
    let k = PT_PER_MM as f32;
    c.transform([k, 0.0, 0.0, k, 0.0, 0.0]);
    c.set_line_cap(pdf_writer::types::LineCapStyle::RoundCap);
    c.set_line_join(pdf_writer::types::LineJoinStyle::RoundJoin);
    let rgb = |role: Role| {
        let [r, g, b] = super::colour(role);
        (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0)
    };
    for item in list.printable() {
        match item {
            Item::Path(p) if p.fill => {
                let (r, g, b) = rgb(p.role);
                c.set_fill_rgb(r, g, b);
                trace(&mut c, &p.path);
                c.fill_nonzero();
            }
            Item::Path(p) => {
                let (r, g, b) = rgb(p.role);
                c.set_stroke_rgb(r, g, b);
                c.set_line_width(p.width as f32);
                c.set_dash_pattern(p.dash.iter().flatten().map(|d| *d as f32), 0.0);
                trace(&mut c, &p.path);
                c.stroke();
            }
            Item::Text(t) => {
                let (at, size) = super::baseline(t);
                let (sin, cos) = t.angle.to_radians().sin_cos();
                let (r, g, b) = rgb(t.role);
                let mut glyphs = Vec::with_capacity(t.text.len() * 2);
                for ch in t.text.chars() {
                    let id = remapper.get(text::glyph(ch).0).unwrap_or(0);
                    glyphs.extend_from_slice(&id.to_be_bytes());
                }
                c.set_fill_rgb(r, g, b);
                c.begin_text();
                c.set_font(font, size as f32);
                c.set_text_matrix([cos as f32, sin as f32, -sin as f32, cos as f32, at.u as f32, at.v as f32]);
                c.show(Str(&glyphs));
                c.end_text();
            }
        }
    }
    c.finish().into_vec()
}

fn trace(c: &mut Content, path: &kurbo::BezPath) {
    let f = |v: f64| v as f32;
    let mut last = kurbo::Point::ZERO;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                c.move_to(f(p.x), f(p.y));
                last = p;
            }
            PathEl::LineTo(p) => {
                c.line_to(f(p.x), f(p.y));
                last = p;
            }
            PathEl::QuadTo(a, p) => {
                // PDF has no quadratic segments: raise to a cubic.
                let q = kurbo::QuadBez::new(last, a, p).raise();
                c.cubic_to(f(q.p1.x), f(q.p1.y), f(q.p2.x), f(q.p2.y), f(p.x), f(p.y));
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                c.cubic_to(f(a.x), f(a.y), f(b.x), f(b.y), f(p.x), f(p.y));
                last = p;
            }
            PathEl::ClosePath => {
                c.close_path();
            }
        }
    }
}
