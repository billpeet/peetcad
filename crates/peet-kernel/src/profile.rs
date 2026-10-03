//! Sketch splines as kernel curves and the surfaces they sweep.
//!
//! A sketch spline is a B-spline given by its degree, knots and control points
//! ([`peet_sketch::Spline`]). The kernel's curve is made from those same numbers, lifted
//! into space, so the edge of a solid is exactly the curve drawn in the sketch.

use std::f64::consts::TAU;
use std::sync::Arc;

use peet_math::{DVec2, DVec3, Frame};
use peet_sketch::SplinePiece;

use crate::KernelError;
use crate::geom::Surface;
use crate::nurbs::{NurbsCurve, NurbsSurface};

/// The curve of `piece` (a sketch spline, or a stretch of one) in space: `lift` places a
/// point of the sketch plane, and must be an affine map (a plane's coordinates are). The
/// curve runs the way the spline does, over the stretch's own parameter range
/// ([`SplinePiece::range`]), and has the same point at every parameter.
pub fn spline_curve(
    piece: &SplinePiece,
    lift: impl Fn(DVec2) -> DVec3,
) -> Result<NurbsCurve, KernelError> {
    let spline = piece.to_spline();
    NurbsCurve::new(
        spline.degree(),
        spline.knots().to_vec(),
        spline.control_points().iter().map(|p| lift(*p)).collect(),
        None,
    )
    .map_err(|e| KernelError::InvalidInput(format!("a spline of the profile can't be used: {e}")))
}

/// The surface `profile` sweeps when it is turned about `frame`'s Z axis from the angle
/// `from` over `sweep` (radians, less than a full turn; the angle is counted from
/// `frame`'s X axis, where the profile is given). `u` runs round the axis from 0 to 1,
/// `v` is the profile's own parameter. Exact: a circle is a rational quadratic, and
/// turning is linear in the cosine and sine of the angle.
pub fn revolved_surface(
    profile: &NurbsCurve,
    frame: &Frame,
    from: f64,
    sweep: f64,
) -> Result<Surface, KernelError> {
    let invalid = |m: String| KernelError::InvalidInput(m);
    if sweep.abs() >= TAU - 1e-9 {
        return Err(invalid(
            "a freeform surface can't go all the way round its axis".to_owned(),
        ));
    }
    let round = NurbsCurve::arc(&Frame::WORLD, 1.0, from, sweep);
    let round_weights = round
        .weights()
        .map(<[f64]>::to_vec)
        .unwrap_or_else(|| vec![1.0; round.control_points().len()]);
    let count = round.control_points().len() * profile.control_points().len();
    let mut net = Vec::with_capacity(count);
    let mut weights = Vec::with_capacity(count);
    for (corner, weight) in round.control_points().iter().zip(&round_weights) {
        for (j, p) in profile.control_points().iter().enumerate() {
            let l = frame.to_local(*p);
            net.push(frame.to_world(DVec3::new(
                l.x * corner.x - l.y * corner.y,
                l.x * corner.y + l.y * corner.x,
                l.z,
            )));
            weights.push(weight * profile.weights().map_or(1.0, |w| w[j]));
        }
    }
    NurbsSurface::new(
        2,
        profile.degree(),
        round.knots().to_vec(),
        profile.knots().to_vec(),
        net,
        Some(weights),
    )
    .map(|s| Surface::Nurbs(Arc::new(s)))
    .map_err(|e| invalid(format!("the spline can't be turned about the axis: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::Plane;
    use peet_sketch::Curve;

    fn wave() -> Curve {
        Curve::spline_through(
            &[
                DVec2::new(5.0, 0.0),
                DVec2::new(9.0, 6.0),
                DVec2::new(6.0, 13.0),
                DVec2::new(11.0, 20.0),
            ],
            false,
        )
    }

    #[test]
    fn the_kernel_curve_is_the_sketch_curve() {
        let plane = Plane::front();
        let sketch = wave();
        for (t0, t1) in [(0.0, 1.0), (0.13, 0.71)] {
            let stretch = sketch.sub_curve(t0, t1);
            let piece = stretch.as_spline().unwrap();
            let curve = spline_curve(piece, |p| plane.from_plane_coords(p)).unwrap();
            assert_eq!(curve.domain(), piece.range());
            let mut worst: f64 = 0.0;
            for i in 0..=200 {
                let t = f64::from(i) / 200.0;
                let drawn = plane.from_plane_coords(stretch.point_at(t));
                worst = worst.max(curve.point(piece.param(t)).distance(drawn));
            }
            assert!(worst < 1e-12, "{worst}");
        }
    }

    #[test]
    fn a_turned_spline_stays_at_its_distance_from_the_axis() {
        let sketch = wave();
        let piece = sketch.as_spline().unwrap();
        // The sketch's x is the distance from the axis, its y the height along it.
        let frame = Frame::WORLD;
        let profile = spline_curve(piece, |p| DVec3::new(p.x, 0.0, p.y)).unwrap();
        let Surface::Nurbs(surface) = revolved_surface(&profile, &frame, 0.3, 2.0).unwrap() else {
            panic!("a freeform surface")
        };
        let (lo, hi) = surface.domain();
        for i in 0..=10 {
            for j in 0..=10 {
                let uv = lo + (hi - lo) * DVec2::new(f64::from(i), f64::from(j)) / 10.0;
                let p = surface.point(uv);
                let on_profile = profile.point(uv.y);
                assert!((p.truncate().length() - on_profile.x).abs() < 1e-12);
                assert!((p.z - on_profile.z).abs() < 1e-12);
                let angle = p.y.atan2(p.x);
                assert!((0.3 - 1e-9..=2.3 + 1e-9).contains(&angle));
            }
        }
        assert!(revolved_surface(&profile, &frame, 0.0, TAU).is_err());
    }
}
