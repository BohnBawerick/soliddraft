//! The sketch: 2D entities on a plane, the constraints between them and the
//! dimensions that drive them. Coordinates are in the plane's (u, v) frame, in
//! millimetres. The solver ([`solve`]) keeps the geometry consistent with the
//! constraints; [`profile`] turns the closed loops into kernel profiles.
//!
//! Every entity owns its points: a line has two, an arc a centre and two ends,
//! a circle a centre and a radius. Points of different entities are joined by
//! coincident constraints, the way SolidWorks and ezpz model a sketch, so an
//! entity can be deleted without rewriting its neighbours.
//!
//! The solver is ezpz, but none of its types appear in this crate's interface,
//! so another solver can take its place.

pub mod profile;
mod shapes;
pub mod solve;
pub mod tools;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tools_tests;

pub use profile::{ProfileError, Region, RegionLoop};
pub use shapes::{Shape, SketchError};

use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

/// Identifies an entity within one sketch. Ids are never reused.
pub type EntityId = u32;
/// Identifies a constraint within one sketch. Ids are never reused.
pub type ConstraintId = u32;

/// The sketch origin is a fixed point entity with this id.
pub const ORIGIN: EntityId = 0;

/// A point in sketch plane coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct P2 {
    /// Horizontal coordinate.
    pub u: f64,
    /// Vertical coordinate.
    pub v: f64,
}

impl P2 {
    /// The plane origin.
    pub const ZERO: P2 = P2 { u: 0.0, v: 0.0 };

    /// A point from its coordinates.
    pub const fn new(u: f64, v: f64) -> Self {
        P2 { u, v }
    }

    /// Distance to `o`.
    pub fn dist(self, o: P2) -> f64 {
        ((self.u - o.u).powi(2) + (self.v - o.v).powi(2)).sqrt()
    }

    /// The point a fraction `t` of the way from `self` to `o`.
    pub fn lerp(self, o: P2, t: f64) -> P2 {
        P2::new(self.u + (o.u - self.u) * t, self.v + (o.v - self.v) * t)
    }

    /// The point halfway to `o`.
    pub fn mid(self, o: P2) -> P2 {
        self.lerp(o, 0.5)
    }

    /// Both coordinates multiplied by `s`.
    pub fn scale(self, s: f64) -> P2 {
        P2::new(self.u * s, self.v * s)
    }

    /// Length as a vector.
    pub fn len(self) -> f64 {
        (self.u * self.u + self.v * self.v).sqrt()
    }

    /// The unit vector in the same direction, or (1, 0) for a zero vector.
    pub fn normalized(self) -> P2 {
        let l = self.len();
        if l < 1e-12 { P2::new(1.0, 0.0) } else { self.scale(1.0 / l) }
    }

    /// Rotated a quarter turn counter-clockwise.
    pub fn perp(self) -> P2 {
        P2::new(-self.v, self.u)
    }

    /// Dot product.
    pub fn dot(self, o: P2) -> f64 {
        self.u * o.u + self.v * o.v
    }

    /// The z component of the cross product, positive when `o` lies counter-clockwise.
    pub fn cross(self, o: P2) -> f64 {
        self.u * o.v - self.v * o.u
    }

    /// Angle from the u axis, in radians.
    pub fn angle(self) -> f64 {
        self.v.atan2(self.u)
    }

    /// The unit vector at angle `a` from the u axis.
    pub fn from_angle(a: f64) -> P2 {
        P2::new(a.cos(), a.sin())
    }
}

impl std::ops::Add for P2 {
    type Output = P2;
    fn add(self, o: P2) -> P2 {
        P2::new(self.u + o.u, self.v + o.v)
    }
}

impl std::ops::Sub for P2 {
    type Output = P2;
    fn sub(self, o: P2) -> P2 {
        P2::new(self.u - o.u, self.v - o.v)
    }
}

/// The geometry of an entity, with the points it owns.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntityKind {
    /// A lone point.
    Point {
        /// Its position.
        p: P2,
    },
    /// A straight segment.
    Line {
        /// Start point.
        a: P2,
        /// End point.
        b: P2,
    },
    /// Counter-clockwise from `start` to `end` around `center`.
    Arc {
        /// Centre.
        center: P2,
        /// Start point.
        start: P2,
        /// End point.
        end: P2,
    },
    /// A full circle.
    Circle {
        /// Centre.
        center: P2,
        /// Radius.
        radius: f64,
    },
}

