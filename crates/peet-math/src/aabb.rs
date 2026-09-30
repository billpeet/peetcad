use glam::DVec3;
use serde::{Deserialize, Serialize};

/// An axis-aligned bounding box. The default value is empty.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Aabb {
    /// A box containing nothing. Extending it with a point gives a box around that point.
    pub const EMPTY: Self = Self {
        min: DVec3::splat(f64::INFINITY),
        max: DVec3::splat(f64::NEG_INFINITY),
    };

    pub fn from_points(points: impl IntoIterator<Item = DVec3>) -> Self {
        let mut aabb = Self::EMPTY;
        for p in points {
            aabb.extend(p);
        }
        aabb
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn extend(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> DVec3 {
        if self.is_empty() {
            DVec3::ZERO
        } else {
            (self.min + self.max) * 0.5
        }
    }

    pub fn size(&self) -> DVec3 {
        if self.is_empty() {
            DVec3::ZERO
        } else {
            self.max - self.min
        }
    }

    /// Radius of the sphere around [`Self::center`] that encloses the box.
    pub fn bounding_radius(&self) -> f64 {
        self.size().length() * 0.5
    }

    pub fn contains(&self, p: DVec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_extend() {
        let mut b = Aabb::default();
        assert!(b.is_empty());
        assert_eq!(b.size(), DVec3::ZERO);
        assert_eq!(b.center(), DVec3::ZERO);
        b.extend(DVec3::new(1.0, 2.0, 3.0));
        assert!(!b.is_empty());
        b.extend(DVec3::new(-1.0, 0.0, 5.0));
        assert_eq!(b.min, DVec3::new(-1.0, 0.0, 3.0));
        assert_eq!(b.max, DVec3::new(1.0, 2.0, 5.0));
        assert_eq!(b.center(), DVec3::new(0.0, 1.0, 4.0));
        assert!(b.contains(DVec3::new(0.0, 1.0, 4.0)));
        assert!(!b.contains(DVec3::new(0.0, 1.0, 6.0)));
    }

    #[test]
    fn union_with_empty_is_identity() {
        let b = Aabb::from_points([DVec3::ZERO, DVec3::ONE]);
        assert_eq!(b.union(&Aabb::EMPTY), b);
    }
}
