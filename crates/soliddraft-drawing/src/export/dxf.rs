//! DXF from a display list: LINE, ARC, CIRCLE, SOLID (arrowheads) and TEXT entities on
//! the layers of [`LAYERS`](super::LAYERS), with ISO line types and weights, in
//! millimetres. Curves go through [`lines_and_arcs`], so a shop gets arcs and never
//! splines.
//!
//! Dimensions are DIMENSION entities: rotated, diameter, radius and ordinate dimensions
//! with their definition points, measured value and text, each drawing the display
//! list's own lines, arrows and text from an anonymous block, so a CAD program shows
//! exactly what the sheet shows and can still edit them as dimensions. Section hatching
//! is HATCH entities with the pattern's angle and spacing over the cut regions.

use dxf::entities::{
    Arc, Circle, DiameterDimension, DimensionBase, Entity, EntityType, Line, OrdinateDimension, RadialDimension,
    RotatedDimension, Solid, Text,
};
use dxf::enums::{AcadVersion, DimensionType, Units};
use dxf::tables::{DimStyle, Layer as DxfLayer, LineType, Style};
use dxf::{Block, Color, Point};

use std::collections::BTreeMap;

use super::arcs::{Piece, lines_and_arcs};
use super::{LAYERS, baseline, layer};
use crate::DrawingError;
use crate::display::{DimensionGeometry, DisplayList, HatchRecord, Item, Owner, Role};
use crate::ids::AnnotationId;

/// How far converted curves may stray from the display list's, in millimetres.
pub const ARC_TOLERANCE: f64 = 0.01;

/// The text style every TEXT entity uses.
const STYLE: &str = "SOLIDDRAFT";

/// The dimension style every DIMENSION entity uses.
const DIMSTYLE: &str = "SOLIDDRAFT";