/// One piece of sketch geometry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// Its id, unique within the sketch.
    pub id: EntityId,
    /// Its geometry.
    pub kind: EntityKind,
    /// Construction geometry guides other entities and never becomes a profile.
    #[serde(default)]
    pub construction: bool,
}

impl Entity {
    /// How many points the entity owns (see [`Entity::point`]).
    pub fn point_count(&self) -> u8 {
        match self.kind {
            EntityKind::Point { .. } | EntityKind::Circle { .. } => 1,
            EntityKind::Line { .. } => 2,
            EntityKind::Arc { .. } => 3,
        }
    }

    /// Point `index`: a point's own point; a line's start and end; an arc's
    /// centre, start and end; a circle's centre.
    pub fn point(&self, index: u8) -> Option<P2> {
        match (self.kind, index) {
            (EntityKind::Point { p }, 0) => Some(p),
            (EntityKind::Line { a, .. }, 0) => Some(a),
            (EntityKind::Line { b, .. }, 1) => Some(b),
            (EntityKind::Arc { center, .. }, 0) => Some(center),
            (EntityKind::Arc { start, .. }, 1) => Some(start),
            (EntityKind::Arc { end, .. }, 2) => Some(end),
            (EntityKind::Circle { center, .. }, 0) => Some(center),
            _ => None,
        }
    }

    /// Moves point `index` (see [`Entity::point`]) to `p`. Other indices are ignored.
    pub fn set_point(&mut self, index: u8, p: P2) {
        match (&mut self.kind, index) {
            (EntityKind::Point { p: q }, 0) => *q = p,
            (EntityKind::Line { a, .. }, 0) => *a = p,
            (EntityKind::Line { b, .. }, 1) => *b = p,
            (EntityKind::Arc { center, .. }, 0) => *center = p,
            (EntityKind::Arc { start, .. }, 1) => *start = p,
            (EntityKind::Arc { end, .. }, 2) => *end = p,
            (EntityKind::Circle { center, .. }, 0) => *center = p,
            _ => {}
        }
    }

    /// Every point the entity owns, in index order.
    pub fn points(&self) -> Vec<P2> {
        (0..self.point_count()).filter_map(|i| self.point(i)).collect()
    }

    /// Moves every point by `d`.
    pub fn translate(&mut self, d: P2) {
        for i in 0..self.point_count() {
            if let Some(p) = self.point(i) {
                self.set_point(i, p.add(d));
            }
        }
    }

    /// The radius of an arc or circle.
    pub fn radius(&self) -> Option<f64> {
        match self.kind {
            EntityKind::Circle { radius, .. } => Some(radius),
            EntityKind::Arc { center, start, .. } => Some(center.dist(start)),
            _ => None,
        }
    }

    /// True for arcs and circles.
    pub fn is_curve(&self) -> bool {
        matches!(self.kind, EntityKind::Arc { .. } | EntityKind::Circle { .. })
    }

    /// True for lines.
    pub fn is_line(&self) -> bool {
        matches!(self.kind, EntityKind::Line { .. })
    }

    /// True for lone points.
    pub fn is_point(&self) -> bool {
        matches!(self.kind, EntityKind::Point { .. })
    }

    /// The length of a line.
    pub fn length(&self) -> Option<f64> {
        match self.kind {
            EntityKind::Line { a, b } => Some(a.dist(b)),
            _ => None,
        }
    }

