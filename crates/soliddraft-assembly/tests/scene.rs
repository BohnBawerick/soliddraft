//! The scene, colours, picking and the hierarchy, on an assembly made of display copies
//! alone (no kernel).

use glam::DVec3;
use soliddraft_assembly::bvh::Ray;
use soliddraft_assembly::{
    Aabb, Assembly, AssemblyDocument, AssemblyPackage, DisplayMesh, Instance, OccurrencePath, OpenOptions, Placement,
    RefCopy, Scene,
};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId};
use soliddraft_materials::{Appearance, Srgb};

/// A root with a grid of 4 x 5 bricks (components 3 to 22, sources 1 and 2) and two
/// copies of a sub-assembly of two plates (components 23 and 24; inside, 2 and 3).
fn sample() -> AssemblyDocument {
    let brick = (DocumentId::new(), ContentHash::of_bytes(b"brick"));
    let plate = (DocumentId::new(), ContentHash::of_bytes(b"plate"));
    let mut sub = Assembly::new("Sub");
    let s = sub.add_source(DocRef::new(plate.0, DocKind::Part, "plate.dwprt", plate.1.clone()));
    sub.add_component(s, Placement::IDENTITY);
    sub.add_component(s, Placement::translation(DVec3::new(0.0, 0.0, 5.0)));
    let sub_id = DocumentId::new();
    let sub_rev = sub.semantic_hash();

    let mut root = Assembly::new("Root");
    let b = root.add_source(DocRef::new(brick.0, DocKind::Part, "brick.dwprt", brick.1.clone()));
    let a = root.add_source(DocRef::new(sub_id, DocKind::Assembly, "sub.dwasm", sub_rev.clone()));
    for i in 0..4 {
        for j in 0..5 {
            root.add_component(b, Placement::translation(DVec3::new(i as f64 * 20.0, j as f64 * 20.0, 0.0)));
        }
    }
    root.add_component(a, Placement::translation(DVec3::new(200.0, 0.0, 0.0)));
    root.add_component(a, Placement::translation(DVec3::new(300.0, 0.0, 0.0)));
    let copies = [
        RefCopy::part(brick.0, brick.1, &DisplayMesh::cuboid(DVec3::ZERO, DVec3::new(10.0, 10.0, 10.0)), "test"),
        RefCopy::part(plate.0, plate.1, &DisplayMesh::cuboid(DVec3::ZERO, DVec3::new(50.0, 50.0, 2.0)), "test"),
        RefCopy::assembly(sub_id, sub_rev, &sub.truth_entries()),
    ];
    let bytes = root.to_bytes(DocumentId::new(), None, &copies).unwrap();
    let package = AssemblyPackage::from_bytes(bytes).unwrap();
    let mut doc = AssemblyDocument::open_package(package, None, OpenOptions::default()).unwrap();
    doc.finish_loading();
    doc
}

#[test]
fn the_scene_groups_instances_by_definition_and_follows_edits() {
    let mut doc = sample();
    assert_eq!(doc.store().counts()[5], 0, "nothing failed");
    let mut scene = Scene::new(&mut doc);
    assert_eq!(scene.instances().len(), 24, "20 bricks and 4 plates");
    let draws = scene.draws(|_| true);
    assert_eq!(draws.iter().map(|d| d.count).collect::<Vec<_>>(), vec![20, 4]);
    assert_eq!(scene.take_dirty(), std::iter::once(0..24).collect::<Vec<_>>(), "everything after a rebuild");
    assert_eq!(scene.take_new_meshes(usize::MAX).len(), 2);

    // Hide one brick: only its instance changes.
    let first: OccurrencePath = "3".parse().unwrap();
    doc.set_hidden(std::slice::from_ref(&first), true).unwrap();
    scene.update(&mut doc);
    let slot = scene.slot(doc.table().find(&first).unwrap()).unwrap();
    assert_ne!(scene.instances()[slot as usize].flags & Instance::HIDDEN, 0);
    assert_eq!(scene.take_dirty(), vec![slot..slot + 1]);

    // Colour one copy of the sub-assembly: both its plates, and only its plates.
    let red = Appearance::new("red", "Red", Srgb::new(200, 20, 20));
    let right: OccurrencePath = "24".parse().unwrap();
    doc.set_colour(std::slice::from_ref(&right), Some(red.clone())).unwrap();
    scene.update(&mut doc);
    let coloured: Vec<&Instance> = scene.instances().iter().filter(|i| i.flags & Instance::COLOURED != 0).collect();
    assert_eq!(coloured.len(), 2);
    assert_eq!(coloured[0].colour.to_le_bytes()[..3], [200, 20, 20]);
    // A part below with a colour of its own keeps it when the outer one changes.
    let inner: OccurrencePath = "24/3".parse().unwrap();
    let blue = Appearance::new("blue", "Blue", Srgb::new(20, 20, 200));
    doc.set_colour(std::slice::from_ref(&inner), Some(blue)).unwrap();
    doc.set_colour(std::slice::from_ref(&right), Some(Appearance::new("green", "Green", Srgb::new(20, 200, 20))))
        .unwrap();
    let t = doc.table();
    assert_eq!(t.colour(t.find(&inner).unwrap()).unwrap().id.as_str(), "blue");
    assert_eq!(t.colour(t.find(&"24/2".parse().unwrap()).unwrap()).unwrap().id.as_str(), "green");
    assert!(doc.undo() && doc.undo());
    let t = doc.table();
    assert_eq!(t.colour(t.find(&inner).unwrap()).unwrap().id.as_str(), "red");
    assert!(doc.undo());
    assert!(doc.table().colour(doc.table().find(&inner).unwrap()).is_none());
    assert!(doc.assembly().appearances.is_empty() && doc.assembly().snapshots.is_empty());

    // Selecting the sub-assembly marks its parts.
    let row = doc.table().find(&right).unwrap();
    scene.set_selection(doc.table(), &[row]);
    let selected = scene.instances().iter().filter(|i| i.flags & Instance::SELECTED != 0).count();
    assert_eq!(selected, 2);
}

