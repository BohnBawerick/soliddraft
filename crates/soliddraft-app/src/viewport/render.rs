//! wgpu side of the viewport. Runs inside egui-wgpu's paint callback: the
//! scene is drawn to offscreen targets in `prepare` (MSAA colour, depth, and
//! a two-channel 32-bit id buffer), a small window of the id buffer around the
//! cursor is copied back for picking, and `paint` blits the colour result into
//! egui's own render pass.
//!
//! The id buffer is `Rg32Uint`. The first channel is 0 for the open part and,
//! for an assembly, the occurrence's row plus one; the second is the entity: a
//! two-bit kind and a 30-bit id. Both are good for this session only.
//!
//! Passes, in order: the shadow map from the light fixed above the model (only
//! when the geometry changed), the ambient occlusion prepass, sampling and blur
//! (full tier only), the colour pass (faces, the ground shadow, edges as wide
//! lines, ghosts) and the id pass. Depth is reversed (near 1, far 0) on every
//! camera pass. `look.rs` holds the presets and the tiers.

use super::assembly::{AssemblyFrame, AssemblyRenderer};
use crate::document::DisplayMesh;
use crate::viewport::environment;
use crate::viewport::look::{Preset, Tier};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::{Mat4, Vec3};
use soliddraft_kernel_api::BodyId;
use soliddraft_materials::{Appearance, Shading};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const KIND_SHIFT: u32 = 30;
pub const KIND_MASK: u32 = 0b11 << KIND_SHIFT;
pub const ID_MASK: u32 = !KIND_MASK;

/// What the id buffer reported under the cursor: a pick id (see
/// `Document::pick`) with the kind it was drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picked {
    Face(u32),
    Edge(u32),
    Vertex(u32),
}

impl Picked {
    pub fn decode(v: u32) -> Option<Self> {
        let id = v & ID_MASK;
        if id == 0 {
            return None;
        }
        match v >> KIND_SHIFT {
            1 => Some(Picked::Face(id)),
            2 => Some(Picked::Edge(id)),
            3 => Some(Picked::Vertex(id)),
            _ => None,
        }
    }

    pub fn pick(self) -> u32 {
        match self {
            Picked::Face(p) | Picked::Edge(p) | Picked::Vertex(p) => p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DisplayMode {
    #[default]
    ShadedWithEdges,
    Shaded,
    Wireframe,
}

impl DisplayMode {
    pub const ALL: [DisplayMode; 3] = [DisplayMode::ShadedWithEdges, DisplayMode::Shaded, DisplayMode::Wireframe];

    pub fn label(&self) -> &'static str {
        match self {
            DisplayMode::ShadedWithEdges => "Shaded with edges",
            DisplayMode::Shaded => "Shaded",
            DisplayMode::Wireframe => "Wireframe",
        }
    }
}

/// Window of the id buffer around the cursor, read back from the GPU.
#[derive(Clone, Debug, Default)]
pub struct PickSample {
    pub origin: [u32; 2],
    pub width: u32,
    pub height: u32,
    /// Texels: occurrence (0 for the open part) and entity.
    pub data: Vec<[u32; 2]>,
    pub serial: u64,
}

impl PickSample {
    /// The entity under a pixel: a kind and an id (see [`Picked::decode`]).
    pub fn at(&self, x: u32, y: u32) -> Option<u32> {
        self.texel(x, y).map(|t| t[1])
    }

    /// Both channels under a pixel.
    pub fn texel(&self, x: u32, y: u32) -> Option<[u32; 2]> {
        if x < self.origin[0] || y < self.origin[1] {
            return None;
        }
        let (lx, ly) = (x - self.origin[0], y - self.origin[1]);
        if lx >= self.width || ly >= self.height {
            return None;
        }
        self.data.get((ly * PICK_ROW_TEXELS + lx) as usize).copied()
    }
}

/// Everything the UI thread hands the GPU thread for one frame.
pub struct FrameInput {
    pub view: Mat4,
    /// Reversed-depth projection (`Camera::projection`).
    pub proj: Mat4,
    pub eye: Vec3,
    /// Camera right, up and toward-the-viewer directions; the studio rig follows them.
    pub basis: [Vec3; 3],
    pub orthographic: bool,
    /// Distance from the eye to the orbit target.
    pub distance: f32,
    pub size_px: [u32; 2],
    pub pixels_per_point: f32,
    pub display: DisplayMode,
    pub preset: Preset,
    pub hover_id: u32,
    pub selected_id: u32,
    pub selected_feature: u32,
    pub selected_body: Option<BodyId>,
    pub cursor_px: Option<[u32; 2]>,
    /// Document meshes with their version; re-uploaded when the version changes.
    pub meshes: (u64, Vec<(BodyId, Arc<DisplayMesh>)>),
    /// Added or removed material, with whether it removes.
    pub ghost: Option<(Arc<DisplayMesh>, bool)>,
    /// Drawn instead of the document bodies when set; picking still uses the bodies.
    pub preview: Option<Arc<DisplayMesh>>,
    pub pick_out: Arc<Mutex<PickSample>>,
    /// An assembly to draw with the part, if one is open.
    pub assembly: Option<AssemblyFrame>,
}

pub struct ViewportCallback(pub FrameInput);

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<ViewportRenderer>() {
            r.prepare(device, queue, &self.0);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = resources.get::<ViewportRenderer>() {
            r.paint(pass);
        }
    }
}

/// Mirrors `Globals` in scene.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    inv_proj: [[f32; 4]; 4],
    sun_view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    viewport: [f32; 4],
    pull: [f32; 4],
    key: [f32; 4],
    fill: [f32; 4],
    rim: [f32; 4],
    sun: [f32; 4],
    ground: [f32; 4],
    shadow: [f32; 4],
    look: [f32; 4],
    sh: [[f32; 4]; 9],
    hover_id: u32,
    selected_id: u32,
    /// Faces made by this feature are highlighted (0 for none).
    selected_feature: u32,
    _pad: u32,
}

