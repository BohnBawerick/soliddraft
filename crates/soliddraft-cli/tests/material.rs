//! `soliddraft material` in a fresh process: the libraries, a SolidWorks import into My
//! library, and a saved part's material, appearances and mass.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;

use serde_json::Value;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::sketch::{P2, Sketch, SolveStatus, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

/// A folder of its own for each test: the config folder and the part.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soliddraft-cli-material-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    dir
}

/// A 40 x 30 x 10 block, 12 000 mm³, saved in `dir`.
fn save_block(dir: &Path) -> PathBuf {
    let kernel: Arc<dyn Kernel> = Arc::new(CatallaxKernel::new());
    let mut doc = Document::new("Block", kernel);
    let mut sketch = Sketch::new();
    sketch.rectangle(P2::new(0.0, 0.0), 40.0, 30.0).unwrap();
    solve(&mut sketch, None, true);
    assert_eq!(sketch.status, SolveStatus::FullyDefined);
    let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
    let add = |doc: &mut Document, kind| {
        doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
            .unwrap()
            .created
            .unwrap()
    };
    let s = add(&mut doc, FeatureKind::Sketch(SketchFeature { plane, sketch }));
    add(
        &mut doc,
        FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: 10.0 },
            reverse: false,
        }),
    );
    let path = dir.join("block.dwprt");
    doc.save(&path).unwrap();
    path
}

fn run(dir: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_soliddraft"))
        .args(args)
        .env("SOLIDDRAFT_CONFIG_DIR", dir.join("config"))
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

fn json(dir: &Path, args: &[&str]) -> Value {
    let (ok, stdout, stderr) = run(dir, args);
    assert!(ok, "{args:?}: {stderr}");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

#[test]
fn a_part_gets_a_material_a_colour_and_a_mass() {
    let dir = scratch("part");
    let part = save_block(&dir);
    let p = part.to_str().unwrap();

    let info = json(&dir, &["material", "info", p, "--json"]);
    assert!(info["material"].is_null());
    assert_eq!(info["mass_unknown"], "assign a material");

    let (ok, stdout, stderr) = run(&dir, &["material", "set", p, "steel-aisi-4130"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("set material Steel AISI 4130"), "{stdout}");
    let info = json(&dir, &["material", "info", p, "--json"]);
    assert_eq!(info["material"]["id"], "steel-aisi-4130");
    let kg = info["mass"]["mass_kg"].as_f64().unwrap();
    assert!((kg - 12000.0 * 7833.0 * 1e-9).abs() < 1e-9, "{kg}");
    assert_eq!(info["appearance"]["id"], "steel-machined");

    // Paint the top face: `faces` numbers the faces, `paint --face` takes the number.
    let faces = json(&dir, &["material", "faces", p, "--json"]);
    let top = faces.as_array().unwrap().iter().find(|f| f["label"] == "Boss-Extrude1 · End cap").unwrap();
    let n = top["face"].to_string();
    let (ok, _, stderr) = run(&dir, &["material", "paint", p, "paint-red", "--face", &n]);
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = run(&dir, &["material", "paint", p, "anodised-blue", "--feature", "Boss-Extrude1"]);
    assert!(ok, "{stderr}");
    let faces = json(&dir, &["material", "faces", p, "--json"]);
    let ids: Vec<&str> = faces.as_array().unwrap().iter().map(|f| f["appearance"].as_str().unwrap()).collect();
    assert_eq!(ids.iter().filter(|a| **a == "paint-red").count(), 1, "{ids:?}");
    assert_eq!(ids.iter().filter(|a| **a == "anodised-blue").count(), 5, "{ids:?}");

    let report = json(&dir, &["part", "--json", p]);
    assert_eq!(report["material"], "steel-aisi-4130");
    assert!((report["mass_kg"].as_f64().unwrap() - kg).abs() < 1e-12);

    // Removing the material makes the mass unknown again; unknown ids are refused.
    assert!(run(&dir, &["material", "set", p, "none"]).0);
    assert!(json(&dir, &["material", "info", p, "--json"])["mass"].is_null());
    let (ok, _, stderr) = run(&dir, &["material", "set", p, "unobtainium"]);
    assert!(!ok && stderr.contains("no library has the material unobtainium"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_solidworks_library_imports_into_my_library() {
    let dir = scratch("import");
    let sldmat = dir.join("Shop.sldmat");
    std::fs::write(
        &sldmat,
        r#"<?xml version="1.0"?>
<mstns:materials xmlns:mstns="http://www.solidworks.com/sldmaterials" version="2008.03">
  <classification name="Plastics">
    <material name="ABS">
      <swatchcolor RGB="f0f0e6"/>
      <physicalproperties><DENS displayname="Mass density" value="1050"/></physicalproperties>
    </material>
    <material name="Shop steel">
      <xhatch name="ANSI32 (Steel)" angle="0" scale="1"/>
      <physicalproperties><DENS value="7850"/><EX value="2.1e11"/></physicalproperties>
    </material>
  </classification>
</mstns:materials>"#,
    )
    .unwrap();
    let out = json(&dir, &["material", "import", sldmat.to_str().unwrap(), "--json"]);
    assert_eq!(out["materials"], 2);
    assert_eq!(out["renamed"][0], serde_json::json!(["abs", "abs-2"]), "the built-in ABS keeps its id");
    let library = PathBuf::from(out["library"].as_str().unwrap());
    assert_eq!(library, dir.join("config/libraries/my-library.toml"));

    let libraries = json(&dir, &["material", "libraries", "--json"]);
    assert!(libraries["issues"].as_array().unwrap().is_empty(), "{libraries}");
    let mine = libraries["libraries"].as_array().unwrap().iter().find(|l| l["origin"] == "user").unwrap();
    assert_eq!(mine["materials"], 2);
    let shown = json(&dir, &["material", "show", "shop-steel", "--json"]);
    assert_eq!(shown["library"], "My library");
    assert_eq!(shown["material"]["hatch"], "ANSI32");
    assert_eq!(shown["material"]["category"], "Plastics");
    assert_eq!(shown["material"]["provenance"]["density_kg_m3"]["source"]["kind"], "sldmat");
    assert_eq!(shown["material"]["provenance"]["density_kg_m3"]["source"]["file"], "Shop.sldmat");
    let (ok, stdout, stderr) = run(&dir, &["material", "show", "shop-steel"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("density_kg_m3 = 7850 kg/m³ (imported from Shop.sldmat)"), "{stdout}");
    let (_, stdout, _) = run(&dir, &["material", "show", "steel-aisi-4130"]);
    assert!(stdout.contains("density_kg_m3 = 7833 kg/m³ (source unknown)"), "{stdout}");

    let (ok, stdout, _) = run(&dir, &["material", "check", library.to_str().unwrap()]);
    assert!(ok, "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}
