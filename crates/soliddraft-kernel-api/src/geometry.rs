//! Plain geometric value types that cross the kernel boundary.

use serde::{Deserialize, Serialize};

/// A point or vector in model space, in millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 3]", into = "[f64; 3]")]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Vec3 {
    /// The origin.
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    /// The unit X axis.
    pub const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    /// The unit Y axis.
    pub const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    /// The unit Z axis.
    pub const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

    /// Builds a vector from its components.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    /// Dot product.
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    /// Cross product.
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }

    /// Euclidean length.
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// This vector scaled to unit length, or `None` when it is (nearly) zero or not finite.
    pub fn normalized(self) -> Option<Vec3> {
        let len = self.length();
        (len.is_finite() && len > 1e-12).then(|| self * (1.0 / len))
    }

    /// True when every component is finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Distance to another point.
    pub fn distance(self, o: Vec3) -> f64 {
        (self - o).length()
    }
}

impl From<[f64; 3]> for Vec3 {
    fn from(a: [f64; 3]) -> Self {
        Vec3::new(a[0], a[1], a[2])
    }
}

impl From<Vec3> for [f64; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl std::ops::Mul<f64> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl std::ops::Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// A point in the 2D coordinates of a sketch plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f64; 2]", into = "[f64; 2]")]
pub struct Point2 {
    /// Coordinate along the plane's X direction.
    pub u: f64,
    /// Coordinate along the plane's Y direction.
    pub v: f64,
}

impl Point2 {
    /// Builds a point from its plane coordinates.
    pub const fn new(u: f64, v: f64) -> Self {
        Point2 { u, v }
    }
}

impl From<[f64; 2]> for Point2 {
    fn from(a: [f64; 2]) -> Self {
        Point2::new(a[0], a[1])
    }
}

impl From<Point2> for [f64; 2] {
    fn from(p: Point2) -> Self {
        [p.u, p.v]
    }
}

/// A sketch plane: an origin and a right-handed frame whose Z axis is `normal`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    /// Origin of the plane's 2D coordinates.
    pub origin: Vec3,
    /// Unit normal. Extrusion goes along it by default.
    pub normal: Vec3,
    /// Unit direction of the plane's U axis, perpendicular to `normal`.
    pub x_dir: Vec3,
}

impl Plane {
    /// The world XY plane (normal +Z).
    pub const XY: Plane = Plane { origin: Vec3::ZERO, normal: Vec3::Z, x_dir: Vec3::X };
    /// The world YZ plane (normal +X).
    pub const YZ: Plane = Plane { origin: Vec3::ZERO, normal: Vec3::X, x_dir: Vec3::Y };
    /// The world ZX plane (normal +Y).
    pub const ZX: Plane = Plane { origin: Vec3::ZERO, normal: Vec3::Y, x_dir: Vec3::Z };

    /// The same plane moved so its origin is at `origin`.
    pub fn with_origin(self, origin: Vec3) -> Plane {
        Plane { origin, ..self }
    }

    /// The plane's V axis, `normal x x_dir`.
    pub fn y_dir(&self) -> Vec3 {
        self.normal.cross(self.x_dir)
    }

    /// Maps plane coordinates to model space.
    pub fn to_world(&self, p: Point2) -> Vec3 {
        self.origin + self.x_dir * p.u + self.y_dir() * p.v
    }

    /// Checks that the frame is finite, unit length and orthogonal.
    pub fn is_well_formed(&self) -> bool {
        let unit = |v: Vec3| v.is_finite() && (v.length() - 1.0).abs() < 1e-9;
        self.origin.is_finite() && unit(self.normal) && unit(self.x_dir) && self.normal.dot(self.x_dir).abs() < 1e-9
    }
}

/// An axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Length of the box diagonal.
    pub fn diagonal(&self) -> f64 {
        self.min.distance(self.max)
    }

    /// Centre of the box.
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}
