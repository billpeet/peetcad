//! The global tolerance model.
//!
//! Every geometric comparison in PeetCAD goes through these constants and helpers,
//! instead of ad hoc epsilons scattered through the code. The values are chosen for
//! mechanical parts in millimetres: a part up to ~10 m across still has several orders
//! of magnitude of `f64` headroom below [`LINEAR`].

use glam::DVec3;

/// Two points closer than this (in mm) are considered coincident.
pub const LINEAR: f64 = 1e-6;

/// Two directions whose angle differs by less than this (in radians) are considered parallel.
pub const ANGULAR: f64 = 1e-9;

/// Returns true if `a` and `b` are equal within [`LINEAR`].
#[inline]
pub fn lengths_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= LINEAR
}

/// Returns true if `len` is zero within [`LINEAR`].
#[inline]
pub fn is_zero_length(len: f64) -> bool {
    len.abs() <= LINEAR
}

/// Returns true if two points coincide within [`LINEAR`].
#[inline]
pub fn points_coincide(a: DVec3, b: DVec3) -> bool {
    a.distance_squared(b) <= LINEAR * LINEAR
}

/// Returns true if two unit vectors point the same way (within [`ANGULAR`]).
#[inline]
pub fn directions_eq(a: DVec3, b: DVec3) -> bool {
    // For unit vectors |a x b| = sin(angle), which is ≈ angle for small angles.
    a.dot(b) > 0.0 && a.cross(b).length() <= ANGULAR
}

/// Returns true if two unit vectors are parallel or anti-parallel (within [`ANGULAR`]).
#[inline]
pub fn directions_parallel(a: DVec3, b: DVec3) -> bool {
    a.cross(b).length() <= ANGULAR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert!(lengths_eq(1.0, 1.0 + LINEAR * 0.5));
        assert!(!lengths_eq(1.0, 1.0 + LINEAR * 2.0));
        assert!(is_zero_length(-LINEAR * 0.5));
        assert!(points_coincide(
            DVec3::ONE,
            DVec3::ONE + DVec3::X * LINEAR * 0.5
        ));
    }

    #[test]
    fn directions() {
        assert!(directions_eq(DVec3::X, DVec3::X));
        assert!(!directions_eq(DVec3::X, -DVec3::X));
        assert!(directions_parallel(DVec3::X, -DVec3::X));
        assert!(!directions_parallel(DVec3::X, DVec3::Y));
        let nearly_x = DVec3::new(1.0, 1e-12, 0.0).normalize();
        assert!(directions_eq(DVec3::X, nearly_x));
    }
}
