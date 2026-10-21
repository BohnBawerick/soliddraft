//! A bounding volume hierarchy over occurrences, on the CPU.
//!
//! The id buffer says which occurrence is under the cursor; the hierarchy answers the
//! questions that follow and the ones the id buffer cannot: the exact point a ray hits,
//! every occurrence in a box, every occurrence a camera can see. It holds part
//! occurrences only (sub-assemblies are drawn through their parts).
//!
//! Boxes are `f32`, relative to an origin chosen when the hierarchy is built, so a
//! million occurrences cost about 32 MB of nodes. Built with binned surface-area splits;
//! after a move the boxes are refitted without rebuilding the tree, which stays correct
//! (every box still holds its contents) and degrades in quality only after large moves.

use glam::{DVec3, Vec3};

use crate::occurrence::{OccurrenceIndex, OccurrenceTable};
use crate::placement::Aabb;

/// How many occurrences a leaf holds at most.
const LEAF: usize = 4;
/// Split candidates per axis.
const BINS: usize = 12;

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    /// For a leaf, the first item; for an inner node, the left child (the right one
    /// follows it).
    first: u32,
    max: Vec3,
    /// Items in a leaf; 0 for an inner node.
    count: u32,
}

/// A hierarchy over the part occurrences of an [`OccurrenceTable`].
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    origin: DVec3,
    nodes: Vec<Node>,
    /// Occurrence rows, in leaf order.
    items: Vec<u32>,
    /// For each row in the table, its leaf node, or `u32::MAX` when it is not in the
    /// hierarchy.
    leaf_of: Vec<u32>,
    parent: Vec<u32>,
}

/// A ray in the root's frame.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    /// Where it starts.
    pub origin: DVec3,
    /// Its direction, not necessarily unit.
    pub direction: DVec3,
}

/// Six planes bounding what a camera sees, each `(normal, d)` with the inside where
/// `normal . p + d >= 0`, in the root's frame.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    /// The planes.
    pub planes: [(DVec3, f64); 6],
}

impl Frustum {
    /// The frustum of a view-projection matrix (clip depth 0 to 1, as wgpu uses).
    pub fn from_view_proj(m: glam::DMat4) -> Frustum {
        let r = |i: usize| m.row(i);
        let planes = [r(3) + r(0), r(3) - r(0), r(3) + r(1), r(3) - r(1), r(2), r(3) - r(2)].map(|p| {
            let n = p.truncate();
            let len = n.length().max(1e-300);
            (n / len, p.w / len)
        });
        Frustum { planes }
    }
}

fn to_f32(p: DVec3, origin: DVec3) -> Vec3 {
    (p - origin).as_vec3()
}

impl Bvh {
    /// Builds the hierarchy over every part occurrence that is not suppressed or
    /// removed and has bounds. Hidden ones are included: showing them again needs no
    /// rebuild; queries skip them by flag when they want to.
    pub fn build(table: &OccurrenceTable) -> Bvh {
        Bvh::build_from(table.all_bounds(), table.all_flags(), table.bounds(OccurrenceIndex::ROOT).center())
    }