/// Writes the display list as a DXF R2000 drawing.
pub fn dxf(list: &DisplayList) -> Result<Vec<u8>, DrawingError> {
    let mut d = dxf::Drawing::new();
    d.header.version = AcadVersion::R2000;
    d.header.default_drawing_units = Units::Millimeters;
    d.header.drawing_units = dxf::enums::DrawingUnits::Metric;
    d.header.minimum_drawing_extents = Point::new(0.0, 0.0, 0.0);
    d.header.maximum_drawing_extents = Point::new(list.paper.0, list.paper.1, 0.0);
    d.add_line_type(line_type("HIDDEN", "Hidden __ __ __", &[3.0, -1.5]));
    d.add_line_type(line_type("CENTER", "Center ____ _ ____", &[8.0, -1.5, 1.5, -1.5]));
    d.add_line_type(line_type("PHANTOM", "Phantom ____ _ _ ____", &[8.0, -1.5, 1.5, -1.5, 1.5, -1.5]));
    d.add_style(Style { name: STYLE.to_owned(), primary_font_file_name: "arial.ttf".to_owned(), ..Default::default() });
    for name in LAYERS {
        let line_type = match name {
            "HIDDEN" | "CENTER" | "PHANTOM" => name,
            "CUTTING" => "CENTER",
            _ => "CONTINUOUS",
        };
        d.add_layer(DxfLayer {
            name: name.to_owned(),
            color: Color::from_index(7),
            line_type_name: line_type.to_owned(),
            ..Default::default()
        });
    }
    let owners: Vec<AnnotationId> = {
        let mut seen = Vec::new();
        for r in &list.dimensions {
            if !seen.contains(&r.owner) {
                seen.push(r.owner);
            }
        }
        seen
    };
    // The dimension style the anonymous blocks were drawn with, so a CAD program that
    // redraws a dimension draws it the same size.
    let text_height = list
        .items
        .iter()
        .find_map(|i| match i {
            Item::Text(t) if matches!(t.owner, Owner::Annotation(a) if owners.contains(&a)) => Some(t.height),
            _ => None,
        })
        .unwrap_or(3.5);
    d.add_dim_style(DimStyle {
        name: DIMSTYLE.to_owned(),
        dimensioning_text_height: text_height,
        dimensioning_arrow_size: text_height,
        dimension_extension_line_offset: 1.0,
        dimension_extension_line_extension: 2.0,
        dimension_line_gap: 0.8,
        dimension_text_style: STYLE.to_owned(),
        ..Default::default()
    });
    let mut blocks: BTreeMap<AnnotationId, Vec<Entity>> = owners.iter().map(|o| (*o, Vec::new())).collect();
    for item in list.printable() {
        let entities = item_entities(item);
        match item.owner() {
            Owner::Annotation(a) | Owner::AnnotationEnd(a, _) if blocks.contains_key(&a) => {
                blocks.get_mut(&a).expect("listed").extend(entities);
            }
            // Hatching goes out as HATCH entities.
            _ if item.role() == Role::Hatch && !list.hatches.is_empty() => {}
            _ => {
                for e in entities {
                    d.add_entity(e);
                }
            }
        }
    }
    // One anonymous block per dimension; a dimension set's lines all go in the block of
    // its first value.
    let mut n = 0;
    for owner in &owners {
        let mut first = true;
        for r in list.dimensions.iter().filter(|r| r.owner == *owner) {
            n += 1;
            let name = format!("*D{n}");
            let entities = if first { blocks.remove(owner).unwrap_or_default() } else { Vec::new() };
            first = false;
            let mut block = Block { name: name.clone(), entities, ..Default::default() };
            block.set_is_anonymous(true);
            d.add_block(block);
            let at = |p: soliddraft_kernel_api::Point2| Point::new(p.u, p.v, 0.0);
            let base = DimensionBase {
                version: dxf::enums::Version::R2010,
                block_name: name,
                text_mid_point: at(r.text_mid),
                actual_measurement: r.value,
                text: control_codes(&r.text),
                dimension_style_name: DIMSTYLE.to_owned(),
                is_block_reference_referenced_by_this_block_only: true,
                is_at_user_defined_location: true,
                ..Default::default()
            };
            let specific = match r.geometry {
                DimensionGeometry::Linear { p1, p2, line, angle } => EntityType::RotatedDimension(RotatedDimension {
                    dimension_base: DimensionBase {
                        definition_point_1: at(line),
                        dimension_type: DimensionType::RotatedHorizontalOrVertical,
                        ..base
                    },
                    definition_point_2: at(p1),
                    definition_point_3: at(p2),
                    rotation_angle: angle,
                    ..Default::default()
                }),
                DimensionGeometry::Diameter { near, far } => EntityType::DiameterDimension(DiameterDimension {
                    dimension_base: DimensionBase {
                        definition_point_1: at(far),
                        dimension_type: DimensionType::Diameter,
                        ..base
                    },
                    definition_point_2: at(near),
                    leader_length: 0.0,
                }),
                DimensionGeometry::Radius { centre, on } => EntityType::RadialDimension(RadialDimension {
                    dimension_base: DimensionBase {
                        definition_point_1: at(centre),
                        dimension_type: DimensionType::Radius,
                        ..base
                    },
                    definition_point_2: at(on),
                    leader_length: 0.0,
                }),
                DimensionGeometry::Ordinate { origin, feature, leader_end, along_x } => {
                    EntityType::OrdinateDimension(OrdinateDimension {
                        dimension_base: DimensionBase {
                            definition_point_1: at(origin),
                            dimension_type: DimensionType::Ordinate,
                            is_ordinate_x_type: along_x,
                            ..base
                        },
                        definition_point_2: at(feature),
                        definition_point_3: at(leader_end),
                    })
                }
            };
            let mut e = Entity::new(specific);
            e.common.layer = "DIMENSION".to_owned();
            d.add_entity(e);
        }
    }
    let mut out = Vec::new();
    d.save(&mut out).map_err(|e| DrawingError::Io(format!("DXF: {e}")))?;
    finish(&out, &list.hatches)
}

