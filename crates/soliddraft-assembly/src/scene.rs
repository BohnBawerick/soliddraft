//! What the renderer draws for an open assembly, packed for the GPU but owned by no
//! GPU API.
//!
//! One instance per part occurrence, grouped by definition so each definition is one
//! instanced draw. An instance holds the occurrence's placement as `f32` relative to
//! the scene's origin (the placements stay `f64` in the table; the camera is made
//! relative to the same origin in `f64` before it becomes `f32`), its row in the
//! occurrence table, its flags, and its assembly-level colour.
//!
//! [`Scene::update`] applies the document's [`Changes`]: a move rewrites the moved
//! instances, a hide or a colour rewrites their flags, a mesh that arrived adds its
//! definition's draw. The renderer uploads only the instance ranges that changed
//! ([`Scene::take_dirty`]).
//!
//! The id buffer holds indices that are good for this session only: the occurrence's
//! row in the table plus one, and the face's index in its definition's mesh plus one
//! ([`Scene::decode`]). They are never written anywhere; a selection that must last is
//! a path and a persistent face name ([`crate::document::Selection`]).

use std::ops::Range;

use glam::{DMat3, DVec3};

use crate::document::{AssemblyDocument, Changes};
use crate::occurrence::{Flags, OccurrenceIndex, OccurrenceTable};
use crate::placement::Placement;
use crate::store::{DefinitionIndex, DefinitionStore};
use soliddraft_tessellation::Level;

/// One instance as the renderer uploads it: 64 bytes of little-endian `f32` and `u32`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    /// The placement as three rows of an affine matrix: a point `p` goes to
    /// `(row0 . (p, 1), row1 . (p, 1), row2 . (p, 1))`, relative to the scene origin.
    pub rows: [[f32; 4]; 3],
    /// The occurrence's row in the table.
    pub row: u32,
    /// [`Instance::HIDDEN`], [`Instance::SELECTED`], [`Instance::HOVERED`],
    /// [`Instance::COLOURED`].
    pub flags: u32,
    /// The assembly-level colour, sRGB, red in the low byte; used when
    /// [`Instance::COLOURED`] is set.
    pub colour: u32,
    /// Metallic in the low byte and roughness in the next, each 0 to 255.
    pub finish: u32,
}

impl Instance {
    /// Not drawn: hidden, suppressed or removed.
    pub const HIDDEN: u32 = 1;
    /// Selected.
    pub const SELECTED: u32 = 2;
    /// Under the cursor.
    pub const HOVERED: u32 = 4;
    /// Shows the assembly-level colour in place of the part's own colours.
    pub const COLOURED: u32 = 8;
}

/// What the id buffer found: an occurrence, and the face of its part's display mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Picked {
    /// The occurrence.
    pub occurrence: OccurrenceIndex,
    /// The face's index in the display mesh of the occurrence's definition.
    pub face: Option<u32>,
}

/// One instanced draw: a definition's mesh for its instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// The definition.
    pub definition: DefinitionIndex,
    /// Its first instance slot.
    pub first_instance: u32,
    /// How many instances.
    pub count: u32,
}

/// An open assembly's instances and draws.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    origin: DVec3,
    instances: Vec<Instance>,
    /// Slot ranges of each definition, by definition index; empty for none.
    ranges: Vec<Range<u32>>,
    slot_of_row: Vec<u32>,
    /// Definitions whose mesh the renderer has not taken yet.
    new_meshes: Vec<DefinitionIndex>,
    /// Coarse and fine meshes the renderer has not taken yet.
    new_levels: Vec<(DefinitionIndex, Level)>,
    dirty: Vec<u32>,
    all_dirty: bool,
    /// Bumped when every instance and draw must be taken again.
    generation: u64,
    selected: Vec<OccurrenceIndex>,
    hovered: Option<OccurrenceIndex>,
}

const NO_SLOT: u32 = u32::MAX;

impl Scene {
    /// The scene for an open assembly as it is now. Takes the document's pending
    /// changes, which the new scene already reflects.
    pub fn new(doc: &mut AssemblyDocument) -> Scene {
        doc.take_changes();
        let mut s = Scene::default();
        s.rebuild(doc.table(), doc.store());
        s
    }