/// Appearances one body can show; a face indexing past them shows the first.
pub const MAX_APPEARANCES: usize = 64;

/// One appearance as `Shade` in scene.wgsl reads it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuShade {
    /// Linear base colour and opacity.
    base: [f32; 4],
    /// Linear emitted light; w unused.
    emissive: [f32; 4],
    /// Metallic, roughness; zw unused.
    params: [f32; 4],
}

impl From<Shading> for GpuShade {
    fn from(s: Shading) -> Self {
        let e = s.emissive;
        GpuShade { base: s.base_color, emissive: [e[0], e[1], e[2], 0.0], params: [s.metallic, s.roughness, 0.0, 0.0] }
    }
}

/// Mirrors `Body` in scene.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BodyUniform {
    flags: u32,
    _pad: [u32; 3],
    tint: [f32; 4],
    shades: [GpuShade; MAX_APPEARANCES],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FaceVertex {
    position: [f32; 3],
    normal: [f32; 3],
    face_id: u32,
    feature_id: u32,
    appearance: u32,
}

/// One edge segment, drawn as a quad.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LineInstance {
    a: [f32; 3],
    b: [f32; 3],
    id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TaggedVertex {
    position: [f32; 3],
    id: u32,
}

struct GpuBody {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    lines: wgpu::Buffer,
    line_count: u32,
    points: wgpu::Buffer,
    point_count: u32,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bounds: Option<(Vec3, Vec3)>,
}

/// The ambient occlusion targets, full tier only.
struct AoTargets {
    depth: wgpu::TextureView,
    normals: wgpu::TextureView,
    raw: wgpu::TextureView,
    sample_bind_group: wgpu::BindGroup,
    blur_bind_group: wgpu::BindGroup,
    blurred: wgpu::TextureView,
}

struct Targets {
    size: [u32; 2],
    color_msaa: wgpu::TextureView,
    color: wgpu::TextureView,
    depth_msaa: wgpu::TextureView,
    id: wgpu::Texture,
    id_view: wgpu::TextureView,
    id_depth: wgpu::TextureView,
    blit_bind_group: wgpu::BindGroup,
    ao: Option<AoTargets>,
    /// Group 2 of the scene passes: the occlusion result, or a white texel.
    frame_bind_group: wgpu::BindGroup,
}

/// The shadow light's frame for one set of bounds: where the map looks from and
/// where the ground sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunFrame {
    pub view_proj: Mat4,
    /// Centre of the ground disc, on the plane under the model's lowest point.
    pub ground: Vec3,
    pub ground_radius: f32,
    /// Radius of the model's bounding sphere.
    pub radius: f32,
    /// World size of one shadow-map texel.
    pub texel: f32,
}

/// The light fixed above the model, tilted like the environment's softbox so the
/// shadow falls beside the part and not only under it. It is fixed to the model, not
/// the camera, so the map is redrawn only when the geometry changes.
pub fn sun_frame(lo: Vec3, hi: Vec3) -> SunFrame {
    let center = (lo + hi) / 2.0;
    let radius = ((hi - lo).length() / 2.0).max(1.0);
    let toward = environment::softbox_direction();
    let ground_radius = radius * 2.6;
    let eye = center + toward * radius * 3.0;
    let up = if toward.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
    let view = glam::camera::rh::view::look_at_mat4(eye, center, up);
    let half = radius * 1.05;
    // Deep enough to reach the far rim of the ground disc behind the model.
    let far = radius * 3.0 + (ground_radius * ground_radius + (hi.z - lo.z).powi(2)).sqrt() + radius;
    let proj = glam::camera::rh::proj::directx::orthographic(-half, half, -half, half, radius, far);
    SunFrame {
        view_proj: proj * view,
        ground: Vec3::new(center.x, center.y, lo.z - radius * 1e-3),
        ground_radius,
        radius,
        texel: 2.0 * half / SHADOW_SIZE as f32,
    }
}

const MSAA: u32 = 4;
const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// Bytes per id texel.
const ID_TEXEL: u32 = 8;
/// Depth24Plus filters with a comparison sampler on WebGL2 as well.
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const SHADOW_SIZE: u32 = 2048;
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// Edge width in points.
const EDGE_WIDTH: f32 = 1.3;
/// How far edges are pulled toward the viewer, as a fraction of their distance.
const EDGE_PULL: f32 = 0.003;
/// Reversed depth: the near plane clears to 1 and far is 0.
const DEPTH_CLEAR: f32 = 0.0;
const DEPTH_COMPARE: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;
/// Texels per read-back row: 512 bytes, a multiple of the 256-byte copy alignment.
pub const PICK_ROW_TEXELS: u32 = 64;
pub const PICK_ROWS: u32 = 32;

