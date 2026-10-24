//! Tolerance-based sketch tools. These retain analytic lines and circular arcs;
//! Catallax's existing converter supplies the exact kernel representation.

use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::{ConstraintKind, Entity, EntityId, EntityKind, ORIGIN, P2, PointRef, Sketch};

const EPS: f64 = 1e-6;

/// The endpoint to extend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The start of a line or counter-clockwise arc.
    Start,
    /// The end of a line or counter-clockwise arc.
    End,
}

/// One sketch tool invocation. All coordinates are in the sketch's plane, in mm.
/// Copies are independent geometry, not associative patterns. Counts include the seed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// Add seed geometry for a tool, using the same entities as the pointer tools.
    Add {
        /// Analytic geometry to add.
        kind: EntityKind,
        /// Exclude this entity from profiles.
        #[serde(default)]
        construction: bool,
    },
    /// A regular polygon, counter-clockwise from its first vertex.
    Polygon {
        /// Circumcentre.
        center: P2,
        /// First vertex, which sets radius and rotation.
        vertex: P2,
        /// Number of sides, at least three.
        sides: u32,
    },
    /// Remove the picked interval between intersections with other sketch curves.
    Trim {
        /// Line or arc to trim.
        entity: EntityId,
        /// A point identifying the interval to remove.
        at: P2,
    },
    /// Extend one endpoint to the nearest intersection with a bounded entity.
    Extend {
        /// Line or arc to extend.
        entity: EntityId,
        /// Which endpoint moves.
        end: End,
        /// The limiting line, arc or circle.
        boundary: EntityId,
    },
    /// Copy an ordered, connected line/arc chain at a signed distance to its left.
    /// A negative distance offsets to the right. A lone circle uses inward positive.
    Offset {
        /// Chain entities in walking order; the first entity sets the direction.
        entities: Vec<EntityId>,
        /// Signed distance, mm. Adjacent curves meet at miter intersections.
        distance: f64,
    },
    /// Copy geometry across an infinite line, reversing reflected arc endpoints.
    Mirror {
        /// Entities to copy.
        entities: Vec<EntityId>,
        /// First point on the mirror axis.
        axis_start: P2,
        /// Second point on the mirror axis.
        axis_end: P2,
    },
    /// Copy geometry at repeated translations.
    LinearPattern {
        /// Seed entities.
        entities: Vec<EntityId>,
        /// Translation between instances.
        step: P2,
        /// Total instance count, including the seed.
        count: u32,
    },
    /// Copy geometry at repeated rotations about a point.
    CircularPattern {
        /// Seed entities.
        entities: Vec<EntityId>,
        /// Rotation centre.
        center: P2,
        /// Angle between instances, in degrees.
        angle_degrees: f64,
        /// Total instance count, including the seed.
        count: u32,
    },
}

/// A refused tool leaves the sketch unchanged.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

type Result<T, E = ToolError> = std::result::Result<T, E>;

fn refuse<T>(message: &str) -> Result<T> {
    Err(ToolError(message.into()))
}

fn finite(p: P2) -> bool {
    p.u.is_finite() && p.v.is_finite()
}

fn entity(kind: EntityKind) -> Entity {
    Entity { id: 0, kind, construction: false }
}

fn valid(kind: EntityKind) -> bool {
    match kind {
        EntityKind::Point { p } => finite(p),
        EntityKind::Line { a, b } => finite(a) && finite(b) && a.dist(b).is_finite() && a.dist(b) > EPS,
        EntityKind::Arc { center, start, end } => {
            let r = center.dist(start);
            finite(center)
                && finite(start)
                && finite(end)
                && r.is_finite()
                && r > EPS
                && start.dist(end) > EPS
                && (center.dist(end) - r).abs() <= EPS
        }
        EntityKind::Circle { center, radius } => finite(center) && radius.is_finite() && radius > EPS,
    }
}

