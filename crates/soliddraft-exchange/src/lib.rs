//! Part export: STEP AP242 from the kernel's exact body, STL, 3MF and OBJ from its meshes.
//!
//! Every export returns the file and a report of how far the file may lie from the exact
//! body. A STEP report carries the kernel's proven deviation. A mesh report carries the
//! chord height the mesh was made at, which the kernel guarantees after rounding to the
//! mesh's 32-bit coordinates, and every requested control the kernel could not guarantee.
//!
//! See `docs/exchange.md`.

pub mod mesh;
mod obj;
mod stl;
mod three_mf;

use std::path::Path;

use serde::Serialize;
use soliddraft_kernel_api::{BodyId, Kernel, KernelError, StepOptions, StepReport};
use soliddraft_tessellation::ExportTolerance;

pub use mesh::Welded;

/// A file format SolidDraft exports parts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// STEP AP242, exact geometry.
    Step,
    /// Binary STL, triangles.
    Stl,
    /// 3MF core, one closed mesh in millimetres.
    ThreeMf,
    /// Wavefront OBJ, triangles with face groups and vertex normals.
    Obj,
}

impl Format {
    /// Every format, in menu order.
    pub const ALL: [Format; 4] = [Format::Step, Format::Stl, Format::ThreeMf, Format::Obj];

    /// The file extension written, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Step => "step",
            Format::Stl => "stl",
            Format::ThreeMf => "3mf",
            Format::Obj => "obj",
        }
    }

    /// The format's name for people.
    pub fn name(self) -> &'static str {
        match self {
            Format::Step => "STEP AP242",
            Format::Stl => "STL",
            Format::ThreeMf => "3MF",
            Format::Obj => "OBJ",
        }
    }

    /// The format a path's extension names, ignoring case. `.stp` is STEP too.
    pub fn of_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "step" | "stp" => Some(Format::Step),
            "stl" => Some(Format::Stl),
            "3mf" => Some(Format::ThreeMf),
            "obj" => Some(Format::Obj),
            _ => None,
        }
    }
}

/// How far an exported mesh may lie from the exact body.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeshReport {
    /// Triangles written.
    pub triangles: usize,
    /// Vertices in the indexed mesh, shared across faces except where OBJ keeps normals separate.
    pub vertices: usize,
    /// The kernel's bound, in millimetres, on the distance between the mesh and the exact
    /// surface, after rounding to the file's 32-bit coordinates.
    pub chord_height: f64,
    /// True when every edge joins exactly two triangles that run it in opposite
    /// directions, so the mesh bounds a volume.
    pub closed: bool,
    /// Requested controls the kernel could not guarantee, in words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unavailable_controls: Vec<String>,
}

/// What an export wrote, by kind.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Report {
    /// Exact geometry, with the kernel's proven deviation.
    Step(StepReport),
    /// A mesh, with its chord height.
    Mesh(MeshReport),
}

impl Report {
    /// One line for a status bar or a terminal.
    pub fn summary(&self) -> String {
        match self {
            Report::Step(r) => {
                let mut s = format!(
                    "{} faces, {} edges; within {} mm of the exact part (proven)",
                    r.faces, r.edges, r.max_deviation_mm
                );
                if !r.within_tolerance {
                    s += "; over the requested tolerance";
                }
                s
            }
            Report::Mesh(r) => {
                let mut s = format!("{} triangles; within {} mm of the exact part", r.triangles, r.chord_height);
                if !r.closed {
                    s += "; the mesh is not closed";
                }
                s
            }
        }
    }
}

/// An exported file and its report.
#[derive(Clone, Debug)]
pub struct Export {
    /// The file's bytes.
    pub bytes: Vec<u8>,
    /// What was written.
    pub report: Report,
}

/// Why an export failed. A failed export writes nothing.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The kernel refused the export or its mesh.
    #[error(transparent)]
    Kernel(#[from] KernelError),
    /// The export settings are out of range.
    #[error("{0}")]
    Invalid(String),
    /// 3MF needs a closed mesh and the kernel's mesh is not.
    #[error("3MF needs a closed mesh; this one has open or doubled edges")]
    NotClosed,
    /// Writing the package failed.
    #[error("cannot write the package: {0}")]
    Package(String),
}

/// Exports `body` as `format`. `name` is the part's name, written into the file; meshes use
/// `tolerance`, the document's export setting.
pub fn export(
    kernel: &dyn Kernel,
    body: BodyId,
    name: &str,
    format: Format,
    tolerance: &ExportTolerance,
) -> Result<Export, ExportError> {
    match format {
        Format::Step => {
            let options = StepOptions { product_name: name.to_owned(), ..StepOptions::default() };
            let step = kernel.export_step(body, &options)?;
            Ok(Export { bytes: step.text.into_bytes(), report: Report::Step(step.report) })
        }
        Format::Stl | Format::ThreeMf | Format::Obj => {
            tolerance.check().map_err(ExportError::Invalid)?;
            let mesh = kernel.tessellate(body, &tolerance.params())?;
            let welded = mesh::weld(&mesh);
            let closed = welded.is_closed();
            let bytes = match format {
                Format::Obj => obj::write(&mesh, name, tolerance.chord_height),
                Format::Stl => stl::write(&welded, name),
                Format::ThreeMf if closed => three_mf::write(&welded, name)?,
                Format::ThreeMf => return Err(ExportError::NotClosed),
                Format::Step => unreachable!(),
            };
            let report = MeshReport {
                triangles: welded.triangles.len(),
                vertices: if format == Format::Obj { mesh.positions.len() } else { welded.positions.len() },
                chord_height: tolerance.chord_height,
                closed,
                unavailable_controls: mesh.unavailable_controls,
            };
            Ok(Export { bytes, report: Report::Mesh(report) })
        }
    }
}
