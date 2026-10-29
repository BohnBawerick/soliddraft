//! wgpu side of assemblies: every part occurrence drawn as an instance of its
//! definition's mesh.
//!
//! All loaded meshes share one vertex buffer and one index buffer; each definition is a
//! range in them. All instances share one buffer, grouped by definition
//! (`soliddraft_assembly::Scene`). Uploads follow the scene: new meshes are appended as
//! they arrive, and only the instance ranges that changed are written.
//!
//! Where the device has compute, storage buffers in the vertex stage and indirect
//! first-instance (native and WebGPU), the GPU culls every frame (`cull.wgsl`): one
//! thread per instance drops what the camera cannot see and what is under
//! [`Detail::drop_px`] on screen, sends what is under [`Detail::box_px`] to a list drawn
//! as bounding boxes, and writes the rest into per-definition lists. Each pass then
//! draws the meshes with one `multi_draw_indexed_indirect` call and the boxes with one
//! more. Elsewhere (WebGL2) every instance is drawn with one `draw_indexed` per
//! definition. The id pass writes the occurrence's row and the face into the
//! `Rg32Uint` id target either way.
//!
//! When culling, each instance also picks one of its definition's three display levels
//! (coarse, standard, fine; see `soliddraft-tessellation`): the coarsest whose error
//! covers at most a pixel on screen. A finer level is taken at once; a coarser one only
//! after the error has stayed under a tenth of a pixel for 120 frames, so instances do
//! not flicker between levels as the camera moves. Each instance keeps its level in a
//! word of GPU memory. Levels not uploaded yet stand in with the nearest one there,
//! coarser first, and the definitions that wanted them are read back every few frames
//! ([`AssemblyRenderer::take_wanted`]), so the app can ask the document to load them.
//! The device budget caps the finest level drawn. The standard level is the
//! definition's own mesh; WebGL2 draws only that.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use glam::{DMat4, DVec3};
use soliddraft_assembly::{DefinitionIndex, DisplayMesh, Instance, MeshBounds, Scene, SharedMesh};
use soliddraft_tessellation::{DeviceBudget, Level, Usage};

/// How much detail to draw, in pixels of radius on screen: the "light mode" knob.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detail {
    /// Instances smaller than this are not drawn.
    pub drop_px: f32,
    /// Instances smaller than this are drawn as their bounding box.
    pub box_px: f32,
}

impl Default for Detail {
    fn default() -> Detail {
        Detail { drop_px: 0.5, box_px: 3.0 }
    }
}

/// What the frame hands the assembly renderer.
pub struct AssemblyFrame {
    /// The scene, shared with whoever applies the document's changes to it.
    pub scene: Arc<Mutex<Scene>>,
    /// Display meshes that arrived since the last frame.
    pub meshes: Vec<(DefinitionIndex, SharedMesh)>,
    /// The camera's view-projection in the root's frame.
    pub view_proj: DMat4,
    /// The eye in the root's frame.
    pub eye: DVec3,
    /// How much detail to draw.
    pub detail: Detail,
    /// Coarse and fine meshes that arrived since the last frame.
    pub levels: Vec<(DefinitionIndex, Level, SharedMesh)>,
    /// Each new definition's deviation per level, in millimetres, coarsest first
    /// (`Definition::deviations`). A definition left out is taken as automatic quality
    /// at its mesh's size.
    pub deviations: Vec<(DefinitionIndex, [f32; 3])>,
    /// How much display data the app lets the GPU hold: it caps the finest level drawn.
    pub budget: DeviceBudget,
}

/// What the last frame drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssemblyStats {
    /// Definitions with a mesh on the GPU.
    pub draws: u32,
    /// Instances of those definitions, hidden ones included, before culling.
    pub instances: u32,
    /// Triangles of those instances at full detail, before culling.
    pub triangles: u64,
    /// Bytes uploaded this frame.
    pub uploaded: u64,
    /// True when one indirect call drew every mesh.
    pub multi_draw: bool,
    /// True when the GPU culled this frame.
    pub culled: bool,
    /// Bytes the assembly holds on the GPU: meshes, instances, tables and lists.
    pub gpu_bytes: u64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    face: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct IndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// A definition as the cull pass and the box pass read it.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct DefGpu {
    sphere: [f32; 4],
    lo: [f32; 4],
    hi: [f32; 4],
    /// The standard level: index count (0 until uploaded), first index, base vertex,
    /// and the definition's first instance slot.
    mesh: [u32; 4],
    /// Each level's deviation in millimetres, coarsest first.
    dev: [f32; 4],
    /// x: the definition's instance count; y: one bit per level that is uploaded.
    count: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CullParams {
    view_proj: [[f32; 4]; 4],
    planes: [[f32; 4]; 6],
    params: [f32; 4],
    count: [u32; 4],
    pick: [f32; 4],
    viewport: [f32; 4],
}

/// A buffer that grows fourfold when full, keeping what it holds.
struct Pool {
    buffer: wgpu::Buffer,
    label: &'static str,
    usage: wgpu::BufferUsages,
    len: u64,
}

