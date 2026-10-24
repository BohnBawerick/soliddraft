//! A windowless frame from the viewport's renderer, shaders, camera and part meshes.

use std::{path::Path, process::ExitCode, sync::Arc};

use egui_wgpu::wgpu;
use serde_json::json;
use soliddraft_app::{
    document::Document,
    viewport::{look::Tier, offscreen::Offscreen},
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
    let kernel = Arc::new(CatallaxKernel::with_operation_log());
    let doc = Document::from_model(soliddraft_model::Document::open(part, kernel.clone())?);
    let mut renderer = Renderer::new().await?;
    renderer.capture(&doc, Path::new(png))?;
    let mut metadata = renderer.metadata.clone();
    metadata["kernel"] = serde_json::to_value(kernel.backend())?;
    metadata["diagnostics"] = kernel.take_diagnostics()?;
    std::fs::write(Path::new(png).with_extension("json"), serde_json::to_vec_pretty(&metadata)?)?;
    println!("{metadata}");
    Ok(())
}

/// A retained GPU for bundle-only captures of the live MCP document.
pub(crate) struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    offscreen: Offscreen,
    metadata: serde_json::Value,
}

impl Renderer {
    pub(crate) async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await?;
        let info = adapter.get_info();
        let desc = wgpu::DeviceDescriptor {
            required_features: adapter.features() & wgpu::Features::INDIRECT_FIRST_INSTANCE,
            ..Default::default()
        };
        let (device, queue) = adapter.request_device(&desc).await?;
        let offscreen = Offscreen::new(&device, &queue, Tier::of(info.backend));
        let metadata = json!({"renderer":"soliddraft_app::viewport::render::ViewportRenderer",
            "adapter":info.name,"backend":format!("{:?}",info.backend),"size":[960,720]});
        Ok(Self { device, queue, offscreen, metadata })
    }

    pub(crate) fn capture(&mut self, doc: &Document, png: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if doc.display_body().is_none() || doc.meshes.is_empty() {
            return Err("part has no displayed solid".into());
        }
        let picture = self.offscreen.capture(&self.device, &self.queue, doc, [960, 720])?;
        image::save_buffer(png, &picture.pixels, 960, 720, image::ColorType::Rgba8)?;
        std::fs::write(png.with_extension("json"), serde_json::to_vec_pretty(&self.metadata)?)?;
        Ok(())
    }
}
