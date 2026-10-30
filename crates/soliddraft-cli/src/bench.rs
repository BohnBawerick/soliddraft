//! `soliddraft asm bench`: open a large assembly and use it the way a person would,
//! timing every frame.
//!
//! It renders headless through the app's own viewport renderer on the machine's GPU, so
//! what it measures is what the viewport does: each frame takes in what the loader has
//! found (within a 4 ms budget), applies the document's changes to the scene, uploads
//! what changed, draws the colour and id passes, reads the pick window back and waits
//! for the GPU. A frame's time is all of that, end to end.
//!
//! The run: open (structure only), frames while meshes load with the camera orbiting,
//! more orbit frames once loaded, then the interactions: pick and select under the
//! cursor, move that part, move its whole module, hide and show the module, hide and
//! show the part, colour the module, and undo everything. Each interaction is timed by
//! itself and by the frame that shows it.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_wgpu::wgpu;
use glam::{DQuat, DVec3, Mat4};
use serde_json::{Value, json};
use soliddraft_app::viewport::assembly::{AssemblyFrame, Detail};
use soliddraft_app::viewport::look::{Preset, Tier};
use soliddraft_app::viewport::render::{DisplayMode, FrameInput, PickSample, ViewportRenderer};
use soliddraft_assembly::bvh::Ray;
use soliddraft_assembly::{AssemblyDocument, Flags, OccurrenceIndex, OccurrencePath, OpenOptions, Placement, Scene};
use soliddraft_materials::{Appearance, Srgb};
use soliddraft_tessellation::DeviceBudget;

/// How many arrived meshes a frame uploads at most while an assembly loads.
const MESHES_PER_FRAME: usize = 1_000;

/// The frame budget that counts as a stall.
const STALL: Duration = Duration::from_millis(100);

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    tier: Tier,
}

fn gpu() -> Result<Gpu, String> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    let info = adapter.get_info();
    let features = adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE;
    let a = adapter.limits();
    let limits = wgpu::Limits {
        max_buffer_size: a.max_buffer_size,
        max_storage_buffer_binding_size: a.max_storage_buffer_binding_size,
        ..Default::default()
    };
    let desc = wgpu::DeviceDescriptor { required_features: features, required_limits: limits, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&desc)).map_err(|e| e.to_string())?;
    Ok(Gpu { device, queue, adapter: format!("{} ({:?})", info.name, info.backend), tier: Tier::of(info.backend) })
}

/// The process's resident memory, from /proc on Linux.
fn resident() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = text.lines().find(|l| l.starts_with("VmRSS:"))?.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// This thread's time on a core and time spent runnable but waiting for one, in
/// nanoseconds, from Linux's schedstat. The second is what other processes cost a frame
/// on a busy machine.
fn schedstat() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let mut f = text.split_whitespace().map(|v| v.parse::<u64>().unwrap_or(0));
    (f.next().unwrap_or(0), f.next().unwrap_or(0))
}

/// One frame's time, by phase.
#[derive(Clone, Copy, Default)]
struct Frame {
    wall: Duration,
    /// Taking in what the loader found.
    poll: Duration,
    /// Applying the document's changes to the scene and picking the meshes to upload.
    scene: Duration,
    /// The renderer's CPU work: uploads, the cull and draw commands, submitting.
    prepare: Duration,
    /// Waiting for the GPU to finish the frame.
    gpu: Duration,
    /// Runnable but waiting for a core (other processes).
    runqueue: Duration,
}

/// Frames with their summary.
#[derive(Default)]
struct Frames(Vec<Frame>);

impl Frames {
    fn push(&mut self, f: Frame) {
        self.0.push(f);
    }

