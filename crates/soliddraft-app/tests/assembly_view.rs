//! Assemblies through the real viewport renderer, headless: instanced draws, the
//! `Rg32Uint` id buffer, and a pick that names the occurrence and the face under the
//! cursor, before and after a move and a hide; and, where the GPU culls, parts drawn as
//! boxes or not at all as they get small on screen, and each placement drawn at the
//! level of detail its size on screen needs.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use glam::{DVec3, Mat4, Vec3};
use soliddraft_app::viewport::assembly::{AssemblyFrame, Detail};
use soliddraft_app::viewport::look::{Preset, Tier};
use soliddraft_app::viewport::render::{DisplayMode, FrameInput, PickSample, ViewportRenderer};
use soliddraft_assembly::{
    Assembly, AssemblyDocument, AssemblyPackage, DefinitionIndex, DisplayMesh, OccurrencePath, OpenOptions, Placement,
    RefCopy, Scene, SharedMesh,
};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId};
use soliddraft_tessellation::{DeviceBudget, Level};

fn device() -> Option<(wgpu::Device, wgpu::Queue, Tier)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let features = adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE;
    let desc = wgpu::DeviceDescriptor { required_features: features, ..Default::default() };
    let tier = Tier::of(adapter.get_info().backend);
    let (device, queue) = pollster::block_on(adapter.request_device(&desc)).ok()?;
    Some((device, queue, tier))
}

/// Ten bricks in a row along x, 10 mm cubes 20 mm apart.
fn sample() -> AssemblyDocument {
    let brick = (DocumentId::new(), ContentHash::of_bytes(b"brick"));
    let mut root = Assembly::new("Row");
    let s = root.add_source(DocRef::new(brick.0, DocKind::Part, "brick.dwprt", brick.1.clone()));
    for i in 0..10 {
        root.add_component(s, Placement::translation(DVec3::new(i as f64 * 20.0, 0.0, 0.0)));
    }
    let mesh = DisplayMesh::cuboid(DVec3::ZERO, DVec3::splat(10.0));
    let bytes = root.to_bytes(DocumentId::new(), None, &[RefCopy::part(brick.0, brick.1, &mesh, "test")]).unwrap();
    let mut doc =
        AssemblyDocument::open_package(AssemblyPackage::from_bytes(bytes).unwrap(), None, OpenOptions::default())
            .unwrap();
    doc.finish_loading();
    doc
}

const SIZE: [u32; 2] = [256, 256];

struct View {
    renderer: ViewportRenderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pick: Arc<Mutex<PickSample>>,
    /// Coarse and fine meshes and deviations for the next frame.
    levels: Vec<(DefinitionIndex, Level, SharedMesh)>,
    deviations: Vec<(DefinitionIndex, [f32; 3])>,
}

impl View {
    fn new(renderer: ViewportRenderer, device: wgpu::Device, queue: wgpu::Queue) -> View {
        let pick = Arc::new(Mutex::new(PickSample::default()));
        View { renderer, device, queue, pick, levels: Vec::new(), deviations: Vec::new() }
    }

    /// Renders until a pick window that this frame's scene produced has come back.
    fn frame(&mut self, scene: &Arc<Mutex<Scene>>, doc: &AssemblyDocument, new_meshes: bool) -> [u32; 2] {
        self.frame_from(scene, doc, new_meshes, 200.0, Detail::default())
    }

