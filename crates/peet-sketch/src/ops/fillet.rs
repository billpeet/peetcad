use std::collections::HashMap;

use peet_math::tolerance;

use super::OpError;
use crate::sketch::{ConstraintId, ConstraintKind, EntityId, EntityKind, Sketch};

/// Groups of point entities joined (directly or through a chain) by coincident relations.
/// Shared with the other editing operations.
pub(super) struct PointGroups {
    parent: HashMap<EntityId, EntityId>,
}

impl PointGroups {
    pub(super) fn new(sketch: &Sketch) -> Self {
        let mut groups = Self {
            parent: HashMap::new(),
        };
        for (_, c) in sketch.constraints() {
            if let ConstraintKind::Coincident(a, b) = c.kind {
                let (ra, rb) = (groups.root(a), groups.root(b));
                if ra != rb {
                    groups.parent.insert(ra, rb);
                }
            }
        }
        groups
    }

    pub(super) fn root(&self, mut p: EntityId) -> EntityId {
        while let Some(&q) = self.parent.get(&p) {
            p = q;
        }
        p
    }

    /// Same point entity, or joined by coincident relations.
    pub(super) fn same(&self, a: EntityId, b: EntityId) -> bool {
        a == b || self.root(a) == self.root(b)
    }

    /// All points in `p`'s group, `p` included.
    pub(super) fn members(&self, p: EntityId) -> Vec<EntityId> {
        let root = self.root(p);
        let mut out = vec![p];
        out.extend(
            self.parent
                .keys()
                .copied()
                .filter(|&q| q != p && self.root(q) == root),
        );
        if root != p && !out.contains(&root) {
            out.push(root);
        }
        out
    }
}