#[test]
fn colours_survive_saving_and_do_not_change_the_geometry() {
    let mut doc = sample();
    let before = doc.assembly().geometry_hash();
    let p: OccurrencePath = "23".parse().unwrap();
    doc.set_colour(std::slice::from_ref(&p), Some(Appearance::new("red", "Red", Srgb::new(200, 20, 20)))).unwrap();
    assert_eq!(doc.assembly().geometry_hash(), before);
    let dir = std::env::temp_dir().join(format!("soliddraft-scene-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = doc.save(Some(&dir.join("coloured.dwasm"))).unwrap();
    let mut back = AssemblyDocument::open(&path, OpenOptions::default()).unwrap();
    back.finish_loading();
    let t = back.table();
    assert_eq!(t.colour(t.find(&"23/2".parse().unwrap()).unwrap()).unwrap().id.as_str(), "red");
    assert!(t.colour(t.find(&"24/2".parse().unwrap()).unwrap()).is_none());
}

#[test]
fn picks_name_occurrences_and_faces() {
    let mut doc = sample();
    let scene = Scene::new(&mut doc);
    let row = doc.table().find(&"7".parse().unwrap()).unwrap();
    assert!(scene.slot(row).is_some());
    let picked = scene.decode([row.0 + 1, (1 << 30) | 6]).unwrap();
    assert_eq!(picked.occurrence, row);
    let sel = doc.selection(&picked);
    assert_eq!(sel.path.to_string(), "7");
    assert_eq!(sel.face.as_deref(), Some("+z"), "face 5 of the cuboid is its top");
    assert!(scene.decode([0, 0]).is_none(), "nothing there");
}

#[test]
fn the_hierarchy_answers_rays_and_regions_and_follows_moves() {
    let mut doc = sample();
    let target: OccurrencePath = "6".parse().unwrap(); // the fourth brick: (0, 60)
    let ray = Ray { origin: DVec3::new(5.0, 65.0, 100.0), direction: DVec3::NEG_Z };
    let table = doc.table().clone();
    let hits = doc.bvh().along_ray(&table, &ray);
    let row = doc.table().find(&target).unwrap();
    assert_eq!(hits.first().map(|h| h.0), Some(row));
    assert!((hits[0].1 - 90.0).abs() < 1e-3);
    let region = Aabb { min: DVec3::new(-1.0, -1.0, -1.0), max: DVec3::new(31.0, 31.0, 11.0) };
    assert_eq!(doc.bvh().in_box(&table, &region).len(), 4);

    doc.move_to(&[(target.clone(), Placement::translation(DVec3::new(500.0, 500.0, 0.0)))]).unwrap();
    let table = doc.table().clone();
    assert!(doc.bvh().along_ray(&table, &ray).iter().all(|h| h.0 != row), "moved away");
    let there = Ray { origin: DVec3::new(505.0, 505.0, 100.0), direction: DVec3::NEG_Z };
    assert_eq!(doc.bvh().along_ray(&table, &there).first().map(|h| h.0), Some(row));
}