    /// The kind as a word for the interface.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            EntityKind::Point { .. } => "Point",
            EntityKind::Line { .. } => "Line",
            EntityKind::Arc { .. } => "Arc",
            EntityKind::Circle { .. } => "Circle",
        }
    }

    /// Start and sweep angles of an arc (sweep in (0, 2π)).
    pub fn arc_angles(&self) -> Option<(f64, f64)> {
        let EntityKind::Arc { center, start, end } = self.kind else { return None };
        let a0 = start.sub(center).angle();
        let mut sweep = end.sub(center).angle() - a0;
        while sweep <= 1e-9 {
            sweep += std::f64::consts::TAU;
        }
        Some((a0, sweep))
    }

    /// The point at the middle of the entity, used to place relation glyphs.
    pub fn midpoint(&self) -> P2 {
        match self.kind {
            EntityKind::Point { p } => p,
            EntityKind::Line { a, b } => a.mid(b),
            EntityKind::Arc { center, start, .. } => {
                let (a0, sweep) = self.arc_angles().unwrap_or((0.0, 0.0));
                center.add(P2::from_angle(a0 + sweep / 2.0).scale(center.dist(start)))
            }
            EntityKind::Circle { center, radius } => center.add(P2::new(0.0, radius)),
        }
    }

    /// Nearest point on the entity's curve to `p` and the distance to it.
    pub fn nearest(&self, p: P2) -> (P2, f64) {
        match self.kind {
            EntityKind::Point { p: q } => (q, q.dist(p)),
            EntityKind::Line { a, b } => {
                let ab = b.sub(a);
                let t = if ab.len() < 1e-12 { 0.0 } else { p.sub(a).dot(ab) / ab.dot(ab) };
                let q = a.lerp(b, t.clamp(0.0, 1.0));
                (q, q.dist(p))
            }
            EntityKind::Circle { center, radius } => {
                let d = p.sub(center);
                let q = if d.len() < 1e-12 {
                    center.add(P2::new(radius, 0.0))
                } else {
                    center.add(d.normalized().scale(radius))
                };
                (q, q.dist(p))
            }
            EntityKind::Arc { center, start, end } => {
                let r = center.dist(start);
                let (a0, sweep) = self.arc_angles().unwrap_or((0.0, 0.0));
                let mut a = p.sub(center).angle() - a0;
                while a < 0.0 {
                    a += std::f64::consts::TAU;
                }
                if a <= sweep {
                    let q = center.add(P2::from_angle(a0 + a).scale(r));
                    (q, q.dist(p))
                } else {
                    let (ds, de) = (start.dist(p), end.dist(p));
                    if ds < de { (start, ds) } else { (end, de) }
                }
            }
        }
    }

    /// Points along the curve for drawing and hit testing.
    pub fn polyline(&self, segments: usize) -> Vec<P2> {
        match self.kind {
            EntityKind::Point { p } => vec![p],
            EntityKind::Line { a, b } => vec![a, b],
            EntityKind::Circle { center, radius } => (0..=segments)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / segments as f64;
                    center.add(P2::from_angle(a).scale(radius))
                })
                .collect(),
            EntityKind::Arc { center, start, .. } => {
                let r = center.dist(start);
                let (a0, sweep) = self.arc_angles().unwrap_or((0.0, 0.0));
                let n = ((segments as f64 * sweep / std::f64::consts::TAU).ceil() as usize).max(4);
                (0..=n).map(|i| center.add(P2::from_angle(a0 + sweep * i as f64 / n as f64).scale(r))).collect()
            }
        }
    }
}

/// A point of an entity: the entity and the point index (see [`Entity::point`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PointRef {
    /// The entity that owns the point.
    pub entity: EntityId,
    /// The point index within the entity.
    pub index: u8,
}

impl PointRef {
    /// A reference to point `index` of `entity`.
    pub const fn new(entity: EntityId, index: u8) -> Self {
        PointRef { entity, index }
    }

    /// The sketch origin.
    pub const ORIGIN: PointRef = PointRef::new(ORIGIN, 0);
}

/// Which measurement a distance dimension takes between its two points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceKind {
    /// The straight distance.
    Aligned,
    /// The difference in u.
    Horizontal,
    /// The difference in v.
    Vertical,
}

/// The two ends a horizontal or vertical relation aligns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Align {
    /// The two ends of a line.
    Line(EntityId),
    /// Two points, of the same entity or of different ones.
    Points(PointRef, PointRef),
}

