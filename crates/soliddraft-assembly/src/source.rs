//! Where display meshes and exact bodies come from when an assembly's own copies do
//! not have them.
//!
//! An assembly normally shows its parts from the display copies in its file. A copy can
//! be missing (the cache was dropped, or a Rebuild needs the current revision of a
//! part), and exact work (measure, mass, drawing projection) needs the kernel body. Both
//! start from a part file's bytes; a [`DisplaySource`] turns them into a display mesh.
//!
//! - [`FileSource`] reads the display data the part file keeps for its current geometry
//!   (see `soliddraft_model::display`). It needs no kernel, so a browser or a lightweight
//!   open shows parts without rebuilding them.
//! - [`KernelSource`] reads the same display data first, and otherwise rebuilds the part
//!   on a kernel and tessellates it. It also builds the exact part.

use std::sync::Arc;

use soliddraft_kernel_api::Kernel;
use soliddraft_model::{Document, display};
use soliddraft_tessellation::{Level, Quality};

pub use soliddraft_tessellation::mesh::display_mesh;

use crate::mesh::DisplayMesh;

/// Makes display meshes from part files.
pub trait DisplaySource: Send + Sync {
    /// The display mesh of the part in `part_file` (the bytes of a `.dwprt`), with
    /// what made it: kernel, version, level, parameters and geometry hash.
    fn part_mesh(&self, part_file: &[u8]) -> Result<(DisplayMesh, String), String>;

    /// The display mesh of the part at `quality` instead of its own. A source that
    /// cannot make one shows the part at its own quality, which is the default.
    fn part_mesh_at(&self, part_file: &[u8], quality: &Quality) -> Result<(DisplayMesh, String), String> {
        let _ = quality;
        self.part_mesh(part_file)
    }

    /// The display mesh of the part at `level` of `quality` (`None` for its own). By
    /// default the standard level as above, and the coarse level of the part's own
    /// quality from the display data its file keeps; anything else needs a kernel.
    fn part_mesh_level(
        &self,
        part_file: &[u8],
        quality: Option<&Quality>,
        level: Level,
    ) -> Result<(DisplayMesh, String), String> {
        match (level, quality) {
            (Level::Standard, None) => self.part_mesh(part_file),
            (Level::Standard, Some(q)) => self.part_mesh_at(part_file, q),
            (Level::Coarse, None) => display::read(part_file, Level::Coarse)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "the part file has no coarse display data for its current shape".into()),
            _ => Err(format!("the {level} level needs a kernel")),
        }
    }

    /// The kernel source behind this one, for work that needs exact parts.
    fn kernel_source(&self) -> Option<&KernelSource> {
        None
    }
}

/// Display meshes from the display data part files keep, without a kernel.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileSource;

impl DisplaySource for FileSource {
    fn part_mesh(&self, part_file: &[u8]) -> Result<(DisplayMesh, String), String> {
        display::read(part_file, Level::Standard).map_err(|e| e.to_string())?.ok_or_else(|| {
            "the part file has no display data for its current shape; open it with a kernel to make some".into()
        })
    }
}

/// Display meshes and exact parts from a kernel.
pub struct KernelSource {
    kernel: Arc<dyn Kernel>,
    quality: Option<Quality>,
}

impl KernelSource {
    /// A source on `kernel` that shows each part at the quality saved in it.
    pub fn new(kernel: Arc<dyn Kernel>) -> KernelSource {
        KernelSource { kernel, quality: None }
    }

    /// A source that shows every part at `quality` instead of its own.
    pub fn with_quality(kernel: Arc<dyn Kernel>, quality: Quality) -> KernelSource {
        KernelSource { kernel, quality: Some(quality) }
    }

    /// The kernel.
    pub fn kernel(&self) -> &Arc<dyn Kernel> {
        &self.kernel
    }

    /// The part in `part_file`, rebuilt: the exact body that measure, mass and drawing
    /// projection need.
    pub fn exact(&self, part_file: &[u8]) -> Result<Document, String> {
        Document::from_bytes(part_file, self.kernel.clone()).map_err(|e| e.to_string())
    }
}

impl KernelSource {
    /// The part rebuilt and tessellated at `level` of `quality`, or of its own.
    fn rebuilt(
        &self,
        part_file: &[u8],
        quality: Option<&Quality>,
        level: Level,
    ) -> Result<(DisplayMesh, String), String> {
        let doc = self.exact(part_file)?;
        let quality = quality.copied().unwrap_or(doc.settings().viewport);
        let mut data =
            doc.display_data(&quality, &[level]).ok_or("the part has no body")?.map_err(|e| e.to_string())?;
        let d = data.pop().ok_or("the kernel made no mesh")?;
        Ok((d.mesh, d.made_by))
    }
}

impl DisplaySource for KernelSource {
    fn kernel_source(&self) -> Option<&KernelSource> {
        Some(self)
    }

    /// The part file's own standard level when it has one for its current shape and no
    /// other quality was asked for; otherwise the part rebuilt and tessellated.
    fn part_mesh(&self, part_file: &[u8]) -> Result<(DisplayMesh, String), String> {
        if self.quality.is_none()
            && let Ok(Some(found)) = display::read(part_file, Level::Standard)
        {
            return Ok(found);
        }
        self.rebuilt(part_file, self.quality.as_ref(), Level::Standard)
    }

    fn part_mesh_at(&self, part_file: &[u8], quality: &Quality) -> Result<(DisplayMesh, String), String> {
        self.rebuilt(part_file, Some(quality), Level::Standard)
    }

    /// The part file's own display data for the coarse and standard levels of its own
    /// quality; otherwise the part rebuilt and tessellated at that level.
    fn part_mesh_level(
        &self,
        part_file: &[u8],
        quality: Option<&Quality>,
        level: Level,
    ) -> Result<(DisplayMesh, String), String> {
        let quality = quality.or(self.quality.as_ref());
        if quality.is_none()
            && level != Level::Fine
            && let Ok(Some(found)) = display::read(part_file, level)
        {
            return Ok(found);
        }
        self.rebuilt(part_file, quality, level)
    }
}
