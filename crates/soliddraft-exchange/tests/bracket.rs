//! The bracket exported to STEP, STL and 3MF on Catallax, through the same entry the
//! application and the command line use.

use std::f64::consts::PI;
use std::io::{Cursor, Read};

use soliddraft_exchange::{ExportError, Format, Report, export};
use soliddraft_kernel_api::{BodyId, Kernel};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_tessellation::ExportTolerance;

/// 120 x 90 x 10 plate and 10 x 90 x 60 flange, less the 40 x 12 slot through the flange,
/// with the radius 12 boss, less its radius 6 bore and four radius 4.5 holes.
const BRACKET_VOLUME: f64 = 108_000.0 + 54_000.0 - 9_000.0 - 4_800.0 + (2_880.0 - 1_080.0 - 810.0) * PI;

fn bracket(k: &CatallaxKernel) -> BodyId {
    let recipe = soliddraft_recipe::parse(include_str!("../../../recipes/bracket.json")).unwrap();
    soliddraft_recipe::build(k, &recipe).unwrap().1
}

#[test]
fn step_carries_the_proven_deviation_and_the_body_faces() {
    let k = CatallaxKernel::new();
    let body = bracket(&k);
    let out = export(&k, body, "bracket", Format::Step, &ExportTolerance::default()).unwrap();
    let Report::Step(r) = &out.report else { panic!("a STEP report") };
    assert_eq!(r.faces, k.topology(body).unwrap().faces.len());
    assert!(r.within_tolerance && r.max_deviation_mm <= 1e-7, "{r:?}");
    let text = String::from_utf8(out.bytes).unwrap();
    assert!(text.starts_with("ISO-10303-21;") && text.ends_with("END-ISO-10303-21;\n"));
    assert!(out.report.summary().contains("(proven)"), "{}", out.report.summary());
}

#[test]
fn meshes_are_closed_and_within_their_chord_height() {
    let k = CatallaxKernel::new();
    let body = bracket(&k);
    let tolerance = ExportTolerance::default();
    let area = k.mass_properties(body).unwrap().area;
    let exact = k.mass_properties(body).unwrap().volume;
    assert!((exact - BRACKET_VOLUME).abs() < 1e-6, "{exact} against {BRACKET_VOLUME}");

    let stl = export(&k, body, "bracket", Format::Stl, &tolerance).unwrap();
    let Report::Mesh(r) = &stl.report else { panic!("a mesh report") };
    assert!(r.closed, "the bracket's mesh is closed");
    assert_eq!(r.chord_height, tolerance.chord_height);
    assert!(!r.unavailable_controls.is_empty(), "Catallax cannot guarantee the angle; the report says so");
    assert_eq!(stl.bytes.len(), 84 + 50 * r.triangles);
    assert_eq!(u32::from_le_bytes(stl.bytes[80..84].try_into().unwrap()) as usize, r.triangles);

    // A closed mesh within the chord height of every face encloses a volume within area
    // times the chord height of the exact one.
    let welded = soliddraft_exchange::mesh::weld(&k.tessellate(body, &tolerance.params()).unwrap());
    assert!((welded.volume() - exact).abs() <= area * tolerance.chord_height, "{} against {exact}", welded.volume());

    let package = export(&k, body, "bracket", Format::ThreeMf, &tolerance).unwrap();
    assert_eq!(package.report, stl.report, "STL and 3MF share one mesh");
    let mut zip = zip::ZipArchive::new(Cursor::new(&package.bytes)).unwrap();
    let mut model = String::new();
    zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut model).unwrap();
    assert!(zip.by_name("[Content_Types].xml").is_ok() && zip.by_name("_rels/.rels").is_ok());
    assert!(model.contains("unit=\"millimeter\""));
    assert_eq!(model.matches("<vertex ").count(), r.vertices);
    assert_eq!(model.matches("<triangle ").count(), r.triangles);
    assert_eq!(package.bytes, export(&k, body, "bracket", Format::ThreeMf, &tolerance).unwrap().bytes, "repeatable");
}

#[test]
fn bad_settings_and_released_bodies_are_refused() {
    let k = CatallaxKernel::new();
    let body = bracket(&k);
    let zero = ExportTolerance { chord_height: 0.0, ..ExportTolerance::default() };
    assert!(matches!(export(&k, body, "b", Format::Stl, &zero), Err(ExportError::Invalid(_))));
    k.release(body).unwrap();
    assert!(matches!(export(&k, body, "b", Format::Step, &ExportTolerance::default()), Err(ExportError::Kernel(_))));
}

#[test]
fn formats_follow_extensions() {
    use std::path::Path;
    assert_eq!(Format::of_path(Path::new("a/b.STP")), Some(Format::Step));
    assert_eq!(Format::of_path(Path::new("b.step")), Some(Format::Step));
    assert_eq!(Format::of_path(Path::new("b.stl")), Some(Format::Stl));
    assert_eq!(Format::of_path(Path::new("b.3mf")), Some(Format::ThreeMf));
    assert_eq!(Format::of_path(Path::new("b.obj")), None);
    for f in Format::ALL {
        assert_eq!(Format::of_path(Path::new(&format!("x.{}", f.extension()))), Some(f));
    }
}