impl Pool {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages) -> Pool {
        let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 1 << 22,
            usage,
            mapped_at_creation: false,
        });
        Pool { buffer, label, usage, len: 0 }
    }

    /// Makes room for `bytes` in all, at most the device's limit, so the pool does not
    /// copy itself again and again while a large assembly streams in.
    fn reserve(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, bytes: u64) {
        let size = bytes.min(device.limits().max_buffer_size).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if size <= self.buffer.size() {
            return;
        }
        let bigger = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size,
            usage: self.usage,
            mapped_at_creation: false,
        });
        if self.len > 0 {
            encoder.copy_buffer_to_buffer(
                &self.buffer,
                0,
                &bigger,
                0,
                self.len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
            );
        }
        self.buffer = bigger;
    }

    /// Appends `bytes`; returns where they start, or `None` when the device cannot hold
    /// them.
    fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        bytes: &[u8],
    ) -> Option<u64> {
        let at = self.len;
        let need = at + bytes.len() as u64;
        let limit = device.limits().max_buffer_size;
        if need > limit {
            // Past what the device allows in one buffer: these meshes stay undrawn.
            log::warn!("{}: {need} bytes would pass the device's buffer limit of {limit}", self.label);
            return None;
        }
        if need > self.buffer.size() {
            // Grow by four, so a large assembly copies its meshes a few times, not a dozen.
            let size = need.max(self.buffer.size() * 4).min(limit).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
            let bigger = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size,
                usage: self.usage,
                mapped_at_creation: false,
            });
            if self.len > 0 {
                // The new bytes are written by the queue before this copy runs; the copy
                // moves only what was there before, so the two never overlap.
                encoder.copy_buffer_to_buffer(
                    &self.buffer,
                    0,
                    &bigger,
                    0,
                    self.len.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
                );
            }
            self.buffer = bigger;
        }
        // Writes go in whole words; pad the tail.
        if bytes.len().is_multiple_of(4) {
            queue.write_buffer(&self.buffer, at, bytes);
        } else {
            let mut padded = bytes.to_vec();
            padded.resize(bytes.len().next_multiple_of(4), 0);
            queue.write_buffer(&self.buffer, at, &padded);
        }
        self.len = need.next_multiple_of(4);
        Some(at)
    }
}

/// Where one level of a definition's mesh lives in the pools, and its bounds.
#[derive(Clone, Copy)]
struct MeshRange {
    base_vertex: i32,
    first_index: u32,
    index_count: u32,
    lo: [f32; 3],
    hi: [f32; 3],
}

/// The buffers culling needs, sized for one scene generation.
struct Cull {
    params: wgpu::Buffer,
    slot_defs: wgpu::Buffer,
    defs: wgpu::Buffer,
    /// The per-definition draws with zero instances, copied over `args` each frame.
    args_template: wgpu::Buffer,
    args: wgpu::Buffer,
    visible: wgpu::Buffer,
    proxy_template: wgpu::Buffer,
    proxy_args: wgpu::Buffer,
    proxies: wgpu::Buffer,
    /// One word per instance (its level and how many frames it has been quiet), then
    /// one per definition (the levels its instances wanted but could not have).
    levels: wgpu::Buffer,
    /// Where the wanted words are copied to be read back.
    readback: wgpu::Buffer,
    compute_group: wgpu::BindGroup,
    draw_group: wgpu::BindGroup,
    definitions: u32,
    instances: u32,
}

/// Draws assemblies into the viewport's targets.
pub struct AssemblyRenderer {
    color: wgpu::RenderPipeline,
    id: wgpu::RenderPipeline,
    culled_color: Option<[wgpu::RenderPipeline; 4]>,
    cull_pipeline: Option<wgpu::ComputePipeline>,
    cull_layout: Option<wgpu::BindGroupLayout>,
    draw_layout: Option<wgpu::BindGroupLayout>,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: Pool,
    indices: Pool,
    cube: (wgpu::Buffer, wgpu::Buffer),
    /// Each definition's levels, coarsest first.
    meshes: Vec<[Option<MeshRange>; 3]>,
    /// Each definition's deviations per level, coarsest first.
    deviations: Vec<Option<[f32; 3]>>,
    /// Each definition's standard mesh, whose face numbers every level's ids use.
    standard: Vec<Option<SharedMesh>>,
    /// Triangles uploaded, for the device budget.
    triangles: u64,
    /// Frames prepared, to space the wanted readbacks.
    frames: u64,
    readback: Arc<Mutex<Readback>>,
    /// Levels the GPU wanted and did not have, from the last readback.
    wanted: Vec<(DefinitionIndex, Level)>,
    instances: Option<wgpu::Buffer>,
    generation: u64,
    indirect: Option<wgpu::Buffer>,
    draws: Vec<IndirectArgs>,
    cull: Option<Cull>,
    /// Meshes held back from a frame that rebuilt everything else.
    deferred: Vec<(DefinitionIndex, SharedMesh)>,
    deferred_levels: Vec<(DefinitionIndex, Level, SharedMesh)>,
    multi_draw: bool,
    active: bool,
    culling: bool,
    /// The scene last drawn. Held weakly, so its address is not reused by another while
    /// it is compared.
    scene: std::sync::Weak<Mutex<Scene>>,
    /// What the last frame drew.
    pub stats: AssemblyStats,
}

const INSTANCE_SIZE: u64 = std::mem::size_of::<Instance>() as u64;

/// A level's slot in the per-definition tables.
const LEVELS: usize = 3;
/// Read the wanted levels back every this many frames.
const WANTED_EVERY: u64 = 15;

/// The state of the wanted-levels readback.
#[derive(Default)]
struct Readback {
    /// A copy was recorded this frame and must be mapped once submitted.
    copied: bool,
    /// Mapping was asked for and has not finished.
    pending: bool,
    /// Mapped and ready to read.
    ready: bool,
    /// The buffer being mapped: a new scene generation may replace the culling
    /// buffers while it is.
    buffer: Option<wgpu::Buffer>,
}

