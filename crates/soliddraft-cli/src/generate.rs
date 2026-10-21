//! `soliddraft asm generate`: a large test assembly for the scale test.
//!
//! The assembly is a root that places `modules` sub-assemblies on a grid. Each module
//! places its share of the unique parts once each, individually, then repeats every one
//! of them with one linear pattern, so the placements come from both mechanisms:
//! individual records (`placements.bin` once a module has more than 1,024 components) and
//! a generator. With the defaults that is 100,000 unique parts, 50 modules of 2,000
//! parts, and 1,000,000 part occurrences.
//!
//! The parts are real part files: blocks (a rectangle extruded) and pins (a circle
//! extruded) with seeded random sizes, so each one is unique. Their recipes are written
//! without building them on a kernel (100,000 kernel rebuilds would take most of an
//! hour); opening one rebuilds it on the kernel as usual. The display copies in the
//! assemblies are the same shapes made directly: a box of 12 triangles, or a 16-sided
//! prism of 60. Their faces are named "top", "bottom", "side" and so on rather than with
//! the kernel's persistent names, so a Rebuild replaces them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use glam::DVec3;
use soliddraft_assembly::{Assembly, ComponentId, DisplayMesh, MeshFace, PatternRule, Placement, RefCopy, SourceId};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId};
use soliddraft_kernel_api::*;
use soliddraft_model::sketch::{P2, Sketch, solve::solve};
use soliddraft_model::{
    Command, DatumPlane, Document, EndCondition, ExtrudeFeature, ExtrudeOp, FeatureKind, ProfileSource, SketchFeature,
    SketchPlane,
};

/// What to generate.
pub(crate) struct Spec {
    pub unique: usize,
    pub placements: usize,
    pub modules: usize,
    pub part_files: bool,
    pub seed: u64,
}

/// What was generated.
pub(crate) struct Generated {
    pub root: PathBuf,
    pub parts_seconds: f64,
    pub assemblies_seconds: f64,
    pub bytes: u64,
}

/// A kernel that builds nothing: the generator records recipes without geometry.
struct NoKernel;

fn none<T>() -> KernelResult<T> {
    Err(KernelError::Unsupported("the generator writes recipes without building them".into()))
}

impl Kernel for NoKernel {
    fn backend(&self) -> BackendInfo {
        BackendInfo { name: "none".into(), version: "generator".into() }
    }
    fn create_body_from_profile(&self, _: &OpTag, _: &Profile) -> KernelResult<OpResult> {
        none()
    }
    fn extrude(&self, _: &OpTag, _: BodyId, _: &Extrude) -> KernelResult<OpResult> {
        none()
    }
    fn revolve(&self, _: &OpTag, _: BodyId, _: &Revolve) -> KernelResult<OpResult> {
        none()
    }
    fn boolean(&self, _: &OpTag, _: BooleanOp, _: BodyId, _: BodyId) -> KernelResult<OpResult> {
        none()
    }
    fn fillet(&self, _: &OpTag, _: BodyId, _: &[EdgeId], _: f64) -> KernelResult<OpResult> {
        none()
    }
    fn chamfer(&self, _: &OpTag, _: BodyId, _: &[EdgeId], _: f64) -> KernelResult<OpResult> {
        none()
    }
    fn shell(&self, _: &OpTag, _: BodyId, _: &[FaceId], _: f64) -> KernelResult<OpResult> {
        none()
    }
    fn tessellate(&self, _: BodyId, _: &TessellationParams) -> KernelResult<Mesh> {
        none()
    }
    fn tessellate_edges(&self, _: BodyId, _: &TessellationParams) -> KernelResult<Vec<EdgePolyline>> {
        none()
    }
    fn mass_properties(&self, _: BodyId) -> KernelResult<MassProperties> {
        none()
    }
    fn validate(&self, _: BodyId) -> KernelResult<Validation> {
        none()
    }
    fn topology(&self, _: BodyId) -> KernelResult<Topology> {
        none()
    }
    fn history(&self, _: BodyId) -> KernelResult<Vec<HistoryRecord>> {
        none()
    }
    fn release(&self, _: BodyId) -> KernelResult<()> {
        Ok(())
    }
}

/// SplitMix64: a small seeded generator, so the same seed gives the same assembly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number from `lo` to `hi`, in steps of 0.01.
    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        let steps = ((hi - lo) * 100.0) as u64;
        lo + (self.next() % (steps + 1)) as f64 / 100.0
    }
}

/// One unique part: its shape.
#[derive(Clone, Copy)]
enum Shape {
    /// A block `w` x `d` x `h`, centred in plan, from z = 0.
    Block { w: f64, d: f64, h: f64 },
    /// A pin of diameter `dia` and height `h`, centred in plan, from z = 0.
    Pin { dia: f64, h: f64 },
}

