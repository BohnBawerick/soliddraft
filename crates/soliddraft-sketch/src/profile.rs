//! From sketch entities to kernel profiles: the non-construction lines and
//! arcs are chained into closed loops at their shared ends, circles stand on
//! their own, and loops nested inside another loop become its holes.
//!
//! Each region remembers which sketch entity made each of its segments, so a
//! feature built from it can name its faces after the sketch.

use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

use crate::{Entity, EntityId, EntityKind, P2, PointRef, Sketch};
use serde::{Deserialize, Serialize};
use soliddraft_kernel_api::{Loop, Plane, Point2, Profile, Segment};

/// Two ends closer than this join even when no coincident constraint joins them.
const JOIN_TOLERANCE: f64 = 1e-6;

/// Segments per full turn when an arc or circle is sampled into a polygon.
const SAMPLES_PER_TURN: f64 = 48.0;

/// Why the sketch has no usable profile.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ProfileError {
    /// Nothing closed is drawn.
    #[error("the sketch has no closed profile; chain lines and arcs end to end")]
    NoLoop,
    /// A line or arc has an end that joins nothing. Names the entity.
    #[error("an end of {0} is open; join it to the next entity")]
    OpenEnd(String),
    /// Three or more lines or arcs meet at one point. Names one of them.
    #[error("{0} branches: three entities meet at one point")]
    Branch(String),
    /// A line or arc has no length, a curve no radius, or a loop no area. Names the entity.
    #[error("{0} is degenerate: it has no length or encloses no area")]
    Degenerate(String),
    /// The profile failed the kernel interface's own check.
    #[error("the profile is not valid for the kernel: {0}")]
    Check(#[from] soliddraft_kernel_api::ProfileError),
}

/// A closed chain of segments with the sketch entity behind each one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegionLoop {
    /// The segments, each starting where the previous one ends.
    pub segments: Vec<Segment>,
    /// The sketch entity each segment came from, same length as `segments`.
    pub entities: Vec<EntityId>,
}

impl RegionLoop {
    /// Signed enclosed area, positive when the loop runs counter-clockwise. A lone
    /// circle has no direction and counts as positive.
    pub fn signed_area(&self) -> f64 {
        self.segments.iter().map(segment_area).sum()
    }
}

/// A closed region: one outer loop and the holes directly inside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// The boundary, counter-clockwise.
    pub outer: RegionLoop,
    /// The holes, clockwise.
    pub holes: Vec<RegionLoop>,
}

impl Region {
    /// The kernel profile on `plane`. Loop 0 is the outer loop, 1.. the holes in order.
    pub fn to_profile(&self, plane: Plane) -> Profile {
        Profile {
            plane,
            outer: Loop(self.outer.segments.clone()),
            holes: self.holes.iter().map(|h| Loop(h.segments.clone())).collect(),
        }
    }

    /// The sketch entity behind segment `segment` of loop `loop_index` (0 = outer).
    pub fn entity(&self, loop_index: usize, segment: usize) -> Option<EntityId> {
        let lp = if loop_index == 0 { &self.outer } else { self.holes.get(loop_index - 1)? };
        lp.entities.get(segment).copied()
    }

    /// Enclosed area: the outer loop minus the holes.
    pub fn area(&self) -> f64 {
        self.outer.signed_area().abs() - self.holes.iter().map(|h| h.signed_area().abs()).sum::<f64>()
    }
}

/// Area swept by the segment around the plane origin, half the integral of
/// `u dv - v du`. Summed over a closed loop it gives the signed enclosed area.
fn segment_area(s: &Segment) -> f64 {
    match *s {
        Segment::Line { from, to } => 0.5 * (from.u * to.v - to.u * from.v),
        Segment::Arc { center: c, from, to, clockwise } => {
            let r2 = (from.u - c.u).powi(2) + (from.v - c.v).powi(2);
            let sweep = arc_sweep(c, from, to, clockwise);
            0.5 * (c.u * (to.v - from.v) - c.v * (to.u - from.u) + r2 * sweep)
        }
        Segment::Circle { radius, .. } => PI * radius * radius,
    }
}