// Degree inputs name right angles exactly. Keep their zero components exact
// rather than sending trig residuals into Catallax's rational arc conversion.
fn rotation_sin_cos(degrees: f64) -> (f64, f64) {
    match degrees.rem_euclid(360.0) {
        0.0 => (0.0, 1.0),
        90.0 => (1.0, 0.0),
        180.0 => (0.0, -1.0),
        270.0 => (-1.0, 0.0),
        angle => angle.to_radians().sin_cos(),
    }
}

fn ends(kind: EntityKind) -> Option<[P2; 2]> {
    match kind {
        EntityKind::Line { a, b } => Some([a, b]),
        EntityKind::Arc { start, end, .. } => Some([start, end]),
        _ => None,
    }
}

fn reversed(kind: EntityKind) -> EntityKind {
    match kind {
        EntityKind::Line { a, b } => EntityKind::Line { a: b, b: a },
        // Arcs are always CCW in the sketch model. Offset uses a separate walking direction.
        _ => kind,
    }
}

fn set_end(kind: &mut EntityKind, end: End, p: P2) {
    match (kind, end) {
        (EntityKind::Line { a, .. }, End::Start) => *a = p,
        (EntityKind::Line { b, .. }, End::End) => *b = p,
        (EntityKind::Arc { start, .. }, End::Start) => *start = p,
        (EntityKind::Arc { end, .. }, End::End) => *end = p,
        _ => {}
    }
}

fn parameter(kind: EntityKind, p: P2) -> Option<f64> {
    match kind {
        EntityKind::Line { a, b } => {
            let d = b - a;
            Some((p - a).dot(d) / d.dot(d))
        }
        EntityKind::Arc { center, start, .. } => {
            let (_, sweep) = entity(kind).arc_angles()?;
            let a = ((p - center).angle() - (start - center).angle()).rem_euclid(TAU);
            Some(if a > TAU - EPS / center.dist(start) { 0.0 } else { a / sweep })
        }
        _ => None,
    }
}

fn on_curve(kind: EntityKind, p: P2) -> bool {
    entity(kind).nearest(p).1 <= EPS
}

fn circle(kind: EntityKind) -> Option<(P2, f64)> {
    match kind {
        EntityKind::Arc { center, start, .. } => Some((center, center.dist(start))),
        EntityKind::Circle { center, radius } => Some((center, radius)),
        _ => None,
    }
}

/// Intersections of the infinite supporting lines/circles, never tessellated curves.
fn intersections(a: EntityKind, b: EntityKind) -> Vec<P2> {
    if let (EntityKind::Line { a: p, b: q }, EntityKind::Line { a: r, b: s }) = (a, b) {
        let (d, e) = (q - p, s - r);
        let den = d.cross(e);
        if den.abs() <= 1e-12 * d.len() * e.len() {
            return Vec::new();
        }
        return vec![p + d.scale((r - p).cross(e) / den)];
    }
    if let EntityKind::Line { a: p, b: q } = a {
        let Some((c, r)) = circle(b) else { return Vec::new() };
        let d = (q - p).normalized();
        let t = (c - p).dot(d);
        let foot = p + d.scale(t);
        let h2 = r * r - (foot - c).dot(foot - c);
        if h2 < -EPS * r.max(1.0) {
            return Vec::new();
        }
        let h = h2.max(0.0).sqrt();
        if h <= EPS { vec![foot] } else { vec![foot - d.scale(h), foot + d.scale(h)] }
    } else if matches!(b, EntityKind::Line { .. }) {
        intersections(b, a)
    } else if let (Some((c, r)), Some((d, s))) = (circle(a), circle(b)) {
        let delta = d - c;
        let len = delta.len();
        if len <= EPS || len > r + s + EPS || len < (r - s).abs() - EPS {
            return Vec::new();
        }
        let along = (r * r - s * s + len * len) / (2.0 * len);
        let foot = c + delta.scale(along / len);
        let h2 = r * r - along * along;
        if h2 < -EPS * r.max(1.0) {
            return Vec::new();
        }
        let off = delta.perp().scale(h2.max(0.0).sqrt() / len);
        if off.len() <= EPS { vec![foot] } else { vec![foot - off, foot + off] }
    } else {
        Vec::new()
    }
}

