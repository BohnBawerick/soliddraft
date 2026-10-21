//! What a quality setting gives one body: each level's deviation and triangle count.

use serde::Serialize;
use soliddraft_kernel_api::{BodyId, Kernel, KernelResult, TessellationParams};

use crate::level::Level;
use crate::policy::Levels;
use crate::quality::Quality;

/// One level of a [`Report`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelReport {
    /// The level.
    pub level: Level,
    /// The largest distance from the mesh to the exact surface, in millimetres, when a
    /// chord height bounds it.
    pub deviation_mm: Option<f64>,
    /// Triangles in the mesh.
    pub triangles: usize,
    /// What the kernel was asked for.
    pub params: TessellationParams,
}

/// What a quality setting gives one body.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// The setting.
    pub quality: Quality,
    /// The body's bounding diagonal, in millimetres, which the automatic setting
    /// scales with.
    pub diagonal_mm: f64,
    /// Every level, coarsest first.
    pub levels: Vec<LevelReport>,
}

impl Report {
    /// The entry for `level`.
    pub fn level(&self, level: Level) -> &LevelReport {
        &self.levels[level.index()]
    }
}

/// Tessellates `body` at every level of `quality`, in one kernel call, and reports
/// each level's deviation and triangle count.
pub fn report(kernel: &dyn Kernel, body: BodyId, quality: &Quality) -> KernelResult<Report> {
    let diagonal_mm = kernel.topology(body)?.bbox.diagonal();
    let levels = Levels::new(quality, diagonal_mm);
    let meshes = kernel.tessellate_levels(body, levels.all())?;
    Ok(Report {
        quality: *quality,
        diagonal_mm,
        levels: Level::ALL
            .into_iter()
            .zip(meshes)
            .map(|(level, mesh)| LevelReport {
                level,
                deviation_mm: levels.deviation(level),
                triangles: mesh.triangle_count(),
                params: *levels.params(level),
            })
            .collect(),
    })
}