impl Shape {
    fn random(rng: &mut Rng) -> Shape {
        if rng.next().is_multiple_of(2) {
            Shape::Block { w: rng.between(5.0, 45.0), d: rng.between(5.0, 45.0), h: rng.between(2.0, 50.0) }
        } else {
            Shape::Pin { dia: rng.between(3.0, 40.0), h: rng.between(5.0, 50.0) }
        }
    }

    /// The part, recorded without building it.
    fn part(&self, name: String) -> Document {
        let kernel: Arc<dyn Kernel> = Arc::new(NoKernel);
        let mut doc = Document::new(name, kernel);
        let mut sketch = Sketch::new();
        let h = match *self {
            Shape::Block { w, d, h } => {
                sketch.rectangle(P2::new(0.0, 0.0), w, d).expect("positive sizes");
                h
            }
            Shape::Pin { dia, h } => {
                sketch.circle_at(P2::new(0.0, 0.0), dia).expect("a positive diameter");
                h
            }
        };
        solve(&mut sketch, None, true);
        let plane = SketchPlane::Datum { datum: DatumPlane::Top, offset: 0.0 };
        let mut add = |kind| {
            doc.execute(Command::AddFeature { kind, name: None, position: None, via_assistant: false })
                .expect("recording a feature")
                .created
                .expect("a new feature")
        };
        let s = add(FeatureKind::Sketch(SketchFeature { plane, sketch }));
        add(FeatureKind::Extrude(ExtrudeFeature {
            profile: ProfileSource::Sketch { feature: s },
            operation: ExtrudeOp::Boss,
            end: EndCondition::Blind { depth: h },
            reverse: false,
        }));
        doc
    }

    /// The display mesh of the same shape.
    fn mesh(&self) -> DisplayMesh {
        match *self {
            Shape::Block { w, d, h } => {
                let mut m = DisplayMesh::cuboid(DVec3::new(-w / 2.0, -d / 2.0, 0.0), DVec3::new(w / 2.0, d / 2.0, h));
                for (f, name) in m.faces.iter_mut().zip(["left", "right", "front", "back", "bottom", "top"]) {
                    f.name = name.into();
                }
                m
            }
            Shape::Pin { dia, h } => prism(dia / 2.0, h, 16),
        }
    }

    fn height(&self) -> f64 {
        match *self {
            Shape::Block { h, .. } | Shape::Pin { h, .. } => h,
        }
    }
}