    fn rebuild(&mut self, table: &OccurrenceTable, store: &DefinitionStore) {
        // The origin: the middle of the parts' positions, so f32 keeps the most precision
        // where the parts are.
        let mut lo = DVec3::INFINITY;
        let mut hi = DVec3::NEG_INFINITY;
        let flags = table.all_flags();
        let defs = table.definitions();
        let parts: Vec<u32> = (0..table.len() as u32)
            .filter(|&r| !flags[r as usize].has(Flags::ASSEMBLY) && !flags[r as usize].has(Flags::REMOVED))
            .collect();
        for &r in &parts {
            let t = table.world(OccurrenceIndex(r)).translation;
            lo = lo.min(t);
            hi = hi.max(t);
        }
        self.origin = if parts.is_empty() { DVec3::ZERO } else { (lo + hi) * 0.5 };
        // Group by definition: count, then place.
        let mut count = vec![0u32; store.len()];
        for &r in &parts {
            count[defs[r as usize].0 as usize] += 1;
        }
        let mut start = 0u32;
        self.ranges = count
            .iter()
            .map(|&n| {
                let r = start..start + n;
                start += n;
                r
            })
            .collect();
        let mut next: Vec<u32> = self.ranges.iter().map(|r| r.start).collect();
        self.instances = vec![Instance::default(); parts.len()];
        self.slot_of_row = vec![NO_SLOT; table.len()];
        for &r in &parts {
            let d = defs[r as usize].0 as usize;
            let slot = next[d];
            next[d] += 1;
            self.slot_of_row[r as usize] = slot;
            self.instances[slot as usize] = self.instance(table, OccurrenceIndex(r));
        }
        self.new_meshes = store.iter().filter(|(_, d)| d.mesh.is_some()).map(|(i, _)| i).collect();
        self.new_levels = store
            .iter()
            .flat_map(|(i, d)| {
                [Level::Coarse, Level::Fine].into_iter().filter(|l| d.level_mesh(*l).is_some()).map(move |l| (i, l))
            })
            .collect();
        self.dirty.clear();
        self.all_dirty = true;
        self.generation += 1;
    }

    fn instance(&self, table: &OccurrenceTable, r: OccurrenceIndex) -> Instance {
        let mut i = Instance { row: r.0, ..Instance::default() };
        i.rows = affine_rows(&table.world(r), self.origin);
        let f = table.flags(r);
        let mut flags = 0;
        if !f.drawn_part() {
            flags |= Instance::HIDDEN;
        }
        if self.selected.iter().any(|s| *s == r || is_under(table, r, *s)) {
            flags |= Instance::SELECTED;
        }
        if self.hovered.is_some_and(|h| h == r || is_under(table, r, h)) {
            flags |= Instance::HOVERED;
        }
        if let Some(a) = table.colour(r) {
            flags |= Instance::COLOURED;
            let c = a.base_color;
            i.colour = u32::from_le_bytes([c.r, c.g, c.b, (a.opacity.clamp(0.0, 1.0) * 255.0).round() as u8]);
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            i.finish = q(a.metallic) | q(a.roughness) << 8;
        }
        i.flags = flags;
        i
    }

    /// Takes in what changed in the document since the last update.
    pub fn update(&mut self, doc: &mut AssemblyDocument) {
        let changes: Changes = doc.take_changes();
        if changes.structure {
            self.selected.retain(|r| doc.table().find(&doc.table().path(*r)) == Some(*r));
            self.rebuild(doc.table(), doc.store());
            return;
        }
        for d in changes.meshes {
            self.new_meshes.push(d);
        }
        self.new_levels.extend(changes.levels);
        let table = doc.table();
        for r in changes.moved.iter().chain(&changes.flags).chain(&changes.colours) {
            self.refresh(table, *r);
        }
    }

    fn refresh(&mut self, table: &OccurrenceTable, r: OccurrenceIndex) {
        let slot = self.slot_of_row.get(r.0 as usize).copied().unwrap_or(NO_SLOT);
        if slot != NO_SLOT {
            self.instances[slot as usize] = self.instance(table, r);
            self.dirty.push(slot);
        }
    }

