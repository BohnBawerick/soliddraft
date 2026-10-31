//! SolidDraft: a parametric CAD application in Rust. Phase 3: the assistant.
//!
//! The crate is a library so the integration tests can drive the real app
//! through `egui_kittest`; `main.rs` is the thin binary.

// The interface modules are public only so the integration tests can reach them.
#![allow(missing_docs)]

pub mod app;
pub mod app_tools;
pub mod assembly;
pub mod chat;
pub mod command_line;
#[macro_use]
pub mod command_tools;
pub mod commands;
pub mod dimension_edit;
pub mod display_quality;
pub mod document;
pub mod drag;
pub mod drawing;
pub mod file;
pub mod fillet_collect;
mod grid;
pub mod icons;
pub mod import;
pub mod journal;
#[cfg(target_arch = "wasm32")]
pub mod journal_browser;
pub mod locations;
pub mod mates;
pub mod measure;
pub mod navigation;
pub mod palette;
pub mod panels;
pub mod platform;
pub mod preferences;
pub mod properties;
pub mod recovery;
pub mod ribbon;
pub mod settings;
pub mod sketch_tools;
pub mod sketcher;
pub mod state;
pub mod subassembly;
pub mod theme;
pub mod tree;
pub mod tree_visibility;
pub mod viewport;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// The sketch model, solver and profiles: the plumbing crate the sketcher draws on.
pub use soliddraft_sketch as sketch;

use document::{Command, Document, ExtrudeParams, PlaneKind, ProfileKind};
use soliddraft_kernel_api::Kernel;
use soliddraft_tessellation::{Advanced, Quality};
use std::sync::Arc;

/// The Catallax backend used by both the desktop and browser application.
pub fn kernel() -> Arc<dyn Kernel> {
    Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new())
}

/// Hands the drawing library the sheet templates, sheet formats and drafting styles
/// File locations lists, so they join the built-in ones. Call once at startup, before
/// the first drawing.
pub fn load_reference_files() {
    use soliddraft_document::LocationKind;
    let locations = settings::file_locations();
    let mut templates = locations.files(LocationKind::DrawingTemplates);
    templates.extend(locations.files(LocationKind::SheetFormats));
    soliddraft_drawing::library::use_files(templates, locations.files(LocationKind::DraftingStyles));
    for problem in &soliddraft_drawing::library::library().problems {
        log::warn!("File locations: {problem}");
    }
}

/// An empty part.
pub fn new_document() -> Document {
    Document::new("Part1", kernel())
}

/// The sample part used by `--demo` and the screenshots: a base plate, a flange and a boss.
pub fn demo_document() -> Document {
    let mut doc = new_document();
    populate_demo(&mut doc);
    doc
}

/// Adds the sample features to a document, retaining its Catallax diagnostics.
pub fn populate_demo(doc: &mut Document) {
    for params in [
        ExtrudeParams {
            sketch: None,
            profile: ProfileKind::Rectangle { width: 120.0, height: 90.0 },
            plane: PlaneKind::Top,
            depth: 10.0,
            flip: false,
            end: Default::default(),
        },
        ExtrudeParams {
            sketch: None,
            profile: ProfileKind::Rectangle { width: 120.0, height: 60.0 },
            plane: PlaneKind::Front,
            depth: 10.0,
            flip: true,
            end: Default::default(),
        },
        ExtrudeParams {
            sketch: None,
            profile: ProfileKind::Circle { diameter: 24.0 },
            plane: PlaneKind::Top,
            depth: 24.0,
            flip: false,
            end: Default::default(),
        },
    ] {
        doc.apply(Command::AddExtrude { params, via_assistant: false }).ok();
    }
    doc.selection = None;
    doc.dirty = false;
}

/// One body tessellated to well over 10,000 triangles, for the frame-time benchmark.
pub fn bench_document() -> Document {
    let mut doc = new_document();
    let edges = Advanced { chord_height: 0.0, max_edge_length: 2.0, ..Advanced::default() };
    doc.set_viewport_quality(Quality::Custom(edges)).expect("valid advanced values");
    doc.apply(Command::AddExtrude {
        params: ExtrudeParams {
            sketch: None,
            profile: ProfileKind::Rectangle { width: 100.0, height: 60.0 },
            plane: PlaneKind::Top,
            depth: 40.0,
            flip: false,
            end: Default::default(),
        },
        via_assistant: false,
    })
    .ok();
    doc.selection = None;
    doc
}