/// A prism of `n` sides around the z axis: the side (two triangles per facet), then the
/// bottom and top caps (fans).
fn prism(r: f64, h: f64, n: usize) -> DisplayMesh {
    let mut m = DisplayMesh::default();
    let at = |i: usize| {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        (a.cos(), a.sin())
    };
    for i in 0..n {
        let ((c0, s0), (c1, s1)) = (at(i), at(i + 1));
        let base = m.positions.len() as u32;
        for (c, s, z) in [(c0, s0, 0.0), (c1, s1, 0.0), (c1, s1, h), (c0, s0, h)] {
            m.positions.push([(r * c) as f32, (r * s) as f32, z as f32]);
            m.normals.push([c as f32, s as f32, 0.0]);
        }
        m.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    m.faces.push(MeshFace { first: 0, count: 2 * n as u32, name: "side".into() });
    for (z, normal, name) in [(0.0f32, -1.0f32, "bottom"), (h as f32, 1.0, "top")] {
        let base = m.positions.len() as u32;
        let first = m.indices.len() as u32 / 3;
        for i in 0..n {
            let (c, s) = at(i);
            m.positions.push([(r * c) as f32, (r * s) as f32, z]);
            m.normals.push([0.0, 0.0, normal]);
        }
        for i in 1..n as u32 - 1 {
            if normal > 0.0 {
                m.indices.extend([base, base + i, base + i + 1]);
            } else {
                m.indices.extend([base, base + i + 1, base + i]);
            }
        }
        m.faces.push(MeshFace { first, count: n as u32 - 2, name: name.into() });
    }
    m
}

/// Writes the assembly into `dir`.
pub(crate) fn generate(dir: &Path, spec: &Spec) -> Result<Generated, String> {
    if spec.unique == 0 || spec.modules == 0 || spec.placements < spec.unique {
        return Err("the assembly needs unique parts, modules, and at least one placement per part".into());
    }
    let per_module = spec.unique.div_ceil(spec.modules);
    let repeats = (spec.placements / spec.unique).max(1) as u32;
    std::fs::create_dir_all(dir.join("parts")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("modules")).map_err(|e| e.to_string())?;
    let started = Instant::now();

    // The parts, on every core: shape, recipe, file.
    let mut rng = Rng(spec.seed);
    let shapes: Vec<Shape> = (0..spec.unique).map(|_| Shape::random(&mut rng)).collect();
    let refs: Vec<(DocumentId, ContentHash)> = {
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
        let mut out = vec![(DocumentId::new(), ContentHash::of_bytes(b"")); spec.unique];
        let chunks: Vec<Vec<(usize, DocumentId, ContentHash)>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    let (next, shapes) = (&next, &shapes);
                    scope.spawn(move || -> Result<Vec<(usize, DocumentId, ContentHash)>, String> {
                        let mut done = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= shapes.len() {
                                return Ok(done);
                            }
                            if spec.part_files {
                                let doc = shapes[i].part(format!("P{i:06}"));
                                let bytes = doc.to_bytes().map_err(|e| e.to_string())?;
                                // No fsync per file: these are test files, written once.
                                std::fs::write(part_path(dir, i), bytes).map_err(|e| e.to_string())?;
                                done.push((i, doc.id(), doc.semantic_hash()));
                            } else {
                                done.push((
                                    i,
                                    DocumentId::new(),
                                    ContentHash::of_bytes(format!("part {i}").as_bytes()),
                                ));
                            }
                        }
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().expect("a generator thread")).collect::<Result<_, _>>()
        })?;
        for (i, id, rev) in chunks.into_iter().flatten() {
            out[i] = (id, rev);
        }
        out
    };
    let parts_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let meshes: Vec<RefCopy> = shapes
        .iter()
        .zip(&refs)
        .map(|(s, (id, rev))| RefCopy::part(*id, rev.clone(), &s.mesh(), "generator"))
        .collect();

    // The modules: each part once on a grid, then a pattern stacking every one of them.
    let cell = 60.0;
    let side = (per_module as f64).sqrt().ceil() as usize;
    let mut module_refs = Vec::new();
    let mut module_copies = Vec::new();
    let mut bytes = 0u64;
    for m in 0..spec.modules {
        let range = m * per_module..((m + 1) * per_module).min(spec.unique);
        if range.is_empty() {
            break;
        }
        let mut a = Assembly::new(format!("Module {m:03}"));
        let path = dir.join(format!("modules/m{m:03}.dwasm"));
        let mut seeds: Vec<ComponentId> = Vec::new();
        for (k, i) in range.clone().enumerate() {
            let mut r = DocRef::new(refs[i].0, DocKind::Part, "", refs[i].1.clone());
            r.set_path(&path, &part_path(dir, i));
            let s: SourceId = a.add_source(r);
            let at = DVec3::new((k % side) as f64 * cell, (k / side) as f64 * cell, 0.0);
            seeds.push(a.add_component(s, Placement::translation(at)));
        }
        let tallest = range.clone().map(|i| shapes[i].height()).fold(0.0, f64::max);
        if repeats > 1 {
            a.add_pattern(
                seeds,
                PatternRule::Linear {
                    direction: [0.0, 0.0, 1.0],
                    spacing: tallest + 5.0,
                    count: repeats,
                    second: None,
                },
            );
        }
        let id = DocumentId::new();
        let copies: Vec<RefCopy> = range.clone().map(|i| meshes[i].clone()).collect();
        let file = a.to_bytes(id, Some("generator".into()), &copies).map_err(|e| e.to_string())?;
        bytes += file.len() as u64;
        std::fs::write(&path, file).map_err(|e| e.to_string())?;
        let rev = a.semantic_hash();
        module_copies.push(RefCopy::assembly(id, rev.clone(), &a.truth_entries()));
        module_refs.push((id, rev, path));
    }

    // The root: the modules on a grid, carrying every copy so it opens on its own.
    let mut root = Assembly::new("Scale test");
    let root_path = dir.join("root.dwasm");
    let span = side as f64 * cell + 500.0;
    let across = (module_refs.len() as f64).sqrt().ceil() as usize;
    for (k, (id, rev, path)) in module_refs.iter().enumerate() {
        let mut r = DocRef::new(*id, DocKind::Assembly, "", rev.clone());
        r.set_path(&root_path, path);
        let s = root.add_source(r);
        let at = DVec3::new((k % across) as f64 * span, (k / across) as f64 * span, 0.0);
        root.add_component(s, Placement::translation(at));
    }
    let mut copies = module_copies;
    copies.extend(meshes);
    let file = root.to_bytes(DocumentId::new(), Some("generator".into()), &copies).map_err(|e| e.to_string())?;
    bytes += file.len() as u64;
    std::fs::write(&root_path, file).map_err(|e| e.to_string())?;
    Ok(Generated { root: root_path, parts_seconds, assemblies_seconds: started.elapsed().as_secs_f64(), bytes })
}

fn part_path(dir: &Path, i: usize) -> PathBuf {
    dir.join(format!("parts/p{i:06}.dwprt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prisms_are_closed_and_face_outward() {
        let m = prism(5.0, 10.0, 16);
        m.check().unwrap();
        assert_eq!(m.triangle_count(), 32 + 2 * 14);
        for t in m.indices.chunks(3) {
            let p = |k: usize| glam::Vec3::from_array(m.positions[t[k] as usize]);
            let n = glam::Vec3::from_array(m.normals[t[0] as usize]);
            let centre = (p(0) + p(1) + p(2)) / 3.0;
            assert!((p(1) - p(0)).cross(p(2) - p(0)).dot(n) > 0.0, "counter-clockwise from outside at {centre}");
        }
    }
}
