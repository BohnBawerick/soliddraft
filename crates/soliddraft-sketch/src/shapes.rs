//! Builders for common fully constrained shapes, used by the sketch tools and
//! the model. Each shape is held in place relative to the origin, so solving
//! with analysis leaves it [`SolveStatus::FullyDefined`](crate::SolveStatus).

use serde::{Deserialize, Serialize};

use crate::{Align, ConstraintId, ConstraintKind, DistanceKind, EntityId, EntityKind, P2, PointRef, Sketch};

/// What a shape builder added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// Every entity added, in the order each builder documents.
    pub entities: Vec<EntityId>,
    /// Every constraint added, dimensions included.
    pub constraints: Vec<ConstraintId>,
    /// The dimension constraints a user edits, in the order each builder documents.
    pub dimensions: Vec<ConstraintId>,
}

/// Why a shape builder refused its input. The sketch is unchanged when it does.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SketchError {
    /// A size is zero, negative or not finite.
    #[error("{what} {value} is not a positive number")]
    NonPositive {
        /// Which size.
        what: &'static str,
        /// The value given.
        value: f64,
    },
    /// The corner radius leaves no straight edge.
    #[error("radius {radius} is too large; it must be below {limit}")]
    RadiusTooLarge {
        /// The radius given.
        radius: f64,
        /// Half the shorter side.
        limit: f64,
    },
    /// A slot is not longer than it is wide.
    #[error("slot length {length} must be greater than its width {width}")]
    SlotTooShort {
        /// The overall length given.
        length: f64,
        /// The width given.
        width: f64,
    },
    /// A closed loop needs at least three points.
    #[error("a loop needs at least 3 points, got {0}")]
    TooFewPoints(usize),
}

fn positive(what: &'static str, value: f64) -> Result<f64, SketchError> {
    if value.is_finite() && value > 0.0 { Ok(value) } else { Err(SketchError::NonPositive { what, value }) }
}

/// Collects ids while a builder runs.
struct Recorder<'a> {
    sketch: &'a mut Sketch,
    shape: Shape,
}

impl Recorder<'_> {
    fn add(&mut self, kind: EntityKind, construction: bool) -> EntityId {
        let id = self.sketch.add(kind, construction);
        self.shape.entities.push(id);
        id
    }

    fn line(&mut self, a: P2, b: P2) -> EntityId {
        self.add(EntityKind::Line { a, b }, false)
    }

    fn arc(&mut self, center: P2, start: P2, end: P2) -> EntityId {
        self.add(EntityKind::Arc { center, start, end }, false)
    }

    fn constrain(&mut self, kind: ConstraintKind) -> ConstraintId {
        let id = self.sketch.constrain(kind);
        self.shape.constraints.push(id);
        id
    }

    fn join(&mut self, a: PointRef, b: PointRef) {
        self.constrain(ConstraintKind::Coincident { a, b });
    }

    fn dimension(&mut self, kind: ConstraintKind) -> ConstraintId {
        let id = self.constrain(kind);
        self.shape.dimensions.push(id);
        id
    }

    fn distance(&mut self, a: PointRef, b: PointRef, kind: DistanceKind, value: f64, label: P2) -> ConstraintId {
        self.dimension(ConstraintKind::Distance { a, b, kind, value, label })
    }

    /// Holds a construction point at `at` and makes it the midpoint of a construction
    /// line joined to `a` and `b`, which pins the shape's centre.
    fn center_on(&mut self, at: P2, a: PointRef, b: PointRef) {
        let (pa, pb) = (self.sketch.point(a).unwrap_or(at), self.sketch.point(b).unwrap_or(at));
        let center = self.add(EntityKind::Point { p: at }, true);
        let diagonal = self.add(EntityKind::Line { a: pa, b: pb }, true);
        self.join(PointRef::new(diagonal, 0), a);
        self.join(PointRef::new(diagonal, 1), b);
        self.constrain(ConstraintKind::Midpoint { point: PointRef::new(center, 0), line: diagonal });
        self.constrain(ConstraintKind::Fix { point: PointRef::new(center, 0), at });
    }
}

