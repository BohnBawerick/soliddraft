//! Section A-A of the bracket on SMLib: the golden counts, digest and region areas in
//! `recipes/golden/smlib-sections.json`, identical across three runs.
//!
//! Set `SOLIDDRAFT_BLESS=1` to rewrite the golden file after a deliberate change.

use std::path::Path;

use serde_json::json;
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_api::contract;
use soliddraft_kernel_smlib::SmlibKernel;

#[test]
fn bracket_section_matches_golden() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/golden/smlib-sections.json");
    let k = SmlibKernel::new();
    let body = contract::bracket(&k);
    let spec = contract::bracket_section_spec();
    let frame = spec.view.frame().unwrap();
    let mut runs = Vec::new();
    for _ in 0..3 {
        let section = k.section_view(body, &spec).expect("section A-A");
        let (visible, hidden) = section.curves.counts();
        let areas: Vec<f64> = section.region_areas(&frame).iter().map(|a| (a * 1e3).round() / 1e3).collect();
        runs.push(json!({
            "visible": visible,
            "hidden": hidden,
            "regions": section.regions.len(),
            "region_areas": areas,
            "digest": section.curves.digest(),
        }));
    }
    assert!(runs.windows(2).all(|w| w[0] == w[1]), "section A-A differs between runs: {runs:#?}");
    k.release(body).unwrap();
    assert_eq!(k.live_bodies().unwrap(), 0, "the section left bodies behind");
    let got = json!({ "backend": k.backend().name, "sections": { "bracket_a_a": runs[0] } });
    if std::env::var_os("SOLIDDRAFT_BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
        return;
    }
    let text = std::fs::read_to_string(&path).expect("recipes/golden/smlib-sections.json; run with SOLIDDRAFT_BLESS=1");
    let want: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(want, got, "section A-A no longer matches the golden file");
}