/// Signed angle swept from `from` to `to`, positive counter-clockwise.
fn arc_sweep(c: Point2, from: Point2, to: Point2, clockwise: bool) -> f64 {
    let a0 = (from.v - c.v).atan2(from.u - c.u);
    let a1 = (to.v - c.v).atan2(to.u - c.u);
    if clockwise { -(a0 - a1).rem_euclid(TAU) } else { (a1 - a0).rem_euclid(TAU) }
}

/// A closed polygon through the loop: each segment's start, then points along its
/// arcs. The last point is not repeated.
fn sample(segments: &[Segment]) -> Vec<P2> {
    let mut out = Vec::new();
    for s in segments {
        match *s {
            Segment::Line { from, .. } => out.push(p2(from)),
            Segment::Arc { center, from, to, clockwise } => {
                let sweep = arc_sweep(center, from, to, clockwise);
                let r = (from.u - center.u).hypot(from.v - center.v);
                let a0 = (from.v - center.v).atan2(from.u - center.u);
                let n = (SAMPLES_PER_TURN * sweep.abs() / TAU).ceil().max(4.0) as usize;
                out.extend((0..n).map(|i| {
                    let a = a0 + sweep * i as f64 / n as f64;
                    P2::new(center.u + r * a.cos(), center.v + r * a.sin())
                }));
            }
            Segment::Circle { center, radius } => {
                let n = SAMPLES_PER_TURN as usize;
                out.extend((0..n).map(|i| {
                    let a = TAU * i as f64 / n as f64;
                    P2::new(center.u + radius * a.cos(), center.v + radius * a.sin())
                }));
            }
        }
    }
    out
}

