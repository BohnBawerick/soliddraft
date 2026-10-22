//! The phase D3 exit test, through the `soliddraft draw` binary as separate processes:
//!
//! - Section A-A of the bracket, cut on the top view through the boss centre, and Detail
//!   B of the boss on the front view at twice its parent's scale, as in frame 6 of the
//!   drawing examples, with ordinate dimensions, a geometric tolerance, a datum, a surface
//!   finish, a revision table and a revision symbol; the SVG matches
//!   `recipes/golden/catallax-bracket-d3.svg`;
//! - the section's three cut regions are hatched, in the SVG and as HATCH entities in
//!   the DXF;
//! - the DXF re-reads with the `dxf` crate with every dimension a DIMENSION entity, and
//!   passes ezdxf's audit when ezdxf is installed;
//! - a two-sheet drawing with the detail on sheet 2: the label at the detail circle on
//!   sheet 1 names sheet 2 and its zone, the label under the detail names sheet 1 and
//!   its zone, and the PDF has two pages;
//! - `soliddraft draw --update` after the plate gets thicker remakes every view, the
//!   section's regions included, and leaves nothing dangling.
//!
//! Set `SOLIDDRAFT_BLESS=1` to rewrite the golden files after a deliberate change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// 2026-10-14, so the title block's date is fixed.
const DATE_EPOCH: &str = "1791936000";

fn recipes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes")
}

