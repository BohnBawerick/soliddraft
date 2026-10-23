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
    let [width, height] = [960, 720];
    let picture =
        Offscreen::new(&device, &queue, Tier::of(info.backend)).capture(&device, &queue, &doc, [width, height])?;
    image::save_buffer(png, &picture.pixels, width, height, image::ColorType::Rgba8)?;
    let metadata = json!({"renderer":"soliddraft_app::viewport::render::ViewportRenderer",
        "adapter":info.name,"backend":format!("{:?}",info.backend),"size":[width,height],
        "kernel":kernel.backend(),"diagnostics":kernel.take_diagnostics()?});
    std::fs::write(Path::new(png).with_extension("json"), serde_json::to_vec_pretty(&metadata)?)?;
    println!("{metadata}");
    Ok(())
}