/// What a constraint says. Relations hold geometry in place; dimensions also carry
/// a value the user edits and a label position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Two points are the same point.
    Coincident {
        /// First point.
        a: PointRef,
        /// Second point.
        b: PointRef,
    },
    /// The point lies on the line (extended) or on the arc or circle.
    PointOn {
        /// The point.
        point: PointRef,
        /// The line, arc, circle or point it lies on.
        entity: EntityId,
    },
    /// The target runs parallel to the u axis.
    Horizontal {
        /// A line or two points.
        target: Align,
    },
    /// The target runs parallel to the v axis.
    Vertical {
        /// A line or two points.
        target: Align,
    },
    /// Two lines are parallel.
    Parallel {
        /// First line.
        a: EntityId,
        /// Second line.
        b: EntityId,
    },
    /// Two lines are at a right angle.
    Perpendicular {
        /// First line.
        a: EntityId,
        /// Second line.
        b: EntityId,
    },
    /// A line touches a curve, or two curves touch.
    Tangent {
        /// First entity.
        a: EntityId,
        /// Second entity.
        b: EntityId,
    },
    /// Equal lengths of two lines, or equal radii of two arcs or circles.
    Equal {
        /// First entity.
        a: EntityId,
        /// Second entity.
        b: EntityId,
    },
    /// The point sits halfway along the line.
    Midpoint {
        /// The point.
        point: PointRef,
        /// The line.
        line: EntityId,
    },
    /// Two arcs or circles share a centre.
    Concentric {
        /// First curve.
        a: EntityId,
        /// Second curve.
        b: EntityId,
    },
    /// The point stays where it is.
    Fix {
        /// The point.
        point: PointRef,
        /// Where it is held.
        at: P2,
    },
    /// A driving distance between two points.
    Distance {
        /// First point.
        a: PointRef,
        /// Second point.
        b: PointRef,
        /// Which measurement.
        kind: DistanceKind,
        /// The distance, positive.
        value: f64,
        /// Where the label is drawn.
        label: P2,
    },
    /// A driving perpendicular distance between a point and a line.
    PointLineDistance {
        /// The point.
        point: PointRef,
        /// The line.
        line: EntityId,
        /// The distance, positive.
        value: f64,
        /// Where the label is drawn.
        label: P2,
    },
    /// A driving angle between two lines, in degrees.
    Angle {
        /// First line.
        a: EntityId,
        /// Second line.
        b: EntityId,
        /// The angle in degrees.
        value: f64,
        /// Where the label is drawn.
        label: P2,
    },
    /// A driving radius (or diameter) of an arc or circle.
    Radius {
        /// The arc or circle.
        entity: EntityId,
        /// The radius, or the diameter when `diameter` is set.
        value: f64,
        /// The value is a diameter.
        diameter: bool,
        /// Where the label is drawn.
        label: P2,
    },
}

/// A constraint in a sketch.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    /// Its id, unique within the sketch.
    pub id: ConstraintId,
    /// What it says.
    pub kind: ConstraintKind,
}

impl Constraint {
    /// True for constraints with a value: distances, angles and radii.
    pub fn is_dimension(&self) -> bool {
        matches!(
            self.kind,
            ConstraintKind::Distance { .. }
                | ConstraintKind::PointLineDistance { .. }
                | ConstraintKind::Angle { .. }
                | ConstraintKind::Radius { .. }
        )
    }