    fn summary(&self) -> Value {
        if self.0.is_empty() {
            return json!(null);
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let percentiles = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
            json!({ "p50_ms": at(0.5), "p95_ms": at(0.95), "max_ms": v[v.len() - 1],
                    "over_100_ms": v.iter().filter(|t| **t > ms(STALL)).count() })
        };
        let worst = self.0.iter().max_by_key(|f| f.wall).expect("not empty");
        json!({
            "frames": self.0.len(),
            "wall": percentiles(self.0.iter().map(|f| ms(f.wall)).collect()),
            "without_runqueue_wait": percentiles(self.0.iter().map(|f| ms(f.wall.saturating_sub(f.runqueue))).collect()),
            "worst": { "wall_ms": ms(worst.wall), "poll_ms": ms(worst.poll), "scene_ms": ms(worst.scene),
                       "prepare_ms": ms(worst.prepare), "gpu_ms": ms(worst.gpu), "runqueue_ms": ms(worst.runqueue) },
        })
    }

    fn stalls(&self) -> usize {
        self.0.iter().filter(|f| f.wall.saturating_sub(f.runqueue) > STALL).count()
    }
}

struct Bench {
    gpu: Gpu,
    renderer: ViewportRenderer,
    pick: Arc<Mutex<PickSample>>,
    scene: Arc<Mutex<Scene>>,
    size: [u32; 2],
    centre: DVec3,
    distance: f64,
    angle: f64,
    detail: Detail,
}

impl Bench {
    /// One frame, end to end. Returns its time and the texel under the cursor, when a
    /// pick window came back.
    fn frame(&mut self, doc: &mut AssemblyDocument) -> (Frame, Option<[u32; 2]>) {
        let started = Instant::now();
        let (_, wait0) = schedstat();
        let mut f = Frame::default();
        doc.poll(Duration::from_millis(4));
        f.poll = started.elapsed();
        let (meshes, levels) = {
            let mut scene = self.scene.lock().unwrap();
            scene.update(doc);
            (scene.take_new_meshes(MESHES_PER_FRAME), scene.take_new_levels())
        };
        let meshes: Vec<_> =
            meshes.into_iter().filter_map(|d| doc.store().get(d).mesh.clone().map(|m| (d, m))).collect();
        let deviations = meshes.iter().map(|(d, _)| (*d, doc.store().get(*d).deviations())).collect();
        let levels = levels
            .into_iter()
            .filter_map(|(d, l)| doc.store().get(d).level_mesh(l).cloned().map(|m| (d, l, m)))
            .collect();
        f.scene = started.elapsed() - f.poll;
        let dir = DQuat::from_rotation_z(self.angle) * DVec3::new(1.0, -1.0, 0.8).normalize();
        let eye = self.centre + dir * self.distance;
        let view = glam::camera::rh::view::look_at_mat4(eye.as_vec3(), self.centre.as_vec3(), glam::Vec3::Z);
        let aspect = self.size[0] as f32 / self.size[1] as f32;
        // Reversed depth, like the viewport's camera.
        let proj =
            glam::camera::rh::proj::directx::perspective_infinite_reverse(0.6, aspect, (self.distance * 0.01) as f32);
        let view_proj: Mat4 = proj * view;
        let input = FrameInput {
            view,
            proj,
            eye: eye.as_vec3(),
            basis: basis(view),
            orthographic: false,
            distance: self.distance as f32,
            size_px: self.size,
            pixels_per_point: 1.0,
            display: DisplayMode::Shaded,
            preset: Preset::default(),
            backdrop: soliddraft_app::theme::viewport_backdrop(),
            hover_id: 0,
            selected_id: 0,
            selected_feature: 0,
            selected_body: None,
            cursor_px: Some([self.size[0] / 2, self.size[1] / 2]),
            meshes: (0, Vec::new()),
            ghost: None,
            preview: None,
            preview_pickable: false,
            pick_revision: None,
            pick_out: self.pick.clone(),
            grid: None,
            assembly: Some(AssemblyFrame {
                scene: self.scene.clone(),
                meshes,
                view_proj: view_proj.as_dmat4(),
                eye,
                detail: self.detail,
                levels,
                deviations,
                budget: DeviceBudget::default(),
            }),
        };
        let serial = self.pick.lock().unwrap().serial;
        let t = Instant::now();
        self.renderer.prepare(&self.gpu.device, &self.gpu.queue, &input);
        f.prepare = t.elapsed();
        let t = Instant::now();
        let _ = self.gpu.device.poll(wgpu::PollType::wait_indefinitely());
        f.gpu = t.elapsed();
        doc.request_levels(&self.renderer.take_assembly_wanted());
        f.wall = started.elapsed();
        f.runqueue = Duration::from_nanos(schedstat().1.saturating_sub(wait0));
        let p = self.pick.lock().unwrap();
        let texel = (p.serial > serial).then(|| p.texel(self.size[0] / 2, self.size[1] / 2)).flatten();
        (f, texel)
    }

