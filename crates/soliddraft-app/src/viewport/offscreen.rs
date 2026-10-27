//! The viewport drawn into a texture instead of a window, with the same renderer,
//! shaders and camera: the part thumbnail a save stores, and `soliddraft screenshot`.

use std::error::Error;
use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;

use super::camera::Camera;
use super::look::Tier;
use super::render::{FrameInput, ViewportRenderer};
use crate::document::Document;

/// The size of the thumbnail stored in a part file, in pixels.
pub const THUMBNAIL: [u32; 2] = [512, 384];

/// The texture format pictures are drawn in.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A viewport renderer of its own, for pictures drawn without a window. Making one
/// compiles the shaders, so the app keeps it between saves.
pub struct Offscreen {
    renderer: ViewportRenderer,
    /// Bumped on every picture, so the renderer uploads the meshes of whichever
    /// document it is given: two documents can have the same mesh version.
    uploads: u64,
}

/// An RGBA picture, row by row from the top.
pub struct Picture {
    pub size: [u32; 2],
    pub pixels: Vec<u8>,
}

impl Picture {
    /// The picture as a PNG file's bytes.
    pub fn png(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        let [width, height] = self.size;
        let image = image::RgbaImage::from_raw(width, height, self.pixels.clone()).ok_or("the picture is cut short")?;
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png)?;
        Ok(png.into_inner())
    }
}

impl Offscreen {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, tier: Tier) -> Offscreen {
        Offscreen { renderer: ViewportRenderer::new(device, queue, FORMAT, tier), uploads: 0 }
    }

    /// Draws `doc` as a new viewport first shows it, fitted to its bounds from the
    /// default view, and reads the pixels back.
    pub fn capture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        size: [u32; 2],
    ) -> Result<Picture, Box<dyn Error>> {
        let [width, height] = size;
        let mut camera = Camera::default();
        camera.fit(doc.bounds());
        camera.tick(10.0);
        self.uploads += 1;
        let input = FrameInput {
            view: camera.view(),
            proj: camera.projection(width as f32 / height as f32),
            eye: camera.eye(),
            basis: [camera.right(), camera.up(), camera.direction()],
            orthographic: camera.orthographic,
            distance: camera.distance,
            size_px: [width, height],
            pixels_per_point: 1.0,
            display: Default::default(),
            preset: Default::default(),
            // Pictures are kept in files and shown in previews, so they take the
            // light backdrop whatever mode the window is in.
            backdrop: [crate::theme::LIGHT.viewport_top, crate::theme::LIGHT.viewport_bottom],
            hover_id: 0,
            selected_id: 0,
            selected_feature: 0,
            selected_body: None,
            cursor_px: None,
            meshes: (self.uploads, doc.meshes.iter().map(|(b, m)| (*b, m.clone())).collect()),
            ghost: None,
            preview: doc.provisional_mesh(),
            preview_pickable: false,
            pick_revision: None,
            pick_out: Arc::new(Mutex::new(Default::default())),
            assembly: None,
        };
        self.renderer.prepare(device, queue, &input);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless viewport"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headless pixels"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("viewport blit"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.paint(&mut pass);
        }
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let (send, recv) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = send.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely())?;
        recv.recv()??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let pixels = mapped.chunks(row as usize).flat_map(|r| r[..(width * 4) as usize].iter().copied()).collect();
        drop(mapped);
        buffer.unmap();
        Ok(Picture { size, pixels })
    }
}
