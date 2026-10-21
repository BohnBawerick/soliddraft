//! Rigid placements in double precision.
//!
//! A component sits in its assembly by a rotation and a translation, in millimetres,
//! with no scale and no mirror: scaling would change engineering dimensions and mass, and
//! a mirrored component is an opposite-hand part of its own. Placements stay `f64` on the
//! CPU; the renderer gets `f32` relative to a nearby origin.

use glam::{DMat3, DQuat, DVec3};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A rigid placement: rotate, then translate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The rotation, a unit quaternion.
    pub rotation: DQuat,
    /// The translation in millimetres.
    pub translation: DVec3,
}

impl Default for Placement {
    fn default() -> Placement {
        Placement::IDENTITY
    }
}

impl Placement {
    /// No rotation, no translation.
    pub const IDENTITY: Placement = Placement { rotation: DQuat::IDENTITY, translation: DVec3::ZERO };

    /// A translation alone.
    pub fn translation(t: DVec3) -> Placement {
        Placement { rotation: DQuat::IDENTITY, translation: t }
    }

    /// A rotation and a translation. The rotation is normalised.
    pub fn new(rotation: DQuat, translation: DVec3) -> Placement {
        Placement { rotation: rotation.normalize(), translation }
    }

    /// True for the identity, exactly.
    pub fn is_identity(&self) -> bool {
        *self == Placement::IDENTITY
    }

    /// `self` after `inner`: a point placed by `inner` within a frame placed by `self`.
    pub fn compose(&self, inner: &Placement) -> Placement {
        Placement {
            rotation: (self.rotation * inner.rotation).normalize(),
            translation: self.translation + self.rotation * inner.translation,
        }
    }

    /// The placement that undoes this one.
    pub fn inverse(&self) -> Placement {
        let r = self.rotation.inverse();
        Placement { rotation: r, translation: -(r * self.translation) }
    }

    /// A point, placed.
    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }

    /// A direction, rotated.
    pub fn rotate(&self, v: DVec3) -> DVec3 {
        self.rotation * v
    }

    /// The rotation as a matrix, columns being the placed axes.
    pub fn matrix(&self) -> DMat3 {
        DMat3::from_quat(self.rotation)
    }
}

/// The JSON shape: `{"translation": [x, y, z], "rotation": [x, y, z, w]}`, each left
/// out when it is the identity's.
#[derive(Serialize, Deserialize)]
struct PlacementJson {
    #[serde(default, skip_serializing_if = "is_zero")]
    translation: [f64; 3],
    #[serde(default = "identity_quat", skip_serializing_if = "is_identity_quat")]
    rotation: [f64; 4],
}

fn is_zero(t: &[f64; 3]) -> bool {
    *t == [0.0; 3]
}