    /// Frames until a fresh pick window has come back; returns their times and the texel.
    fn frames_until_pick(&mut self, doc: &mut AssemblyDocument, frames: &mut Frames) -> Option<[u32; 2]> {
        for _ in 0..4 {
            let (t, texel) = self.frame(doc);
            frames.push(t);
            if texel.is_some() {
                return texel;
            }
        }
        None
    }
}

/// Times one interaction and the frame that shows it.
fn step(
    steps: &mut Vec<Value>,
    name: &str,
    doc: &mut AssemblyDocument,
    b: &mut Bench,
    f: impl FnOnce(&mut AssemblyDocument, &mut Bench) -> Result<String, String>,
) -> Result<(), String> {
    let started = Instant::now();
    let detail = f(doc, b)?;
    let edit = started.elapsed();
    let (frame, _) = b.frame(doc);
    steps.push(json!({
        "step": name, "detail": detail,
        "edit_ms": edit.as_secs_f64() * 1000.0, "frame_ms": frame.wall.as_secs_f64() * 1000.0,
        "frame_without_runqueue_ms": frame.wall.saturating_sub(frame.runqueue).as_secs_f64() * 1000.0,
        "uploaded_bytes": b.renderer.assembly_stats().uploaded,
    }));
    Ok(())
}

/// The camera's right, up and back directions in world space.
fn basis(view: Mat4) -> [glam::Vec3; 3] {
    let world = view.inverse();
    [world.x_axis.truncate(), world.y_axis.truncate(), world.z_axis.truncate()]
}

/// A frame with no assembly: what the viewport draws while one is being opened.
fn empty_frame(size: [u32; 2], pick: Arc<Mutex<PickSample>>) -> FrameInput {
    FrameInput {
        view: Mat4::IDENTITY,
        proj: Mat4::IDENTITY,
        eye: glam::Vec3::Z,
        basis: basis(Mat4::IDENTITY),
        orthographic: false,
        distance: 1.0,
        size_px: size,
        pixels_per_point: 1.0,
        display: DisplayMode::Shaded,
        preset: Preset::default(),
        backdrop: soliddraft_app::theme::viewport_backdrop(),
        hover_id: 0,
        selected_id: 0,
        selected_feature: 0,
        selected_body: None,
        cursor_px: None,
        meshes: (0, Vec::new()),
        ghost: None,
        preview: None,
        preview_pickable: false,
        pick_revision: None,
        pick_out: pick,
        assembly: None,
        grid: None,
    }
}