    /// The value of a dimension.
    pub fn value(&self) -> Option<f64> {
        match self.kind {
            ConstraintKind::Distance { value, .. }
            | ConstraintKind::PointLineDistance { value, .. }
            | ConstraintKind::Angle { value, .. }
            | ConstraintKind::Radius { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Sets the value of a dimension. Relations are left alone.
    pub fn set_value(&mut self, v: f64) {
        match &mut self.kind {
            ConstraintKind::Distance { value, .. }
            | ConstraintKind::PointLineDistance { value, .. }
            | ConstraintKind::Angle { value, .. }
            | ConstraintKind::Radius { value, .. } => *value = v,
            _ => {}
        }
    }

    /// Where a dimension's label is drawn.
    pub fn label(&self) -> Option<P2> {
        match self.kind {
            ConstraintKind::Distance { label, .. }
            | ConstraintKind::PointLineDistance { label, .. }
            | ConstraintKind::Angle { label, .. }
            | ConstraintKind::Radius { label, .. } => Some(label),
            _ => None,
        }
    }

    /// Moves a dimension's label. Relations are left alone.
    pub fn set_label(&mut self, p: P2) {
        match &mut self.kind {
            ConstraintKind::Distance { label, .. }
            | ConstraintKind::PointLineDistance { label, .. }
            | ConstraintKind::Angle { label, .. }
            | ConstraintKind::Radius { label, .. } => *label = p,
            _ => {}
        }
    }

    /// The constraint's name for the interface.
    pub fn name(&self) -> &'static str {
        match self.kind {
            ConstraintKind::Coincident { .. } => "Coincident",
            ConstraintKind::PointOn { .. } => "On entity",
            ConstraintKind::Horizontal { .. } => "Horizontal",
            ConstraintKind::Vertical { .. } => "Vertical",
            ConstraintKind::Parallel { .. } => "Parallel",
            ConstraintKind::Perpendicular { .. } => "Perpendicular",
            ConstraintKind::Tangent { .. } => "Tangent",
            ConstraintKind::Equal { .. } => "Equal",
            ConstraintKind::Midpoint { .. } => "Midpoint",
            ConstraintKind::Concentric { .. } => "Concentric",
            ConstraintKind::Fix { .. } => "Fixed",
            ConstraintKind::Distance { kind: DistanceKind::Aligned, .. } => "Distance",
            ConstraintKind::Distance { kind: DistanceKind::Horizontal, .. } => "Horizontal distance",
            ConstraintKind::Distance { kind: DistanceKind::Vertical, .. } => "Vertical distance",
            ConstraintKind::PointLineDistance { .. } => "Distance to line",
            ConstraintKind::Angle { .. } => "Angle",
            ConstraintKind::Radius { diameter: false, .. } => "Radius",
            ConstraintKind::Radius { diameter: true, .. } => "Diameter",
        }
    }

    /// Every entity the constraint touches.
    pub fn entities(&self) -> Vec<EntityId> {
        let mut v = match self.kind {
            ConstraintKind::Coincident { a, b } => vec![a.entity, b.entity],
            ConstraintKind::PointOn { point, entity } => vec![point.entity, entity],
            ConstraintKind::Horizontal { target } | ConstraintKind::Vertical { target } => match target {
                Align::Line(l) => vec![l],
                Align::Points(a, b) => vec![a.entity, b.entity],
            },
            ConstraintKind::Parallel { a, b }
            | ConstraintKind::Perpendicular { a, b }
            | ConstraintKind::Tangent { a, b }
            | ConstraintKind::Equal { a, b }
            | ConstraintKind::Concentric { a, b }
            | ConstraintKind::Angle { a, b, .. } => vec![a, b],
            ConstraintKind::Midpoint { point, line } => vec![point.entity, line],
            ConstraintKind::Fix { point, .. } => vec![point.entity],
            ConstraintKind::Distance { a, b, .. } => vec![a.entity, b.entity],
            ConstraintKind::PointLineDistance { point, line, .. } => vec![point.entity, line],
            ConstraintKind::Radius { entity, .. } => vec![entity],
        };
        v.dedup();
        v
    }

    /// The points the constraint touches directly.
    pub fn points(&self) -> Vec<PointRef> {
        match self.kind {
            ConstraintKind::Coincident { a, b } | ConstraintKind::Distance { a, b, .. } => vec![a, b],
            ConstraintKind::PointOn { point, .. }
            | ConstraintKind::Midpoint { point, .. }
            | ConstraintKind::Fix { point, .. }
            | ConstraintKind::PointLineDistance { point, .. } => vec![point],
            ConstraintKind::Horizontal { target: Align::Points(a, b) }
            | ConstraintKind::Vertical { target: Align::Points(a, b) } => vec![a, b],
            _ => Vec::new(),
        }
    }
}

/// How well the constraints pin the sketch down, from the last solve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum SolveStatus {
    /// Nothing is drawn.
    #[default]
    Empty,
    /// Some entity can still move.
    UnderDefined,
    /// Nothing can move.
    FullyDefined,
    /// Constraints that cannot all hold at once; the listed ones gave way.
    OverDefined {
        /// The constraints that do not hold, sorted.
        conflicting: Vec<ConstraintId>,
    },
    /// The solver could not run.
    Failed(String),
}

impl SolveStatus {
    /// The status as words for the interface.
    pub fn label(&self) -> &'static str {
        match self {
            SolveStatus::Empty => "Empty",
            SolveStatus::UnderDefined => "Under defined",
            SolveStatus::FullyDefined => "Fully defined",
            SolveStatus::OverDefined { .. } => "Over defined",
            SolveStatus::Failed(_) => "Solver failed",
        }
    }
}