/// Rounds the corner at `corner` (a point where two curves end, directly or through a
/// coincident relation) with an arc of `radius` mm. The curves are trimmed back to the
/// tangent points; the arc gets tangent and coincident relations and a radius dimension.
/// Returns the new arc.
///
/// Only corners between two lines are supported. The corner vertex disappears, so
/// relations on the two corner endpoints (the coincident relation between them, and any
/// ties to other points) are removed. Other relations on the lines are kept, except those
/// that depend on the lines' extent and would no longer hold after trimming: length
/// dimensions become driven (reference) dimensions, and equal-length and midpoint relations
/// on the lines are removed. The arc is construction geometry if both lines are.
///
/// Errors: [`OpError::NotConnected`] unless exactly two curves end at the corner,
/// [`OpError::Unsupported`] for non-line curves, parallel lines or a non-positive radius,
/// [`OpError::TooLarge`] when a tangent point would fall at or beyond either line's far end.
pub fn fillet(sketch: &mut Sketch, corner: EntityId, radius: f64) -> Result<EntityId, OpError> {
    if sketch.entity(corner).is_none() {
        return Err(OpError::Missing(corner));
    }
    if sketch.kind(corner) != Some(EntityKind::Point) {
        return Err(OpError::Unsupported("a fillet needs a corner point"));
    }
    if radius <= tolerance::LINEAR {
        return Err(OpError::Unsupported("a fillet radius must be positive"));
    }

    // Curves with an endpoint (not a centre) in the corner's coincident group.
    let groups = PointGroups::new(sketch);
    let group = groups.members(corner);
    let mut ends: Vec<(EntityId, EntityId)> = Vec::new(); // (curve, its endpoint at the corner)
    for (id, _) in sketch.entities() {
        if let Some((s, e)) = sketch.endpoints(id) {
            for p in [s, e] {
                if group.contains(&p) {
                    ends.push((id, p));
                }
            }
        }
    }
    let [(l1, p1), (l2, p2)] = ends[..] else {
        return Err(OpError::NotConnected);
    };
    if l1 == l2 {
        return Err(OpError::NotConnected);
    }
    if sketch.kind(l1) != Some(EntityKind::Line) || sketch.kind(l2) != Some(EntityKind::Line) {
        return Err(OpError::Unsupported(
            "fillets are only supported between two lines",
        ));
    }
    for id in [l1, l2, p1, p2] {
        if sketch.entity(id).is_some_and(|e| e.locked) {
            return Err(OpError::Sketch(crate::SketchError::Locked(id)));
        }
    }

    // Geometry: unit directions from the corner along each line.
    let far = |line: EntityId, near: EntityId| {
        let (s, e) = sketch.endpoints(line).expect("a line");
        sketch.point(if s == near { e } else { s })
    };
    let vertex = sketch.point(p1);
    let (f1, f2) = (far(l1, p1), far(l2, p2));
    let (len1, len2) = (f1.distance(vertex), f2.distance(sketch.point(p2)));
    let (Some(u1), Some(u2)) = ((f1 - vertex).try_normalize(), (f2 - vertex).try_normalize())
    else {
        return Err(OpError::TooLarge);
    };
    let sin = u1.perp_dot(u2);
    if sin.abs() <= tolerance::ANGULAR {
        return Err(OpError::Unsupported("the lines are parallel"));
    }
    // Half of the angle between the lines.
    let half = sin.abs().atan2(u1.dot(u2)) / 2.0;
    let setback = radius / half.tan();
    if setback >= len1 - tolerance::LINEAR || setback >= len2 - tolerance::LINEAR {
        return Err(OpError::TooLarge);
    }
    let t1 = vertex + u1 * setback;
    let t2 = vertex + u2 * setback;
    let center = vertex + (u1 + u2).normalize() * (radius / half.sin());

    // Detach a shared corner point so each line keeps its own endpoint.
    let p2 = if p1 == p2 {
        let own = sketch.add_point(t2);
        let construction = sketch_construction(sketch, l2);
        if let Some(e) = sketch.entity_mut(own) {
            e.owner = Some(l2);
            e.construction = construction;
        }
        sketch.replace_curve_point(l2, p1, own);
        if let Some(e) = sketch.entity_mut(p1)
            && e.owner == Some(l2)
        {
            e.owner = Some(l1);
        }
        own
    } else {
        p2
    };

    // The corner vertex is gone: drop relations on its points, and relations on the lines
    // that depend on their (now shorter) extent.
    let mut stale: Vec<ConstraintId> = Vec::new();
    let mut to_driven: Vec<ConstraintId> = Vec::new();
    for (id, c) in sketch.constraints() {
        let on = |e: EntityId| c.kind.entities().contains(&e);
        let on_lines = on(l1) || on(l2);
        match c.kind {
            _ if on(p1) || on(p2) => stale.push(id),
            ConstraintKind::Length(_) if on_lines => to_driven.push(id),
            ConstraintKind::Equal(a, b)
                if on_lines
                    && sketch.kind(a) == Some(EntityKind::Line)
                    && sketch.kind(b) == Some(EntityKind::Line) =>
            {
                stale.push(id)
            }
            ConstraintKind::Midpoint { line, .. } if line == l1 || line == l2 => stale.push(id),
            _ => {}
        }
    }
    for id in stale {
        sketch.remove_constraint(id);
    }
    for id in to_driven {
        if let Some(d) = sketch.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
            d.driving = false;
        }
    }

    sketch.set_point(p1, t1);
    sketch.set_point(p2, t2);
    sketch.update_driven_dimensions();

    // Counter-clockwise from whichever tangent point makes the short way round.
    let (from, to, from_pt, to_pt) = if (t1 - center).perp_dot(t2 - center) > 0.0 {
        (t1, t2, p1, p2)
    } else {
        (t2, t1, p2, p1)
    };
    let arc = sketch.add_arc(center, from, to);
    if sketch_construction(sketch, l1) && sketch_construction(sketch, l2) {
        sketch.set_construction(arc, true);
    }
    let (arc_start, arc_end) = sketch.endpoints(arc).expect("an arc");
    sketch.add_constraint(ConstraintKind::Coincident(arc_start, from_pt))?;
    sketch.add_constraint(ConstraintKind::Coincident(arc_end, to_pt))?;
    sketch.add_constraint(ConstraintKind::Tangent(l1, arc))?;
    sketch.add_constraint(ConstraintKind::Tangent(l2, arc))?;
    sketch.add_dimension(ConstraintKind::Radius(arc), radius)?;
    Ok(arc)
}

fn sketch_construction(sketch: &Sketch, id: EntityId) -> bool {
    sketch.entity(id).is_some_and(|e| e.construction)
}

#[cfg(test)]
mod tests {
    use peet_math::DVec2;

    use super::*;
    use crate::curve::Curve;
    use crate::shapes::rectangle;
    use crate::shapes::testing::{assert_satisfied, close, count};

    /// Two lines `a -> corner -> b`, joined by a coincident relation. Returns the lines and
    /// the first line's corner point.
    fn corner_lines(
        s: &mut Sketch,
        a: DVec2,
        corner: DVec2,
        b: DVec2,
    ) -> (EntityId, EntityId, EntityId) {
        let l1 = s.add_line(a, corner);
        let l2 = s.add_line(corner, b);
        let (_, e) = s.endpoints(l1).unwrap();
        let (st, _) = s.endpoints(l2).unwrap();
        s.add_constraint(ConstraintKind::Coincident(e, st)).unwrap();
        (l1, l2, e)
    }

