use std::f64::consts::TAU;

use peet_math::{DVec2, tolerance};

use super::OpError;
use super::trim::{bind_endpoint, move_endpoint};
use crate::curve::Curve;
use crate::sketch::{EntityId, Sketch, SketchError};

/// Extends the end of a line or arc nearest `pick` until it meets the next curve, and adds
/// a point-on-curve relation there.
///
/// Lines extend along their infinite line, arcs along their circle. Only the bounded
/// extent of the other curves counts (construction geometry included). The endpoint moves
/// (relations on it that no longer hold, and a length dimension, are dropped) and gets a
/// coincident relation instead if it lands on another curve's endpoint.
pub fn extend(sketch: &mut Sketch, curve: EntityId, pick: DVec2) -> Result<(), OpError> {
    let entity = sketch.entity(curve).ok_or(OpError::Missing(curve))?;
    if entity.locked {
        return Err(SketchError::Locked(curve).into());
    }
    let c = sketch.curve(curve).ok_or(OpError::Missing(curve))?;
    if c.is_closed() {
        return Err(OpError::Unsupported("only lines and arcs can be extended"));
    }
    let (start, end) = sketch.endpoints(curve).ok_or(OpError::Missing(curve))?;
    let at_end = pick.distance(c.end()) <= pick.distance(c.start());
    let len = c.length();
    let slack = tolerance::LINEAR / len.max(tolerance::LINEAR);
    // Arc parameters beyond the arc run up to one full turn.
    let period = match c {
        Curve::Arc { sweep, .. } => TAU / sweep,
        _ => f64::INFINITY,
    };

    // Distance (in parameter units) beyond the chosen end of each hit.
    let mut hits: Vec<(f64, f64, EntityId)> = Vec::new(); // (beyond, t, curve)
    for (other, e) in sketch.entities() {
        if other == curve || !e.kind().is_curve() {
            continue;
        }
        let Some(oc) = sketch.curve(other) else {
            continue;
        };
        for hit in c.intersect_unbounded(&oc) {
            if !oc.contains_param(hit.t_b) {
                continue;
            }
            let t = hit.t_a;
            let beyond = match (c, at_end) {
                (Curve::Arc { .. }, true) => t - 1.0,
                (Curve::Arc { .. }, false) if t > 1.0 => period - t,
                (Curve::Arc { .. }, false) => -1.0, // on the arc itself
                (_, true) => t - 1.0,
                (_, false) => -t,
            };
            if beyond > slack {
                hits.push((beyond, t, other));
            }
        }
    }
    let (best, t, _) = hits
        .iter()
        .copied()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .ok_or(OpError::NoIntersection)?;
    let bounding: Vec<EntityId> = hits
        .iter()
        .filter(|h| (h.0 - best) * len <= tolerance::LINEAR)
        .map(|h| h.2)
        .collect();

    let point = if at_end { end } else { start };
    let moved = move_endpoint(sketch, curve, point, c.point_at(t));
    bind_endpoint(sketch, moved, &bounding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::ConstraintKind;

    fn close(a: DVec2, b: DVec2) -> bool {
        a.distance(b) < 1e-9
    }

    fn has(s: &Sketch, kind: ConstraintKind) -> bool {
        s.constraints().any(|(_, c)| c.kind == kind)
    }

    #[test]
    fn line_extends_to_the_nearest_curve_at_the_picked_end() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(2.0, 0.0));
        let near = s.add_line(DVec2::new(5.0, -1.0), DVec2::new(5.0, 1.0));
        s.add_line(DVec2::new(8.0, -1.0), DVec2::new(8.0, 1.0));
        let behind = s.add_circle(DVec2::new(-4.0, 0.0), 1.0);
        let len = s.add_constraint(ConstraintKind::Length(l)).unwrap();
        s.add_constraint(ConstraintKind::Horizontal(l)).unwrap();
        let (a, b) = s.endpoints(l).unwrap();

        extend(&mut s, l, DVec2::new(1.9, 0.1)).unwrap();
        assert!(close(s.point(b), DVec2::new(5.0, 0.0)));
        assert!(close(s.point(a), DVec2::ZERO));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: b,
                curve: near
            }
        ));
        assert!(s.constraint(len).is_none(), "length changed");
        assert!(has(&s, ConstraintKind::Horizontal(l)));

        // The start extends to the near side of the circle.
        extend(&mut s, l, DVec2::new(0.1, 0.0)).unwrap();
        assert!(close(s.point(a), DVec2::new(-3.0, 0.0)));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: a,
                curve: behind
            }
        ));
    }

    #[test]
    fn misses_are_reported() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(2.0, 0.0));
        // Above the extension: the bounded segment is not hit.
        s.add_line(DVec2::new(5.0, 1.0), DVec2::new(5.0, 2.0));
        // Parallel.
        s.add_line(
            DVec2::new(3.0, 0.0),
            DVec2::new(3.0, 0.0) + DVec2::new(1.0, 1e-12),
        );
        assert_eq!(
            extend(&mut s, l, DVec2::new(2.0, 0.0)),
            Err(OpError::NoIntersection)
        );
        let c = s.add_circle(DVec2::new(10.0, 10.0), 1.0);
        assert!(matches!(
            extend(&mut s, c, DVec2::new(11.0, 10.0)),
            Err(OpError::Unsupported(_))
        ));
    }

    #[test]
    fn arc_extends_along_its_circle() {
        let mut s = Sketch::new();
        let arc = s.add_arc(DVec2::ZERO, DVec2::new(5.0, 0.0), DVec2::new(0.0, 5.0));
        let wall = s.add_line(DVec2::new(-10.0, 0.0), DVec2::new(-1.0, 0.0));
        let floor = s.add_line(DVec2::new(1.0, -10.0), DVec2::new(1.0, -1.0));
        let (a, b) = s.endpoints(arc).unwrap();
        extend(&mut s, arc, DVec2::new(0.5, 5.0)).unwrap();
        assert!(close(s.point(b), DVec2::new(-5.0, 0.0)));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: b,
                curve: wall
            }
        ));
        // Backwards from the start: clockwise to where x = 1 below the axis.
        extend(&mut s, arc, DVec2::new(5.0, 0.5)).unwrap();
        let expected = DVec2::new(1.0, -(24f64).sqrt());
        assert!(close(s.point(a), expected));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: a,
                curve: floor
            }
        ));
        assert!(s.curve(arc).unwrap().distance(DVec2::new(5.0, 0.0)) < 1e-9);
    }

    #[test]
    fn landing_on_an_endpoint_is_coincident() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(2.0, 0.0));
        let other = s.add_line(DVec2::new(5.0, 0.0), DVec2::new(5.0, 3.0));
        extend(&mut s, l, DVec2::new(2.0, 0.0)).unwrap();
        let (_, b) = s.endpoints(l).unwrap();
        let (o, _) = s.endpoints(other).unwrap();
        assert!(close(s.point(b), DVec2::new(5.0, 0.0)));
        assert!(has(&s, ConstraintKind::Coincident(b, o)));
    }
}
