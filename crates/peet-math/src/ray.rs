use glam::DVec3;

/// A half-line: `origin + t * direction`. The direction is kept normalized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: DVec3,
    pub direction: DVec3,
}

impl Ray {
    /// Creates a ray. `direction` is normalized, so it must be non-zero.
    pub fn new(origin: DVec3, direction: DVec3) -> Self {
        Self {
            origin,
            direction: direction.normalize(),
        }
    }

    pub fn at(&self, t: f64) -> DVec3 {
        self.origin + self.direction * t
    }

    /// Closest point on the (infinite) line to `p`, as a ray parameter.
    pub fn closest_param(&self, p: DVec3) -> f64 {
        (p - self.origin).dot(self.direction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_point() {
        let r = Ray::new(DVec3::ZERO, DVec3::X * 3.0);
        assert_eq!(r.direction, DVec3::X);
        assert_eq!(r.closest_param(DVec3::new(2.0, 5.0, -1.0)), 2.0);
        assert_eq!(r.at(2.0), DVec3::new(2.0, 0.0, 0.0));
    }
}