/// The DXF entities of one display item.
fn item_entities(item: &Item) -> Vec<Entity> {
    let role = item.role();
    // Line weights in hundredths of a millimetre, on each entity: the dxf crate keeps
    // layer weights to itself.
    let weight = match item {
        Item::Path(p) => (p.width * 100.0).round() as i16,
        Item::Text(_) => 0,
    };
    let make = |specific: EntityType| {
        let mut e = Entity::new(specific);
        e.common.layer = layer(role).to_owned();
        e.common.lineweight_enum_value = weight;
        if role == Role::Error {
            e.common.color = Color::from_index(1);
        }
        e
    };
    let mut out = Vec::new();
    match item {
        Item::Path(p) if p.fill => {
            // Arrowheads are triangles: a SOLID repeats the last corner.
            let corners: Vec<Point> = p
                .path
                .elements()
                .iter()
                .filter_map(|el| match el {
                    kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => Some(Point::new(q.x, q.y, 0.0)),
                    _ => None,
                })
                .collect();
            let corners: Vec<Point> = corners
                .iter()
                .enumerate()
                .filter(|(i, q)| *i == 0 || (q.x, q.y) != (corners[0].x, corners[0].y))
                .map(|(_, q)| q.clone())
                .collect();
            if let [a, b, c] = corners.as_slice() {
                out.push(make(EntityType::Solid(Solid::new(a.clone(), b.clone(), c.clone(), c.clone()))));
            } else {
                for piece in lines_and_arcs(&p.path, ARC_TOLERANCE) {
                    out.push(make(entity(piece)));
                }
            }
        }
        Item::Path(p) => {
            for piece in lines_and_arcs(&p.path, ARC_TOLERANCE) {
                out.push(make(entity(piece)));
            }
        }
        Item::Text(t) => {
            let (at, _) = baseline(t);
            out.push(make(EntityType::Text(Text {
                location: Point::new(at.u, at.v, 0.0),
                text_height: t.height,
                value: control_codes(&t.text),
                rotation: t.angle,
                text_style_name: STYLE.to_owned(),
                ..Default::default()
            })));
        }
    }
    out
}

/// One DXF group code and its value, as text.
type Pair = (i32, String);

/// The DXF text as group code pairs, and the line ending it uses.
fn pairs(text: &str) -> Result<(Vec<Pair>, &'static str), DrawingError> {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.lines().collect();
    if !lines.len().is_multiple_of(2) {
        return Err(DrawingError::Format("DXF: an odd number of lines".into()));
    }
    let mut out = Vec::with_capacity(lines.len() / 2);
    for w in lines.as_chunks::<2>().0 {
        let code = w[0].trim().parse().map_err(|_| DrawingError::Format(format!("DXF: bad group code {:?}", w[0])))?;
        out.push((code, w[1].to_owned()));
    }
    Ok((out, ending))
}