fn point_at(kind: EntityKind, t: f64) -> P2 {
    let [a, b] = ends(kind).expect("line or arc");
    if t == 0.0 {
        return a;
    }
    if t == 1.0 {
        return b;
    }
    match kind {
        EntityKind::Line { .. } => a.lerp(b, t),
        EntityKind::Arc { center, start, .. } => {
            let (angle, sweep) = entity(kind).arc_angles().expect("arc");
            center + P2::from_angle(angle + sweep * t).scale(center.dist(start))
        }
        _ => unreachable!(),
    }
}

impl Sketch {
    fn tool_entity(&self, id: EntityId) -> Result<Entity> {
        self.entity(id)
            .copied()
            .filter(|e| e.id != ORIGIN && valid(e.kind))
            .ok_or_else(|| ToolError(format!("entity {id} is missing, degenerate or the origin")))
    }

    fn tool_selection(&self, ids: &[EntityId]) -> Result<Vec<Entity>> {
        if ids.is_empty() {
            return refuse("select at least one entity");
        }
        let mut selected = Vec::with_capacity(ids.len());
        for &id in ids {
            if selected.iter().any(|e: &Entity| e.id == id) {
                return refuse("selection repeats an entity");
            }
            selected.push(self.tool_entity(id)?);
        }
        Ok(selected)
    }