#[derive(Default)]
struct Readback {
    /// A copy has been submitted and its map callback has not fired yet.
    pending: bool,
    /// The map callback fired; the buffer can be read.
    ready: bool,
    origin: [u32; 2],
    serial: u64,
}

struct Layouts {
    /// Group 0 of the passes that need only the uniforms (shadow, occlusion).
    uniforms: wgpu::BindGroupLayout,
    body: wgpu::BindGroupLayout,
    frame: wgpu::BindGroupLayout,
    ao_inputs: wgpu::BindGroupLayout,
    blit: wgpu::BindGroupLayout,
}

pub struct ViewportRenderer {
    tier: Tier,
    face_pipeline: wgpu::RenderPipeline,
    ground_pipeline: wgpu::RenderPipeline,
    edge_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    id_face_pipeline: wgpu::RenderPipeline,
    id_edge_pipeline: wgpu::RenderPipeline,
    id_vertex_pipeline: wgpu::RenderPipeline,
    /// Normal prepass, occlusion and blur; full tier only.
    ao_pipelines: Option<[wgpu::RenderPipeline; 3]>,
    blit_pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    uniforms_bind_group: wgpu::BindGroup,
    layouts: Layouts,
    sampler: wgpu::Sampler,
    shadow_view: wgpu::TextureView,
    sun: Option<SunFrame>,
    /// What the shadow map was last drawn from: mesh version and preview.
    shadow_key: Option<(u64, usize)>,
    white: wgpu::TextureView,
    sh: [[f32; 4]; 9],
    targets: Option<Targets>,
    bodies: HashMap<BodyId, GpuBody>,
    /// `u64::MAX` means nothing uploaded yet, so version 0 (an empty document) still uploads.
    mesh_version: u64,
    /// Keyed by the `Arc` address of the ghost mesh; never dereferenced.
    ghost: Option<(usize, bool, GpuBody)>,
    preview: Option<(usize, GpuBody)>,
    pick_buffer: wgpu::Buffer,
    readback: Arc<Mutex<Readback>>,
    display: DisplayMode,
    frame: u64,
    assembly: AssemblyRenderer,
}

impl ViewportRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, target_format: wgpu::TextureFormat, tier: Tier) -> Self {
        log::info!("viewport renderer: {} tier", tier.label());
        let shader = device.create_shader_module(wgpu::include_wgsl!("scene.wgsl"));