/// Finishes a DXF file the dxf crate wrote. The crate writes no handle on a symbol
/// table's head and gives a block's end the block's own handle, which strict readers
/// reject, and it has no HATCH entity. Each of those gets a fresh handle above the
/// file's handle seed, and the hatches go at the end of the ENTITIES section with the
/// model space as their owner.
fn finish(bytes: &[u8], hatches: &[HatchRecord]) -> Result<Vec<u8>, DrawingError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DrawingError::Format("DXF: not text".into()))?;
    let (p, ending) = pairs(text)?;
    let seed = p
        .windows(2)
        .position(|w| w[0] == (9, "$HANDSEED".to_owned()))
        .map(|i| i + 1)
        .ok_or(DrawingError::Format("DXF: no $HANDSEED".into()))?;
    let mut next =
        u64::from_str_radix(p[seed].1.trim(), 16).map_err(|_| DrawingError::Format("DXF: bad handle seed".into()))?;
    let mut fresh = || {
        next += 1;
        format!("{:X}", next - 1)
    };
    let mut fixed: Vec<Pair> = Vec::with_capacity(p.len());
    let mut block_handle: Option<String> = None;
    let mut i = 0;
    while i < p.len() {
        let pair = &p[i];
        fixed.push(pair.clone());
        if *pair == (0, "TABLE".to_owned())
            && p.get(i + 1).is_some_and(|q| q.0 == 2)
            && p.get(i + 2).is_some_and(|q| q.0 != 5)
        {
            fixed.push(p[i + 1].clone());
            fixed.push((5, fresh()));
            i += 2;
            continue;
        }
        if *pair == (0, "BLOCK".to_owned()) {
            block_handle = p.get(i + 1).filter(|q| q.0 == 5).map(|q| q.1.clone());
        }
        if *pair == (0, "ENDBLK".to_owned())
            && let Some(h) = p.get(i + 1).filter(|q| q.0 == 5)
            && Some(&h.1) == block_handle.as_ref()
        {
            fixed.push((5, fresh()));
            i += 2;
            continue;
        }
        i += 1;
    }
    let mut p = fixed;
    // The model space's block record owns model space entities.
    let mut owner = None;
    for (i, pair) in p.iter().enumerate() {
        if pair.0 == 0 && pair.1 == "BLOCK_RECORD" {
            let mut handle = None;
            let mut name = None;
            for q in p[i + 1..].iter().take_while(|q| q.0 != 0) {
                match q.0 {
                    5 => handle = Some(q.1.clone()),
                    2 => name = Some(q.1.clone()),
                    _ => {}
                }
            }
            if name.is_some_and(|n| n.eq_ignore_ascii_case("*Model_Space")) {
                owner = handle;
            }
        }
    }
    let owner = owner.ok_or(DrawingError::Format("DXF: no model space block record".into()))?;
    let entities = p
        .windows(2)
        .position(|w| w[0] == (0, "SECTION".to_owned()) && w[1] == (2, "ENTITIES".to_owned()))
        .ok_or(DrawingError::Format("DXF: no ENTITIES section".into()))?;
    let end = entities
        + p[entities..]
            .iter()
            .position(|q| *q == (0, "ENDSEC".to_owned()))
            .ok_or(DrawingError::Format("DXF: open ENTITIES section".into()))?;
    let mut add: Vec<Pair> = Vec::new();
    let num = |v: f64| format!("{v:.6}");
    for h in hatches {
        add.push((0, "HATCH".into()));
        add.push((5, fresh()));
        add.push((330, owner.clone()));
        add.push((100, "AcDbEntity".into()));
        add.push((8, "HATCH".into()));
        add.push((100, "AcDbHatch".into()));
        for (code, v) in [(10, 0.0), (20, 0.0), (30, 0.0), (210, 0.0), (220, 0.0), (230, 1.0)] {
            add.push((code, num(v)));
        }
        add.push((2, "ANSI31".into()));
        add.push((70, "0".into()));
        add.push((71, "0".into()));
        add.push((91, h.loops.len().to_string()));
        for (i, l) in h.loops.iter().enumerate() {
            // Polyline boundaries; the first is the external one.
            add.push((92, if i == 0 { "3".into() } else { "2".into() }));
            add.push((72, "0".into()));
            add.push((73, "1".into()));
            add.push((93, l.len().to_string()));
            for q in l {
                add.push((10, num(q.u)));
                add.push((20, num(q.v)));
            }
            add.push((97, "0".into()));
        }
        // ANSI31 is lines at 45 degrees, 3.175 mm apart: turned and scaled to the
        // pattern, its one line runs at the pattern's angle and spacing.
        let (sin, cos) = h.angle.to_radians().sin_cos();
        add.push((75, "0".into()));
        add.push((76, "1".into()));
        add.push((52, num(h.angle - 45.0)));
        add.push((41, num(h.spacing / 3.175)));
        add.push((77, "0".into()));
        add.push((78, "1".into()));
        add.push((53, num(h.angle)));
        add.push((43, num(0.0)));
        add.push((44, num(0.0)));
        add.push((45, num(-h.spacing * sin)));
        add.push((46, num(h.spacing * cos)));
        add.push((79, "0".into()));
        add.push((98, "0".into()));
    }
    p.splice(end..end, add);
    let seed = p.iter().position(|q| q.1 == "$HANDSEED").expect("found above") + 1;
    p[seed].1 = fresh();
    let mut out = String::with_capacity(text.len() + 64 * hatches.len());
    for (code, value) in p {
        out.push_str(&format!("{code: >3}{ending}{value}{ending}"));
    }
    Ok(out.into_bytes())
}

fn entity(piece: Piece) -> EntityType {
    match piece {
        Piece::Line(a, b) => EntityType::Line(Line::new(Point::new(a.x, a.y, 0.0), Point::new(b.x, b.y, 0.0))),
        Piece::Arc { center, radius, start_deg, end_deg } => EntityType::Arc(Arc::new(
            Point::new(center.x, center.y, 0.0),
            radius,
            normalize(start_deg),
            normalize(end_deg),
        )),
        Piece::Circle { center, radius } => {
            EntityType::Circle(Circle::new(Point::new(center.x, center.y, 0.0), radius))
        }
    }
}