    /// Renders looking straight down at the point (55, 5) from `height` above it.
    fn frame_from(
        &mut self,
        scene: &Arc<Mutex<Scene>>,
        doc: &AssemblyDocument,
        new_meshes: bool,
        height: f32,
        detail: Detail,
    ) -> [u32; 2] {
        let eye = Vec3::new(55.0, 5.0, height);
        let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::new(55.0, 5.0, 0.0), Vec3::Y);
        // Reversed depth, like the viewport's camera.
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(0.5, 1.0, 1.0);
        let view_proj: Mat4 = proj * view;
        let mut meshes = Vec::new();
        if new_meshes {
            for d in scene.lock().unwrap().take_new_meshes(usize::MAX) {
                meshes.push((d, doc.store().get(d).mesh.clone().unwrap()));
            }
        }
        let mut input = FrameInput {
            view,
            proj,
            eye,
            basis: [Vec3::X, Vec3::Y, Vec3::Z],
            orthographic: false,
            distance: height,
            size_px: SIZE,
            pixels_per_point: 1.0,
            display: DisplayMode::Shaded,
            preset: Preset::default(),
            backdrop: soliddraft_app::theme::viewport_backdrop(),
            hover_id: 0,
            selected_id: 0,
            selected_feature: 0,
            selected_body: None,
            cursor_px: Some([SIZE[0] / 2, SIZE[1] / 2]),
            meshes: (0, Vec::new()),
            ghost: None,
            preview: None,
            preview_pickable: false,
            pick_revision: None,
            pick_out: self.pick.clone(),
            assembly: Some(AssemblyFrame {
                scene: scene.clone(),
                meshes,
                view_proj: view_proj.as_dmat4(),
                eye: eye.as_dvec3(),
                detail,
                levels: std::mem::take(&mut self.levels),
                deviations: std::mem::take(&mut self.deviations),
                budget: DeviceBudget::default(),
            }),
        };
        // The pick window comes back a frame late: wait for one drawn from here.
        let serial = self.pick.lock().unwrap().serial + 1;
        for _ in 0..8 {
            self.renderer.prepare(&self.device, &self.queue, &input);
            self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            if let Some(a) = &mut input.assembly {
                a.meshes.clear();
                a.levels.clear();
            }
            let p = self.pick.lock().unwrap();
            if p.serial > serial {
                return p.texel(SIZE[0] / 2, SIZE[1] / 2).unwrap();
            }
        }
        panic!("no pick came back");
    }
}

#[test]
fn picks_name_the_occurrence_and_face_under_the_cursor() {
    let Some((device, queue, tier)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let renderer = ViewportRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, tier);
    let mut v = View::new(renderer, device, queue);
    let mut doc = sample();
    let scene = Arc::new(Mutex::new(Scene::new(&mut doc)));

    // The camera looks down at x = 55, the gap between the bricks at 40 and 60.
    let texel = v.frame(&scene, &doc, true);
    assert_eq!(texel, [0, 0], "the gap between bricks is empty");
    let stats = v.renderer.assembly_stats();
    assert_eq!((stats.draws, stats.instances, stats.triangles), (1, 10, 120));

    // The fourth brick (components are 2 to 11) moves under the cursor.
    let fourth: OccurrencePath = "5".parse().unwrap();
    doc.move_to(&[(fourth.clone(), Placement::translation(DVec3::new(50.0, 0.0, 0.0)))]).unwrap();
    scene.lock().unwrap().update(&mut doc);
    let texel = v.frame(&scene, &doc, false);
    let picked = scene.lock().unwrap().decode(texel).expect("a brick under the cursor");
    let sel = doc.selection(&picked);
    assert_eq!(sel.path, fourth);
    assert_eq!(sel.face.as_deref(), Some("+z"), "looking down, the top face");
    assert!(v.renderer.assembly_stats().uploaded < 1024, "a move uploads only the moved instance");

    doc.set_hidden(std::slice::from_ref(&fourth), true).unwrap();
    scene.lock().unwrap().update(&mut doc);
    assert_eq!(v.frame(&scene, &doc, false), [0, 0], "a hidden brick is not drawn or picked");
}