        let stages = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stages,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding: u32, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
            count: None,
        };
        let sampler_entry = |binding: u32, kind| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(kind),
            count: None,
        };
        let d2 = wgpu::TextureViewDimension::D2;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, wgpu::TextureSampleType::Depth, d2),
                sampler_entry(2, wgpu::SamplerBindingType::Comparison),
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::Cube),
                sampler_entry(4, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let layouts = Layouts {
            uniforms: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("uniforms"),
                entries: &[uniform_entry(0)],
            }),
            body: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("body"),
                entries: &[uniform_entry(0)],
            }),
            frame: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("frame"),
                entries: &[texture_entry(0, wgpu::TextureSampleType::Float { filterable: false }, d2)],
            }),
            ao_inputs: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("occlusion inputs"),
                entries: &[
                    texture_entry(0, wgpu::TextureSampleType::Depth, d2),
                    texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }, d2),
                ],
            }),
            blit: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blit"),
                entries: &[
                    texture_entry(0, wgpu::TextureSampleType::Float { filterable: true }, d2),
                    sampler_entry(1, wgpu::SamplerBindingType::Filtering),
                ],
            }),
        };

        let pipeline_layout = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
            let groups: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|g| Some(*g)).collect();
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            })
        };
        let scene_layout = pipeline_layout("scene", &[&globals_layout, &layouts.body, &layouts.frame]);
        let uniforms_layout = pipeline_layout("uniforms", &[&layouts.uniforms]);
        let ao_layout = pipeline_layout("occlusion", &[&layouts.uniforms, &layouts.ao_inputs]);
        let blit_pipeline_layout = pipeline_layout("blit", &[&layouts.blit]);

        let face_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FaceVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Uint32, 4 => Uint32],
        };
        let line_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32],
        };
        let point_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TaggedVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
        };

        struct Desc<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            vs: &'a str,
            fs: Option<&'a str>,
            buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
            format: Option<wgpu::TextureFormat>,
            blend: Option<wgpu::BlendState>,
            samples: u32,
            cull: Option<wgpu::Face>,
            depth: Option<wgpu::DepthStencilState>,
        }
        let pipeline = |d: Desc<'_>| {
            let targets = [d.format.map(|format| wgpu::ColorTargetState {
                format,
                blend: d.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(d.label),
                layout: Some(d.layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(d.vs),
                    compilation_options: Default::default(),
                    buffers: d.buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: d.cull,
                    ..Default::default()
                },
                depth_stencil: d.depth,
                multisample: wgpu::MultisampleState { count: d.samples, ..Default::default() },
                fragment: d.fs.map(|fs| wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: if d.format.is_some() { &targets } else { &[] },
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let depth = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(DEPTH_COMPARE),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let premul = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let back = Some(wgpu::Face::Back);
        let faces = [Some(face_layout.clone())];
        let lines = [Some(line_layout)];
        let face_pipeline = pipeline(Desc {
            label: "faces",
            layout: &scene_layout,
            vs: "vs_face",
            fs: Some("fs_face"),
            buffers: &faces,
            format: Some(COLOR_FORMAT),
            blend: premul,
            samples: MSAA,
            cull: back,
            depth: depth(true),
        });
        let ground_pipeline = pipeline(Desc {
            label: "ground",
            layout: &scene_layout,
            vs: "vs_ground",
            fs: Some("fs_ground"),
            buffers: &[],
            format: Some(COLOR_FORMAT),
            blend: premul,
            samples: MSAA,
            cull: None,
            depth: depth(false),
        });
        let edge_pipeline = pipeline(Desc {
            label: "edges",
            layout: &scene_layout,
            vs: "vs_edge",
            fs: Some("fs_edge"),
            buffers: &lines,
            format: Some(COLOR_FORMAT),
            blend: premul,
            samples: MSAA,
            cull: None,
            depth: depth(true),
        });
        let shadow_pipeline = pipeline(Desc {
            label: "shadow",
            layout: &uniforms_layout,
            vs: "vs_shadow",
            fs: None,
            buffers: &faces,
            format: None,
            blend: None,
            samples: 1,
            cull: None,
            depth: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
        });
        let id_face_pipeline = pipeline(Desc {
            label: "id faces",
            layout: &scene_layout,
            vs: "vs_id_face",
            fs: Some("fs_id"),
            buffers: &faces,
            format: Some(ID_FORMAT),
            blend: None,
            samples: 1,
            cull: back,
            depth: depth(true),
        });
        let id_edge_pipeline = pipeline(Desc {
            label: "id edges",
            layout: &scene_layout,
            vs: "vs_id_edge",
            fs: Some("fs_id"),
            buffers: &lines,
            format: Some(ID_FORMAT),
            blend: None,
            samples: 1,
            cull: None,
            depth: depth(true),
        });
        let id_vertex_pipeline = pipeline(Desc {
            label: "id vertices",
            layout: &scene_layout,
            vs: "vs_id_vertex",
            fs: Some("fs_id"),
            buffers: &[Some(point_layout)],
            format: Some(ID_FORMAT),
            blend: None,
            samples: 1,
            cull: None,
            depth: depth(true),
        });
        let ao_pipelines = tier.has_ambient_occlusion().then(|| {
            let fullscreen = |label, fs, format| {
                pipeline(Desc {
                    label,
                    layout: &ao_layout,
                    vs: "vs_blit",
                    fs: Some(fs),
                    buffers: &[],
                    format: Some(format),
                    blend: None,
                    samples: 1,
                    cull: None,
                    depth: None,
                })
            };
            [
                pipeline(Desc {
                    label: "occlusion normals",
                    layout: &uniforms_layout,
                    vs: "vs_normals",
                    fs: Some("fs_normals"),
                    buffers: &faces,
                    format: Some(NORMAL_FORMAT),
                    blend: None,
                    samples: 1,
                    cull: back,
                    depth: depth(true),
                }),
                fullscreen("occlusion", "fs_ssao", AO_FORMAT),
                fullscreen("occlusion blur", "fs_ao_blur", AO_FORMAT),
            ]
        });

        let blit_fs = if target_format.is_srgb() { "fs_blit_linear" } else { "fs_blit_gamma" };
        let blit_pipeline = pipeline(Desc {
            label: "blit",
            layout: &blit_pipeline_layout,
            vs: "vs_blit",
            fs: Some(blit_fs),
            buffers: &[],
            format: Some(target_format),
            blend: None,
            samples: 1,
            cull: None,
            depth: None,
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shadow_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow map"),
                size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let env = environment::build();
        let env_view = upload_environment(device, queue, &env);
        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&env_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&env_sampler) },
            ],
        });
        let uniforms_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &layouts.uniforms,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let white = device
            .create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("white"),
                    size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: AO_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255],
            )
            .create_view(&Default::default());
        let pick_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick readback"),
            size: (PICK_ROW_TEXELS * ID_TEXEL * PICK_ROWS) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Self {
            tier,
            face_pipeline,
            ground_pipeline,
            edge_pipeline,
            shadow_pipeline,
            id_face_pipeline,
            id_edge_pipeline,
            id_vertex_pipeline,
            ao_pipelines,
            blit_pipeline,
            globals,
            globals_bind_group,
            uniforms_bind_group,
            layouts,
            sampler,
            shadow_view,
            sun: None,
            shadow_key: None,
            white,
            sh: env.irradiance,
            targets: None,
            bodies: HashMap::new(),
            mesh_version: u64::MAX,
            ghost: None,
            preview: None,
            pick_buffer,
            readback: Arc::new(Mutex::new(Readback::default())),
            display: DisplayMode::default(),
            frame: 0,
            assembly: AssemblyRenderer::new(device, COLOR_FORMAT, DEPTH_FORMAT, MSAA, ID_FORMAT),
        }
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// True when assemblies are culled on the GPU on this device.
    pub fn assembly_culls(&self) -> bool {
        self.assembly.culls()
    }

    /// What the last frame drew of the assembly.
    pub fn assembly_stats(&self) -> super::assembly::AssemblyStats {
        self.assembly.stats
    }

    /// The assembly definitions and levels the GPU wanted to draw and did not have
    /// uploaded: pass them to `AssemblyDocument::request_levels`.
    pub fn take_assembly_wanted(
        &mut self,
    ) -> Vec<(soliddraft_assembly::DefinitionIndex, soliddraft_tessellation::Level)> {
        self.assembly.take_wanted()
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }
        let tex = |label: &str, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let sampled = ra | wgpu::TextureUsages::TEXTURE_BINDING;
        let color_msaa = tex("color msaa", COLOR_FORMAT, MSAA, ra).create_view(&Default::default());
        let color = tex("color", COLOR_FORMAT, 1, sampled).create_view(&Default::default());
        let depth_msaa = tex("depth msaa", DEPTH_FORMAT, MSAA, ra).create_view(&Default::default());
        let id = tex("id", ID_FORMAT, 1, ra | wgpu::TextureUsages::COPY_SRC);
        let id_view = id.create_view(&Default::default());
        let id_depth = tex("id depth", DEPTH_FORMAT, 1, ra).create_view(&Default::default());
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layouts.blit,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&color) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let ao = self.ao_pipelines.is_some().then(|| {
            let depth = tex("occlusion depth", DEPTH_FORMAT, 1, sampled).create_view(&Default::default());
            let normals = tex("occlusion normals", NORMAL_FORMAT, 1, sampled).create_view(&Default::default());
            let raw = tex("occlusion raw", AO_FORMAT, 1, sampled).create_view(&Default::default());
            let blurred = tex("occlusion", AO_FORMAT, 1, sampled).create_view(&Default::default());
            let inputs = |label, second: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &self.layouts.ao_inputs,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(second) },
                    ],
                })
            };
            let sample_bind_group = inputs("occlusion inputs", &normals);
            let blur_bind_group = inputs("occlusion blur inputs", &raw);
            AoTargets { depth, normals, raw, sample_bind_group, blur_bind_group, blurred }
        });
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &self.layouts.frame,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(ao.as_ref().map_or(&self.white, |a| &a.blurred)),
            }],
        });
        self.targets = Some(Targets {
            size,
            color_msaa,
            color,
            depth_msaa,
            id,
            id_view,
            id_depth,
            blit_bind_group,
            ao,
            frame_bind_group,
        });
    }

    fn upload_body(&self, device: &wgpu::Device, mesh: &DisplayMesh, flags: u32) -> GpuBody {
        let table = mesh.appearances.len().min(MAX_APPEARANCES);
        if mesh.appearances.len() > MAX_APPEARANCES {
            log::warn!("a body shows {} appearances; the first {MAX_APPEARANCES} are drawn", mesh.appearances.len());
        }
        let verts: Vec<FaceVertex> = (0..mesh.positions.len())
            .map(|i| FaceVertex {
                position: mesh.positions[i].to_array(),
                normal: mesh.normals[i].to_array(),
                face_id: mesh.face_picks[i],
                feature_id: mesh.face_features.get(i).copied().unwrap_or(0),
                appearance: mesh.face_appearances.get(i).copied().filter(|a| (*a as usize) < table).unwrap_or(0),
            })
            .collect();
        let mut shades = [GpuShade::from(Appearance::default_part().shading()); MAX_APPEARANCES];
        for (slot, s) in shades.iter_mut().zip(&mesh.appearances) {
            *slot = (*s).into();
        }
        let mut lines: Vec<LineInstance> = Vec::new();
        for e in &mesh.edges {
            lines.extend(e.segments.as_chunks::<2>().0.iter().map(|s| LineInstance {
                a: s[0].to_array(),
                b: s[1].to_array(),
                id: e.pick,
            }));
        }
        let points: Vec<TaggedVertex> =
            mesh.corners.iter().map(|v| TaggedVertex { position: v.position.to_array(), id: v.pick }).collect();
        let non_empty = |v: &[u8]| if v.is_empty() { vec![0u8; 32] } else { v.to_vec() };
        let make = |label: &str, bytes: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &non_empty(bytes),
                usage,
            })
        };
        let uniform = make(
            "body",
            bytemuck::bytes_of(&BodyUniform { flags, _pad: [0; 3], tint: [0.0; 4], shades }),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("body"),
            layout: &self.layouts.body,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        GpuBody {
            vertices: make("faces", bytemuck::cast_slice(&verts), wgpu::BufferUsages::VERTEX),
            indices: make("indices", bytemuck::cast_slice(&mesh.indices), wgpu::BufferUsages::INDEX),
            index_count: mesh.indices.len() as u32,
            lines: make("edges", bytemuck::cast_slice(&lines), wgpu::BufferUsages::VERTEX),
            line_count: lines.len() as u32,
            points: make("points", bytemuck::cast_slice(&points), wgpu::BufferUsages::VERTEX),
            point_count: points.len() as u32,
            uniform,
            bind_group,
            bounds: mesh.bounds(),
        }
    }

    /// Draws a frame into the offscreen targets and reads back the pick window. The
    /// paint callback calls it; headless tests and benchmarks call it directly.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, input: &FrameInput) {
        self.frame += 1;
        log::debug!("prepare frame {} size {:?}", self.frame, input.size_px);
        self.display = input.display;
        self.ensure_targets(device, input.size_px);

        let (version, meshes) = &input.meshes;
        if *version != self.mesh_version || self.bodies.len() != meshes.len() {
            self.bodies = meshes.iter().map(|(id, m)| (*id, self.upload_body(device, m, 0))).collect();
            self.mesh_version = *version;
        }
        match &input.ghost {
            Some((g, removes)) => {
                let ptr = Arc::as_ptr(g) as usize;
                if self.ghost.as_ref().map(|(p, r, _)| (*p, *r)) != Some((ptr, *removes)) {
                    let flags = if *removes { 2 | 8 } else { 2 };
                    self.ghost = Some((ptr, *removes, self.upload_body(device, g, flags)));
                }
            }
            None => self.ghost = None,
        }
        match &input.preview {
            Some(p) => {
                let ptr = Arc::as_ptr(p) as usize;
                if self.preview.as_ref().map(|(q, _)| *q) != Some(ptr) {
                    self.preview = Some((ptr, self.upload_body(device, p, 4)));
                }
            }
            None => self.preview = None,
        }
        for (id, body) in &self.bodies {
            // Only the flags change per frame; the appearance table stays as uploaded.
            let flags: u32 = if input.selected_body == Some(*id) { 1 } else { 0 };
            queue.write_buffer(&body.uniform, 0, bytemuck::bytes_of(&flags));
        }

        let look = input.preset.look(self.tier);
        let show_faces = self.display != DisplayMode::Wireframe;
        let show_edges = self.display != DisplayMode::Shaded;
        let preview_key = self.preview.as_ref().map_or(0, |(p, _)| *p);
        // The preview stands in for the document bodies in the colour pass only.
        let shown: Vec<&GpuBody> = match self.preview.as_ref().map(|(_, b)| b) {
            Some(p) => vec![p],
            None => self.bodies.values().collect(),
        };
        let bounds = shown.iter().filter_map(|b| b.bounds).reduce(|(lo, hi), (l, h)| (lo.min(l), hi.max(h)));
        let shadows = look.shadows && show_faces && bounds.is_some();
        let draw_shadow_map = shadows && self.shadow_key != Some((self.mesh_version, preview_key));
        if draw_shadow_map && let Some((lo, hi)) = bounds {
            self.sun = Some(sun_frame(lo, hi));
        }
        let ao = look.ambient_occlusion > 0.0 && show_faces && self.ao_pipelines.is_some();
        let ground = shadows && self.sun.is_some_and(|s| input.eye.z > s.ground.z);

        let size = self.targets.as_ref().map(|t| t.size).unwrap_or([1, 1]);
        let view_proj = input.proj * input.view;
        let [right, up, back] = input.basis;
        let rig = |r: f32, u: f32, b: f32, intensity: f32| {
            let d = (right * r + up * u + back * b).normalize();
            [d.x, d.y, d.z, intensity * look.rig]
        };
        let sun = self.sun.unwrap_or(SunFrame {
            view_proj: Mat4::IDENTITY,
            ground: Vec3::ZERO,
            ground_radius: 1.0,
            radius: 1.0,
            texel: 1.0,
        });
        let toward_sun = environment::softbox_direction();
        let model_radius = bounds.map_or(100.0, |(lo, hi)| ((hi - lo).length() / 2.0).max(1.0));
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::bytes_of(&Globals {
                view_proj: view_proj.to_cols_array_2d(),
                view: input.view.to_cols_array_2d(),
                proj: input.proj.to_cols_array_2d(),
                inv_proj: input.proj.inverse().to_cols_array_2d(),
                sun_view_proj: sun.view_proj.to_cols_array_2d(),
                eye: [input.eye.x, input.eye.y, input.eye.z, if input.orthographic { 1.0 } else { 0.0 }],
                viewport: [size[0] as f32, size[1] as f32, EDGE_WIDTH * input.pixels_per_point, input.pixels_per_point],
                pull: [EDGE_PULL, EDGE_PULL * input.distance, model_radius * 0.06, model_radius * 0.002],
                key: rig(-0.45, 0.3, 0.85, 0.8),
                fill: rig(0.85, 0.1, 0.5, 0.3),
                rim: rig(0.2, 0.6, -0.75, 0.15),
                sun: [toward_sun.x, toward_sun.y, toward_sun.z, if shadows { look.sun } else { 0.0 }],
                ground: [sun.ground.x, sun.ground.y, sun.ground.z, sun.ground_radius],
                shadow: [
                    look.shadow_strength,
                    0.007 * look.shadow_softness,
                    if shadows { 1.0 } else { 0.0 },
                    sun.texel,
                ],
                look: [
                    look.environment,
                    look.reflections,
                    if ao { look.ambient_occlusion } else { 0.0 },
                    (environment::LEVELS - 1) as f32,
                ],
                sh: self.sh,
                hover_id: input.hover_id,
                selected_id: input.selected_id,
                selected_feature: input.selected_feature,
                _pad: 0,
            }),
        );

        // Pump map callbacks and publish a finished readback before issuing a new one.
        let _ = device.poll(wgpu::PollType::Poll);
        let (pending, ready, origin, serial) = {
            let rb = self.readback.lock().unwrap();
            (rb.pending, rb.ready, rb.origin, rb.serial)
        };
        if ready {
            {
                let view = self.pick_buffer.slice(..).get_mapped_range().expect("mapped pick buffer");
                let data: Vec<[u32; 2]> = bytemuck::cast_slice::<u8, [u32; 2]>(&view).to_vec();
                let mut out = input.pick_out.lock().unwrap();
                out.origin = origin;
                out.width = PICK_ROW_TEXELS;
                out.height = PICK_ROWS;
                out.data = data;
                out.serial = serial;
            }
            self.pick_buffer.unmap();
            let mut rb = self.readback.lock().unwrap();
            rb.ready = false;
            rb.pending = false;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        self.assembly.prepare(device, queue, &mut encoder, input.assembly.as_ref(), input.size_px, input.cursor_px);
        let targets = self.targets.as_ref().expect("targets");
        let draw_faces = |pass: &mut wgpu::RenderPass<'_>, bodies: &mut dyn Iterator<Item = &GpuBody>, groups: bool| {
            for body in bodies {
                if groups {
                    pass.set_bind_group(1, &body.bind_group, &[]);
                }
                pass.set_vertex_buffer(0, body.vertices.slice(..));
                pass.set_index_buffer(body.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..body.index_count, 0, 0..1);
            }
        };
        let draw_lines = |pass: &mut wgpu::RenderPass<'_>, bodies: &mut dyn Iterator<Item = &GpuBody>| {
            for body in bodies {
                if body.line_count > 0 {
                    pass.set_bind_group(1, &body.bind_group, &[]);
                    pass.set_vertex_buffer(0, body.lines.slice(..));
                    pass.draw(0..6, 0..body.line_count);
                }
            }
        };
        let depth_attachment = |view, clear| {
            Some(wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            })
        };
        let color_attachment = |view, store| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store },
            })
        };

        if draw_shadow_map {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: depth_attachment(&self.shadow_view, 1.0),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.uniforms_bind_group, &[]);
            draw_faces(&mut pass, &mut shown.iter().copied(), false);
            drop(pass);
            self.shadow_key = Some((self.mesh_version, preview_key));
        }

        if ao && let (Some([normals, sample, blur]), Some(t)) = (&self.ao_pipelines, &targets.ao) {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("occlusion normals"),
                    color_attachments: &[color_attachment(&t.normals, wgpu::StoreOp::Store)],
                    depth_stencil_attachment: depth_attachment(&t.depth, DEPTH_CLEAR),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(normals);
                pass.set_bind_group(0, &self.uniforms_bind_group, &[]);
                draw_faces(&mut pass, &mut shown.iter().copied(), false);
            }
            for (label, pipeline, inputs, out) in [
                ("occlusion", sample, &t.sample_bind_group, &t.raw),
                ("occlusion blur", blur, &t.blur_bind_group, &t.blurred),
            ] {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[color_attachment(out, wgpu::StoreOp::Store)],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.uniforms_bind_group, &[]);
                pass.set_bind_group(1, inputs, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.color_msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.color),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_msaa,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(2, &targets.frame_bind_group, &[]);
            let ghost = self.ghost.as_ref().map(|(_, _, b)| b);
            if show_faces {
                pass.set_pipeline(&self.face_pipeline);
                draw_faces(&mut pass, &mut shown.iter().copied(), true);
                self.assembly.draw(&mut pass);
                // The assembly binds its own group 0.
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
            }
            if ground {
                // The ground darkens behind the opaque faces and in front of the background.
                pass.set_pipeline(&self.ground_pipeline);
                pass.draw(0..6, 0..1);
            }
            if show_faces && let Some(g) = ghost {
                pass.set_pipeline(&self.face_pipeline);
                draw_faces(&mut pass, &mut std::iter::once(g), true);
            }
            if show_edges || ghost.is_some() {
                pass.set_pipeline(&self.edge_pipeline);
                if show_edges {
                    draw_lines(&mut pass, &mut shown.iter().copied().chain(ghost));
                } else {
                    draw_lines(&mut pass, &mut ghost.into_iter());
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.id_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(2, &targets.frame_bind_group, &[]);
            pass.set_pipeline(&self.id_face_pipeline);
            draw_faces(&mut pass, &mut self.bodies.values(), true);
            pass.set_pipeline(&self.id_edge_pipeline);
            draw_lines(&mut pass, &mut self.bodies.values());
            pass.set_pipeline(&self.id_vertex_pipeline);
            for body in self.bodies.values() {
                if body.point_count > 0 {
                    pass.set_bind_group(1, &body.bind_group, &[]);
                    pass.set_vertex_buffer(0, body.points.slice(..));
                    pass.draw(0..6, 0..body.point_count);
                }
            }
            self.assembly.draw_ids(&mut pass);
        }

        let mut issued = None;
        if let (Some(cursor), false) = (input.cursor_px, pending || ready) {
            let w = PICK_ROW_TEXELS.min(size[0]);
            let h = PICK_ROWS.min(size[1]);
            let x = cursor[0].saturating_sub(w / 2).min(size[0] - w);
            let y = cursor[1].saturating_sub(h / 2).min(size[1] - h);
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &targets.id,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.pick_buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(PICK_ROW_TEXELS * ID_TEXEL),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            issued = Some([x, y]);
        }
        queue.submit([encoder.finish()]);
        self.assembly.after_submit();
        if let Some(origin) = issued {
            {
                let mut rb = self.readback.lock().unwrap();
                rb.pending = true;
                rb.ready = false;
                rb.origin = origin;
                rb.serial = self.frame;
            }
            let state = self.readback.clone();
            self.pick_buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let mut rb = state.lock().unwrap();
                if result.is_ok() {
                    rb.ready = true;
                } else {
                    rb.pending = false;
                }
            });
        }
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>) {
        log::debug!("paint");
        let Some(targets) = &self.targets else { return };
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &targets.blit_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Uploads the baked environment as a cube map with one mip level per roughness step.
fn upload_environment(device: &wgpu::Device, queue: &wgpu::Queue, env: &environment::Environment) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("environment"),
        size: wgpu::Extent3d { width: environment::SIZE, height: environment::SIZE, depth_or_array_layers: 6 },
        mip_level_count: environment::LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, texels) in env.levels.iter().enumerate() {
        let size = environment::SIZE >> level;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(texels),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 8), rows_per_image: Some(size) },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("environment"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    fn project(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * Vec4::new(p.x, p.y, p.z, 1.0);
        c.truncate() / c.w
    }

    #[test]
    fn the_shadow_map_covers_the_model_and_the_ground_disc() {
        let (lo, hi) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(120.0, 90.0, 60.0));
        let sun = sun_frame(lo, hi);
        let center = (lo + hi) / 2.0;
        let c = project(sun.view_proj, center);
        assert!(c.x.abs() < 1e-4 && c.y.abs() < 1e-4);
        for corner in [lo, hi, Vec3::new(lo.x, hi.y, lo.z), Vec3::new(hi.x, lo.y, hi.z)] {
            let p = project(sun.view_proj, corner);
            assert!(p.x.abs() <= 1.0 && p.y.abs() <= 1.0 && (0.0..=1.0).contains(&p.z), "{corner} -> {p}");
        }
        // Every point of the ground disc lies within the map's depth range.
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let g = sun.ground + Vec3::new(a.cos(), a.sin(), 0.0) * sun.ground_radius;
            let z = project(sun.view_proj, g).z;
            assert!((0.0..=1.0).contains(&z), "ground {g} depth {z}");
        }
        assert!(sun.ground.z < lo.z);
        // The light is above the model: the top of the model is nearer to it than the bottom.
        assert!(
            project(sun.view_proj, Vec3::new(60.0, 45.0, 60.0)).z
                < project(sun.view_proj, Vec3::new(60.0, 45.0, 0.0)).z
        );
    }

    /// What wgpu's WebGL2 backend does with each entry point the basic tier creates a
    /// pipeline for: translate it to GLSL ES 3.00. A shader feature WebGL2 lacks fails
    /// here instead of in a browser.
    #[test]
    fn the_basic_tier_translates_to_webgl2() {
        use naga::back::glsl;
        let module = naga::front::wgsl::parse_str(include_str!("scene.wgsl")).expect("scene.wgsl parses");
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .expect("scene.wgsl validates");
        let options =
            glsl::Options { version: glsl::Version::Embedded { version: 300, is_webgl: true }, ..Default::default() };
        let vertex =
            ["vs_face", "vs_ground", "vs_edge", "vs_shadow", "vs_id_face", "vs_id_edge", "vs_id_vertex", "vs_blit"];
        let fragment = ["fs_face", "fs_ground", "fs_edge", "fs_id", "fs_blit_linear", "fs_blit_gamma"];
        let stages = vertex
            .iter()
            .map(|e| (naga::ShaderStage::Vertex, e))
            .chain(fragment.iter().map(|e| (naga::ShaderStage::Fragment, e)));
        for (stage, entry) in stages {
            let pipeline =
                glsl::PipelineOptions { shader_stage: stage, entry_point: entry.to_string(), multiview: None };
            let mut out = String::new();
            glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, Default::default())
                .and_then(|mut w| w.write())
                .unwrap_or_else(|e| panic!("{entry} does not translate to GLSL ES 3.00: {e}"));
            assert!(out.starts_with("#version 300 es"), "{entry}");
        }
    }

    #[test]
    fn globals_match_the_shader_layout() {
        // Five matrices, ten vectors, nine harmonics and four ids.
        assert_eq!(std::mem::size_of::<Globals>(), 5 * 64 + 10 * 16 + 9 * 16 + 16);
        assert_eq!(std::mem::size_of::<LineInstance>(), 28);
        assert_eq!(std::mem::size_of::<BodyUniform>(), 32 + 48 * MAX_APPEARANCES);
    }
}