impl AssemblyRenderer {
    /// Pipelines for the viewport's colour target (`samples` samples) and its id target.
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
        id_format: wgpu::TextureFormat,
    ) -> AssemblyRenderer {
        let shader = device.create_shader_module(wgpu::include_wgsl!("assembly.wgsl"));
        let uniform = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("assembly globals"),
            entries: &[uniform(wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("assembly"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: INSTANCE_SIZE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Uint32x4],
        };
        let direct_buffers = [Some(vertex_layout.clone()), Some(instance_layout)];
        let culled_buffers = [Some(vertex_layout)];
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        vs: &str,
                        fs: &str,
                        format,
                        samples| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(true),
                    // Depth is reversed, as in the rest of the viewport: near is 1.
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let color =
            pipeline("assembly faces", &pipeline_layout, &direct_buffers, "vs_main", "fs_main", color_format, samples);
        let id = pipeline("assembly ids", &pipeline_layout, &direct_buffers, "vs_id", "fs_id", id_format, 1);

        let multi_draw = device.features().contains(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        let limits = device.limits();
        let can_cull = multi_draw
            && limits.max_storage_buffers_per_shader_stage >= 8
            && limits.max_compute_workgroup_size_x >= 256
            && limits.max_compute_invocations_per_workgroup >= 256;
        let (mut culled_color, mut cull_pipeline, mut cull_layout, mut draw_layout) = (None, None, None, None);
        if can_cull {
            let storage = |binding, visibility, read_only| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let v = wgpu::ShaderStages::VERTEX;
            let dl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("assembly culled draws"),
                entries: &[
                    storage(0, v, true),
                    storage(1, v, true),
                    storage(2, v, true),
                    storage(3, v, true),
                    storage(4, v, true),
                ],
            });
            let culled_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("assembly culled"),
                bind_group_layouts: &[Some(&layout), Some(&dl)],
                immediate_size: 0,
            });
            culled_color = Some([
                pipeline(
                    "assembly culled faces",
                    &culled_layout,
                    &culled_buffers,
                    "vs_culled",
                    "fs_main",
                    color_format,
                    samples,
                ),
                pipeline(
                    "assembly boxes",
                    &culled_layout,
                    &culled_buffers,
                    "vs_proxy",
                    "fs_main",
                    color_format,
                    samples,
                ),
                pipeline("assembly culled ids", &culled_layout, &culled_buffers, "vs_culled_id", "fs_id", id_format, 1),
                pipeline("assembly box ids", &culled_layout, &culled_buffers, "vs_proxy_id", "fs_id", id_format, 1),
            ]);
            let c = wgpu::ShaderStages::COMPUTE;
            let cl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("assembly cull"),
                entries: &[
                    uniform(c),
                    storage(1, c, true),
                    storage(2, c, true),
                    storage(3, c, true),
                    storage(4, c, false),
                    storage(5, c, false),
                    storage(6, c, false),
                    storage(7, c, false),
                    storage(8, c, false),
                ],
            });
            let cull_shader = device.create_shader_module(wgpu::include_wgsl!("cull.wgsl"));
            let cull_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("assembly cull"),
                bind_group_layouts: &[Some(&cl)],
                immediate_size: 0,
            });
            cull_pipeline = Some(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("assembly cull"),
                layout: Some(&cull_pipeline_layout),
                module: &cull_shader,
                entry_point: Some("cull_main"),
                compilation_options: Default::default(),
                cache: None,
            }));
            cull_layout = Some(cl);
            draw_layout = Some(dl);
        }

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("assembly globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("assembly globals"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        AssemblyRenderer {
            color,
            id,
            culled_color,
            cull_pipeline,
            cull_layout,
            draw_layout,
            globals,
            bind_group,
            vertices: Pool::new(device, "assembly vertices", wgpu::BufferUsages::VERTEX),
            indices: Pool::new(device, "assembly indices", wgpu::BufferUsages::INDEX),
            cube: unit_cube(device),
            meshes: Vec::new(),
            deviations: Vec::new(),
            standard: Vec::new(),
            triangles: 0,
            frames: 0,
            readback: Arc::default(),
            wanted: Vec::new(),
            instances: None,
            generation: 0,
            indirect: None,
            draws: Vec::new(),
            cull: None,
            deferred: Vec::new(),
            deferred_levels: Vec::new(),
            multi_draw,
            active: false,
            culling: false,
            scene: std::sync::Weak::new(),
            stats: AssemblyStats::default(),
        }
    }

    /// True when this device culls on the GPU.
    pub fn culls(&self) -> bool {
        self.cull_pipeline.is_some()
    }

    /// Forgets everything uploaded: for when another assembly is shown.
    pub fn clear(&mut self, device: &wgpu::Device) {
        self.vertices = Pool::new(device, "assembly vertices", wgpu::BufferUsages::VERTEX);
        self.indices = Pool::new(device, "assembly indices", wgpu::BufferUsages::INDEX);
        self.meshes.clear();
        self.deviations.clear();
        self.standard.clear();
        self.triangles = 0;
        self.wanted.clear();
        // A readback still in flight belongs to the last assembly.
        self.readback = Arc::new(Mutex::new(Readback::default()));
        self.instances = None;
        self.indirect = None;
        self.draws.clear();
        self.cull = None;
        self.deferred.clear();
        self.deferred_levels.clear();
        self.generation = 0;
    }

    /// Uploads what changed, culls on the GPU where it can, and sets the camera.
    /// Without a frame nothing is drawn.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: Option<&AssemblyFrame>,
        size_px: [u32; 2],
        cursor_px: Option<[u32; 2]>,
    ) {
        let Some(frame) = frame else {
            self.active = false;
            return;
        };
        // Another assembly: what was uploaded for the last one goes.
        if !std::sync::Weak::ptr_eq(&self.scene, &Arc::downgrade(&frame.scene)) {
            self.clear(device);
            self.scene = Arc::downgrade(&frame.scene);
        }
        self.active = true;
        let mut uploaded = 0u64;
        let mut draws_changed = false;
        // A frame that makes the instance buffer and the culling tables leaves its meshes
        // to the next frame, so the two large jobs do not land in one frame.
        let new_generation =
            self.instances.is_none() || frame.scene.lock().map(|s| s.generation() != self.generation).unwrap_or(true);
        let incoming: Vec<(DefinitionIndex, SharedMesh)> = if new_generation {
            self.deferred.extend(frame.meshes.iter().cloned());
            Vec::new()
        } else {
            let mut all = std::mem::take(&mut self.deferred);
            all.extend(frame.meshes.iter().cloned());
            all
        };
        // A wanted-levels readback that finished: take it before anything else.
        self.take_readback();
        for (def, dev) in &frame.deviations {
            let i = def.0 as usize;
            if self.deviations.len() <= i {
                self.deviations.resize(i + 1, None);
            }
            self.deviations[i] = Some(*dev);
        }
        // Standard meshes first, so a level arriving with its standard mesh can number
        // its faces like it.
        for (def, mesh) in &incoming {
            let i = def.0 as usize;
            if self.standard.len() <= i {
                self.standard.resize(i + 1, None);
            }
            self.standard[i] = Some(mesh.clone());
        }
        let levels: Vec<(DefinitionIndex, Level, SharedMesh)> = if new_generation {
            self.deferred_levels.extend(frame.levels.iter().cloned());
            Vec::new()
        } else {
            let mut all = std::mem::take(&mut self.deferred_levels);
            all.extend(frame.levels.iter().cloned());
            all
        };
        // This frame's meshes go up as one write per pool, however many there are.
        let mut new_defs = Vec::new();
        let (mut all_verts, mut all_idx) = (Vec::new(), Vec::new());
        let mut placed = Vec::with_capacity(incoming.len() + levels.len());
        // Without culling on the GPU only the standard level is drawn.
        let culls = self.cull_pipeline.is_some();
        let uploads = incoming
            .iter()
            .map(|(d, m)| (*d, Level::Standard, m))
            .chain(levels.iter().filter(|(_, l, _)| culls && *l != Level::Standard).map(|(d, l, m)| (*d, *l, m)));
        for (def, level, mesh) in uploads {
            let numbering = self.standard.get(def.0 as usize).cloned().flatten();
            let (verts, idx) = split_by_face_like(mesh, numbering.as_deref().filter(|_| level != Level::Standard));
            let b = mesh.bounds();
            placed.push((def, level, all_verts.len(), all_idx.len(), idx.len(), b));
            all_verts.extend(verts);
            all_idx.extend(idx);
        }
        if !placed.is_empty() && self.vertices.len == 0 {
            // The first meshes tell how big the rest will be: size both pools for every
            // definition at that average, with a quarter to spare.
            let definitions = frame.scene.lock().map(|s| s.ranges().len()).unwrap_or(0) as u64;
            let per = |bytes: usize| bytes as u64 / placed.len() as u64 * definitions * 5 / 4;
            self.vertices.reserve(device, encoder, per(all_verts.len() * std::mem::size_of::<Vertex>()));
            self.indices.reserve(device, encoder, per(all_idx.len() * 4));
        }
        let at = if placed.is_empty() {
            None
        } else {
            let vat = self.vertices.push(device, queue, encoder, bytemuck::cast_slice(&all_verts));
            let iat = vat.and_then(|_| self.indices.push(device, queue, encoder, bytemuck::cast_slice(&all_idx)));
            vat.zip(iat)
        };
        if let Some((vat, iat)) = at {
            uploaded += (all_verts.len() * std::mem::size_of::<Vertex>() + all_idx.len() * 4) as u64;
            self.triangles += all_idx.len() as u64 / 3;
            let base = vat / std::mem::size_of::<Vertex>() as u64;
            for (def, level, v, i0, count, b) in placed {
                let i = def.0 as usize;
                if self.meshes.len() <= i {
                    self.meshes.resize(i + 1, [None; LEVELS]);
                }
                self.meshes[i][level.index()] = Some(MeshRange {
                    base_vertex: (base + v as u64) as i32,
                    first_index: (iat / 4) as u32 + i0 as u32,
                    index_count: count as u32,
                    lo: b.min.as_vec3().to_array(),
                    hi: b.max.as_vec3().to_array(),
                });
                if level == Level::Standard {
                    draws_changed = true;
                }
                new_defs.push(def);
            }
        }
        let mut scene = frame.scene.lock().expect("the scene lock");
        let origin = scene.origin();
        let n = scene.instances().len();
        let mut rebuilt = false;
        if scene.generation() != self.generation || self.instances.is_none() {
            self.generation = scene.generation();
            let mut usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
            if self.cull_pipeline.is_some() {
                usage |= wgpu::BufferUsages::STORAGE;
            }
            // Made with its contents (mapped at creation), so a million instances go up
            // without a second staging copy; what the scene marked dirty is in it already.
            scene.take_dirty();
            let contents: &[u8] = if n == 0 { &[0; 64] } else { bytemuck::cast_slice(scene.instances()) };
            use wgpu::util::DeviceExt;
            self.instances = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("assembly instances"),
                contents,
                usage,
            }));
            uploaded += contents.len() as u64;
            draws_changed = true;
            rebuilt = true;
        }
        let instances = self.instances.clone().expect("made above");
        for range in scene.take_dirty() {
            let data = &scene.instances()[range.start as usize..range.end as usize];
            queue.write_buffer(&instances, range.start as u64 * INSTANCE_SIZE, bytemuck::cast_slice(data));
            uploaded += std::mem::size_of_val(data) as u64;
        }
        if self.cull_pipeline.is_some() {
            if rebuilt || self.cull.is_none() {
                let (cull, bytes) = self.make_cull(device, &instances, scene.ranges(), n as u32);
                self.cull = Some(cull);
                uploaded += bytes;
            } else {
                uploaded += self.write_defs(queue, scene.ranges(), &new_defs);
            }
        }
        if draws_changed {
            let standard = Level::Standard.index();
            let meshes = &self.meshes;
            self.draws = scene
                .draws(|d| meshes.get(d.0 as usize).is_some_and(|m| m[standard].is_some()))
                .into_iter()
                .map(|d| {
                    let m = self.meshes[d.definition.0 as usize][standard].expect("filtered above");
                    IndirectArgs {
                        index_count: m.index_count,
                        instance_count: d.count,
                        first_index: m.first_index,
                        base_vertex: m.base_vertex,
                        first_instance: d.first_instance,
                    }
                })
                .collect();
            if self.multi_draw && self.cull.is_none() && !self.draws.is_empty() {
                let bytes: &[u8] = bytemuck::cast_slice(&self.draws);
                let fits = self.indirect.as_ref().is_some_and(|b| b.size() >= bytes.len() as u64);
                if !fits {
                    self.indirect = Some(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("assembly draws"),
                        size: (bytes.len() as u64).next_power_of_two(),
                        usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }));
                }
                queue.write_buffer(self.indirect.as_ref().expect("made above"), 0, bytes);
                uploaded += bytes.len() as u64;
            }
        }
        drop(scene);
        let relative = frame.view_proj * DMat4::from_translation(origin);
        let eye = frame.eye - origin;
        let vp = relative.as_mat4();
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                view_proj: vp.to_cols_array_2d(),
                eye: [eye.x as f32, eye.y as f32, eye.z as f32, 1.0],
            }),
        );
        self.culling = false;
        if let (Some(cull), Some(pipeline)) = (&self.cull, &self.cull_pipeline) {
            // Pixels per unit of radius at depth 1: the length of the matrix's y row,
            // which a rigid view leaves as the projection's y scale, times half the height.
            let y_scale = relative.row(1).truncate().length() * f64::from(size_px[1]) * 0.5;
            let planes = frustum_planes(&relative);
            // A perspective matrix gives w from the position; a parallel one keeps it 1.
            let perspective = relative.row(3).truncate().length() > 1e-12;
            // The id pass draws only what may cover the window read back for picking.
            let pick = cursor_px.map(|c| {
                let (w, h) = (super::render::PICK_ROW_TEXELS.min(size_px[0]), super::render::PICK_ROWS.min(size_px[1]));
                let x = c[0].saturating_sub(w / 2).min(size_px[0].saturating_sub(w)) as f32;
                let y = c[1].saturating_sub(h / 2).min(size_px[1].saturating_sub(h)) as f32;
                [x - 1.0, y - 1.0, x + w as f32 + 1.0, y + h as f32 + 1.0]
            });
            let usage = Usage { triangles: self.triangles, bytes: self.vertices.len + self.indices.len };
            let ceiling = frame.budget.ceiling(usage).index() as u32;
            let params = CullParams {
                view_proj: vp.to_cols_array_2d(),
                planes,
                params: [y_scale as f32, frame.detail.drop_px, frame.detail.box_px, f32::from(perspective)],
                count: [cull.instances, u32::from(pick.is_some()), cull.definitions, ceiling],
                pick: pick.unwrap_or_default(),
                viewport: [size_px[0] as f32, size_px[1] as f32, 0.0, 0.0],
            };
            queue.write_buffer(&cull.params, 0, bytemuck::bytes_of(&params));
            encoder.copy_buffer_to_buffer(&cull.args_template, 0, &cull.args, 0, cull.args.size());
            encoder.copy_buffer_to_buffer(&cull.proxy_template, 0, &cull.proxy_args, 0, cull.proxy_args.size());
            let (wanted_at, wanted_len) = (u64::from(cull.instances) * 4, u64::from(cull.definitions) * 4);
            encoder.clear_buffer(&cull.levels, wanted_at, Some(wanted_len));
            if cull.instances > 0 {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("assembly cull"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &cull.compute_group, &[]);
                pass.dispatch_workgroups(cull.instances.div_ceil(256), 1, 1);
            }
            self.frames += 1;
            let mut rb = self.readback.lock().expect("the readback lock");
            if self.frames.is_multiple_of(WANTED_EVERY) && !rb.copied && !rb.pending && !rb.ready {
                encoder.copy_buffer_to_buffer(&cull.levels, wanted_at, &cull.readback, 0, wanted_len);
                rb.copied = true;
            }
            drop(rb);
            self.culling = true;
        }
        let gpu_bytes = self.vertices.buffer.size()
            + self.indices.buffer.size()
            + self.instances.as_ref().map_or(0, |b| b.size())
            + self.indirect.as_ref().map_or(0, |b| b.size())
            + self.cull.as_ref().map_or(0, |c| {
                [&c.slot_defs, &c.defs, &c.args_template, &c.args, &c.visible, &c.proxies, &c.levels]
                    .iter()
                    .map(|b| b.size())
                    .sum()
            });
        self.stats = AssemblyStats {
            draws: self.draws.len() as u32,
            instances: self.draws.iter().map(|d| d.instance_count).sum(),
            triangles: self.draws.iter().map(|d| d.index_count as u64 / 3 * d.instance_count as u64).sum(),
            uploaded,
            multi_draw: self.multi_draw,
            culled: self.culling,
            gpu_bytes,
        };
    }

    /// The culling buffers for a scene generation, made with their contents: which
    /// definition each slot is, the definitions, and the draws with no instances yet
    /// (the colour pass's half, then the id pass's). Returns them and the bytes they hold.
    fn make_cull(
        &self,
        device: &wgpu::Device,
        instances: &wgpu::Buffer,
        ranges: &[std::ops::Range<u32>],
        n: u32,
    ) -> (Cull, u64) {
        use wgpu::BufferUsages as U;
        use wgpu::util::DeviceExt;
        let mut slot_defs = vec![0u32; n as usize];
        let mut defs = Vec::with_capacity(ranges.len());
        let mut args = Vec::with_capacity(2 * LEVELS * ranges.len());
        for (i, r) in ranges.iter().enumerate() {
            slot_defs[r.start as usize..r.end as usize].fill(i as u32);
            let (d, a) = self.def_gpu(i, r);
            defs.push(d);
            args.extend(a);
        }
        // The id pass's half: the same draws, reading the second half of the lists.
        let lists = LEVELS as u32 * n;
        let id_args: Vec<IndirectArgs> =
            args.iter().map(|a| IndirectArgs { first_instance: a.first_instance + lists, ..*a }).collect();
        args.extend(id_args);
        if defs.is_empty() {
            defs.push(DefGpu::default());
            args.extend([IndirectArgs::default(); 2 * LEVELS]);
        }
        // Every instance starts with no level, so its first frame picks one outright.
        let mut words = vec![u32::MAX; n as usize];
        words.resize(n as usize + defs.len(), 0);
        let proxy = IndirectArgs { index_count: 36, ..IndirectArgs::default() };
        let proxies_template = [proxy, IndirectArgs { first_instance: n, ..proxy }];
        let filled = |label, contents: &[u8], usage| {
            let padded;
            let contents = if contents.is_empty() {
                padded = [0u8; 16];
                &padded[..]
            } else {
                contents
            };
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        let empty = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(16),
                usage,
                mapped_at_creation: false,
            })
        };
        let params = empty("assembly cull params", std::mem::size_of::<CullParams>() as u64, U::UNIFORM | U::COPY_DST);
        let slot_defs_buf =
            filled("assembly slot definitions", bytemuck::cast_slice(&slot_defs), U::STORAGE | U::COPY_DST);
        let defs_buf = filled("assembly definitions", bytemuck::cast_slice(&defs), U::STORAGE | U::COPY_DST);
        let args_bytes: &[u8] = bytemuck::cast_slice(&args);
        let args_template = filled("assembly draw template", args_bytes, U::COPY_SRC | U::COPY_DST);
        let args_buf = empty("assembly culled draws", args_bytes.len() as u64, U::STORAGE | U::INDIRECT | U::COPY_DST);
        let visible = empty("assembly visible", 2 * u64::from(lists) * 4, U::STORAGE);
        let levels = filled("assembly levels", bytemuck::cast_slice(&words), U::STORAGE | U::COPY_SRC | U::COPY_DST);
        let readback = empty("assembly wanted readback", defs.len() as u64 * 4, U::COPY_DST | U::MAP_READ);
        let proxy_template =
            filled("assembly box template", bytemuck::cast_slice(&proxies_template), U::COPY_SRC | U::COPY_DST);
        let proxy_args = empty("assembly box draws", 40, U::STORAGE | U::INDIRECT | U::COPY_DST);
        let proxies = empty("assembly boxes", 2 * n as u64 * 4, U::STORAGE);
        fn entry(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: b.as_entire_binding() }
        }
        let compute_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("assembly cull"),
            layout: self.cull_layout.as_ref().expect("culling is on"),
            entries: &[
                entry(0, &params),
                entry(1, instances),
                entry(2, &slot_defs_buf),
                entry(3, &defs_buf),
                entry(4, &args_buf),
                entry(5, &visible),
                entry(6, &proxy_args),
                entry(7, &proxies),
                entry(8, &levels),
            ],
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("assembly culled draws"),
            layout: self.draw_layout.as_ref().expect("culling is on"),
            entries: &[
                entry(0, instances),
                entry(1, &visible),
                entry(2, &proxies),
                entry(3, &slot_defs_buf),
                entry(4, &defs_buf),
            ],
        });
        let bytes = (slot_defs.len() * 4 + std::mem::size_of_val(defs.as_slice()) + args_bytes.len() + 40) as u64
            + words.len() as u64 * 4;
        let cull = Cull {
            params,
            slot_defs: slot_defs_buf,
            defs: defs_buf,
            args_template,
            args: args_buf,
            visible,
            proxy_template,
            proxy_args,
            proxies,
            levels,
            readback,
            compute_group,
            draw_group,
            definitions: defs.len() as u32,
            instances: n,
        };
        (cull, bytes)
    }

    /// A definition's table entry and its draws, one per level, coarsest first. The
    /// instances of level `l` are listed from `3 * start + l * count`.
    fn def_gpu(&self, def: usize, range: &std::ops::Range<u32>) -> (DefGpu, [IndirectArgs; LEVELS]) {
        let levels = self.meshes.get(def).copied().unwrap_or([None; LEVELS]);
        let count = range.end - range.start;
        let mut d = DefGpu::default();
        d.mesh[3] = range.start;
        d.count = [count, 0, 0, 0];
        let args = std::array::from_fn(|l| {
            let mut a = IndirectArgs {
                first_instance: LEVELS as u32 * range.start + l as u32 * count,
                ..IndirectArgs::default()
            };
            if let Some(m) = levels[l] {
                a.index_count = m.index_count;
                a.first_index = m.first_index;
                a.base_vertex = m.base_vertex;
                d.count[1] |= 1 << l;
            }
            a
        });
        // Bounds and the "has a mesh" test follow the standard level.
        if let Some(m) = levels[Level::Standard.index()] {
            let (lo, hi) = (glam::Vec3::from_array(m.lo), glam::Vec3::from_array(m.hi));
            let c = (lo + hi) * 0.5;
            d.sphere = [c.x, c.y, c.z, (hi - lo).length() * 0.5];
            d.lo = [lo.x, lo.y, lo.z, 0.0];
            d.hi = [hi.x, hi.y, hi.z, 0.0];
            d.mesh = [m.index_count, m.first_index, m.base_vertex as u32, range.start];
            let dev = self.deviations.get(def).copied().flatten().unwrap_or_else(|| {
                let levels = soliddraft_tessellation::Levels::new(&Default::default(), f64::from((hi - lo).length()));
                Level::ALL.map(|l| levels.deviation(l).unwrap_or(0.0) as f32)
            });
            d.dev = [dev[0], dev[1], dev[2], 0.0];
        }
        (d, args)
    }

    /// Writes the table entries of `defs`, merged into runs (definitions closer than
    /// 256 apart share a run), so a frame that takes in thousands of meshes makes a
    /// handful of writes.
    fn write_defs(&self, queue: &wgpu::Queue, ranges: &[std::ops::Range<u32>], defs: &[DefinitionIndex]) -> u64 {
        let Some(cull) = &self.cull else { return 0 };
        let mut list: Vec<usize> = defs.iter().map(|d| d.0 as usize).filter(|i| *i < ranges.len()).collect();
        list.sort_unstable();
        list.dedup();
        let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
        for i in list {
            match runs.last_mut() {
                Some(r) if i < r.end + 256 => r.end = i + 1,
                _ => runs.push(i..i + 1),
            }
        }
        let mut bytes = 0;
        for run in runs {
            let (defs, args): (Vec<DefGpu>, Vec<[IndirectArgs; LEVELS]>) =
                run.clone().map(|i| self.def_gpu(i, &ranges[i])).unzip();
            let args: Vec<IndirectArgs> = args.into_iter().flatten().collect();
            queue.write_buffer(
                &cull.defs,
                (run.start * std::mem::size_of::<DefGpu>()) as u64,
                bytemuck::cast_slice(&defs),
            );
            let arg = std::mem::size_of::<IndirectArgs>();
            let at = (run.start * LEVELS * arg) as u64;
            queue.write_buffer(&cull.args_template, at, bytemuck::cast_slice(&args));
            let lists = LEVELS as u32 * cull.instances;
            let id_args: Vec<IndirectArgs> =
                args.iter().map(|a| IndirectArgs { first_instance: a.first_instance + lists, ..*a }).collect();
            let half = (cull.definitions as usize * LEVELS * arg) as u64;
            queue.write_buffer(&cull.args_template, half + at, bytemuck::cast_slice(&id_args));
            bytes += (std::mem::size_of_val(defs.as_slice()) + 2 * std::mem::size_of_val(args.as_slice())) as u64;
        }
        bytes
    }

    fn submit(&self, pass: &mut wgpu::RenderPass<'_>, ids: bool) {
        let (true, Some(instances)) = (self.active, &self.instances) else { return };
        if let (true, Some(cull), Some(pipelines)) = (self.culling, &self.cull, &self.culled_color) {
            let [faces, boxes, face_ids, box_ids] = pipelines;
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &cull.draw_group, &[]);
            // The id pass draws the second half of each list.
            let arg = std::mem::size_of::<IndirectArgs>() as u64;
            let draws = cull.definitions * LEVELS as u32;
            let (args_at, proxy_at) = if ids { (u64::from(draws) * arg, arg) } else { (0, 0) };
            pass.set_pipeline(if ids { face_ids } else { faces });
            pass.set_vertex_buffer(0, self.vertices.buffer.slice(..));
            pass.set_index_buffer(self.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.multi_draw_indexed_indirect(&cull.args, args_at, draws);
            pass.set_pipeline(if ids { box_ids } else { boxes });
            pass.set_vertex_buffer(0, self.cube.0.slice(..));
            pass.set_index_buffer(self.cube.1.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed_indirect(&cull.proxy_args, proxy_at);
            return;
        }
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(if ids { &self.id } else { &self.color });
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.buffer.slice(..));
        pass.set_vertex_buffer(1, instances.slice(..));
        pass.set_index_buffer(self.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
        match (&self.indirect, self.multi_draw) {
            (Some(indirect), true) => pass.multi_draw_indexed_indirect(indirect, 0, self.draws.len() as u32),
            _ => {
                for d in &self.draws {
                    pass.draw_indexed(
                        d.first_index..d.first_index + d.index_count,
                        d.base_vertex,
                        d.first_instance..d.first_instance + d.instance_count,
                    );
                }
            }
        }
    }

    /// Reads a finished wanted-levels readback into [`AssemblyRenderer::take_wanted`].
    fn take_readback(&mut self) {
        let buffer = match self.readback.lock() {
            Ok(mut r) if r.ready => r.buffer.take(),
            _ => None,
        };
        let Some(buffer) = buffer else { return };
        {
            let view = buffer.slice(..).get_mapped_range().expect("mapped wanted readback");
            let words: &[u32] = bytemuck::cast_slice(&view);
            for (def, w) in words.iter().enumerate().filter(|(_, w)| **w != 0) {
                for level in Level::ALL.into_iter().filter(|l| w & (1 << l.index()) != 0) {
                    self.wanted.push((DefinitionIndex(def as u32), level));
                }
            }
        }
        buffer.unmap();
        if let Ok(mut r) = self.readback.lock() {
            r.ready = false;
            r.pending = false;
        }
    }

    /// Call after the frame's commands are submitted: maps the wanted levels copied
    /// this frame for reading.
    pub fn after_submit(&self) {
        let Some(cull) = &self.cull else { return };
        let mut rb = self.readback.lock().expect("the readback lock");
        if !rb.copied {
            return;
        }
        rb.copied = false;
        rb.pending = true;
        rb.buffer = Some(cull.readback.clone());
        let state = self.readback.clone();
        cull.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if let Ok(mut r) = state.lock() {
                r.pending = false;
                r.ready = result.is_ok();
            }
        });
    }

    /// The definitions and levels the GPU wanted to draw and did not have, since the
    /// last call: hand them to `AssemblyDocument::request_levels`.
    pub fn take_wanted(&mut self) -> Vec<(DefinitionIndex, Level)> {
        std::mem::take(&mut self.wanted)
    }

    /// Draws the shaded faces into the colour pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.submit(pass, false);
    }

    /// Draws occurrence and face ids into the id pass.
    pub fn draw_ids(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.submit(pass, true);
    }
}