    fn check_arc(s: &Sketch, arc: EntityId, center: DVec2, radius: f64) {
        let Curve::Arc {
            center: c,
            radius: r,
            sweep,
            ..
        } = s.curve(arc).unwrap()
        else {
            panic!("expected an arc")
        };
        assert!(close(c, center), "centre {c} != {center}");
        assert!((r - radius).abs() < 1e-9);
        assert!(sweep < std::f64::consts::PI, "short way round, got {sweep}");
    }

    #[test]
    fn right_angle_both_turn_directions() {
        // Left turn: (0,0) -> (10,0) -> (10,10).
        let mut s = Sketch::new();
        let (l1, l2, corner) = corner_lines(
            &mut s,
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
        );
        let arc = fillet(&mut s, corner, 2.0).unwrap();
        check_arc(&s, arc, DVec2::new(8.0, 2.0), 2.0);
        assert!(close(s.curve(l1).unwrap().end(), DVec2::new(8.0, 0.0)));
        assert!(close(s.curve(l2).unwrap().start(), DVec2::new(10.0, 2.0)));
        assert_eq!(
            count(&s, "Coincident"),
            2,
            "the corner relation is replaced"
        );
        assert_eq!(count(&s, "Tangent"), 2);
        assert_eq!(count(&s, "Radius"), 1);
        assert_satisfied(&s);

        // Right turn: (0,0) -> (10,0) -> (10,-10), with the second line drawn backwards.
        let mut s = Sketch::new();
        let l1 = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let l2 = s.add_line(DVec2::new(10.0, -10.0), DVec2::new(10.0, 0.0));
        let (_, e1) = s.endpoints(l1).unwrap();
        let (_, e2) = s.endpoints(l2).unwrap();
        s.add_constraint(ConstraintKind::Coincident(e2, e1))
            .unwrap();
        let arc = fillet(&mut s, e2, 3.0).unwrap();
        check_arc(&s, arc, DVec2::new(7.0, -3.0), 3.0);
        assert!(close(s.curve(l2).unwrap().end(), DVec2::new(10.0, -3.0)));
        assert_satisfied(&s);
    }

    #[test]
    fn acute_and_obtuse_corners() {
        for b in [DVec2::new(0.0, 5.0), DVec2::new(20.0, 5.0)] {
            let mut s = Sketch::new();
            let (_, _, corner) = corner_lines(&mut s, DVec2::ZERO, DVec2::new(10.0, 0.0), b);
            let arc = fillet(&mut s, corner, 1.0).unwrap();
            let r = s.curve(arc).unwrap().radius().unwrap();
            assert!((r - 1.0).abs() < 1e-9);
            assert_satisfied(&s);
        }
    }

    #[test]
    fn shared_corner_point_is_split() {
        let mut s = Sketch::new();
        let l1 = s.add_line(DVec2::new(0.0, 4.0), DVec2::ZERO);
        let l2 = s.add_line(DVec2::new(1.0, 0.0), DVec2::new(4.0, 0.0));
        let (_, corner) = s.endpoints(l1).unwrap();
        let (old, _) = s.endpoints(l2).unwrap();
        assert!(s.replace_curve_point(l2, old, corner));
        s.remove_entity(old).unwrap();
        let arc = fillet(&mut s, corner, 1.0).unwrap();
        let (a, _) = s.endpoints(l2).unwrap();
        assert_ne!(a, corner);
        assert_eq!(s.entity(a).unwrap().owner, Some(l2));
        check_arc(&s, arc, DVec2::new(1.0, 1.0), 1.0);
        assert_eq!(count(&s, "Coincident"), 2);
        assert_satisfied(&s);
    }

    #[test]
    fn rectangle_corner_keeps_relations() {
        let mut s = Sketch::new();
        let rect = rectangle(&mut s, DVec2::ZERO, DVec2::new(10.0, 6.0));
        let len = s
            .add_constraint(ConstraintKind::Length(rect.curves[0]))
            .unwrap();
        // Top-right corner: end of the right line.
        let (_, corner) = s.endpoints(rect.curves[1]).unwrap();
        let arc = fillet(&mut s, corner, 2.0).unwrap();
        check_arc(&s, arc, DVec2::new(8.0, 4.0), 2.0);
        assert_eq!(count(&s, "Horizontal"), 2);
        assert_eq!(count(&s, "Vertical"), 2);
        assert_eq!(count(&s, "Coincident"), 5);
        assert!(
            s.constraint(len).unwrap().is_driving(),
            "an untouched line keeps its length"
        );
        assert_satisfied(&s);
    }