    /// Builds from a copy of a table's bounds and flags, as a background build does,
    /// with boxes relative to `origin`.
    pub fn build_from(bounds: &[Aabb], flags: &[crate::occurrence::Flags], origin: DVec3) -> Bvh {
        let len = bounds.len();
        let rows: Vec<u32> = (0..len as u32)
            .filter(|&r| {
                let f = flags[r as usize];
                !f.has(crate::occurrence::Flags::ASSEMBLY)
                    && !f.has(crate::occurrence::Flags::SUPPRESSED)
                    && !f.has(crate::occurrence::Flags::REMOVED)
                    && !bounds[r as usize].is_empty()
            })
            .collect();
        let origin = if origin.is_finite() { origin } else { DVec3::ZERO };
        let mut b = Bvh { origin, nodes: Vec::with_capacity(rows.len() / 2 + 1), items: rows, ..Bvh::default() };
        let boxes: Vec<(Vec3, Vec3)> = (0..len)
            .map(|r| {
                let a = bounds[r];
                if a.is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (to_f32(a.min, origin), to_f32(a.max, origin)) }
            })
            .collect();
        if !b.items.is_empty() {
            b.nodes.push(Node { min: Vec3::ZERO, first: 0, max: Vec3::ZERO, count: 0 });
            b.parent.push(u32::MAX);
            let n = b.items.len();
            b.split(0, 0, n, &boxes);
        }
        b.leaf_of = vec![u32::MAX; len];
        for (i, node) in b.nodes.iter().enumerate() {
            if node.count > 0 {
                for &r in &b.items[node.first as usize..(node.first + node.count) as usize] {
                    b.leaf_of[r as usize] = i as u32;
                }
            }
        }
        b
    }

    fn split(&mut self, node: usize, start: usize, end: usize, boxes: &[(Vec3, Vec3)]) {
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        let (mut clo, mut chi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for &r in &self.items[start..end] {
            let (a, b) = boxes[r as usize];
            lo = lo.min(a);
            hi = hi.max(b);
            let c = (a + b) * 0.5;
            clo = clo.min(c);
            chi = chi.max(c);
        }
        self.nodes[node].min = lo;
        self.nodes[node].max = hi;
        let n = end - start;
        let extent = chi - clo;
        if n <= LEAF || extent.max_element() <= 0.0 {
            self.nodes[node].first = start as u32;
            self.nodes[node].count = n as u32;
            return;
        }
        // Binned surface-area heuristic on the axis of largest centroid spread.
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let bin_of = |r: u32| {
            let (a, b) = boxes[r as usize];
            let c = ((a + b) * 0.5)[axis];
            (((c - clo[axis]) / extent[axis] * BINS as f32) as usize).min(BINS - 1)
        };
        let mut bins = [(0usize, Vec3::INFINITY, Vec3::NEG_INFINITY); BINS];
        for &r in &self.items[start..end] {
            let (a, b) = boxes[r as usize];
            let k = bin_of(r);
            bins[k].0 += 1;
            bins[k].1 = bins[k].1.min(a);
            bins[k].2 = bins[k].2.max(b);
        }
        let area = |lo: Vec3, hi: Vec3| {
            let s = (hi - lo).max(Vec3::ZERO);
            s.x * s.y + s.y * s.z + s.z * s.x
        };
        let mut best = (f32::INFINITY, BINS / 2);
        for split in 1..BINS {
            let (mut nl, mut ll, mut lh) = (0, Vec3::INFINITY, Vec3::NEG_INFINITY);
            for b in &bins[..split] {
                nl += b.0;
                ll = ll.min(b.1);
                lh = lh.max(b.2);
            }
            let (mut nr, mut rl, mut rh) = (0, Vec3::INFINITY, Vec3::NEG_INFINITY);
            for b in &bins[split..] {
                nr += b.0;
                rl = rl.min(b.1);
                rh = rh.max(b.2);
            }
            if nl == 0 || nr == 0 {
                continue;
            }
            let cost = nl as f32 * area(ll, lh) + nr as f32 * area(rl, rh);
            if cost < best.0 {
                best = (cost, split);
            }
        }
        let mut mid = start;
        if best.0.is_finite() {
            for i in start..end {
                if bin_of(self.items[i]) < best.1 {
                    self.items.swap(i, mid);
                    mid += 1;
                }
            }
        }
        if mid == start || mid == end {
            // Every centre in one bin: split by count.
            mid = start + n / 2;
            self.items[start..end].select_nth_unstable_by(n / 2, |a, b| {
                let ca = boxes[*a as usize].0[axis] + boxes[*a as usize].1[axis];
                let cb = boxes[*b as usize].0[axis] + boxes[*b as usize].1[axis];
                ca.total_cmp(&cb)
            });
        }
        let left = self.nodes.len();
        let blank = Node { min: Vec3::ZERO, first: 0, max: Vec3::ZERO, count: 0 };
        self.nodes.push(blank);
        self.nodes.push(blank);
        self.parent.push(node as u32);
        self.parent.push(node as u32);
        self.nodes[node].first = left as u32;
        self.nodes[node].count = 0;
        self.split(left, start, mid, boxes);
        self.split(left + 1, mid, end, boxes);
    }

    /// How many occurrences the hierarchy holds.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when it holds none.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Roughly how many bytes it holds.
    pub fn heap_bytes(&self) -> usize {
        self.nodes.len() * (std::mem::size_of::<Node>() + 4) + self.items.len() * 4 + self.leaf_of.len() * 4
    }

    /// Updates the boxes after the rows in `moved` changed bounds: each one's leaf and
    /// its ancestors. With many rows moved a full pass is cheaper and is used instead.
    pub fn refit(&mut self, table: &OccurrenceTable, moved: &[OccurrenceIndex]) {
        if self.nodes.is_empty() {
            return;
        }
        if moved.len() * 16 > self.nodes.len() {
            self.refit_all(table);
            return;
        }
        let mut dirty: Vec<u32> =
            moved.iter().filter_map(|r| self.leaf_of.get(r.0 as usize).copied()).filter(|&l| l != u32::MAX).collect();
        dirty.sort_unstable();
        dirty.dedup();
        // Children always come after their parents, so going from the highest index
        // down visits every node after the nodes below it.
        let mut queue = std::collections::BTreeSet::from_iter(dirty);
        while let Some(n) = queue.pop_last() {
            self.fit(n as usize, table);
            let p = self.parent[n as usize];
            if p != u32::MAX {
                queue.insert(p);
            }
        }
    }

    /// Updates every box from the table.
    pub fn refit_all(&mut self, table: &OccurrenceTable) {
        for n in (0..self.nodes.len()).rev() {
            self.fit(n, table);
        }
    }

    fn fit(&mut self, n: usize, table: &OccurrenceTable) {
        let node = self.nodes[n];
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        if node.count > 0 {
            for &r in &self.items[node.first as usize..(node.first + node.count) as usize] {
                let b = table.bounds(OccurrenceIndex(r));
                if !b.is_empty() {
                    lo = lo.min(to_f32(b.min, self.origin));
                    hi = hi.max(to_f32(b.max, self.origin));
                }
            }
        } else {
            for c in [node.first as usize, node.first as usize + 1] {
                lo = lo.min(self.nodes[c].min);
                hi = hi.max(self.nodes[c].max);
            }
        }
        self.nodes[n].min = lo;
        self.nodes[n].max = hi;
    }

    fn node_box(&self, n: usize) -> Aabb {
        let node = &self.nodes[n];
        Aabb { min: node.min.as_dvec3() + self.origin, max: node.max.as_dvec3() + self.origin }
    }

    /// Visits the rows in leaves whose boxes `keep` accepts, pruning subtrees it
    /// rejects.
    fn visit(&self, mut keep: impl FnMut(&Aabb) -> bool, mut each: impl FnMut(u32)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            if !keep(&self.node_box(n)) {
                continue;
            }
            let node = &self.nodes[n];
            if node.count > 0 {
                for &r in &self.items[node.first as usize..(node.first + node.count) as usize] {
                    each(r);
                }
            } else {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
            }
        }
    }

    /// Occurrences whose bounds overlap `region`.
    pub fn in_box(&self, table: &OccurrenceTable, region: &Aabb) -> Vec<OccurrenceIndex> {
        let mut out = Vec::new();
        self.visit(
            |b| b.intersects(region),
            |r| {
                if table.bounds(OccurrenceIndex(r)).intersects(region) {
                    out.push(OccurrenceIndex(r));
                }
            },
        );
        out
    }

    /// Occurrences whose bounds a camera may see.
    pub fn in_frustum(&self, table: &OccurrenceTable, frustum: &Frustum) -> Vec<OccurrenceIndex> {
        let inside = |b: &Aabb| {
            frustum.planes.iter().all(|(n, d)| {
                // The box corner furthest along the plane's normal.
                let p = DVec3::new(
                    if n.x >= 0.0 { b.max.x } else { b.min.x },
                    if n.y >= 0.0 { b.max.y } else { b.min.y },
                    if n.z >= 0.0 { b.max.z } else { b.min.z },
                );
                n.dot(p) + d >= 0.0
            })
        };
        let mut out = Vec::new();
        self.visit(inside, |r| {
            if inside(&table.bounds(OccurrenceIndex(r))) {
                out.push(OccurrenceIndex(r));
            }
        });
        out
    }

    /// Occurrences whose bounds `ray` passes through, nearest entry first, with the
    /// ray parameter where it enters each box.
    pub fn along_ray(&self, table: &OccurrenceTable, ray: &Ray) -> Vec<(OccurrenceIndex, f64)> {
        let mut out = Vec::new();
        self.visit(
            |b| ray_box(ray, b).is_some(),
            |r| {
                if let Some(t) = ray_box(ray, &table.bounds(OccurrenceIndex(r))) {
                    out.push((OccurrenceIndex(r), t));
                }
            },
        );
        out.sort_by(|a, b| a.1.total_cmp(&b.1));
        out
    }
}