/// The six planes of a view-projection (clip depth 0 to 1, either way round), each
/// `(n, d)` with the inside where `n . p + d >= 0`, normalised so distances are in
/// model units. A plane at infinity, the far plane of the viewport's reversed
/// projection, has no normal and keeps everything.
fn frustum_planes(m: &DMat4) -> [[f32; 4]; 6] {
    let r = |i: usize| m.row(i);
    [r(3) + r(0), r(3) - r(0), r(3) + r(1), r(3) - r(1), r(2), r(3) - r(2)].map(|p| {
        let len = p.truncate().length();
        if len < 1e-12 * p.w.abs().max(1.0) {
            return [0.0, 0.0, 0.0, 1.0];
        }
        (p / len).as_vec4().to_array()
    })
}

/// The unit cube from 0 to 1, with outward normals: what a box stand-in stretches.
fn unit_cube(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer) {
    use wgpu::util::DeviceExt;
    let mesh = DisplayMesh::cuboid(DVec3::ZERO, DVec3::ONE);
    let (verts, idx) = split_by_face(&mesh);
    let v = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("assembly box"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let i = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("assembly box indices"),
        contents: bytemuck::cast_slice(&idx),
        usage: wgpu::BufferUsages::INDEX,
    });
    (v, i)
}

/// The mesh's vertices, each belonging to one face (a vertex shared by two faces is
/// split), and its indices relative to them.
/// `mesh` split by face as below, with faces numbered as `like` numbers them (by name),
/// so a coarse or fine level's ids name the same faces as the standard mesh.
fn split_by_face_like(mesh: &DisplayMesh, like: Option<&DisplayMesh>) -> (Vec<Vertex>, Vec<u32>) {
    let (mut verts, idx) = split_by_face(mesh);
    if let Some(like) = like {
        let number: std::collections::HashMap<&str, u32> =
            like.faces.iter().enumerate().map(|(i, f)| (f.name.as_str(), i as u32)).collect();
        let map: Vec<u32> = mesh
            .faces
            .iter()
            .enumerate()
            .map(|(i, f)| number.get(f.name.as_str()).copied().unwrap_or(i as u32))
            .collect();
        for v in &mut verts {
            v.face = map.get(v.face as usize).copied().unwrap_or(v.face);
        }
    }
    (verts, idx)
}