fn soliddraft(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(args)
        .current_dir(dir)
        .env("SOURCE_DATE_EPOCH", DATE_EPOCH)
        .output()
        .expect("running soliddraft");
    assert!(out.status.success(), "soliddraft {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn golden(name: &str, got: &str) {
    let path = recipes_dir().join("golden").join(name);
    if std::env::var_os("SOLIDDRAFT_BLESS").is_some() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{}; run with SOLIDDRAFT_BLESS=1", path.display()));
    if want != got {
        let line = want.lines().zip(got.lines()).position(|(a, b)| a != b).unwrap_or(0);
        panic!(
            "{} differs at line {}:\n  want {}\n  got  {}",
            path.display(),
            line + 1,
            want.lines().nth(line).unwrap_or(""),
            got.lines().nth(line).unwrap_or("")
        );
    }
}

/// Entities per layer and kind, from the dxf crate, plus the HATCH count from the file's
/// text (the dxf crate has no HATCH type and skips them).
fn dxf_counts(path: &Path) -> BTreeMap<String, usize> {
    let bytes = std::fs::read(path).unwrap();
    let counts = soliddraft_drawing::export::dxf::count_entities(&bytes).expect("the DXF re-reads");
    assert!(!counts.keys().any(|k| k.ends_with("/SPLINE")), "a spline in the DXF: {counts:?}");
    counts
}

/// Runs ezdxf's audit on a DXF when python3 has ezdxf, which must find nothing to fix;
/// returns false when python3 has no ezdxf.
fn ezdxf_audit(path: &Path) -> bool {
    let script = "import sys, ezdxf\n\
                  doc = ezdxf.readfile(sys.argv[1])\n\
                  auditor = doc.audit()\n\
                  msp = doc.modelspace()\n\
                  print(len(auditor.errors), len(auditor.fixes), len(msp.query('DIMENSION')), len(msp.query('HATCH')))\n";
    let Ok(out) = Command::new("python3").args(["-I", "-c", script]).arg(path).output() else { return false };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("No module named 'ezdxf'") {
            return false;
        }
        panic!("ezdxf could not read {}: {err}", path.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let numbers: Vec<usize> = text.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    assert_eq!(numbers.len(), 4, "{text}");
    assert_eq!(numbers[..2], [0, 0], "ezdxf's audit found errors or fixed something in {}", path.display());
    assert!(numbers[2] > 0 && numbers[3] > 0, "ezdxf sees no dimensions or hatches: {text}");
    true
}

/// The bracket's views, annotations and revisions, as in frame 6. The revisions come
/// first so the views are placed clear of their table.
const FRAME_6: [&str; 23] = [
    "--revision",
    "A:First issue:2026-10-14:PP",
    "--revision",
    "B:Boss bore toleranced:2026-10-14:PP",
    "--section",
    "top:70,45,0:h",
    "--detail",
    "front:70,45,20:16:2",
    "--ordinate",
    "top:h-15:0,0,10;25,23,10;70,45,10;115,23,10;120,90,10",
    "--gdt",
    "front:70,0,30:position:dia0.1(M):A,B@-20,16",
    "--datum",
    "front:60,0,0:A",
    "--datum",
    "top:0,45,60:B",
    "--finish",
    "front:100,0,10:Ra 1.6:removal@0,6",
    "--revision-symbol",
    "top:25,67,10:B@12,-10",
    "--json",
    "--hidden",
    "visible",
];

#[test]
fn bracket_sections_details_and_sheets() {
    let dir = std::env::temp_dir().join(format!("soliddraft-d3-exit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let recipe = dir.join("bracket.json");
    std::fs::copy(recipes_dir().join("bracket.json"), &recipe).unwrap();

    // Frame 6: Section A-A and Detail B on one sheet.
    let mut args = vec!["draw", "bracket.json", "--template", "iso-a3"];
    args.extend(FRAME_6);
    let report: Value = serde_json::from_str(&soliddraft(&dir, &args)).unwrap();
    assert_eq!(report["sheets"], 1, "{report:#}");
    assert_eq!(report["views"], 6, "four standard views, the section and the detail: {report:#}");
    let (drawing, _) = soliddraft_drawing::file::read(&dir.join("bracket.dwdrw")).expect("the drawing opens");
    let section = drawing.sheets[0]
        .views
        .iter()
        .find(|v| matches!(v.kind, soliddraft_drawing::ViewKind::Section { .. }))
        .expect("a section view");
    assert_eq!(section.geometry.hatch.len(), 3, "the plate and boss halves and the flange above the slot");
    let svg = std::fs::read_to_string(dir.join("bracket.svg")).unwrap();
    for text in ["SECTION A-A", "DETAIL B", "SCALE 2:1", "REVISIONS", "Ra 1.6", "Boss bore toleranced"] {
        assert!(svg.contains(&format!(">{text}</text>")), "{text} is not on the sheet");
    }
    assert!(svg.contains(r#"<g id="HATCH""#) && svg.contains(r#"<g id="CUTTING""#), "no hatch or cutting line");
    golden("catallax-bracket-d3.svg", &svg);

    let counts = dxf_counts(&dir.join("bracket.dxf"));
    assert_eq!(counts.get("HATCH/HATCH"), Some(&3), "{counts:?}");
    assert!(counts.get("DIMENSION/DIMENSION").is_some_and(|n| *n >= 8), "{counts:?}");
    assert!(!counts.contains_key("HATCH/LINE"), "hatching goes out as HATCH entities: {counts:?}");
    let dxf = dxf::Drawing::load_file(dir.join("bracket.dxf")).unwrap();
    // Every dimension's lines and text are in its block, none loose in model space.
    let blocks: Vec<&str> = dxf.blocks().map(|b| b.name.as_str()).collect();
    for e in dxf.entities() {
        let base = match &e.specific {
            dxf::entities::EntityType::RotatedDimension(d) => &d.dimension_base,
            dxf::entities::EntityType::DiameterDimension(d) => &d.dimension_base,
            dxf::entities::EntityType::RadialDimension(d) => &d.dimension_base,
            dxf::entities::EntityType::OrdinateDimension(d) => &d.dimension_base,
            _ => continue,
        };
        assert!(blocks.contains(&base.block_name.as_str()), "{} has no block", base.block_name);
        assert!(base.actual_measurement > 0.0 && !base.text.is_empty());
    }
    golden("catallax-bracket-d3-dxf.json", &(serde_json::to_string_pretty(&counts).unwrap() + "\n"));
    if !ezdxf_audit(&dir.join("bracket.dxf")) {
        eprintln!("ezdxf is not installed: skipped its audit");
    }

    // Two sheets: the detail on sheet 2, with a broken-out section, a crop and an
    // auxiliary view on sheet 1.
    let two = [
        "draw",
        "bracket.json",
        "--template",
        "iso-a3",
        "--out",
        "two.dwdrw",
        "--out",
        "two.svg",
        "--out",
        "two.pdf",
        "--out",
        "two.dxf",
        "--detail",
        "front:70,45,20:16:2@2",
        "--section",
        "top:70,45,0:h@2",
        "--broken-out",
        "front:70,0,20:14:45",
        "--aux",
        "front:120,0,5",
        "--display",
        "iso:tangent=phantom",
        "--json",
    ];
    let report: Value = serde_json::from_str(&soliddraft(&dir, &two)).unwrap();
    assert_eq!(report["sheets"], 2, "{report:#}");
    let labels = report["location_labels"].as_array().unwrap();
    let detail = labels
        .iter()
        .find(|l| l["on_view"].as_str().is_some_and(|t| t.starts_with("FROM SHEET 1, ZONE ")))
        .unwrap_or_else(|| panic!("no label points from the detail to sheet 1: {report:#}"));
    let to_detail = detail["on_parent"].as_str().unwrap();
    assert!(to_detail.starts_with("SEE SHEET 2, ZONE "), "{to_detail}");
    let from_detail = detail["on_view"].as_str().unwrap();
    let sheet1 = std::fs::read_to_string(dir.join("two.svg")).unwrap();
    let sheet2 = std::fs::read_to_string(dir.join("two-2.svg")).unwrap();
    assert!(sheet1.contains(&format!(">{to_detail}</text>")), "sheet 1 does not say where the detail is");
    assert!(sheet2.contains(&format!(">{from_detail}</text>")), "sheet 2 does not say where the detail comes from");
    assert!(sheet2.contains(">DETAIL A</text>") && sheet2.contains(">SECTION B-B</text>"));
    assert!(sheet1.contains(">VIEW C</text>"), "the auxiliary view's caption");
    assert!(sheet1.contains(r#"<g id="HATCH""#) && sheet1.contains(r#"<g id="THIN""#), "the broken-out section");
    assert!(sheet2.contains(">2 / 2</text>"));
    let pdf = lopdf::Document::load(dir.join("two.pdf")).expect("the PDF loads");
    assert_eq!(pdf.get_pages().len(), 2);
    assert!(dir.join("two-2.dxf").is_file(), "one DXF per sheet");
    let counts = dxf_counts(&dir.join("two-2.dxf"));
    assert!(counts.get("HATCH/HATCH").is_some_and(|n| *n == 3), "{counts:?}");

    // The plate gets thicker: every view is remade, the cut faces with it.
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&recipe).unwrap()).unwrap();
    let plate = v["steps"].as_array_mut().unwrap().iter_mut().find(|s| s["id"] == "plate").unwrap();
    plate["distance"] = 12.into();
    std::fs::write(&recipe, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let report: Value =
        serde_json::from_str(&soliddraft(&dir, &["draw", "--update", "two.dwdrw", "--out", "two.svg", "--json"]))
            .unwrap();
    assert_eq!(report["views_updated"], 7, "{report:#}");
    assert!(report.get("dangling_views").is_none(), "{report:#}");
    assert_eq!(report["dangling"].as_array().unwrap().len(), 0, "{report:#}");
    let (after, _) = soliddraft_drawing::file::read(&dir.join("two.dwdrw")).unwrap();
    let section =
        after.sheets[1].views.iter().find(|v| matches!(v.kind, soliddraft_drawing::ViewKind::Section { .. })).unwrap();
    let before = section.geometry.hatch.iter().map(|h| h.outer.len()).sum::<usize>();
    assert!(before > 0);
    let labels = soliddraft_drawing::display::location_labels(&after);
    assert!(labels.iter().any(|l| l.on_view().as_deref() == Some(from_detail)), "the labels still point both ways");
    std::fs::remove_dir_all(&dir).unwrap();
}