    /// Marks occurrences selected (replacing the selection), updating their instances.
    pub fn set_selection(&mut self, table: &OccurrenceTable, selected: &[OccurrenceIndex]) {
        let old = std::mem::replace(&mut self.selected, selected.to_vec());
        for r in old.iter().chain(selected) {
            for part in parts_under(table, *r) {
                self.refresh_flags(table, part);
            }
        }
    }

    /// Marks the occurrence under the cursor.
    pub fn set_hover(&mut self, table: &OccurrenceTable, hovered: Option<OccurrenceIndex>) {
        if self.hovered == hovered {
            return;
        }
        let old = std::mem::replace(&mut self.hovered, hovered);
        for r in old.into_iter().chain(hovered) {
            for part in parts_under(table, r) {
                self.refresh_flags(table, part);
            }
        }
    }

    fn refresh_flags(&mut self, table: &OccurrenceTable, r: OccurrenceIndex) {
        let slot = self.slot_of_row.get(r.0 as usize).copied().unwrap_or(NO_SLOT);
        if slot == NO_SLOT {
            return;
        }
        let selected = self.selected.iter().any(|s| *s == r || is_under(table, r, *s));
        let hovered = self.hovered.is_some_and(|h| h == r || is_under(table, r, h));
        let i = &mut self.instances[slot as usize];
        let before = i.flags;
        i.flags &= !(Instance::SELECTED | Instance::HOVERED);
        if selected {
            i.flags |= Instance::SELECTED;
        }
        if hovered {
            i.flags |= Instance::HOVERED;
        }
        if i.flags != before {
            self.dirty.push(slot);
        }
    }

    /// The origin instance placements are relative to.
    pub fn origin(&self) -> DVec3 {
        self.origin
    }

    /// Every instance, grouped by definition.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Bumped whenever every instance and draw must be taken again.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The draws of the definitions in `loaded` (those whose mesh the renderer holds).
    pub fn draws(&self, loaded: impl Fn(DefinitionIndex) -> bool) -> Vec<Draw> {
        self.ranges
            .iter()
            .enumerate()
            .filter(|(d, r)| !r.is_empty() && loaded(DefinitionIndex(*d as u32)))
            .map(|(d, r)| Draw {
                definition: DefinitionIndex(d as u32),
                first_instance: r.start,
                count: r.len() as u32,
            })
            .collect()
    }

    /// Each definition's run of instance slots, by definition index.
    pub fn ranges(&self) -> &[Range<u32>] {
        &self.ranges
    }

    /// Coarse and fine meshes that arrived and the renderer has not taken: the
    /// definition and the level, its mesh in the store.
    pub fn take_new_levels(&mut self) -> Vec<(DefinitionIndex, Level)> {
        std::mem::take(&mut self.new_levels)
    }

    /// Definitions whose meshes arrived and the renderer has not taken, at most `max`
    /// of them: a frame uploads a bounded share while a large assembly loads, and the
    /// rest wait for the next frames.
    pub fn take_new_meshes(&mut self, max: usize) -> Vec<DefinitionIndex> {
        if self.new_meshes.len() <= max {
            return std::mem::take(&mut self.new_meshes);
        }
        let rest = self.new_meshes.split_off(max);
        std::mem::replace(&mut self.new_meshes, rest)
    }

    /// How many arrived meshes the renderer has not taken yet.
    pub fn pending_meshes(&self) -> usize {
        self.new_meshes.len()
    }

    /// The instance ranges to upload since the last call: everything after a rebuild,
    /// otherwise the changed slots merged into runs (two slots closer than 64 apart
    /// share a run, since one larger copy costs less than two calls).
    pub fn take_dirty(&mut self) -> Vec<Range<u32>> {
        if std::mem::take(&mut self.all_dirty) {
            self.dirty.clear();
            return if self.instances.is_empty() {
                Vec::new()
            } else {
                std::iter::once(0..self.instances.len() as u32).collect()
            };
        }
        let mut slots = std::mem::take(&mut self.dirty);
        slots.sort_unstable();
        slots.dedup();
        let mut out: Vec<Range<u32>> = Vec::new();
        for s in slots {
            match out.last_mut() {
                Some(r) if s < r.end + 64 => r.end = s + 1,
                _ => out.push(s..s + 1),
            }
        }
        out
    }