impl Sketch {
    fn record(&mut self) -> Recorder<'_> {
        Recorder { sketch: self, shape: Shape::default() }
    }

    /// An axis-aligned rectangle centred on `center`.
    ///
    /// Entities: lines bottom, right, top, left, each running counter-clockwise,
    /// then the construction centre point and diagonal.
    /// Dimensions: width, height.
    pub fn rectangle(&mut self, center: P2, width: f64, height: f64) -> Result<Shape, SketchError> {
        let (hw, hh) = (positive("width", width)? / 2.0, positive("height", height)? / 2.0);
        let c = [
            center + P2::new(-hw, -hh),
            center + P2::new(hw, -hh),
            center + P2::new(hw, hh),
            center + P2::new(-hw, hh),
        ];
        let mut r = self.record();
        let sides: Vec<EntityId> = (0..4).map(|i| r.line(c[i], c[(i + 1) % 4])).collect();
        for i in 0..4 {
            r.join(PointRef::new(sides[i], 1), PointRef::new(sides[(i + 1) % 4], 0));
        }
        r.constrain(ConstraintKind::Horizontal { target: Align::Line(sides[0]) });
        r.constrain(ConstraintKind::Vertical { target: Align::Line(sides[1]) });
        r.constrain(ConstraintKind::Horizontal { target: Align::Line(sides[2]) });
        r.constrain(ConstraintKind::Vertical { target: Align::Line(sides[3]) });
        let gap = 0.1 * width.min(height) + 2.0;
        r.distance(
            PointRef::new(sides[0], 0),
            PointRef::new(sides[0], 1),
            DistanceKind::Horizontal,
            width,
            P2::new(center.u, center.v - hh - gap),
        );
        r.distance(
            PointRef::new(sides[1], 0),
            PointRef::new(sides[1], 1),
            DistanceKind::Vertical,
            height,
            P2::new(center.u + hw + gap, center.v),
        );
        r.center_on(center, PointRef::new(sides[0], 0), PointRef::new(sides[1], 1));
        Ok(r.shape)
    }

    /// An axis-aligned rectangle with its four corners rounded to `radius`.
    ///
    /// Entities: lines bottom, right, top, left, each running counter-clockwise;
    /// arcs bottom-right, top-right, top-left, bottom-left, arc `i` running from the
    /// end of line `i` to the start of the next; then the construction centre point
    /// and diagonal. The lines are tangent to the arcs and the arcs have equal radii.
    /// Dimensions: width, height, radius.
    pub fn rounded_rectangle(
        &mut self,
        center: P2,
        width: f64,
        height: f64,
        radius: f64,
    ) -> Result<Shape, SketchError> {
        let (hw, hh) = (positive("width", width)? / 2.0, positive("height", height)? / 2.0);
        let rad = positive("radius", radius)?;
        if rad >= hw.min(hh) {
            return Err(SketchError::RadiusTooLarge { radius, limit: hw.min(hh) });
        }
        let (iu, iv) = (hw - rad, hh - rad);
        // Tangent points, counter-clockwise from the left end of the bottom edge.
        let t = [
            P2::new(-iu, -hh),
            P2::new(iu, -hh),
            P2::new(hw, -iv),
            P2::new(hw, iv),
            P2::new(iu, hh),
            P2::new(-iu, hh),
            P2::new(-hw, iv),
            P2::new(-hw, -iv),
        ]
        .map(|p| center + p);
        let k = [P2::new(iu, -iv), P2::new(iu, iv), P2::new(-iu, iv), P2::new(-iu, -iv)].map(|p| center + p);
        let mut r = self.record();
        let lines: Vec<EntityId> = (0..4).map(|i| r.line(t[2 * i], t[2 * i + 1])).collect();
        let arcs: Vec<EntityId> = (0..4).map(|i| r.arc(k[i], t[2 * i + 1], t[(2 * i + 2) % 8])).collect();
        for i in 0..4 {
            r.join(PointRef::new(lines[i], 1), PointRef::new(arcs[i], 1));
            r.join(PointRef::new(arcs[i], 2), PointRef::new(lines[(i + 1) % 4], 0));
        }
        r.constrain(ConstraintKind::Horizontal { target: Align::Line(lines[0]) });
        r.constrain(ConstraintKind::Vertical { target: Align::Line(lines[1]) });
        r.constrain(ConstraintKind::Horizontal { target: Align::Line(lines[2]) });
        r.constrain(ConstraintKind::Vertical { target: Align::Line(lines[3]) });
        for i in 0..4 {
            r.constrain(ConstraintKind::Tangent { a: lines[i], b: arcs[i] });
            r.constrain(ConstraintKind::Tangent { a: lines[(i + 1) % 4], b: arcs[i] });
        }
        for &arc in &arcs[1..] {
            r.constrain(ConstraintKind::Equal { a: arcs[0], b: arc });
        }
        let gap = 0.1 * width.min(height) + 2.0;
        r.distance(
            PointRef::new(lines[3], 0),
            PointRef::new(lines[1], 0),
            DistanceKind::Horizontal,
            width,
            P2::new(center.u, center.v - hh - gap),
        );
        r.distance(
            PointRef::new(lines[0], 0),
            PointRef::new(lines[2], 0),
            DistanceKind::Vertical,
            height,
            P2::new(center.u + hw + gap, center.v),
        );
        r.dimension(ConstraintKind::Radius {
            entity: arcs[0],
            value: rad,
            diameter: false,
            label: center + P2::new(hw + gap, -hh - gap),
        });
        r.center_on(center, PointRef::new(lines[3], 1), PointRef::new(lines[1], 1));
        Ok(r.shape)
    }

    /// A circle of `diameter` with its centre held at `center`.
    ///
    /// Entities: the circle. Dimensions: its diameter.
    pub fn circle_at(&mut self, center: P2, diameter: f64) -> Result<Shape, SketchError> {
        let radius = positive("diameter", diameter)? / 2.0;
        let mut r = self.record();
        let circle = r.add(EntityKind::Circle { center, radius }, false);
        r.constrain(ConstraintKind::Fix { point: PointRef::new(circle, 0), at: center });
        r.dimension(ConstraintKind::Radius {
            entity: circle,
            value: diameter,
            diameter: true,
            label: center + P2::from_angle(std::f64::consts::FRAC_PI_4).scale(radius + 0.5 * radius.min(10.0)),
        });
        Ok(r.shape)
    }

    /// A horizontal stadium slot centred on `center`. `length` is the overall length
    /// from end to end and `width` the diameter of the round ends.
    ///
    /// Entities: lines bottom (left to right) and top (right to left), arcs right and
    /// left, then the construction centre point and the construction line between the
    /// arc centres. The lines are tangent to both arcs and the arcs have equal radii.
    /// Dimensions: distance between the arc centres (`length - width`), arc diameter
    /// (`width`).
    pub fn slot(&mut self, center: P2, length: f64, width: f64) -> Result<Shape, SketchError> {
        let rad = positive("width", width)? / 2.0;
        if positive("length", length)? <= width {
            return Err(SketchError::SlotTooShort { length, width });
        }
        let half = (length - width) / 2.0;
        let p =
            [P2::new(-half, -rad), P2::new(half, -rad), P2::new(half, rad), P2::new(-half, rad)].map(|q| center + q);
        let (kr, kl) = (center + P2::new(half, 0.0), center + P2::new(-half, 0.0));
        let mut r = self.record();
        let bottom = r.line(p[0], p[1]);
        let top = r.line(p[2], p[3]);
        let right = r.arc(kr, p[1], p[2]);
        let left = r.arc(kl, p[3], p[0]);
        r.join(PointRef::new(bottom, 1), PointRef::new(right, 1));
        r.join(PointRef::new(right, 2), PointRef::new(top, 0));
        r.join(PointRef::new(top, 1), PointRef::new(left, 1));
        r.join(PointRef::new(left, 2), PointRef::new(bottom, 0));
        for (line, arc) in [(bottom, right), (top, right), (top, left), (bottom, left)] {
            r.constrain(ConstraintKind::Tangent { a: line, b: arc });
        }
        r.constrain(ConstraintKind::Equal { a: right, b: left });
        let (cl, cr) = (PointRef::new(left, 0), PointRef::new(right, 0));
        r.constrain(ConstraintKind::Horizontal { target: Align::Points(cl, cr) });
        let gap = rad + 2.0;
        r.distance(cl, cr, DistanceKind::Horizontal, length - width, P2::new(center.u, center.v - gap - rad));
        r.dimension(ConstraintKind::Radius {
            entity: right,
            value: width,
            diameter: true,
            label: kr + P2::from_angle(std::f64::consts::FRAC_PI_4).scale(gap),
        });
        r.center_on(center, cl, cr);
        Ok(r.shape)
    }

    /// A closed chain of lines through `points`, joined by coincident constraints.
    ///
    /// Entities: the lines, line `i` running from point `i` to point `i + 1`.
    /// No dimensions.
    pub fn polyline_loop(&mut self, points: &[P2]) -> Result<Shape, SketchError> {
        if points.len() < 3 {
            return Err(SketchError::TooFewPoints(points.len()));
        }
        let n = points.len();
        let mut r = self.record();
        let lines: Vec<EntityId> = (0..n).map(|i| r.line(points[i], points[(i + 1) % n])).collect();
        for i in 0..n {
            r.join(PointRef::new(lines[i], 1), PointRef::new(lines[(i + 1) % n], 0));
        }
        Ok(r.shape)
    }
}