fn split_by_face(mesh: &DisplayMesh) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(mesh.positions.len());
    let mut indices = Vec::with_capacity(mesh.indices.len());
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    let mut touched = Vec::new();
    for (f, face) in mesh.faces.iter().enumerate() {
        for &old in touched.iter() {
            remap[old as usize] = u32::MAX;
        }
        touched.clear();
        let tris = &mesh.indices[face.first as usize * 3..(face.first + face.count) as usize * 3];
        for &i in tris {
            if remap[i as usize] == u32::MAX {
                remap[i as usize] = verts.len() as u32;
                touched.push(i);
                verts.push(Vertex {
                    position: mesh.positions[i as usize],
                    normal: mesh.normals[i as usize],
                    face: f as u32,
                });
            }
            indices.push(remap[i as usize]);
        }
    }
    (verts, indices)
}

/// Asks for indirect first-instance support where the adapter has it, so a pass draws
/// a whole assembly in one call and culls on the GPU, and for the adapter's buffer
/// size limits.
pub fn request_features(config: &mut egui_wgpu::WgpuConfiguration) {
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup {
        let base = setup.device_descriptor.clone();
        setup.device_descriptor = Arc::new(move |adapter| {
            let mut d = base(adapter);
            d.required_features |= adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE;
            // A large assembly's meshes and instances outgrow the default 256 MB buffer
            // and 128 MB storage binding; take what the adapter allows.
            let a = adapter.limits();
            d.required_limits.max_buffer_size = d.required_limits.max_buffer_size.max(a.max_buffer_size);
            d.required_limits.max_storage_buffer_binding_size =
                d.required_limits.max_storage_buffer_binding_size.max(a.max_storage_buffer_binding_size);
            d
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_vertices_are_split_between_faces() {
        // Two faces, one triangle each, sharing the edge from vertex 1 to vertex 2.
        let mesh = DisplayMesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            indices: vec![0, 1, 2, 1, 3, 2],
            faces: vec![
                soliddraft_assembly::MeshFace { first: 0, count: 1, name: "a".into() },
                soliddraft_assembly::MeshFace { first: 1, count: 1, name: "b".into() },
            ],
            edges: Vec::new(),
        };
        let (verts, idx) = split_by_face(&mesh);
        assert_eq!(verts.len(), 6, "the two shared vertices are split");
        assert_eq!(idx, vec![0, 1, 2, 3, 4, 5]);
        for t in idx.chunks(3) {
            let f = verts[t[0] as usize].face;
            assert!(t.iter().all(|i| verts[*i as usize].face == f), "a triangle belongs to one face");
        }
        assert_eq!(split_by_face(&DisplayMesh::cuboid(DVec3::ZERO, DVec3::ONE)).0.len(), 24);
    }

    #[test]
    fn frustum_planes_keep_the_view_inside() {
        let view =
            glam::camera::rh::view::look_at_mat4(glam::Vec3::new(0.0, 0.0, 10.0), glam::Vec3::ZERO, glam::Vec3::Y);
        // The viewport's reversed projection with its far plane at infinity, and a
        // forward one with a finite far plane.
        let reversed = glam::camera::rh::proj::directx::perspective_infinite_reverse(1.0, 1.0, 1.0);
        let forward = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 1.0, 100.0);
        for proj in [reversed, forward] {
            let planes = frustum_planes(&(proj * view).as_dmat4());
            assert!(planes.iter().flatten().all(|v| v.is_finite()));
            let inside = |p: glam::Vec3| planes.iter().all(|q| glam::Vec3::from_slice(&q[..3]).dot(p) + q[3] >= 0.0);
            assert!(inside(glam::Vec3::ZERO));
            assert!(!inside(glam::Vec3::new(0.0, 0.0, 20.0)), "behind the eye");
            assert!(!inside(glam::Vec3::new(100.0, 0.0, 0.0)), "far to the side");
        }
        let far = frustum_planes(&(forward * view).as_dmat4());
        let inside = |p: glam::Vec3| far.iter().all(|q| glam::Vec3::from_slice(&q[..3]).dot(p) + q[3] >= 0.0);
        assert!(!inside(glam::Vec3::new(0.0, 0.0, -200.0)), "beyond the far plane");
    }
}
