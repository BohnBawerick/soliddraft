//! Display meshes: what an assembly draws for a part without its kernel body.
//!
//! The type and its binary form (`display.bin` in `cache/refs`) live in
//! `soliddraft_tessellation::mesh`, because part files keep the same meshes for their own
//! display data. This module adds what only an assembly needs: the box around a mesh.

pub use soliddraft_tessellation::mesh::{DISPLAY_MESH, DisplayMesh, MeshFace, SharedMesh};

use glam::Vec3;

use crate::placement::Aabb;

/// The box around a display mesh's vertices, in the part's own frame.
pub trait MeshBounds {
    /// The box.
    fn bounds(&self) -> Aabb;
}

impl MeshBounds for DisplayMesh {
    fn bounds(&self) -> Aabb {
        Aabb::of_points(self.positions.iter().map(|p| Vec3::from_array(*p).as_dvec3()))
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;

    #[test]
    fn the_box_holds_every_vertex() {
        let m = DisplayMesh::cuboid(DVec3::ZERO, DVec3::new(10.0, 4.0, 2.0));
        assert_eq!(m.bounds().min, DVec3::ZERO);
        assert_eq!(m.bounds().max, DVec3::new(10.0, 4.0, 2.0));
    }
}