/// Entities and constraints on one sketch plane. Entity 0 is the origin.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sketch {
    /// Every entity, the origin first.
    pub entities: Vec<Entity>,
    /// Every constraint, in the order they were added.
    pub constraints: Vec<Constraint>,
    next_entity: EntityId,
    next_constraint: ConstraintId,
    /// Entities that still have a free degree of freedom after the last solve.
    #[serde(skip)]
    pub free: Vec<EntityId>,
    /// The outcome of the last solve.
    #[serde(skip)]
    pub status: SolveStatus,
}

impl Sketch {
    /// An empty sketch holding only the origin.
    pub fn new() -> Self {
        Sketch {
            entities: vec![Entity { id: ORIGIN, kind: EntityKind::Point { p: P2::ZERO }, construction: true }],
            constraints: Vec::new(),
            next_entity: 1,
            next_constraint: 1,
            free: Vec::new(),
            status: SolveStatus::Empty,
        }
    }

    /// Repairs counters after deserialisation, where the private fields may be stale.
    pub fn normalise(&mut self) {
        if !self.entities.iter().any(|e| e.id == ORIGIN) {
            self.entities.insert(0, Entity { id: ORIGIN, kind: EntityKind::Point { p: P2::ZERO }, construction: true });
        }
        self.next_entity = self.entities.iter().map(|e| e.id + 1).max().unwrap_or(1).max(1);
        self.next_constraint = self.constraints.iter().map(|c| c.id + 1).max().unwrap_or(1).max(1);
    }

    /// The entity with this id.
    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    /// The entity with this id, to change.
    pub fn entity_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    /// The constraint with this id.
    pub fn constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.iter().find(|c| c.id == id)
    }

    /// The constraint with this id, to change.
    pub fn constraint_mut(&mut self, id: ConstraintId) -> Option<&mut Constraint> {
        self.constraints.iter_mut().find(|c| c.id == id)
    }

    /// Where a point is.
    pub fn point(&self, r: PointRef) -> Option<P2> {
        self.entity(r.entity)?.point(r.index)
    }

    /// Adds an entity and returns its new id.
    pub fn add(&mut self, kind: EntityKind, construction: bool) -> EntityId {
        let id = self.next_entity;
        self.next_entity += 1;
        self.entities.push(Entity { id, kind, construction });
        id
    }

    /// Adds a constraint unless an identical one exists; returns its id.
    pub fn constrain(&mut self, kind: ConstraintKind) -> ConstraintId {
        if let Some(c) = self.constraints.iter().find(|c| same_relation(&c.kind, &kind)) {
            return c.id;
        }
        let id = self.next_constraint;
        self.next_constraint += 1;
        self.constraints.push(Constraint { id, kind });
        id
    }

    /// Removes a constraint. An unknown id does nothing.
    pub fn remove_constraint(&mut self, id: ConstraintId) {
        self.constraints.retain(|c| c.id != id);
    }

    /// Removes an entity and every constraint that mentions it.
    pub fn remove_entity(&mut self, id: EntityId) {
        if id == ORIGIN {
            return;
        }
        self.entities.retain(|e| e.id != id);
        self.constraints.retain(|c| !c.entities().contains(&id));
    }

    /// Entities other than the origin.
    pub fn drawn(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter().filter(|e| e.id != ORIGIN)
    }

    /// True when nothing but the origin is in the sketch.
    pub fn is_empty(&self) -> bool {
        self.drawn().next().is_none()
    }

    /// Constraints that touch the entity.
    pub fn constraints_of(&self, id: EntityId) -> impl Iterator<Item = &Constraint> {
        self.constraints.iter().filter(move |c| c.entities().contains(&id))
    }

    /// The group of points joined to `r` by coincident constraints, including `r`.
    pub fn coincident_group(&self, r: PointRef) -> Vec<PointRef> {
        let mut group = vec![r];
        let mut changed = true;
        while changed {
            changed = false;
            for c in &self.constraints {
                if let ConstraintKind::Coincident { a, b } = c.kind {
                    let (ia, ib) = (group.contains(&a), group.contains(&b));
                    if ia && !ib {
                        group.push(b);
                        changed = true;
                    } else if ib && !ia {
                        group.push(a);
                        changed = true;
                    }
                }
            }
        }
        group
    }

    /// Bounding box of all drawn geometry.
    pub fn bounds(&self) -> Option<(P2, P2)> {
        let mut acc: Option<(P2, P2)> = None;
        for e in self.drawn() {
            for p in e.polyline(24) {
                acc = Some(match acc {
                    None => (p, p),
                    Some((lo, hi)) => (P2::new(lo.u.min(p.u), lo.v.min(p.v)), P2::new(hi.u.max(p.u), hi.v.max(p.v))),
                });
            }
        }
        acc
    }

    /// Dimensions and relations counted separately, for the status line.
    pub fn counts(&self) -> (usize, usize) {
        let dims = self.constraints.iter().filter(|c| c.is_dimension()).count();
        (dims, self.constraints.len() - dims)
    }
}