    fn add_tool_entities(&mut self, geometry: Vec<(EntityKind, bool)>) -> Result<Vec<EntityId>> {
        if geometry.iter().any(|(kind, _)| !valid(*kind)) {
            return refuse("the tool would produce non-finite or degenerate geometry");
        }
        let ids: Vec<_> = geometry.into_iter().map(|(kind, construction)| self.add(kind, construction)).collect();
        // Copied curves own their points. Join matching endpoints within this new geometry only.
        for (i, &a) in ids.iter().enumerate() {
            let Some(ea) = self.entity(a).copied() else { continue };
            let Some(pa) = ends(ea.kind) else { continue };
            for &b in &ids[i + 1..] {
                let eb = *self.entity(b).expect("new entity");
                let Some(pb) = ends(eb.kind) else { continue };
                for (ia, p) in pa.iter().enumerate() {
                    for (ib, q) in pb.iter().enumerate() {
                        if p.dist(*q) <= EPS {
                            self.constrain(ConstraintKind::Coincident {
                                a: PointRef::new(a, ia as u8 + u8::from(ea.is_curve())),
                                b: PointRef::new(b, ib as u8 + u8::from(eb.is_curve())),
                            });
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    /// Apply one tool atomically. Returns the created or retained entity ids.
    /// Trim/extend keep the target id when possible and drop its old relations,
    /// because dimensions or endpoint relations could otherwise restore deleted geometry.
    pub fn apply_operation(&mut self, operation: &Operation) -> Result<Vec<EntityId>> {
        match operation {
            Operation::Add { kind, construction } => self.add_tool_entities(vec![(*kind, *construction)]),
            Operation::Polygon { center, vertex, sides } => {
                if !finite(*center) || !finite(*vertex) || center.dist(*vertex) <= EPS || !(3..=1000).contains(sides) {
                    return refuse("a polygon needs a finite centre, a distinct vertex and 3 to 1000 sides");
                }
                let d = *vertex - *center;
                let corners: Vec<_> = (0..*sides)
                    .map(|i| {
                        if i == 0 {
                            *vertex
                        } else {
                            let (s, c) = rotation_sin_cos(360.0 * f64::from(i) / f64::from(*sides));
                            *center + P2::new(c * d.u - s * d.v, s * d.u + c * d.v)
                        }
                    })
                    .collect();
                self.add_tool_entities(
                    (0..corners.len())
                        .map(|i| (EntityKind::Line { a: corners[i], b: corners[(i + 1) % corners.len()] }, false))
                        .collect(),
                )
            }
            Operation::Trim { entity: id, at } => self.trim_interval(*id, *at),
            Operation::Extend { entity: id, end, boundary } => self.extend_to(*id, *end, *boundary),
            Operation::Offset { entities, distance } => self.offset_chain(entities, *distance),
            Operation::Mirror { entities, axis_start, axis_end } => {
                if !finite(*axis_start) || !finite(*axis_end) || axis_start.dist(*axis_end) <= EPS {
                    return refuse("the mirror axis needs two distinct finite points");
                }
                let dir = (*axis_end - *axis_start).normalized();
                self.transform_copies(entities, 2, true, |_| {
                    move |p| {
                        let v = p - *axis_start;
                        *axis_start + dir.scale(2.0 * v.dot(dir)) - v
                    }
                })
            }
            Operation::LinearPattern { entities, step, count } => {
                if !finite(*step) || step.len() <= EPS {
                    return refuse("pattern step must be finite and nonzero");
                }
                self.transform_copies(entities, *count, false, |i| {
                    let delta = step.scale(f64::from(i));
                    move |p| p + delta
                })
            }
            Operation::CircularPattern { entities, center, angle_degrees, count } => {
                if !(2..=1000).contains(count) {
                    return refuse("pattern count including the seed must be from 2 to 1000");
                }
                if !finite(*center) || !angle_degrees.is_finite() || angle_degrees.rem_euclid(360.0).abs() < 1e-10 {
                    return refuse("circular pattern needs a finite centre and a nonzero rotation");
                }
                // Reject repeated seed positions, including a duplicated final full-turn instance.
                for i in 1..*count {
                    if (angle_degrees * f64::from(i))
                        .rem_euclid(360.0)
                        .min(360.0 - (angle_degrees * f64::from(i)).rem_euclid(360.0))
                        < 1e-8
                    {
                        return refuse("circular pattern repeats an instance");
                    }
                }
                self.transform_copies(entities, *count, false, |i| {
                    let (s, c) = rotation_sin_cos(angle_degrees * f64::from(i));
                    move |p: P2| {
                        let d = p - *center;
                        *center + P2::new(c * d.u - s * d.v, s * d.u + c * d.v)
                    }
                })
            }
        }
    }

    fn transform_copies<F: Fn(P2) -> P2>(
        &mut self,
        ids: &[EntityId],
        count: u32,
        reflect: bool,
        transform: impl Fn(u32) -> F,
    ) -> Result<Vec<EntityId>> {
        if !(2..=1000).contains(&count) {
            return refuse("pattern count including the seed must be from 2 to 1000");
        }
        let selected = self.tool_selection(ids)?;
        let mut out = Vec::new();
        for i in 1..count {
            let map = transform(i);
            for e in &selected {
                let kind = match e.kind {
                    EntityKind::Point { p } => EntityKind::Point { p: map(p) },
                    EntityKind::Line { a, b } => EntityKind::Line { a: map(a), b: map(b) },
                    EntityKind::Arc { center, start, end } => {
                        let (start, end) = if reflect { (end, start) } else { (start, end) };
                        EntityKind::Arc { center: map(center), start: map(start), end: map(end) }
                    }
                    EntityKind::Circle { center, radius } => EntityKind::Circle { center: map(center), radius },
                };
                out.push((kind, e.construction));
            }
        }
        self.add_tool_entities(out)
    }

    fn replace_tool_entity(&mut self, id: EntityId, pieces: Vec<EntityKind>) -> Result<Vec<EntityId>> {
        if pieces.iter().any(|k| !valid(*k)) {
            return refuse("the edit would produce degenerate geometry");
        }
        let old = self.tool_entity(id)?;
        self.constraints.retain(|c| !c.entities().contains(&id));
        let mut ids = Vec::with_capacity(pieces.len());
        for (i, kind) in pieces.into_iter().enumerate() {
            if i == 0 {
                self.entity_mut(id).expect("target exists").kind = kind;
                ids.push(id);
            } else {
                ids.push(self.add(kind, old.construction));
            }
        }
        if ids.is_empty() {
            self.remove_entity(id);
        }
        Ok(ids)
    }

    fn trim_interval(&mut self, id: EntityId, at: P2) -> Result<Vec<EntityId>> {
        let target = self.tool_entity(id)?;
        if !finite(at) || ends(target.kind).is_none() {
            return refuse("trim needs a line or arc and a finite pick point");
        }
        let mut cuts = vec![0.0, 1.0];
        for other in self.drawn().filter(|e| e.id != id) {
            for p in intersections(target.kind, other.kind) {
                if on_curve(target.kind, p) && on_curve(other.kind, p) {
                    cuts.push(parameter(target.kind, p).expect("line or arc").clamp(0.0, 1.0));
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| point_at(target.kind, *a).dist(point_at(target.kind, *b)) <= EPS);
        let p = target.nearest(at).0;
        let t = parameter(target.kind, p).expect("line or arc");
        if cuts.iter().any(|c| point_at(target.kind, *c).dist(p) <= EPS) {
            return refuse("pick inside a trim interval, not at an endpoint or intersection");
        }
        let Some(pair) = cuts.windows(2).find(|w| w[0] < t && t < w[1]) else {
            return refuse("no trim interval at this point");
        };
        let mut pieces = Vec::with_capacity(2);
        for (a, b) in [(0.0, pair[0]), (pair[1], 1.0)] {
            if point_at(target.kind, a).dist(point_at(target.kind, b)) > EPS {
                let mut kind = target.kind;
                set_end(&mut kind, End::Start, point_at(target.kind, a));
                set_end(&mut kind, End::End, point_at(target.kind, b));
                pieces.push(kind);
            }
        }
        self.replace_tool_entity(id, pieces)
    }

    fn extend_to(&mut self, id: EntityId, end: End, boundary: EntityId) -> Result<Vec<EntityId>> {
        if id == boundary {
            return refuse("extension boundary must differ from the target");
        }
        let target = self.tool_entity(id)?;
        let limit = self.tool_entity(boundary)?;
        let Some([a, b]) = ends(target.kind) else {
            return refuse("extend needs a line or arc");
        };
        let mut candidates: Vec<_> = intersections(target.kind, limit.kind)
            .into_iter()
            .filter_map(|p| {
                if !on_curve(limit.kind, p) {
                    return None;
                }
                let distance = match target.kind {
                    EntityKind::Line { .. } => {
                        let t = parameter(target.kind, p)?;
                        match end {
                            End::Start if t < 0.0 => a.dist(p),
                            End::End if t > 1.0 => b.dist(p),
                            _ => return None,
                        }
                    }
                    EntityKind::Arc { center, .. } => {
                        let (_, sweep) = target.arc_angles()?;
                        let delta = match end {
                            End::Start => ((a - center).angle() - (p - center).angle()).rem_euclid(TAU),
                            End::End => ((p - center).angle() - (b - center).angle()).rem_euclid(TAU),
                        };
                        if sweep + delta >= TAU - EPS / center.dist(a) {
                            return None;
                        }
                        delta * center.dist(a)
                    }
                    _ => return None,
                };
                (distance > EPS).then_some((distance, p))
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, p)) = candidates.first() else {
            return refuse("the boundary has no intersection beyond that endpoint");
        };
        let mut kind = target.kind;
        set_end(&mut kind, end, *p);
        self.replace_tool_entity(id, vec![kind])
    }

    fn offset_chain(&mut self, ids: &[EntityId], distance: f64) -> Result<Vec<EntityId>> {
        if !distance.is_finite() || distance.abs() <= EPS {
            return refuse("offset distance must be finite and nonzero");
        }
        let selected = self.tool_selection(ids)?;
        if selected.len() == 1
            && let EntityKind::Circle { center, radius } = selected[0].kind
        {
            return self.add_tool_entities(vec![(
                EntityKind::Circle { center, radius: radius - distance },
                selected[0].construction,
            )]);
        }
        // Walk the supplied chain. Reverse individual lines and track reversed arcs
        // separately because the entity model stores only counter-clockwise arcs.
        let mut source = Vec::with_capacity(selected.len());
        let mut previous = None;
        for e in &selected {
            let Some([a, b]) = ends(e.kind) else {
                return refuse("offset needs a connected chain of lines and arcs");
            };
            let reverse = if let Some(p) = previous {
                if a.dist(p) <= EPS {
                    false
                } else if b.dist(p) <= EPS {
                    true
                } else {
                    return refuse("offset entities must be in connected walking order");
                }
            } else {
                false
            };
            previous = Some(if reverse { a } else { b });
            source.push((e.kind, reverse));
        }
        let mut out = Vec::with_capacity(source.len());
        for &(kind, reverse) in &source {
            let d = if reverse { -distance } else { distance };
            let shifted = match kind {
                EntityKind::Line { a, b } => {
                    let n = (b - a).normalized().perp().scale(d);
                    let k = EntityKind::Line { a: a + n, b: b + n };
                    if reverse { reversed(k) } else { k }
                }
                EntityKind::Arc { center, start, end } => {
                    let r = center.dist(start);
                    if r - d <= EPS {
                        return refuse("offset collapses an arc radius");
                    }
                    EntityKind::Arc {
                        center,
                        start: center + (start - center).scale((r - d) / r),
                        end: center + (end - center).scale((r - d) / r),
                    }
                }
                _ => unreachable!(),
            };
            out.push(shifted);
        }
        let start = ends(source[0].0).expect("chain")[0];
        let closed = previous.is_some_and(|p| p.dist(start) <= EPS);
        let joins = if closed { out.len() } else { out.len() - 1 };
        for i in 0..joins {
            let j = (i + 1) % out.len();
            let ei = if source[i].1 && matches!(out[i], EntityKind::Arc { .. }) { End::Start } else { End::End };
            let ej = if source[j].1 && matches!(out[j], EntityKind::Arc { .. }) { End::End } else { End::Start };
            let pi = ends(out[i]).expect("chain")[usize::from(ei == End::End)];
            let pj = ends(out[j]).expect("chain")[usize::from(ej == End::End)];
            let joint = if pi.dist(pj) <= EPS {
                pi
            } else {
                intersections(out[i], out[j])
                    .into_iter()
                    .min_by(|a, b| (a.dist(pi) + a.dist(pj)).total_cmp(&(b.dist(pi) + b.dist(pj))))
                    .ok_or_else(|| ToolError("offset neighbours do not intersect".into()))?
            };
            set_end(&mut out[i], ei, joint);
            set_end(&mut out[j], ej, joint);
        }
        // A miter may consume an edge completely. Do not turn it inside out.
        for ((original, reverse), shifted) in source.iter().zip(&out) {
            if let (EntityKind::Line { a, b }, EntityKind::Line { a: c, b: d }) = (*original, *shifted) {
                let sign = if *reverse { -1.0 } else { 1.0 };
                if (b - a).dot(d - c) * sign <= EPS * EPS {
                    return refuse("offset consumes a chain edge");
                }
            }
        }
        for i in 0..out.len() {
            for j in i + 1..out.len() {
                if j == i + 1 || (closed && i == 0 && j == out.len() - 1) {
                    continue;
                }
                if intersections(out[i], out[j]).into_iter().any(|p| on_curve(out[i], p) && on_curve(out[j], p)) {
                    return refuse("offset would make the chain intersect itself");
                }
            }
        }
        self.add_tool_entities(out.into_iter().zip(selected).map(|(kind, e)| (kind, e.construction)).collect())
    }
}