#[test]
fn small_parts_are_drawn_as_boxes_or_not_at_all() {
    let Some((device, queue, tier)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let renderer = ViewportRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, tier);
    let culls = renderer.assembly_culls();
    let mut v = View::new(renderer, device, queue);
    let mut doc = sample();
    let fourth: OccurrencePath = "5".parse().unwrap();
    doc.move_to(&[(fourth.clone(), Placement::translation(DVec3::new(50.0, 0.0, 0.0)))]).unwrap();
    let scene = Arc::new(Mutex::new(Scene::new(&mut doc)));
    let row = doc.table().find(&fourth).unwrap();
    let near = v.frame_from(&scene, &doc, true, 200.0, Detail::default());
    assert_eq!(scene.lock().unwrap().decode(near).map(|p| (p.occurrence, p.face.is_some())), Some((row, true)));
    if !culls {
        eprintln!("this adapter does not cull on the GPU; the rest needs it");
        return;
    }
    // From 1.5 m the 10 mm brick is about 3 px across: drawn as its box, which picks the
    // occurrence and no face.
    let boxed = v.frame_from(&scene, &doc, false, 1500.0, Detail::default());
    assert_eq!(scene.lock().unwrap().decode(boxed).map(|p| (p.occurrence, p.face)), Some((row, None)));
    // With every detail asked for, the same view draws the mesh.
    let full = Detail { drop_px: 0.0, box_px: 0.0 };
    let meshed = v.frame_from(&scene, &doc, false, 1500.0, full);
    assert_eq!(scene.lock().unwrap().decode(meshed).map(|p| (p.occurrence, p.face.is_some())), Some((row, true)));
    // From 20 m it is under half a pixel: not drawn at all.
    assert_eq!(v.frame_from(&scene, &doc, false, 20_000.0, Detail::default()), [0, 0]);
}

/// The brick with its top face named `top` (and the face that had that name named
/// "+z"). Every level's faces are numbered by name like the standard mesh's, so a pick
/// of the top tells which level was drawn.
fn marked(top: &str) -> SharedMesh {
    let mut m = DisplayMesh::cuboid(DVec3::ZERO, DVec3::splat(10.0));
    for f in &mut m.faces {
        if f.name == "+z" {
            f.name = top.into();
        } else if f.name == top {
            f.name = "+z".into();
        }
    }
    Arc::new(m)
}

#[test]
fn each_placement_draws_the_level_its_size_on_screen_needs() {
    let Some((device, queue, tier)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let renderer = ViewportRenderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, tier);
    if !renderer.assembly_culls() {
        eprintln!("this adapter does not cull on the GPU; levels need it");
        return;
    }
    let mut v = View::new(renderer, device, queue);
    let mut doc = sample();
    let fourth: OccurrencePath = "5".parse().unwrap();
    doc.move_to(&[(fourth.clone(), Placement::translation(DVec3::new(50.0, 0.0, 0.0)))]).unwrap();
    let scene = Arc::new(Mutex::new(Scene::new(&mut doc)));
    let def = doc.table().definition(doc.table().find(&fourth).unwrap());
    let top = |v: &mut View, height: f32, new: bool| {
        let texel = v.frame_from(&scene, &doc, new, height, Detail { drop_px: 0.0, box_px: 0.0 });
        let picked = scene.lock().unwrap().decode(texel).expect("the brick under the cursor");
        doc.selection(&picked).face.expect("a face")
    };
    // Levels a factor of four apart. From 400 mm a millimetre is about 1.3 px, so the
    // coarse level's 0.4 mm is within a pixel; from 40 mm it is about 19 px, so only the
    // fine level's 0.025 mm is.
    v.deviations = vec![(def, [0.4, 0.1, 0.025])];

    // Only the standard level is uploaded: it is drawn, and the coarse one is wanted.
    assert_eq!(top(&mut v, 400.0, true), "+z");
    let mut wanted = Vec::new();
    for _ in 0..40 {
        top(&mut v, 400.0, false);
        wanted.extend(v.renderer.take_assembly_wanted());
        if !wanted.is_empty() {
            break;
        }
    }
    assert_eq!(wanted, vec![(def, Level::Coarse)]);

    v.levels = vec![(def, Level::Coarse, marked("-x")), (def, Level::Fine, marked("-y"))];
    assert_eq!(top(&mut v, 400.0, false), "-x", "far: coarse");
    assert_eq!(top(&mut v, 40.0, false), "-y", "close: fine, at once");
    // Back out, it stays fine for a while, then steps down one level: the standard
    // level's error is above the threshold for stepping down again.
    assert_eq!(top(&mut v, 400.0, false), "-y");
    let mut seen = Vec::new();
    for _ in 0..200 {
        let face = top(&mut v, 400.0, false);
        if seen.last() != Some(&face) {
            seen.push(face.clone());
        }
        if face == "+z" {
            break;
        }
    }
    assert_eq!(seen, ["-y", "+z"]);
}