fn normalize(deg: f64) -> f64 {
    deg.rem_euclid(360.0)
}

fn line_type(name: &str, description: &str, pattern: &[f64]) -> LineType {
    LineType {
        name: name.to_owned(),
        description: description.to_owned(),
        element_count: pattern.len() as i32,
        total_pattern_length: pattern.iter().map(|v| v.abs()).sum(),
        dash_dot_space_lengths: pattern.to_vec(),
        ..Default::default()
    }
}

/// The entities of a DXF file counted by "LAYER/TYPE", such as "VISIBLE/LINE".
pub fn count_entities(bytes: &[u8]) -> Result<BTreeMap<String, usize>, DrawingError> {
    let d = dxf::Drawing::load(&mut &bytes[..]).map_err(|e| DrawingError::Format(format!("DXF: {e}")))?;
    let mut out = BTreeMap::new();
    for e in d.entities() {
        let kind = match &e.specific {
            EntityType::Line(_) => "LINE",
            EntityType::Arc(_) => "ARC",
            EntityType::Circle(_) => "CIRCLE",
            EntityType::Solid(_) => "SOLID",
            EntityType::Text(_) => "TEXT",
            EntityType::Spline(_) => "SPLINE",
            EntityType::LwPolyline(_) => "LWPOLYLINE",
            EntityType::Ellipse(_) => "ELLIPSE",
            EntityType::RotatedDimension(_)
            | EntityType::RadialDimension(_)
            | EntityType::DiameterDimension(_)
            | EntityType::OrdinateDimension(_)
            | EntityType::AngularThreePointDimension(_) => "DIMENSION",
            _ => "OTHER",
        };
        *out.entry(format!("{}/{kind}", e.common.layer)).or_default() += 1;
    }
    let text = String::from_utf8_lossy(bytes);
    let hatches = pairs(&text).map(|(p, _)| p.iter().filter(|q| **q == (0, "HATCH".to_owned())).count()).unwrap_or(0);
    if hatches > 0 {
        out.insert("HATCH/HATCH".to_owned(), hatches);
    }
    Ok(out)
}

/// AutoCAD's control codes for the drawing symbols DXF text cannot carry otherwise.
fn control_codes(text: &str) -> String {
    text.replace(['⌀', 'Ø'], "%%c").replace('°', "%%d").replace('±', "%%p")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::layout_sheet;
    use crate::fixture::bracket_a3;

    #[test]
    fn the_fixture_re_reads_with_entities_on_its_layers_and_no_splines() {
        let bytes = dxf(&layout_sheet(&bracket_a3(), 0)).unwrap();
        let counts = count_entities(&bytes).unwrap();
        assert!(!counts.keys().any(|k| k.ends_with("/SPLINE")), "{counts:?}");
        for key in [
            "VISIBLE/LINE",
            "HIDDEN/LINE",
            "CENTER/LINE",
            "DIMENSION/LINE",
            "DIMENSION/SOLID",
            "TEXT/TEXT",
            "FORMAT/LINE",
        ] {
            assert!(counts.get(key).is_some_and(|n| *n > 0), "{key}: {counts:?}");
        }
        assert!(counts["VISIBLE/CIRCLE"] >= 6, "holes, bore and boss come back as circles: {counts:?}");
        let back = dxf::Drawing::load(&mut bytes.as_slice()).unwrap();
        assert!(back.entities().any(|e| matches!(&e.specific, EntityType::Text(t) if t.value == "Bracket")));
        // Dimensions are DIMENSION entities whose blocks draw them.
        let diameter = back
            .entities()
            .find_map(|e| match &e.specific {
                EntityType::DiameterDimension(d) if d.dimension_base.text == "%%c24" => Some(d),
                _ => None,
            })
            .expect("the boss diameter is a DIMENSION");
        assert!((diameter.dimension_base.actual_measurement - 24.0).abs() < 1e-9);
        let block = back.blocks().find(|b| b.name == diameter.dimension_base.block_name).expect("its block");
        assert!(block.entities.iter().any(|e| matches!(&e.specific, EntityType::Text(t) if t.value == "%%c24")));
        assert!(counts.get("DIMENSION/DIMENSION").is_some_and(|n| *n >= 4), "{counts:?}");
    }
}