/// Where a ray enters a box (0 when it starts inside), if it meets it ahead.
pub fn ray_box(ray: &Ray, b: &Aabb) -> Option<f64> {
    if b.is_empty() {
        return None;
    }
    let inv = ray.direction.recip();
    let t1 = (b.min - ray.origin) * inv;
    let t2 = (b.max - ray.origin) * inv;
    let near = t1.min(t2).max_element().max(0.0);
    let far = t1.max(t2).min_element();
    (near <= far && !near.is_nan()).then_some(near)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_enter_boxes() {
        let b = Aabb { min: DVec3::ZERO, max: DVec3::ONE };
        let r = Ray { origin: DVec3::new(-1.0, 0.5, 0.5), direction: DVec3::X };
        assert_eq!(ray_box(&r, &b), Some(1.0));
        let away = Ray { origin: DVec3::new(-1.0, 0.5, 0.5), direction: -DVec3::X };
        assert_eq!(ray_box(&away, &b), None);
        let inside = Ray { origin: DVec3::splat(0.5), direction: DVec3::Z };
        assert_eq!(ray_box(&inside, &b), Some(0.0));
    }

    #[test]
    fn frustum_planes_hold_what_the_camera_sees() {
        let proj = glam::camera::rh::proj::directx::perspective(1.0, 1.0, 1.0, 100.0);
        let view = glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        let f = Frustum::from_view_proj((proj * view).as_dmat4());
        let ahead = DVec3::ZERO;
        let behind = DVec3::new(0.0, 0.0, 20.0);
        assert!(f.planes.iter().all(|(n, d)| n.dot(ahead) + d >= 0.0));
        assert!(f.planes.iter().any(|(n, d)| n.dot(behind) + d < 0.0));
    }
}