/// Even-odd point in polygon test.
fn contains(poly: &[P2], p: P2) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.v > p.v) != (b.v > p.v) && p.u < (b.u - a.u) * (p.v - a.v) / (b.v - a.v) + a.u {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn pt(p: P2) -> Point2 {
    Point2::new(p.u, p.v)
}

fn p2(p: Point2) -> P2 {
    P2::new(p.u, p.v)
}

/// The two ends of a line or arc as point refs.
fn ends(e: &Entity) -> Option<(PointRef, PointRef)> {
    match e.kind {
        EntityKind::Line { .. } => Some((PointRef::new(e.id, 0), PointRef::new(e.id, 1))),
        EntityKind::Arc { .. } => Some((PointRef::new(e.id, 1), PointRef::new(e.id, 2))),
        _ => None,
    }
}

fn name(e: &Entity) -> String {
    format!("{} {}", e.kind_name(), e.id)
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Joins the groups of `i` and `j`; the smaller index becomes the root.
fn union(parent: &mut [usize], i: usize, j: usize) {
    let (ri, rj) = (find(parent, i), find(parent, j));
    parent[ri.max(rj)] = ri.min(rj);
}

/// A line or arc between two joined ends.
struct Edge {
    entity: EntityId,
    /// Centre of an arc, `None` for a line.
    center: Option<P2>,
    /// Node of the start and of the end point.
    ends: [usize; 2],
}

/// A closed loop before it becomes segments.
enum Chain {
    Circle {
        entity: EntityId,
        center: P2,
        radius: f64,
    },
    /// Edge indices with the direction each is walked in (`true` = start to end).
    Edges(Vec<(usize, bool)>),
}

/// The closed loops of a sketch and the first reason some piece did not close.
struct Loops {
    loops: Vec<RegionLoop>,
    /// Loops that are made of lines and arcs, so they can be turned around.
    reversible: Vec<bool>,
    first_error: Option<ProfileError>,
}

/// The point on the perpendicular bisector of `from`-`to` closest to `c`. Ends that
/// were joined within tolerance may sit a little apart; this keeps both radii equal.
fn bisector_center(c: P2, from: P2, to: P2) -> P2 {
    let m = from.mid(to);
    let n = (to - from).perp().normalized();
    m + n.scale((c - m).dot(n))
}

fn segments(edges: &[Edge], nodes: &[P2], chain: &Chain) -> RegionLoop {
    match *chain {
        Chain::Circle { entity, center, radius } => {
            RegionLoop { segments: vec![Segment::Circle { center: pt(center), radius }], entities: vec![entity] }
        }
        Chain::Edges(ref walk) => {
            let mut segs = Vec::with_capacity(walk.len());
            for &(e, forward) in walk {
                let edge = &edges[e];
                let (a, b) = (nodes[edge.ends[0]], nodes[edge.ends[1]]);
                let (from, to) = if forward { (a, b) } else { (b, a) };
                segs.push(match edge.center {
                    None => Segment::Line { from: pt(from), to: pt(to) },
                    // A sketch arc runs counter-clockwise from start to end, so walking it
                    // backwards runs clockwise.
                    Some(c) => Segment::Arc {
                        center: pt(bisector_center(c, from, to)),
                        from: pt(from),
                        to: pt(to),
                        clockwise: !forward,
                    },
                });
            }
            RegionLoop { segments: segs, entities: walk.iter().map(|&(e, _)| edges[e].entity).collect() }
        }
    }
}

/// Every closed loop among the non-construction lines, arcs and circles, in
/// entity order. Pieces that do not close are skipped and the first one is
/// remembered.
fn loops(sketch: &Sketch) -> Loops {
    let mut drawn: Vec<&Entity> = sketch.drawn().filter(|e| !e.construction).collect();
    drawn.sort_by_key(|e| e.id);
    let mut first_error: Option<ProfileError> = None;
    let mut chains: Vec<Chain> = Vec::new();

    // Ends of usable lines and arcs; degenerate ones are reported and left out.
    let mut curves: Vec<(&Entity, PointRef, PointRef)> = Vec::new();
    for e in &drawn {
        match e.kind {
            EntityKind::Circle { center, radius } => {
                if radius.is_finite() && radius > JOIN_TOLERANCE {
                    chains.push(Chain::Circle { entity: e.id, center, radius });
                } else {
                    first_error.get_or_insert(ProfileError::Degenerate(name(e)));
                }
            }
            EntityKind::Line { a, b } if a.dist(b) <= JOIN_TOLERANCE => {
                first_error.get_or_insert(ProfileError::Degenerate(name(e)));
            }
            EntityKind::Arc { center, start, end }
                if center.dist(start) <= JOIN_TOLERANCE || start.dist(end) <= JOIN_TOLERANCE =>
            {
                first_error.get_or_insert(ProfileError::Degenerate(name(e)));
            }
            _ => {
                if let Some((a, b)) = ends(e) {
                    curves.push((e, a, b));
                }
            }
        }
    }

    // Join ends that coincident constraints join, directly or through other points,
    // or that lie on top of each other. Curve ends come first, so the root of a
    // group that holds one is a curve end.
    let mut refs: Vec<PointRef> = curves.iter().flat_map(|&(_, a, b)| [a, b]).collect();
    let n_ends = refs.len();
    for c in &sketch.constraints {
        if let crate::ConstraintKind::Coincident { a, b } = c.kind {
            refs.extend([a, b]);
        }
    }
    let mut index: BTreeMap<(EntityId, u8), usize> = BTreeMap::new();
    for (i, r) in refs.iter().enumerate() {
        index.entry((r.entity, r.index)).or_insert(i);
    }
    let at: Vec<P2> = refs.iter().map(|&r| sketch.point(r).unwrap_or(P2::ZERO)).collect();
    let mut parent: Vec<usize> = (0..refs.len()).collect();
    for c in &sketch.constraints {
        if let crate::ConstraintKind::Coincident { a, b } = c.kind {
            union(&mut parent, index[&(a.entity, a.index)], index[&(b.entity, b.index)]);
        }
    }
    for i in 0..n_ends {
        for j in 0..i {
            if at[i].dist(at[j]) <= JOIN_TOLERANCE {
                union(&mut parent, i, j);
            }
        }
    }
    // One node per group, placed at the group's first end so the result is repeatable.
    let mut node_of_root: BTreeMap<usize, usize> = BTreeMap::new();
    let mut nodes: Vec<P2> = Vec::new();
    let node: Vec<usize> = (0..n_ends)
        .map(|i| {
            let root = find(&mut parent, i);
            *node_of_root.entry(root).or_insert_with(|| {
                nodes.push(at[root]);
                nodes.len() - 1
            })
        })
        .collect();

    let mut edges: Vec<Edge> = Vec::new();
    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (k, &(e, _, _)) in curves.iter().enumerate() {
        let ends = [node[2 * k], node[2 * k + 1]];
        if ends[0] == ends[1] {
            first_error.get_or_insert(ProfileError::Degenerate(name(e)));
            continue;
        }
        let center = match e.kind {
            EntityKind::Arc { center, .. } => Some(center),
            _ => None,
        };
        incident[ends[0]].push(edges.len());
        incident[ends[1]].push(edges.len());
        edges.push(Edge { entity: e.id, center, ends });
    }

    // Connected pieces of edges: a piece is a loop when every node in it has two edges.
    let mut visited = vec![false; edges.len()];
    for first in 0..edges.len() {
        if visited[first] {
            continue;
        }
        let mut piece = vec![first];
        visited[first] = true;
        let mut k = 0;
        while k < piece.len() {
            for n in edges[piece[k]].ends {
                for &e in &incident[n] {
                    if !visited[e] {
                        visited[e] = true;
                        piece.push(e);
                    }
                }
            }
            k += 1;
        }
        piece.sort_unstable();
        let entity_name = |e: usize| sketch.entity(edges[e].entity).map(name).unwrap_or_default();
        let piece_nodes = || piece.iter().flat_map(|&e| edges[e].ends);
        if let Some(n) = piece_nodes().find(|&n| incident[n].len() > 2) {
            first_error.get_or_insert(ProfileError::Branch(entity_name(incident[n][0])));
            continue;
        }
        if let Some(n) = piece_nodes().find(|&n| incident[n].len() < 2) {
            first_error.get_or_insert(ProfileError::OpenEnd(entity_name(incident[n][0])));
            continue;
        }
        // Every node has exactly two edges, so the walk closes on its first edge.
        let start = piece[0];
        let mut walk = vec![(start, true)];
        let (mut edge, mut at_node) = (start, edges[start].ends[1]);
        loop {
            let next = incident[at_node].iter().copied().find(|&e| e != edge).unwrap_or(edge);
            if next == start {
                break;
            }
            let forward = edges[next].ends[0] == at_node;
            at_node = edges[next].ends[usize::from(forward)];
            walk.push((next, forward));
            edge = next;
        }
        chains.push(Chain::Edges(walk));
    }

    let mut out = Loops { loops: Vec::new(), reversible: Vec::new(), first_error };
    for chain in &chains {
        let lp = segments(&edges, &nodes, chain);
        if lp.signed_area().abs() <= JOIN_TOLERANCE * JOIN_TOLERANCE {
            let first = lp.entities.iter().min().and_then(|&id| sketch.entity(id)).map(name).unwrap_or_default();
            out.first_error.get_or_insert(ProfileError::Degenerate(first));
            continue;
        }
        out.reversible.push(matches!(chain, Chain::Edges(_)));
        out.loops.push(lp);
    }
    out
}

/// The loop turned to run counter-clockwise (`ccw`) or clockwise, starting at the
/// segment with the smallest entity id.
fn oriented(lp: &RegionLoop, reversible: bool, ccw: bool) -> RegionLoop {
    let mut lp = lp.clone();
    if reversible && (lp.signed_area() > 0.0) != ccw {
        lp.segments.reverse();
        lp.entities.reverse();
        for s in &mut lp.segments {
            *s = match *s {
                Segment::Line { from, to } => Segment::Line { from: to, to: from },
                Segment::Arc { center, from, to, clockwise } => {
                    Segment::Arc { center, from: to, to: from, clockwise: !clockwise }
                }
                circle @ Segment::Circle { .. } => circle,
            };
        }
    }
    if let Some(start) = (0..lp.entities.len()).min_by_key(|&i| lp.entities[i]) {
        lp.segments.rotate_left(start);
        lp.entities.rotate_left(start);
    }
    lp
}

/// Every closed region of the sketch: outer loops counter-clockwise, holes
/// clockwise. Construction geometry is ignored.
///
/// Ends join when a coincident constraint joins them or they lie within 1e-6 of
/// each other. Pieces that do not close are skipped; they are an error only when
/// nothing closes at all. Loops nest by containment: a loop inside an even number
/// of others is an outer loop, and the loops directly inside it are its holes.
/// Overlapping loops are not detected. Regions are ordered by the smallest entity
/// of their outer loop, holes likewise, and each loop starts at its smallest entity.
pub fn regions(sketch: &Sketch) -> Result<Vec<Region>, ProfileError> {
    let Loops { loops, reversible, first_error } = loops(sketch);
    if loops.is_empty() {
        return Err(first_error.unwrap_or(ProfileError::NoLoop));
    }
    let polygons: Vec<Vec<P2>> = loops.iter().map(|l| sample(&l.segments)).collect();
    let areas: Vec<f64> = loops.iter().map(|l| l.signed_area().abs()).collect();
    let n = loops.len();
    let inside = |outer: usize, inner: usize| {
        outer != inner && areas[outer] > areas[inner] && contains(&polygons[outer], polygons[inner][0])
    };
    let depth: Vec<usize> = (0..n).map(|i| (0..n).filter(|&j| inside(j, i)).count()).collect();
    let parent: Vec<Option<usize>> =
        (0..n).map(|i| (0..n).filter(|&j| inside(j, i)).min_by(|&a, &b| areas[a].total_cmp(&areas[b]))).collect();
    let mut regions: Vec<Region> = (0..n)
        .filter(|&i| depth[i].is_multiple_of(2))
        .map(|i| {
            let mut holes: Vec<RegionLoop> = (0..n)
                .filter(|&h| !depth[h].is_multiple_of(2) && parent[h] == Some(i))
                .map(|h| oriented(&loops[h], reversible[h], false))
                .collect();
            holes.sort_by_key(|h| h.entities[0]);
            Region { outer: oriented(&loops[i], reversible[i], true), holes }
        })
        .collect();
    regions.sort_by_key(|r| r.outer.entities[0]);
    Ok(regions)
}

/// Every closed region as a kernel profile on `plane`, outer loops with their
/// holes. Loops nested two deep are regions of their own again.
pub fn profiles(sketch: &Sketch, plane: Plane) -> Result<Vec<Profile>, ProfileError> {
    regions(sketch)?
        .iter()
        .map(|r| {
            let p = r.to_profile(plane);
            p.check()?;
            Ok(p)
        })
        .collect()
}

/// Outlines of the closed loops, for shading in the sketcher. Empty when nothing closes.
pub fn region_outlines(sketch: &Sketch) -> Vec<Vec<P2>> {
    loops(sketch).loops.iter().map(|l| sample(&l.segments)).collect()
}

impl Sketch {
    /// Every closed region of the sketch; see [`regions`].
    pub fn regions(&self) -> Result<Vec<Region>, ProfileError> {
        regions(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConstraintKind;

    fn square(s: &mut Sketch, o: P2, w: f64, join: bool) -> Vec<EntityId> {
        let c = [o, o + P2::new(w, 0.0), o + P2::new(w, w), o + P2::new(0.0, w)];
        let ids: Vec<EntityId> =
            (0..4).map(|i| s.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, false)).collect();
        if join {
            for i in 0..4 {
                s.constrain(ConstraintKind::Coincident {
                    a: PointRef::new(ids[i], 1),
                    b: PointRef::new(ids[(i + 1) % 4], 0),
                });
            }
        }
        ids
    }

    #[test]
    fn a_square_with_a_circle_inside_is_one_profile_with_a_hole() {
        let mut s = Sketch::new();
        square(&mut s, P2::ZERO, 40.0, true);
        s.add(EntityKind::Circle { center: P2::new(20.0, 20.0), radius: 5.0 }, false);
        let p = profiles(&s, Plane::XY).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].outer.0.len(), 4);
        assert_eq!(p[0].holes.len(), 1);
    }

    #[test]
    fn clockwise_drawing_and_unjoined_ends_still_close() {
        let mut s = Sketch::new();
        let c = [P2::ZERO, P2::new(0.0, 10.0), P2::new(10.0, 10.0), P2::new(10.0, 0.0)];
        for i in 0..4 {
            s.add(EntityKind::Line { a: c[i], b: c[(i + 1) % 4] }, false);
        }
        let p = profiles(&s, Plane::XY).unwrap();
        assert_eq!(p.len(), 1);
        // Counter-clockwise after orientation: the first segment runs the reverse way.
        let Segment::Line { from, to } = p[0].outer.0[0] else { panic!() };
        let area: f64 = p[0]
            .outer
            .0
            .iter()
            .map(|s| match s {
                Segment::Line { from, to } => from.u * to.v - to.u * from.v,
                _ => 0.0,
            })
            .sum();
        assert!(area > 0.0, "{from:?} -> {to:?}");
    }

    #[test]
    fn open_chains_are_reported() {
        let mut s = Sketch::new();
        s.add(EntityKind::Line { a: P2::ZERO, b: P2::new(10.0, 0.0) }, false);
        s.add(EntityKind::Line { a: P2::new(10.0, 0.0), b: P2::new(10.0, 10.0) }, false);
        assert!(matches!(profiles(&s, Plane::XY), Err(ProfileError::OpenEnd(_))));
        assert!(region_outlines(&s).is_empty());
        let mut s = Sketch::new();
        square(&mut s, P2::ZERO, 40.0, true);
        square(&mut s, P2::new(50.0, 0.0), 10.0, true);
        assert_eq!(profiles(&s, Plane::XY).unwrap().len(), 2);
        assert_eq!(region_outlines(&s).len(), 2);
    }

    #[test]
    fn a_slot_is_two_lines_and_two_arcs() {
        let mut s = Sketch::new();
        let r = 6.0;
        // Drawn clockwise; arcs are stored counter-clockwise and bulge outward.
        s.add(EntityKind::Line { a: P2::new(0.0, r), b: P2::new(40.0, r) }, false);
        s.add(EntityKind::Arc { center: P2::new(40.0, 0.0), start: P2::new(40.0, -r), end: P2::new(40.0, r) }, false);
        s.add(EntityKind::Line { a: P2::new(40.0, -r), b: P2::new(0.0, -r) }, false);
        s.add(EntityKind::Arc { center: P2::ZERO, start: P2::new(0.0, r), end: P2::new(0.0, -r) }, false);
        let p = profiles(&s, Plane::XY).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].outer.0.len(), 4);
        p[0].check().unwrap();
        assert!(p[0].outer.0.iter().all(|s| !matches!(s, Segment::Arc { clockwise: true, .. })));
    }

    #[test]
    fn branches_and_degenerate_lines_are_reported() {
        let mut s = Sketch::new();
        let ids = square(&mut s, P2::ZERO, 10.0, true);
        let spur = s.add(EntityKind::Line { a: P2::new(10.0, 0.0), b: P2::new(20.0, 0.0) }, false);
        s.constrain(ConstraintKind::Coincident { a: PointRef::new(ids[0], 1), b: PointRef::new(spur, 0) });
        assert!(matches!(regions(&s), Err(ProfileError::Branch(_))));
        let mut s = Sketch::new();
        s.add(EntityKind::Line { a: P2::ZERO, b: P2::ZERO }, false);
        assert!(matches!(regions(&s), Err(ProfileError::Degenerate(_))));
        assert_eq!(regions(&Sketch::new()), Err(ProfileError::NoLoop));
    }
}
