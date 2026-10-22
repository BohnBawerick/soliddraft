//! A windowless frame from the viewport's renderer, shaders, camera and part meshes.

use std::{
    path::Path,
    process::ExitCode,
    sync::{Arc, Mutex},
};

use egui_wgpu::wgpu;
use serde_json::json;
use soliddraft_app::{
    document::Document,
    viewport::{
        camera::Camera,
        look::Tier,
        render::{FrameInput, ViewportRenderer},
    },
};
use soliddraft_kernel_api::Kernel;
use soliddraft_kernel_catallax::CatallaxKernel;

pub(crate) fn run(args: &[String]) -> ExitCode {
    if args == ["--help"] || args == ["-h"] {
        println!("usage: soliddraft screenshot <part.dwprt> --png <out.png>");
        return ExitCode::SUCCESS;
    }
    let [part, flag, png] = args else {
        eprintln!("usage: soliddraft screenshot <part.dwprt> --png <out.png>");
        return ExitCode::from(2);
    };
    if flag != "--png" {
        return ExitCode::from(2);
    }
    match pollster::block_on(capture(part, png)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("screenshot: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn capture(part: &str, png: &str) -> Result<(), Box<dyn std::error::Error>> {
    let kernel = Arc::new(CatallaxKernel::with_diagnostics(100_000_000));
    let doc = Document::from_model(soliddraft_model::Document::open(part, kernel.clone())?);
    if doc.display_body().is_none() || doc.meshes.is_empty() {
        return Err("part has no displayed solid".into());
    }
    let instance = wgpu::Instance::default();
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await?;
    let info = adapter.get_info();
    let desc = wgpu::DeviceDescriptor {
        required_features: adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE,
        ..Default::default()
    };
    let (device, queue) = adapter.request_device(&desc).await?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = ViewportRenderer::new(&device, &queue, format, Tier::of(info.backend));
    let [width, height] = [960, 720];
    let mut camera = Camera::default();
    camera.fit(doc.bounds());
    camera.tick(10.0);
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
        hover_id: 0,
        selected_id: 0,
        selected_feature: 0,
        selected_body: None,
        cursor_px: None,
        meshes: (doc.mesh_version, doc.meshes.iter().map(|(b, m)| (*b, m.clone())).collect()),
        ghost: None,
        preview: None,
        pick_out: Arc::new(Mutex::new(Default::default())),
        assembly: None,
    };
    renderer.prepare(&device, &queue, &input);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless viewport"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
        renderer.paint(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
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
    let mapped = buffer.slice(..).get_mapped_range();
    let pixels: Vec<u8> = mapped.chunks(row as usize).flat_map(|r| r[..(width * 4) as usize].iter().copied()).collect();
    image::save_buffer(png, &pixels, width, height, image::ColorType::Rgba8)?;
    drop(mapped);
    buffer.unmap();
    let metadata = json!({"renderer":"soliddraft_app::viewport::render::ViewportRenderer",
        "adapter":info.name,"backend":format!("{:?}",info.backend),"size":[width,height],
        "kernel":kernel.backend(),"diagnostics":kernel.take_diagnostics()?});
    std::fs::write(Path::new(png).with_extension("json"), serde_json::to_vec_pretty(&metadata)?)?;
    println!("{metadata}");
    Ok(())
}
