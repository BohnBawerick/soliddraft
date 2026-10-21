//! The bracket's four views on SMLib: the golden counts and digests in
//! `recipes/golden/smlib-views.json`, and under 500 ms each.
//!
//! This is its own test binary so no other test shares the kernel thread while views are
//! timed. The golden file is written by the `soliddraft view` exit test in soliddraft-cli
//! (`SOLIDDRAFT_BLESS=1`), from `recipes/bracket.json`; the bracket built here by the
//! contract module must give the same views.

use std::path::Path;
use std::time::Instant;

use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_api::contract;
use soliddraft_kernel_api::drawing::ViewSpec;
use soliddraft_kernel_smlib::SmlibKernel;

/// The budget per view on the development machine.
const MAX_MILLIS: f64 = 500.0;

#[test]
fn bracket_views_match_golden_and_are_fast() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recipes/golden/smlib-views.json");
    let text = std::fs::read_to_string(&path).expect("recipes/golden/smlib-views.json");
    let golden: serde_json::Value = serde_json::from_str(&text).unwrap();
    let k = SmlibKernel::new();
    let body = contract::bracket(&k);
    for view in contract::BRACKET_VIEWS {
        let spec = ViewSpec::standard(view);
        let mut best = f64::INFINITY;
        let mut digests = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            let curves = k.view_curves(body, &spec).unwrap();
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
            let (visible, hidden) = curves.counts();
            digests.push((visible, hidden, curves.digest()));
        }
        assert!(digests.windows(2).all(|w| w[0] == w[1]), "{} view differs between runs", view.name());
        let want = &golden["views"]["bracket"][view.name()];
        let (visible, hidden, digest) = &digests[0];
        assert_eq!(want["visible"].as_u64(), Some(*visible as u64), "{} visible count", view.name());
        assert_eq!(want["hidden"].as_u64(), Some(*hidden as u64), "{} hidden count", view.name());
        assert_eq!(want["digest"].as_str(), Some(digest.as_str()), "{} digest", view.name());
        assert!(best < MAX_MILLIS, "{} view took {best:.0} ms", view.name());
    }
    k.release(body).unwrap();
}