    /// What an id-buffer texel names: `[occurrence row + 1, kind << 30 | face + 1]`, the
    /// way the assembly shader writes it. `None` for a texel with no assembly
    /// instance.
    pub fn decode(&self, texel: [u32; 2]) -> Option<Picked> {
        let row = texel[0].checked_sub(1)?;
        let occurrence = OccurrenceIndex(row);
        self.slot(occurrence)?;
        let face = (texel[1] & ((1 << 30) - 1)).checked_sub(1);
        Some(Picked { occurrence, face })
    }

    /// The occurrence drawn as instance `slot`.
    pub fn occurrence(&self, slot: u32) -> Option<OccurrenceIndex> {
        self.instances.get(slot as usize).map(|i| OccurrenceIndex(i.row))
    }

    /// The instance slot of a part occurrence.
    pub fn slot(&self, r: OccurrenceIndex) -> Option<u32> {
        self.slot_of_row.get(r.0 as usize).copied().filter(|s| *s != NO_SLOT)
    }

    /// Roughly how many bytes the scene holds.
    pub fn heap_bytes(&self) -> usize {
        self.instances.len() * std::mem::size_of::<Instance>() + self.slot_of_row.len() * 4 + self.ranges.len() * 8
    }
}

/// `p` as affine rows relative to `origin`.
pub fn affine_rows(p: &Placement, origin: DVec3) -> [[f32; 4]; 3] {
    let m = DMat3::from_quat(p.rotation);
    let t = p.translation - origin;
    [
        [m.x_axis.x as f32, m.y_axis.x as f32, m.z_axis.x as f32, t.x as f32],
        [m.x_axis.y as f32, m.y_axis.y as f32, m.z_axis.y as f32, t.y as f32],
        [m.x_axis.z as f32, m.y_axis.z as f32, m.z_axis.z as f32, t.z as f32],
    ]
}

fn is_under(table: &OccurrenceTable, r: OccurrenceIndex, ancestor: OccurrenceIndex) -> bool {
    let mut p = table.parent(r);
    while let Some(q) = p {
        if q == ancestor {
            return true;
        }
        p = table.parent(q);
    }
    false
}

fn parts_under(table: &OccurrenceTable, r: OccurrenceIndex) -> Vec<OccurrenceIndex> {
    if table.flags(r).has(Flags::ASSEMBLY) {
        table.subtree(r).into_iter().filter(|x| !table.flags(*x).has(Flags::ASSEMBLY)).collect()
    } else {
        vec![r]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DQuat;

    #[test]
    fn rows_place_points_like_the_placement() {
        let p = Placement::new(DQuat::from_rotation_z(0.6), DVec3::new(1000.0, 2.0, 3.0));
        let origin = DVec3::new(999.0, 0.0, 0.0);
        let rows = affine_rows(&p, origin);
        let q = DVec3::new(1.0, 2.0, 3.0);
        let want = p.apply(q) - origin;
        let got = DVec3::new(
            rows[0][0] as f64 * q.x + rows[0][1] as f64 * q.y + rows[0][2] as f64 * q.z + rows[0][3] as f64,
            rows[1][0] as f64 * q.x + rows[1][1] as f64 * q.y + rows[1][2] as f64 * q.z + rows[1][3] as f64,
            rows[2][0] as f64 * q.x + rows[2][1] as f64 * q.y + rows[2][2] as f64 * q.z + rows[2][3] as f64,
        );
        assert!((got - want).length() < 1e-4, "{got} {want}");
        assert_eq!(std::mem::size_of::<Instance>(), 64);
    }

    #[test]
    fn dirty_slots_merge_into_runs() {
        let mut s = Scene { instances: vec![Instance::default(); 1000], ..Scene::default() };
        s.dirty = vec![5, 7, 6, 300, 301, 900];
        assert_eq!(s.take_dirty(), vec![5..8, 300..302, 900..901]);
        assert!(s.take_dirty().is_empty());
        s.all_dirty = true;
        assert_eq!(s.take_dirty(), std::iter::once(0..1000).collect::<Vec<_>>());
    }
}
