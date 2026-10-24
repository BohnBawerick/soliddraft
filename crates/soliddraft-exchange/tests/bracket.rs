//! The bracket exported to STEP, STL, 3MF and OBJ on Catallax, through the same entry the
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

// Measurement belongs to this acceptance test, not the application export path.
fn volume(mesh: &soliddraft_exchange::Welded) -> f64 {
    let p = |i: u32| mesh.positions[i as usize].map(f64::from);
    mesh.triangles
        .iter()
        .map(|t| {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let cross = [b[1] * c[2] - b[2] * c[1], b[2] * c[0] - b[0] * c[2], b[0] * c[1] - b[1] * c[0]];
            (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0
        })
        .sum()
}

#[test]
fn step_carries_the_proven_deviation_and_the_body_faces() {
    let k = CatallaxKernel::new();
    let body = bracket(&k);
    let out = export(&k, body, "bracket", Format::Step, &ExportTolerance::default()).unwrap();
    let Report::Step(r) = &out.report else { panic!("a STEP report") };
    assert_eq!(r.faces, k.topology(body).unwrap().faces.len());
    assert!(r.within_tolerance && r.max_deviation_mm <= 1e-7, "{r:?}");
    let text = std::str::from_utf8(&out.bytes).unwrap();
    assert!(text.starts_with("ISO-10303-21;") && text.ends_with("END-ISO-10303-21;\n"));
    if let Some(directory) = std::env::var_os("SOLIDDRAFT_STEP_PROOF_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mass = k.mass_properties(body).unwrap();
        let report = serde_json::json!({
            "report": out.report,
            "body": {"faces": k.topology(body).unwrap().faces.len(), "volume": mass.volume, "area": mass.area},
        });
        std::fs::write(directory.join("bracket.step"), &out.bytes).unwrap();
        std::fs::write(directory.join("step-report.json"), serde_json::to_vec(&report).unwrap()).unwrap();
    }
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
    let source = k.tessellate(body, &tolerance.params()).unwrap();
    let welded = soliddraft_exchange::mesh::weld(&source);
    assert!((volume(&welded) - exact).abs() <= area * tolerance.chord_height, "{} against {exact}", volume(&welded));

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

    let obj = export(&k, body, "bracket", Format::Obj, &tolerance).unwrap();
    let Report::Mesh(obj_report) = &obj.report else { panic!("an OBJ mesh report") };
    assert_eq!(obj_report.chord_height, tolerance.chord_height);
    assert_eq!(obj_report.triangles, r.triangles);
    assert_eq!(obj_report.closed, r.closed);
    assert_eq!(obj_report.unavailable_controls, r.unavailable_controls);
    let text = std::str::from_utf8(&obj.bytes).unwrap();
    assert!(text.lines().any(|l| l == "o bracket"));
    let mut positions = Vec::<[f32; 3]>::new();
    let mut normals = Vec::<[f32; 3]>::new();
    let mut triangles = Vec::new();
    let mut faces = Vec::new();
    let mut face = None;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some(kind @ ("v" | "vn")) => {
                let p = std::array::from_fn(|_| words.next().unwrap().parse::<f32>().unwrap());
                if kind == "v" { positions.push(p) } else { normals.push(p) }
            }
            Some("g") => face = Some(words.next().unwrap().strip_prefix("face_").unwrap().parse::<u64>().unwrap()),
            Some("f") => {
                let t = std::array::from_fn(|_| {
                    let (v, n) = words.next().unwrap().split_once("//").unwrap();
                    let v = v.parse::<u32>().unwrap() - 1;
                    assert_eq!(v, n.parse::<u32>().unwrap() - 1);
                    assert!((v as usize) < positions.len() && (v as usize) < normals.len());
                    v
                });
                assert!(words.next().is_none(), "triangles only");
                triangles.push(t);
                faces.push(face.expect("every triangle belongs to a face"));
            }
            _ => {}
        }
    }
    assert_eq!(positions, source.positions, "decimal vertices round-trip to the proven display coordinates");
    assert_eq!(normals, source.normals, "sharp-face normals survive");
    assert_eq!(positions.len(), obj_report.vertices);
    let expected: Vec<_> = source
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .zip(&source.triangle_faces)
        .filter(|(t, _)| {
            let [a, b, c] = t.map(|i| source.positions[i as usize]);
            a != b && b != c && c != a
        })
        .collect();
    assert_eq!(triangles, expected.iter().map(|(t, _)| **t).collect::<Vec<_>>(), "winding and triangle order survive");
    assert_eq!(faces, expected.iter().map(|(_, f)| f.raw()).collect::<Vec<_>>(), "source face identities survive");
    assert_eq!(triangles.len(), obj_report.triangles);
    let reread = soliddraft_exchange::Welded { positions, triangles };
    assert!((volume(&reread) - exact).abs() <= area * tolerance.chord_height);
    assert_eq!(obj.bytes, export(&k, body, "bracket", Format::Obj, &tolerance).unwrap().bytes, "repeatable");
}

#[test]
fn bad_settings_and_released_bodies_are_refused() {
    let k = CatallaxKernel::new();
    let body = bracket(&k);
    let zero = ExportTolerance { chord_height: 0.0, ..ExportTolerance::default() };
    for format in [Format::Stl, Format::ThreeMf, Format::Obj] {
        assert!(matches!(export(&k, body, "b", format, &zero), Err(ExportError::Invalid(_))));
    }
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
    assert_eq!(Format::of_path(Path::new("b.OBJ")), Some(Format::Obj));
    assert_eq!(Format::of_path(Path::new("b.iges")), None);
    for f in Format::ALL {
        assert_eq!(Format::of_path(Path::new(&format!("x.{}", f.extension()))), Some(f));
    }
}