    #[test]
    fn concave_corner() {
        // The reflex corner (4, 4) of an L-shaped outline.
        let mut s = Sketch::new();
        let pts = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 4.0),
            (4.0, 4.0),
            (4.0, 10.0),
            (0.0, 10.0),
        ]
        .map(|(x, y)| DVec2::new(x, y));
        let lines: Vec<EntityId> = (0..6)
            .map(|i| s.add_line(pts[i], pts[(i + 1) % 6]))
            .collect();
        for i in 0..6 {
            let (_, e) = s.endpoints(lines[i]).unwrap();
            let (st, _) = s.endpoints(lines[(i + 1) % 6]).unwrap();
            s.add_constraint(ConstraintKind::Coincident(e, st)).unwrap();
        }
        let length = s.add_constraint(ConstraintKind::Length(lines[2])).unwrap();
        let (_, reflex) = s.endpoints(lines[2]).unwrap();
        let arc = fillet(&mut s, reflex, 1.0).unwrap();
        check_arc(&s, arc, DVec2::new(5.0, 5.0), 1.0);
        assert!(close(
            s.curve(lines[2]).unwrap().end(),
            DVec2::new(5.0, 4.0)
        ));
        assert!(close(
            s.curve(lines[3]).unwrap().start(),
            DVec2::new(4.0, 5.0)
        ));
        let dim = s.constraint(length).unwrap();
        assert!(
            !dim.is_driving(),
            "a trimmed line's length becomes a reference"
        );
        assert_eq!(dim.dimension.as_ref().unwrap().value, 5.0);
        assert_satisfied(&s);
    }

    #[test]
    fn errors() {
        let mut s = Sketch::new();
        let (_, _, corner) = corner_lines(
            &mut s,
            DVec2::ZERO,
            DVec2::new(2.0, 0.0),
            DVec2::new(2.0, 10.0),
        );
        assert_eq!(fillet(&mut s, corner, 2.5), Err(OpError::TooLarge));
        assert_eq!(count(&s, "Coincident"), 1, "nothing changed");
        assert!(matches!(
            fillet(&mut s, corner, 0.0),
            Err(OpError::Unsupported(_))
        ));

        // Collinear (parallel) lines.
        let (_, _, straight) = corner_lines(
            &mut s,
            DVec2::new(0.0, 5.0),
            DVec2::new(1.0, 5.0),
            DVec2::new(3.0, 5.0),
        );
        assert!(matches!(
            fillet(&mut s, straight, 0.5),
            Err(OpError::Unsupported(_))
        ));

        let lone = s.add_line(DVec2::new(0.0, 20.0), DVec2::new(5.0, 20.0));
        let (end, _) = s.endpoints(lone).unwrap();
        assert_eq!(fillet(&mut s, end, 1.0), Err(OpError::NotConnected));

        // Three lines at one point.
        let (_, _, c3) = corner_lines(
            &mut s,
            DVec2::new(0.0, 30.0),
            DVec2::new(5.0, 30.0),
            DVec2::new(5.0, 35.0),
        );
        let third = s.add_line(DVec2::new(5.0, 30.0), DVec2::new(9.0, 30.0));
        let (t, _) = s.endpoints(third).unwrap();
        s.add_constraint(ConstraintKind::Coincident(t, c3)).unwrap();
        assert_eq!(fillet(&mut s, c3, 1.0), Err(OpError::NotConnected));

        // Line and arc.
        let arc = s.add_arc(
            DVec2::new(0.0, 50.0),
            DVec2::new(2.0, 50.0),
            DVec2::new(0.0, 52.0),
        );
        let line = s.add_line(DVec2::new(2.0, 50.0), DVec2::new(2.0, 40.0));
        let (a, _) = s.endpoints(arc).unwrap();
        let (b, _) = s.endpoints(line).unwrap();
        s.add_constraint(ConstraintKind::Coincident(a, b)).unwrap();
        assert!(matches!(
            fillet(&mut s, a, 0.5),
            Err(OpError::Unsupported(_))
        ));
        assert_eq!(
            fillet(&mut s, EntityId(999), 1.0),
            Err(OpError::Missing(EntityId(999)))
        );
    }
}