/// True when two relations say the same thing (order of operands ignored).
fn same_relation(a: &ConstraintKind, b: &ConstraintKind) -> bool {
    use ConstraintKind::*;
    match (a, b) {
        (Coincident { a: a1, b: b1 }, Coincident { a: a2, b: b2 }) => (a1 == a2 && b1 == b2) || (a1 == b2 && b1 == a2),
        (PointOn { point: p1, entity: e1 }, PointOn { point: p2, entity: e2 }) => p1 == p2 && e1 == e2,
        (Horizontal { target: t1 }, Horizontal { target: t2 }) | (Vertical { target: t1 }, Vertical { target: t2 }) => {
            match (t1, t2) {
                (Align::Line(l1), Align::Line(l2)) => l1 == l2,
                (Align::Points(a1, b1), Align::Points(a2, b2)) => (a1 == a2 && b1 == b2) || (a1 == b2 && b1 == a2),
                _ => false,
            }
        }
        (Parallel { a: a1, b: b1 }, Parallel { a: a2, b: b2 })
        | (Perpendicular { a: a1, b: b1 }, Perpendicular { a: a2, b: b2 })
        | (Tangent { a: a1, b: b1 }, Tangent { a: a2, b: b2 })
        | (Equal { a: a1, b: b1 }, Equal { a: a2, b: b2 })
        | (Concentric { a: a1, b: b1 }, Concentric { a: a2, b: b2 }) => {
            (a1 == a2 && b1 == b2) || (a1 == b2 && b1 == a2)
        }
        (Midpoint { point: p1, line: l1 }, Midpoint { point: p2, line: l2 }) => p1 == p2 && l1 == l2,
        (Fix { point: p1, .. }, Fix { point: p2, .. }) => p1 == p2,
        _ => false,
    }
}

/// Formats a millimetre value for labels: integers without decimals.
pub fn fmt_value(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 { format!("{}", v.round() as i64) } else { format!("{v:.2}") }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn entities_expose_their_points() {
        let mut s = Sketch::new();
        let l = s.add(EntityKind::Line { a: P2::new(0.0, 0.0), b: P2::new(10.0, 0.0) }, false);
        assert_eq!(s.point(PointRef::new(l, 1)), Some(P2::new(10.0, 0.0)));
        let arc = s.add(EntityKind::Arc { center: P2::ZERO, start: P2::new(5.0, 0.0), end: P2::new(0.0, 5.0) }, false);
        let (a0, sweep) = s.entity(arc).unwrap().arc_angles().unwrap();
        assert!(a0.abs() < 1e-9 && (sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        let mid = s.entity(arc).unwrap().midpoint();
        assert!((mid.u - mid.v).abs() < 1e-9);
    }

    #[test]
    fn duplicate_relations_collapse_and_deletion_cascades() {
        let mut s = Sketch::new();
        let a = s.add(EntityKind::Line { a: P2::ZERO, b: P2::new(1.0, 0.0) }, false);
        let b = s.add(EntityKind::Line { a: P2::new(1.0, 0.0), b: P2::new(1.0, 1.0) }, false);
        let c1 = s.constrain(ConstraintKind::Coincident { a: PointRef::new(a, 1), b: PointRef::new(b, 0) });
        let c2 = s.constrain(ConstraintKind::Coincident { a: PointRef::new(b, 0), b: PointRef::new(a, 1) });
        assert_eq!(c1, c2);
        s.constrain(ConstraintKind::Horizontal { target: Align::Line(a) });
        assert_eq!(s.constraints.len(), 2);
        s.remove_entity(b);
        assert_eq!(s.constraints.len(), 1);
        assert_eq!(s.coincident_group(PointRef::new(a, 1)).len(), 1);
    }
}