/// Runs the benchmark on the assembly at `path`.
pub(crate) fn run(path: &Path, size: [u32; 2], orbit_frames: usize) -> Result<Value, String> {
    let before = resident();
    let gpu = gpu()?;
    let renderer = ViewportRenderer::new(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba8UnormSrgb, gpu.tier);

    // Open on another thread, drawing empty frames meanwhile, as the interface does.
    let started = Instant::now();
    let opening =
        AssemblyDocument::open_in_background(path, OpenOptions { check_freshness: true, ..OpenOptions::default() });
    let mut opening_frames = Frames::default();
    let pick = Arc::new(Mutex::new(PickSample::default()));
    let mut renderer = renderer;
    let (mut doc, scene) = loop {
        if let Some(done) = opening.try_take() {
            break done.map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let t = Instant::now();
        let (_, wait0) = schedstat();
        let input = empty_frame(size, pick.clone());
        renderer.prepare(&gpu.device, &gpu.queue, &input);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let wall = t.elapsed();
        let runqueue = Duration::from_nanos(schedstat().1.saturating_sub(wait0));
        opening_frames.push(Frame { wall, prepare: wall, runqueue, ..Frame::default() });
        std::thread::sleep(Duration::from_millis(8));
    };
    let structure = started.elapsed();
    let counts = doc.table().counts();

    // Frame everything: the box of the parts' positions, grown by the largest part.
    let mut lo = DVec3::INFINITY;
    let mut hi = DVec3::NEG_INFINITY;
    for (r, f) in doc.table().all_flags().iter().enumerate() {
        if !f.has(Flags::ASSEMBLY) {
            let t = doc.table().world(OccurrenceIndex(r as u32)).translation;
            lo = lo.min(t);
            hi = hi.max(t);
        }
    }
    let centre = (lo + hi) * 0.5;
    let distance = ((hi - lo).length() * 0.5 + 100.0) / (0.3f64).tan();
    let mut b = Bench {
        gpu,
        renderer,
        pick,
        scene: Arc::new(Mutex::new(scene)),
        size,
        centre,
        distance,
        angle: 0.0,
        detail: Detail::default(),
    };

    // Frames while loading, orbiting.
    let started = Instant::now();
    let mut loading = Frames::default();
    while doc.is_loading() || b.scene.lock().unwrap().pending_meshes() > 0 {
        let (t, _) = b.frame(&mut doc);
        loading.push(t);
        b.angle += 0.01;
    }
    let load_time = started.elapsed();
    let loaded = resident();
    // The hierarchy builds in the background once loading is done; keep drawing until
    // it is ready, as the interface would.
    let started = Instant::now();
    let mut waiting = Frames::default();
    while doc.bvh_ready().is_none() {
        let (t, _) = b.frame(&mut doc);
        waiting.push(t);
        b.angle += 0.01;
    }
    let bvh_ready = started.elapsed();

    // Orbit, loaded.
    let mut orbit = Frames::default();
    let mut stats = b.renderer.assembly_stats();
    for _ in 0..orbit_frames {
        let (t, _) = b.frame(&mut doc);
        orbit.push(t);
        b.angle += 0.02;
        stats = b.renderer.assembly_stats();
    }

    // Interactions. Look at the part in the middle of the table from close by, so the
    // cursor is on it (or on whatever stands in front of it).
    let mut steps: Vec<Value> = Vec::new();
    let mut frames = Frames::default();
    let target = (doc.table().len() / 2..doc.table().len())
        .map(|r| OccurrenceIndex(r as u32))
        .find(|r| doc.table().flags(*r).drawn_part())
        .ok_or("no part to look at")?;
    let bounds = doc.table().bounds(target);
    b.centre = bounds.center();
    b.distance = bounds.diagonal().max(10.0) * 6.0;
    // The pick window is read back a frame late: let a frame drawn from here go first.
    for _ in 0..2 {
        let (t, _) = b.frame(&mut doc);
        frames.push(t);
    }
    let texel = b.frames_until_pick(&mut doc, &mut frames);
    let picked = texel.and_then(|t| b.scene.lock().unwrap().decode(t));
    let Some(picked) = picked else {
        return Err("nothing under the cursor to select".into());
    };
    let part = doc.table().path(picked.occurrence);
    let module: OccurrencePath = OccurrencePath(part.steps()[..1].to_vec());
    step(&mut steps, "select", &mut doc, &mut b, |doc, b| {
        let sel = doc.selection(&picked);
        b.scene.lock().unwrap().set_selection(doc.table(), &[picked.occurrence]);
        Ok(format!("{} face {}", sel.path, sel.face.unwrap_or_default()))
    })?;
    let up = Placement::translation(DVec3::new(0.0, 0.0, 200.0));
    step(&mut steps, "move part", &mut doc, &mut b, |doc, _| {
        doc.move_by(std::slice::from_ref(&part), &up).map_err(|e| e.to_string())?;
        Ok(part.to_string())
    })?;
    let module_parts = {
        let row = doc.table().find(&module).ok_or("no module")?;
        doc.table().subtree(row).len() - 1
    };
    step(&mut steps, "move module", &mut doc, &mut b, |doc, _| {
        doc.move_by(std::slice::from_ref(&module), &up).map_err(|e| e.to_string())?;
        Ok(format!("{module}: {module_parts} occurrences"))
    })?;
    step(&mut steps, "hide module", &mut doc, &mut b, |doc, _| {
        doc.set_hidden(std::slice::from_ref(&module), true).map_err(|e| e.to_string())?;
        Ok(module.to_string())
    })?;
    step(&mut steps, "show module", &mut doc, &mut b, |doc, _| {
        doc.set_hidden(std::slice::from_ref(&module), false).map_err(|e| e.to_string())?;
        Ok(module.to_string())
    })?;
    step(&mut steps, "hide part", &mut doc, &mut b, |doc, _| {
        doc.set_hidden(std::slice::from_ref(&part), true).map_err(|e| e.to_string())?;
        Ok(part.to_string())
    })?;
    step(&mut steps, "show part", &mut doc, &mut b, |doc, _| {
        doc.set_hidden(std::slice::from_ref(&part), false).map_err(|e| e.to_string())?;
        Ok(part.to_string())
    })?;
    step(&mut steps, "colour module", &mut doc, &mut b, |doc, _| {
        let red = Appearance::new("bench-red", "Red", Srgb::new(200, 30, 30));
        doc.set_colour(std::slice::from_ref(&module), Some(red)).map_err(|e| e.to_string())?;
        Ok(module.to_string())
    })?;
    step(&mut steps, "undo all", &mut doc, &mut b, |doc, _| {
        let mut n = 0;
        while doc.undo() {
            n += 1;
        }
        Ok(format!("{n} steps"))
    })?;
    let table = doc.table().clone();
    let leaves = doc.bvh().len();
    let started = Instant::now();
    let ray = Ray { origin: b.centre + DVec3::new(0.0, 0.0, 10_000.0), direction: DVec3::NEG_Z };
    let hits = doc.bvh().along_ray(&table, &ray).len();
    let ray_time = started.elapsed();
    drop(table);
    let after = resident();
    let mib = |b: Option<u64>| b.map(|b| b as f64 / (1024.0 * 1024.0));
    // Frames over 100 ms of their own (run-queue wait left out), in every phase.
    let step_stalls = steps
        .iter()
        .filter(|s| s["frame_without_runqueue_ms"].as_f64().unwrap_or(0.0) > STALL.as_secs_f64() * 1000.0)
        .count();
    let stalls =
        opening_frames.stalls() + loading.stalls() + waiting.stalls() + orbit.stalls() + frames.stalls() + step_stalls;
    Ok(json!({
        "stalls": stalls,
        "file": path.display().to_string(),
        "adapter": b.gpu.adapter,
        "viewport": size,
        "occurrences": { "parts": counts.parts, "assemblies": counts.assemblies },
        "definitions": doc.store().len(),
        "open": {
            "structure_s": structure.as_secs_f64(),
            "frames_while_reading_structure": opening_frames.summary(),
            "load_s": load_time.as_secs_f64(),
            "frames_while_loading": loading.summary(),
        },
        "orbit": orbit.summary(),
        "drawn": { "definitions": stats.draws, "instances": stats.instances, "full_detail_triangles": stats.triangles,
                   "multi_draw": stats.multi_draw, "gpu_culling": stats.culled,
                   "detail_px": { "drop_below": b.detail.drop_px, "box_below": b.detail.box_px },
                   "gpu_mib": stats.gpu_bytes as f64 / (1024.0 * 1024.0) },
        "interactions": steps,
        "bvh": { "leaves": leaves, "ready_after_load_s": bvh_ready.as_secs_f64(), "frames_while_building": waiting.summary(),
                 "ray_ms": ray_time.as_secs_f64() * 1000.0, "ray_hits": hits },
        "memory_mib": {
            "resident_before": mib(before), "resident_loaded": mib(loaded), "resident_end": mib(after),
            "table": mib(Some(doc.table().heap_bytes() as u64)), "store": mib(Some(doc.store().heap_bytes() as u64)),
            "scene": mib(Some(b.scene.lock().unwrap().heap_bytes() as u64)),
        },
    }))
}