fn identity_quat() -> [f64; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn is_identity_quat(r: &[f64; 4]) -> bool {
    *r == identity_quat()
}

impl Serialize for Placement {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        PlacementJson { translation: self.translation.to_array(), rotation: self.rotation.to_array() }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Placement {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Placement, D::Error> {
        let j = PlacementJson::deserialize(d)?;
        let q = DQuat::from_array(j.rotation);
        let len = q.length();
        if !(len.is_finite() && len > 1e-12) || j.translation.iter().any(|v| !v.is_finite()) {
            return Err(serde::de::Error::custom("a placement needs finite numbers and a non-zero rotation"));
        }
        // Keep a written unit quaternion bit for bit, so files round-trip exactly.
        let rotation = if (len - 1.0).abs() < 1e-12 { q } else { q / len };
        Ok(Placement { rotation, translation: DVec3::from_array(j.translation) })
    }
}

/// An axis-aligned box in double precision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// The low corner.
    pub min: DVec3,
    /// The high corner.
    pub max: DVec3,
}

impl Aabb {
    /// The empty box: grows to whatever is added to it.
    pub const EMPTY: Aabb = Aabb { min: DVec3::INFINITY, max: DVec3::NEG_INFINITY };

    /// The box of some points.
    pub fn of_points(points: impl IntoIterator<Item = DVec3>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in points {
            b.add_point(p);
        }
        b
    }

    /// True when nothing was added.
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }

    /// Grows the box to hold `p`.
    pub fn add_point(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    /// Grows the box to hold `other`.
    pub fn add(&mut self, other: &Aabb) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// The union of two boxes.
    pub fn union(&self, other: &Aabb) -> Aabb {
        let mut b = *self;
        b.add(other);
        b
    }

    /// The centre.
    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    /// The size along each axis.
    pub fn size(&self) -> DVec3 {
        self.max - self.min
    }

    /// The length of the diagonal.
    pub fn diagonal(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.size().length() }
    }

    /// Half the surface area, the cost a bounding volume hierarchy weighs.
    pub fn half_area(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let s = self.size();
        s.x * s.y + s.y * s.z + s.z * s.x
    }

    /// The box around this one once placed: tight for the box's corners, so it holds
    /// whatever the box held.
    pub fn placed(&self, p: &Placement) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        // Arvo's method: the placed centre plus the absolute rotation of the half size.
        let m = p.matrix();
        let half = self.size() * 0.5;
        let c = p.apply(self.center());
        let abs = DMat3::from_cols(m.x_axis.abs(), m.y_axis.abs(), m.z_axis.abs());
        let e = abs * half;
        Aabb { min: c - e, max: c + e }
    }

    /// True when the boxes overlap or touch.
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    /// True when `p` is inside or on the box.
    pub fn contains(&self, p: DVec3) -> bool {
        self.min.cmple(p).all() && p.cmple(self.max).all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    #[test]
    fn placements_compose_and_invert() {
        let a = Placement::new(DQuat::from_rotation_z(FRAC_PI_2), DVec3::new(10.0, 0.0, 0.0));
        let b = Placement::translation(DVec3::new(1.0, 0.0, 0.0));
        let p = a.compose(&b).apply(DVec3::ZERO);
        assert!((p - DVec3::new(10.0, 1.0, 0.0)).length() < 1e-12, "{p}");
        let back = a.inverse().compose(&a);
        assert!(back.translation.length() < 1e-12 && back.rotation.angle_between(DQuat::IDENTITY) < 1e-12);
    }

    #[test]
    fn placements_write_only_what_differs_from_the_identity() {
        assert_eq!(serde_json::to_string(&Placement::IDENTITY).unwrap(), "{}");
        let t = Placement::translation(DVec3::new(1.5, 0.0, -2.0));
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"{"translation":[1.5,0.0,-2.0]}"#);
        let r = Placement::new(DQuat::from_rotation_x(0.3), DVec3::new(0.1, 0.2, 0.3));
        let back: Placement = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r, "bit for bit");
        let unnormalised: Placement = serde_json::from_str(r#"{"rotation":[0,0,0,2]}"#).unwrap();
        assert_eq!(unnormalised.rotation, DQuat::IDENTITY);
        assert!(serde_json::from_str::<Placement>(r#"{"rotation":[0,0,0,0]}"#).is_err());
    }

    #[test]
    fn placed_boxes_hold_their_corners() {
        let b = Aabb { min: DVec3::new(0.0, 0.0, 0.0), max: DVec3::new(10.0, 2.0, 1.0) };
        let p = Placement::new(DQuat::from_rotation_z(0.7), DVec3::new(5.0, -3.0, 2.0));
        let placed = b.placed(&p);
        for i in 0..8 {
            let c = DVec3::new(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
            let q = p.apply(c);
            assert!(placed.min.cmple(q + 1e-9).all() && q.cmple(placed.max + 1e-9).all());
        }
        assert!(Aabb::EMPTY.placed(&p).is_empty());
        assert!(Aabb::EMPTY.union(&b) == b);
    }
}
