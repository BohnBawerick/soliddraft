//! The policy, the worker and the cache on the SMLib kernel.

use std::sync::Arc;
use std::time::Duration;

use soliddraft_kernel_api::contract;
use soliddraft_kernel_api::{Kernel, KernelError, Mesh, OpTag, Vec3};
use soliddraft_kernel_smlib::SmlibKernel;
use soliddraft_tessellation::{DeviceBudget, DisplayCache, Level, Levels, Quality, Tessellator, report};

const WAIT: Duration = Duration::from_secs(60);

/// The contract block with its top edges rounded: planes, cylinders and a 50 mm
/// diagonal.
fn rounded_block(k: &dyn Kernel) -> soliddraft_kernel_api::BodyId {
    let b = contract::block(k);
    let t = k.topology(b).unwrap();
    let top: Vec<_> = t.edges.iter().filter(|e| e.bbox.min.z > 10.0 - 1e-6).map(|e| e.id).collect();
    k.fillet(&OpTag::new("round"), b, &top, 3.0).unwrap().body
}

#[test]
fn the_report_gives_each_levels_deviation_and_triangles() {
    let k = SmlibKernel::new();
    let body = rounded_block(&k);
    let r = report(&k, body, &Quality::Auto).unwrap();
    assert!((r.diagonal_mm - 2600f64.sqrt()).abs() < 1e-6, "{}", r.diagonal_mm);
    let deviations: Vec<f64> = r.levels.iter().map(|l| l.deviation_mm.unwrap()).collect();
    assert!((deviations[1] - 0.002 * r.diagonal_mm).abs() < 1e-12);
    assert_eq!(deviations[0], deviations[1] * 4.0);
    assert_eq!(deviations[2], deviations[1] / 4.0);
    // The 3 mm fillets' 15 degree angle bounds the mesh before the coarse and standard
    // chords do (its sagitta is 0.026 mm), so those two levels can come out equal.
    let triangles: Vec<usize> = r.levels.iter().map(|l| l.triangles).collect();
    assert!(triangles.windows(2).all(|w| w[0] <= w[1]), "finer levels never have fewer triangles: {triangles:?}");
    assert!(triangles[2] > triangles[0], "{triangles:?}");
    for l in &r.levels {
        assert_eq!(l.triangles, k.tessellate(body, &l.params).unwrap().triangle_count());
    }

    // The slider moves the levels; a finer position has more triangles.
    let fine = report(&k, body, &Quality::Slider { value: 9 }).unwrap();
    assert!(fine.level(Level::Standard).triangles > r.level(Level::Standard).triangles);
    assert!(fine.level(Level::Standard).deviation_mm < r.level(Level::Standard).deviation_mm);

    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["levels"][1]["level"], "standard");
    assert_eq!(json["quality"]["mode"], "auto");
}

#[test]
fn the_worker_answers_off_the_interface_thread() {
    let k: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let body = rounded_block(k.as_ref());
    let levels = Levels::new(&Quality::Auto, k.topology(body).unwrap().bbox.diagonal());
    let mut t: Tessellator<&str> = Tessellator::new(k.clone());
    let generation = t.request("block", body, levels.all());
    assert_eq!(t.pending(), 1);
    let done = t.wait(WAIT).expect("an answer");
    assert_eq!((done.key, done.generation), ("block", generation));
    let meshes = done.result.unwrap();
    assert_eq!(meshes, k.tessellate_levels(body, levels.all()).unwrap());
    assert_eq!(t.pending(), 0);
    assert!(t.poll().is_empty());
}

#[test]
fn a_new_request_supersedes_the_old_one() {
    let k: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let body = rounded_block(k.as_ref());
    let coarse = Levels::new(&Quality::Slider { value: 1 }, 50.0);
    let fine = Levels::new(&Quality::Slider { value: 10 }, 50.0);
    let mut t: Tessellator<u32> = Tessellator::new(k.clone());
    for _ in 0..5 {
        t.request(7, body, coarse.all());
    }
    let last = t.request(7, body, fine.all());
    let done = t.wait(WAIT).expect("an answer");
    assert_eq!(done.generation, last, "only the latest request is answered");
    assert_eq!(done.result.unwrap()[0], k.tessellate(body, fine.params(Level::Coarse)).unwrap());
    std::thread::sleep(Duration::from_millis(50));
    assert!(t.poll().is_empty());

    // A cancelled request is never answered.
    t.request(8, body, fine.all());
    t.cancel(&8);
    assert_eq!(t.pending(), 0);
    assert!(t.wait(Duration::from_millis(500)).is_none());

    // A body released before its turn answers with an error, not a panic.
    let gone = contract::cylinder(k.as_ref(), Vec3::new(0.0, 0.0, 0.0), 4.0, 4.0);
    k.release(gone).unwrap();
    t.request(9, gone, fine.all());
    let done = t.wait(WAIT).expect("an answer");
    assert!(matches!(done.result, Err(KernelError::UnknownBody(_))));
}

#[test]
fn many_placements_share_one_definitions_meshes() {
    let k: Arc<dyn Kernel> = Arc::new(SmlibKernel::new());
    let bolt = contract::cylinder(k.as_ref(), Vec3::new(0.0, 0.0, 0.0), 3.0, 20.0);
    let levels = Levels::new(&Quality::Auto, k.topology(bolt).unwrap().bbox.diagonal());
    let mut t: Tessellator<&str> = Tessellator::new(k.clone());
    let mut cache: DisplayCache<&str, Mesh> = DisplayCache::new(DeviceBudget::DESKTOP);
    // Ten thousand placements ask for the definition; one request goes out.
    for _ in 0..10_000 {
        if cache.missing(&"bolt", &levels).len() == 3 && t.pending() == 0 {
            t.request("bolt", bolt, levels.all());
        }
    }
    let done = t.wait(WAIT).expect("an answer");
    cache.insert_levels(done.key, &levels, done.result.unwrap());
    assert!(cache.missing(&"bolt", &levels).is_empty());
    let per_placement: Vec<_> = (0..10_000).map(|_| cache.best(&"bolt", &levels, Level::Standard).unwrap().1).collect();
    assert!(per_placement.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])), "every placement draws the same mesh");
    // A small cylinder's circles are bounded by the 15 degree angle, not the chord, at
    // the coarse and standard levels, so those two share one mesh.
    let standard = cache.get(&"bolt", levels.key(Level::Standard)).unwrap();
    let coarse = cache.get(&"bolt", levels.key(Level::Coarse)).unwrap();
    assert!(Arc::ptr_eq(&standard, &coarse) || standard.triangle_count() > coarse.triangle_count());
    assert!(cache.mesh_count() <= 3);
    assert!(
        cache.usage().triangles <= 3 * cache.get(&"bolt", levels.key(Level::Fine)).unwrap().triangle_count() as u64
    );
}
