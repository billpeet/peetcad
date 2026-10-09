use std::f64::consts::TAU;

use peet_math::{DVec2, tolerance};

use super::OpError;
use crate::curve::Curve;
use crate::sketch::{Constraint, ConstraintId, ConstraintKind, EntityId, EntityKind, Sketch};

/// Power trim: removes the piece of `curve` nearest `pick` (a point in sketch coordinates
/// on or near the curve), bounded by its nearest intersections with any other curve
/// (construction included) or the curve's own ends. A curve with no intersections is
/// removed entirely. Trimming a circle leaves an arc.
///
/// Surviving pieces keep the original's constraints where they still make sense, and new
/// endpoints get a point-on-curve (or coincident, at an existing endpoint) relation to the
/// curve that bounded them. Returns the ids of the remaining pieces.
///
/// In detail:
/// - **Shortened** (the piece touches one end): that endpoint moves to the cut. Relations
///   and dimensions on it (and on the curve) that no longer hold there are dropped:
///   coincidences with its old neighbours, fixes, lengths, midpoints and so on.
/// - **Split** (the piece is in the middle): the original keeps the first part; a new curve
///   takes the second part and the original's end point entity with all its relations.
///   A new line is tied collinear to the original (its direction relation copied, or
///   parallel to the original, plus its start on the original); a new arc is concentric
///   with and has the same radius as the original. Points on the curve and tangencies move
///   to whichever piece they touch; lengths and midpoints are dropped.
/// - **Circle**: becomes an arc, which takes over the circle's centre point and relations.
pub fn trim(sketch: &mut Sketch, curve: EntityId, pick: DVec2) -> Result<Vec<EntityId>, OpError> {
    let entity = sketch.entity(curve).ok_or(OpError::Missing(curve))?;
    if entity.locked {
        return Err(crate::sketch::SketchError::Locked(curve).into());
    }
    let c = sketch.curve(curve).ok_or(OpError::Unsupported(
        "only lines, arcs and circles can be trimmed",
    ))?;
    if matches!(c, Curve::Spline(_)) {
        return Err(OpError::Unsupported(
            "trimming a spline. Delete the fit points beyond the cut, or draw the spline again \
             to end where you want it",
        ));
    }
    let cuts = cuts(sketch, curve, &c);

    if let Curve::Circle { .. } = c {
        if cuts.len() < 2 {
            sketch.remove_entity(curve)?;
            return Ok(Vec::new());
        }
        let t = c.project(pick);
        let upper = cuts.iter().position(|k| k.t > t).unwrap_or(0);
        let lower = (upper + cuts.len() - 1) % cuts.len();
        let arc = circle_to_arc(sketch, curve, &cuts[upper], &cuts[lower])?;
        return Ok(vec![arc]);
    }

    if cuts.is_empty() {
        sketch.remove_entity(curve)?;
        return Ok(Vec::new());
    }
    let t = c.closest_point(pick).0;
    let lower = cuts.iter().rev().find(|k| k.t < t);
    let upper = cuts.iter().find(|k| k.t > t);
    let (start, end) = sketch.endpoints(curve).ok_or(OpError::Missing(curve))?;
    match (lower, upper) {
        (None, Some(up)) => {
            let p = move_endpoint(sketch, curve, start, c.point_at(up.t));
            bind_endpoint(sketch, p, &up.curves)?;
            Ok(vec![curve])
        }
        (Some(low), None) => {
            let p = move_endpoint(sketch, curve, end, c.point_at(low.t));
            bind_endpoint(sketch, p, &low.curves)?;
            Ok(vec![curve])
        }
        (Some(low), Some(up)) => {
            let new = split(sketch, curve, &c, low, up)?;
            Ok(vec![curve, new])
        }
        // A pick exactly on the only cut: nothing sensible to remove.
        (None, None) => Err(OpError::NoIntersection),
    }
}

/// Where another curve crosses the trimmed curve.
#[derive(Clone, Debug)]
struct Cut {
    /// Parameter on the trimmed curve.
    t: f64,
    /// The curves meeting it there (usually one).
    curves: Vec<EntityId>,
}

/// Intersections of `c` (the curve `id`) with every other curve, merged by position and
/// sorted by parameter. Hits at a line's or arc's own ends are left out.
fn cuts(sketch: &Sketch, id: EntityId, c: &Curve) -> Vec<Cut> {
    let len = c.length();
    let mut hits: Vec<(f64, EntityId)> = Vec::new();
    for (other, e) in sketch.entities() {
        if other == id || !e.kind().is_curve() {
            continue;
        }
        let Some(oc) = sketch.curve(other) else {
            continue;
        };
        for hit in c.intersect(&oc) {
            let t = if c.is_closed() {
                hit.t_a.rem_euclid(1.0)
            } else {
                hit.t_a
            };
            let interior = c.is_closed()
                || (t * len > tolerance::LINEAR && (1.0 - t) * len > tolerance::LINEAR);
            if interior {
                hits.push((t, other));
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<Cut> = Vec::new();
    for (t, other) in hits {
        match cuts.last_mut() {
            Some(last) if (t - last.t) * len <= tolerance::LINEAR => {
                if !last.curves.contains(&other) {
                    last.curves.push(other);
                }
            }
            _ => cuts.push(Cut {
                t,
                curves: vec![other],
            }),
        }
    }
    // On a circle, parameters just below 1 and just above 0 are the same point.
    if c.is_closed()
        && cuts.len() > 1
        && (1.0 - cuts[cuts.len() - 1].t + cuts[0].t) * len <= tolerance::LINEAR
    {
        let last = cuts.pop().expect("len > 1");
        for other in last.curves {
            if !cuts[0].curves.contains(&other) {
                cuts[0].curves.push(other);
            }
        }
    }
    cuts
}

/// Splits a line or arc: the original keeps `0..low`, a new curve takes `up..1` along with
/// the original's end point entity. Returns the new curve.
fn split(
    sketch: &mut Sketch,
    curve: EntityId,
    c: &Curve,
    low: &Cut,
    up: &Cut,
) -> Result<EntityId, OpError> {
    let (_, end) = sketch.endpoints(curve).ok_or(OpError::Missing(curve))?;
    let construction = sketch.entity(curve).is_some_and(|e| e.construction);
    let on_curve: Vec<ConstraintId> = sketch.constraints_on(curve).collect();
    let end_pos = sketch.point(end);
    let up_pos = c.point_at(up.t);

    // The new piece, ending at the original end point entity.
    let new = match *c {
        Curve::Arc { center, .. } => sketch.add_arc(center, up_pos, end_pos),
        _ => sketch.add_line(up_pos, end_pos),
    };
    sketch.set_construction(new, construction);
    let (new_start, new_end) = sketch.endpoints(new).ok_or(OpError::Missing(new))?;
    sketch.replace_curve_point(new, new_end, end);
    sketch.remove_entity(new_end)?;
    if let Some(e) = sketch.entity_mut(end)
        && e.owner == Some(curve)
    {
        e.owner = Some(new);
    }
    // The original gets a fresh end point at the lower cut.
    let low_point = fresh_endpoint(sketch, curve, end, c.point_at(low.t));

    // Tie the pieces to the same carrier.
    match *c {
        Curve::Arc { .. } => {
            sketch.add_constraint(ConstraintKind::Concentric(new, curve))?;
            sketch.add_constraint(ConstraintKind::Equal(new, curve))?;
        }
        _ => {
            let direction = on_curve.iter().find_map(|&id| {
                let kind = sketch.constraint(id)?.kind;
                let other = |a: EntityId, b: EntityId| if a == curve { b } else { a };
                match kind {
                    ConstraintKind::Horizontal(_) => Some(ConstraintKind::Horizontal(new)),
                    ConstraintKind::Vertical(_) => Some(ConstraintKind::Vertical(new)),
                    ConstraintKind::Parallel(a, b) => {
                        Some(ConstraintKind::Parallel(new, other(a, b)))
                    }
                    ConstraintKind::Perpendicular(a, b) => {
                        Some(ConstraintKind::Perpendicular(new, other(a, b)))
                    }
                    _ => None,
                }
            });
            sketch.add_constraint(direction.unwrap_or(ConstraintKind::Parallel(new, curve)))?;
            sketch.add_constraint(ConstraintKind::PointOnCurve {
                point: new_start,
                curve,
            })?;
        }
    }

    // Relations of other geometry with the curve go to the piece they touch.
    let mid = 0.5 * (low.t + up.t);
    let period = match *c {
        Curve::Arc { sweep, .. } => TAU / sweep,
        _ => f64::INFINITY,
    };
    let in_new = |t: f64| {
        if t > 1.0 && period.is_finite() {
            t - 1.0 < period - t // nearer the arc's end than its start
        } else {
            t >= mid
        }
    };
    for id in on_curve {
        let Some(kind) = sketch.constraint(id).map(|k| k.kind) else {
            continue;
        };
        let contact = match kind {
            ConstraintKind::PointOnCurve { point, .. } => sketch.try_point(point),
            ConstraintKind::Tangent(a, b) => {
                let other = if a == curve { b } else { a };
                sketch.curve(other).and_then(|o| tangent_contact(c, &o))
            }
            _ => None,
        };
        if let Some(p) = contact
            && in_new(c.project(p))
        {
            retarget(sketch, id, curve, new);
        }
    }
    drop_broken(sketch, curve, &[]);

    bind_endpoint(sketch, low_point, &low.curves)?;
    bind_endpoint(sketch, new_start, &up.curves)?;
    Ok(new)
}

/// Replaces a circle by the arc from `from` to `to` (counter-clockwise). The arc takes
/// over the circle's centre point and every relation on the circle.
fn circle_to_arc(
    sketch: &mut Sketch,
    circle: EntityId,
    from: &Cut,
    to: &Cut,
) -> Result<EntityId, OpError> {
    let c = sketch.curve(circle).ok_or(OpError::Missing(circle))?;
    let center = sketch.center(circle).ok_or(OpError::Missing(circle))?;
    let construction = sketch.entity(circle).is_some_and(|e| e.construction);
    let arc = sketch.add_arc(sketch.point(center), c.point_at(from.t), c.point_at(to.t));
    let own_center = sketch.center(arc).ok_or(OpError::Missing(arc))?;
    sketch.replace_curve_point(arc, own_center, center);
    sketch.remove_entity(own_center)?;
    let on_circle: Vec<ConstraintId> = sketch.constraints_on(circle).collect();
    for id in on_circle {
        retarget(sketch, id, circle, arc);
    }
    sketch.remove_entity(circle)?;
    if let Some(e) = sketch.entity_mut(center) {
        e.owner = Some(arc);
    }
    sketch.set_construction(arc, construction);
    let (start, end) = sketch.endpoints(arc).ok_or(OpError::Missing(arc))?;
    bind_endpoint(sketch, start, &from.curves)?;
    bind_endpoint(sketch, end, &to.curves)?;
    Ok(arc)
}

/// Where a tangency between `c` and `other` touches `c`'s carrier.
fn tangent_contact(c: &Curve, other: &Curve) -> Option<DVec2> {
    match (c.center(), other.center()) {
        // Line tangent to a circle: the foot of the centre.
        (None, Some(oc)) => Some(c.point_at(c.project(oc))),
        // Circle tangent to a line: the foot of the centre on the line.
        (Some(cc), None) => Some(other.point_at(other.project(cc))),
        // Two circles: on the line of centres, on the side matching the other's radius.
        (Some(cc), Some(oc)) => {
            let (r, or) = (c.radius()?, other.radius()?);
            let u = (oc - cc).try_normalize()?;
            [cc + u * r, cc - u * r].into_iter().min_by(|a, b| {
                (a.distance(oc) - or)
                    .abs()
                    .total_cmp(&(b.distance(oc) - or).abs())
            })
        }
        (None, None) => None,
    }
}

// ---- Helpers shared with extend ----

/// Moves `curve`'s endpoint `point` to `pos` and drops relations that no longer hold.
/// A point shared with another curve stays put (with its relations) and `curve` gets a
/// fresh endpoint instead. Returns the endpoint now at `pos`.
pub(super) fn move_endpoint(
    sketch: &mut Sketch,
    curve: EntityId,
    point: EntityId,
    pos: DVec2,
) -> EntityId {
    let shared = sketch.curves_using(point).any(|u| u != curve);
    let moved = if shared {
        fresh_endpoint(sketch, curve, point, pos)
    } else {
        sketch.set_point(point, pos);
        point
    };
    drop_broken(sketch, curve, &[moved]);
    moved
}

/// Gives `curve` a new owned point at `pos` in place of `old`. `old` keeps its relations;
/// if nothing uses it any more it's removed (with them).
fn fresh_endpoint(sketch: &mut Sketch, curve: EntityId, old: EntityId, pos: DVec2) -> EntityId {
    let construction = sketch.entity(curve).is_some_and(|e| e.construction);
    let p = sketch.add_point(pos);
    if let Some(e) = sketch.entity_mut(p) {
        e.owner = Some(curve);
        e.construction = construction;
    }
    sketch.replace_curve_point(curve, old, p);
    let next_user = sketch.curves_using(old).next();
    match next_user {
        None => {
            if sketch.entity(old).is_some_and(|e| e.owner == Some(curve)) {
                let _ = sketch.remove_entity(old);
            }
        }
        Some(user) => {
            if let Some(e) = sketch.entity_mut(old)
                && e.owner == Some(curve)
            {
                e.owner = Some(user);
            }
        }
    }
    p
}

/// Removes relations and dimensions on `curve` or `points` that the current geometry no
/// longer satisfies.
pub(super) fn drop_broken(sketch: &mut Sketch, curve: EntityId, points: &[EntityId]) {
    let affected: Vec<ConstraintId> = sketch
        .constraints()
        .filter(|(_, c)| {
            c.kind
                .entities()
                .iter()
                .any(|e| *e == curve || points.contains(e))
        })
        .map(|(id, _)| id)
        .collect();
    for id in affected {
        if sketch
            .constraint(id)
            .is_some_and(|c| !still_holds(sketch, c))
        {
            sketch.remove_constraint(id);
        }
    }
}

/// Whether a constraint is satisfied by the geometry, for the kinds that shortening,
/// lengthening or splitting a curve can break. Directions, tangency, concentricity, radii
/// and angles are properties of the carrier line or circle, which these edits keep, so
/// they are always reported as holding. Driven dimensions only measure, so they hold too.
pub(super) fn still_holds(sketch: &Sketch, c: &Constraint) -> bool {
    use ConstraintKind::*;
    let near = |a: Option<DVec2>, b: Option<DVec2>| match (a, b) {
        (Some(a), Some(b)) => a.distance(b) <= tolerance::LINEAR,
        _ => true,
    };
    let p = |id| sketch.try_point(id);
    if let Some(d) = &c.dimension {
        return match c.kind {
            Distance(..) | Length(_) | HorizontalDistance(..) | VerticalDistance(..)
                if d.driving =>
            {
                sketch
                    .measure(&c.kind)
                    .is_none_or(|v| (v - d.value).abs() <= tolerance::LINEAR)
            }
            _ => true,
        };
    }
    match c.kind {
        Coincident(a, b) => near(p(a), p(b)),
        Fix { point, at } => near(p(point), Some(at)),
        HorizontalPoints(a, b) => match (p(a), p(b)) {
            (Some(a), Some(b)) => (a.y - b.y).abs() <= tolerance::LINEAR,
            _ => true,
        },
        VerticalPoints(a, b) => match (p(a), p(b)) {
            (Some(a), Some(b)) => (a.x - b.x).abs() <= tolerance::LINEAR,
            _ => true,
        },
        PointOnCurve { point, curve } => match (p(point), sketch.curve(curve)) {
            (Some(q), Some(k)) => k.distance_to_line(q) <= tolerance::LINEAR,
            _ => true,
        },
        Midpoint { point, line } => match sketch.curve(line) {
            Some(l) => near(p(point), Some(l.point_at(0.5))),
            None => true,
        },
        Symmetric { a, b, axis } => match (p(a), p(b), sketch.curve(axis)) {
            (Some(a), Some(b), Some(axis)) => {
                let on_axis = axis.distance_to_line(0.5 * (a + b)) <= tolerance::LINEAR;
                let across = axis
                    .line_direction()
                    .is_none_or(|d| d.dot(b - a).abs() <= tolerance::LINEAR);
                on_axis && across
            }
            _ => true,
        },
        Equal(a, b) => {
            let size = |id| match sketch.kind(id)? {
                EntityKind::Line => sketch.curve(id).map(|c| c.length()),
                _ => sketch.curve(id)?.radius(),
            };
            match (size(a), size(b)) {
                (Some(x), Some(y)) => (x - y).abs() <= tolerance::LINEAR,
                _ => true,
            }
        }
        _ => true,
    }
}

/// Relates `point` (a new or moved endpoint) to the curves it was cut or extended to:
/// coincident with one of their endpoints if it sits on one, otherwise on the first curve.
pub(super) fn bind_endpoint(
    sketch: &mut Sketch,
    point: EntityId,
    curves: &[EntityId],
) -> Result<(), OpError> {
    let pos = sketch.point(point);
    if curves.iter().any(|&x| {
        sketch
            .entity(x)
            .is_some_and(|e| e.geometry.points().contains(&point))
    }) {
        return Ok(()); // already joined through a shared point
    }
    for &x in curves {
        if let Some((s, e)) = sketch.endpoints(x) {
            for q in [s, e] {
                if sketch.point(q).distance(pos) <= tolerance::LINEAR {
                    sketch.add_constraint(ConstraintKind::Coincident(point, q))?;
                    return Ok(());
                }
            }
        }
    }
    // A spline can't hold a point: an end cut by one is left where it was cut.
    if let Some(&x) = curves
        .iter()
        .find(|x| sketch.kind(**x).is_some_and(EntityKind::is_analytic))
    {
        sketch.add_constraint(ConstraintKind::PointOnCurve { point, curve: x })?;
    }
    Ok(())
}

/// Replaces `old` by `new` in a constraint, removing it if that makes it invalid.
fn retarget(sketch: &mut Sketch, id: ConstraintId, old: EntityId, new: EntityId) {
    let Some(c) = sketch.constraint(id) else {
        return;
    };
    let kind = map_ids(c.kind, |e| if e == old { new } else { e });
    if sketch.validate(&kind).is_ok() {
        if let Some(c) = sketch.constraint_mut(id) {
            c.kind = kind;
        }
    } else {
        sketch.remove_constraint(id);
    }
}

/// A constraint kind with every entity id passed through `f`.
fn map_ids(kind: ConstraintKind, f: impl Fn(EntityId) -> EntityId) -> ConstraintKind {
    use ConstraintKind::*;
    match kind {
        Coincident(a, b) => Coincident(f(a), f(b)),
        PointOnCurve { point, curve } => PointOnCurve {
            point: f(point),
            curve: f(curve),
        },
        Horizontal(a) => Horizontal(f(a)),
        Vertical(a) => Vertical(f(a)),
        HorizontalPoints(a, b) => HorizontalPoints(f(a), f(b)),
        VerticalPoints(a, b) => VerticalPoints(f(a), f(b)),
        Parallel(a, b) => Parallel(f(a), f(b)),
        Perpendicular(a, b) => Perpendicular(f(a), f(b)),
        Tangent(a, b) => Tangent(f(a), f(b)),
        Equal(a, b) => Equal(f(a), f(b)),
        Concentric(a, b) => Concentric(f(a), f(b)),
        Midpoint { point, line } => Midpoint {
            point: f(point),
            line: f(line),
        },
        Symmetric { a, b, axis } => Symmetric {
            a: f(a),
            b: f(b),
            axis: f(axis),
        },
        Fix { point, at } => Fix {
            point: f(point),
            at,
        },
        Distance(a, b) => Distance(f(a), f(b)),
        Length(a) => Length(f(a)),
        HorizontalDistance(a, b) => HorizontalDistance(f(a), f(b)),
        VerticalDistance(a, b) => VerticalDistance(f(a), f(b)),
        Radius(a) => Radius(f(a)),
        Diameter(a) => Diameter(f(a)),
        Angle(a, b) => Angle(f(a), f(b)),
        DoubledDistance(a, b) => DoubledDistance(f(a), f(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: DVec2, b: DVec2) -> bool {
        a.distance(b) < 1e-9
    }

    fn has(s: &Sketch, kind: ConstraintKind) -> bool {
        s.constraints().any(|(_, c)| c.kind == kind)
    }

    fn line_ends(s: &Sketch, l: EntityId) -> (DVec2, DVec2) {
        let (a, b) = s.endpoints(l).unwrap();
        (s.point(a), s.point(b))
    }

    #[test]
    fn line_line_shortens_and_drops_stale_constraints() {
        let mut s = Sketch::new();
        let h = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let v = s.add_line(DVec2::new(4.0, -5.0), DVec2::new(4.0, 5.0));
        let (a, b) = s.endpoints(h).unwrap();
        s.add_constraint(ConstraintKind::Horizontal(h)).unwrap();
        let len = s.add_constraint(ConstraintKind::Length(h)).unwrap();
        s.fix(h).unwrap();
        // Something attached to the end that is trimmed away.
        let tail = s.add_line(DVec2::new(10.0, 0.0), DVec2::new(10.0, 3.0));
        let (t0, _) = s.endpoints(tail).unwrap();
        s.add_constraint(ConstraintKind::Coincident(b, t0)).unwrap();

        let kept = trim(&mut s, h, DVec2::new(8.0, 0.1)).unwrap();
        assert_eq!(kept, vec![h]);
        let (p, q) = line_ends(&s, h);
        assert!(close(p, DVec2::ZERO) && close(q, DVec2::new(4.0, 0.0)));
        assert!(has(&s, ConstraintKind::Horizontal(h)));
        assert!(s.constraint(len).is_none(), "length changed");
        assert!(has(
            &s,
            ConstraintKind::Fix {
                point: a,
                at: DVec2::ZERO
            }
        ));
        assert!(
            !s.constraints()
                .any(|(_, c)| matches!(c.kind, ConstraintKind::Fix { point, .. } if point == b)),
            "the moved end is no longer fixed"
        );
        assert!(!has(&s, ConstraintKind::Coincident(b, t0)));
        assert!(has(&s, ConstraintKind::PointOnCurve { point: b, curve: v }));
    }

    #[test]
    fn middle_piece_splits_the_line() {
        let mut s = Sketch::new();
        let h = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let v1 = s.add_line(DVec2::new(3.0, -1.0), DVec2::new(3.0, 1.0));
        let v2 = s.add_line(DVec2::new(7.0, -1.0), DVec2::new(7.0, 1.0));
        s.set_construction(v2, true); // construction geometry still bounds trims
        let (_, b) = s.endpoints(h).unwrap();
        s.add_constraint(ConstraintKind::Horizontal(h)).unwrap();
        let len = s.add_constraint(ConstraintKind::Length(h)).unwrap();
        let tail = s.add_line(DVec2::new(10.0, 0.0), DVec2::new(10.0, 3.0));
        let (t0, _) = s.endpoints(tail).unwrap();
        s.add_constraint(ConstraintKind::Coincident(b, t0)).unwrap();
        // A point resting on the right-hand part.
        let dot = s.add_point(DVec2::new(9.0, 0.0));
        let on = s
            .add_constraint(ConstraintKind::PointOnCurve {
                point: dot,
                curve: h,
            })
            .unwrap();

        let kept = trim(&mut s, h, DVec2::new(5.0, 0.0)).unwrap();
        assert_eq!(kept.len(), 2);
        let new = kept[1];
        let (p, q) = line_ends(&s, h);
        assert!(close(p, DVec2::ZERO) && close(q, DVec2::new(3.0, 0.0)));
        let (p, q) = line_ends(&s, new);
        assert!(close(p, DVec2::new(7.0, 0.0)) && close(q, DVec2::new(10.0, 0.0)));
        // The new piece took the old end point and its coincidence.
        let (ns, ne) = s.endpoints(new).unwrap();
        assert_eq!(ne, b);
        assert_eq!(s.entity(b).unwrap().owner, Some(new));
        assert!(has(&s, ConstraintKind::Coincident(b, t0)));
        // Collinear: horizontal copied, start on the original.
        assert!(has(&s, ConstraintKind::Horizontal(new)));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: ns,
                curve: h
            }
        ));
        assert!(s.constraint(len).is_none());
        assert_eq!(
            s.constraint(on).unwrap().kind,
            ConstraintKind::PointOnCurve {
                point: dot,
                curve: new
            }
        );
        // Cut ends bound by the crossing lines.
        let (_, hb) = s.endpoints(h).unwrap();
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: hb,
                curve: v1
            }
        ));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: ns,
                curve: v2
            }
        ));
    }

    #[test]
    fn line_through_circle() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let c = s.add_circle(DVec2::new(5.0, 0.0), 2.0);
        let kept = trim(&mut s, l, DVec2::new(5.0, 0.0)).unwrap();
        assert_eq!(kept.len(), 2);
        let (_, q) = line_ends(&s, kept[0]);
        let (p, _) = line_ends(&s, kept[1]);
        assert!(close(q, DVec2::new(3.0, 0.0)) && close(p, DVec2::new(7.0, 0.0)));
        // No direction relation on the original: the new piece is parallel to it.
        assert!(has(&s, ConstraintKind::Parallel(kept[1], l)));
        let (_, qa) = s.endpoints(l).unwrap();
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: qa,
                curve: c
            }
        ));
    }

    #[test]
    fn circle_becomes_arc() {
        let mut s = Sketch::new();
        let c = s.add_circle(DVec2::new(5.0, 0.0), 2.0);
        let center = s.center(c).unwrap();
        let radius = s.add_constraint(ConstraintKind::Radius(c)).unwrap();
        let fix = s.fix(center).unwrap()[0];
        s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let kept = trim(&mut s, c, DVec2::new(5.0, 2.1)).unwrap();
        assert_eq!(kept.len(), 1);
        let arc = kept[0];
        assert!(s.entity(c).is_none());
        assert_eq!(s.kind(arc), Some(EntityKind::Arc));
        // The arc keeps the centre point, the fix and the radius dimension.
        assert_eq!(s.center(arc), Some(center));
        assert_eq!(s.entity(center).unwrap().owner, Some(arc));
        assert!(s.constraint(fix).is_some());
        assert_eq!(
            s.constraint(radius).unwrap().kind,
            ConstraintKind::Radius(arc)
        );
        // The lower half remains: from (3,0) counter-clockwise to (7,0).
        let (a, b) = line_ends(&s, arc);
        assert!(close(a, DVec2::new(3.0, 0.0)) && close(b, DVec2::new(7.0, 0.0)));
        assert!(s.curve(arc).unwrap().distance(DVec2::new(5.0, -2.0)) < 1e-9);
    }

    #[test]
    fn circle_trimmed_by_two_lines() {
        let mut s = Sketch::new();
        let c = s.add_circle(DVec2::ZERO, 2.0);
        let l1 = s.add_line(DVec2::new(-1.0, -5.0), DVec2::new(-1.0, 5.0));
        let l2 = s.add_line(DVec2::new(1.0, -5.0), DVec2::new(1.0, 5.0));
        let arc = trim(&mut s, c, DVec2::new(2.0, 0.0)).unwrap()[0];
        let h = 3f64.sqrt();
        let (a, b) = line_ends(&s, arc);
        assert!(close(a, DVec2::new(1.0, h)) && close(b, DVec2::new(1.0, -h)));
        assert!(s.curve(arc).unwrap().distance(DVec2::new(-2.0, 0.0)) < 1e-9);
        let (sa, sb) = s.endpoints(arc).unwrap();
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: sa,
                curve: l2
            }
        ));
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: sb,
                curve: l2
            }
        ));
        // On the arc, the piece from its start round to l1 is removed: a shorten.
        let kept = trim(&mut s, arc, DVec2::new(0.0, 2.0)).unwrap();
        assert_eq!(kept, vec![arc]);
        let (a, _) = line_ends(&s, arc);
        assert!(close(a, DVec2::new(-1.0, h)));
        let (sa, _) = s.endpoints(arc).unwrap();
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: sa,
                curve: l1
            }
        ));
        // Put the start back on l2 and trim between the two crossings of l1: a split.
        s.set_point(sa, DVec2::new(1.0, h));
        let kept = trim(&mut s, arc, DVec2::new(-2.0, 0.0)).unwrap();
        assert_eq!(kept.len(), 2);
        let (s0, e0) = line_ends(&s, kept[0]);
        let (s1, e1) = line_ends(&s, kept[1]);
        assert!(close(s0, DVec2::new(1.0, h)) && close(e0, DVec2::new(-1.0, h)));
        assert!(close(s1, DVec2::new(-1.0, -h)) && close(e1, DVec2::new(1.0, -h)));
        assert!(has(&s, ConstraintKind::Concentric(kept[1], arc)));
        assert!(has(&s, ConstraintKind::Equal(kept[1], arc)));
    }

    #[test]
    fn arc_end_is_shortened() {
        let mut s = Sketch::new();
        let arc = s.add_arc(DVec2::ZERO, DVec2::new(5.0, 0.0), DVec2::new(-5.0, 0.0));
        let l = s.add_line(DVec2::new(0.0, -10.0), DVec2::new(0.0, 10.0));
        let kept = trim(&mut s, arc, DVec2::new(4.0, 3.0)).unwrap();
        assert_eq!(kept, vec![arc]);
        let (a, b) = line_ends(&s, arc);
        assert!(close(a, DVec2::new(0.0, 5.0)) && close(b, DVec2::new(-5.0, 0.0)));
        let (sa, _) = s.endpoints(arc).unwrap();
        assert!(has(
            &s,
            ConstraintKind::PointOnCurve {
                point: sa,
                curve: l
            }
        ));
    }

    #[test]
    fn cut_on_existing_endpoint_gives_coincident() {
        let mut s = Sketch::new();
        let a = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let b = s.add_line(DVec2::new(5.0, 0.0), DVec2::new(5.0, 5.0));
        let kept = trim(&mut s, a, DVec2::new(8.0, 0.0)).unwrap();
        assert_eq!(kept, vec![a]);
        let (_, ae) = s.endpoints(a).unwrap();
        let (bs, _) = s.endpoints(b).unwrap();
        assert!(has(&s, ConstraintKind::Coincident(ae, bs)));
    }

    #[test]
    fn isolated_curve_is_deleted() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::X);
        s.add_line(DVec2::new(0.0, 1.0), DVec2::ONE);
        assert_eq!(trim(&mut s, l, DVec2::new(0.5, 0.0)).unwrap(), vec![]);
        assert!(s.entity(l).is_none());
        let c = s.add_circle(DVec2::new(5.0, 5.0), 1.0);
        assert_eq!(trim(&mut s, c, DVec2::new(6.0, 5.0)).unwrap(), vec![]);
        assert!(s.entity(c).is_none());
        assert!(matches!(
            trim(&mut s, Sketch::ORIGIN, DVec2::ZERO),
            Err(OpError::Sketch(_) | OpError::Unsupported(_))
        ));
    }

    #[test]
    fn neighbours_at_the_ends_do_not_bound() {
        // An L corner: trimming the horizontal leg near its free end, with the only other
        // curve meeting it at its far end, deletes it.
        let mut s = Sketch::new();
        let a = s.add_line(DVec2::ZERO, DVec2::new(5.0, 0.0));
        s.add_line(DVec2::ZERO, DVec2::new(0.0, 5.0));
        assert!(trim(&mut s, a, DVec2::new(4.0, 0.0)).unwrap().is_empty());
    }

    #[test]
    fn shared_endpoint_is_detached_not_dragged() {
        let mut s = Sketch::new();
        let a = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let b = s.add_line(DVec2::new(10.0, 0.0), DVec2::new(10.0, 5.0));
        let (_, ae) = s.endpoints(a).unwrap();
        let (bs, _) = s.endpoints(b).unwrap();
        s.replace_curve_point(b, bs, ae);
        s.remove_entity(bs).unwrap();
        s.add_line(DVec2::new(6.0, -1.0), DVec2::new(6.0, 1.0));
        trim(&mut s, a, DVec2::new(8.0, 0.0)).unwrap();
        assert!(close(s.point(ae), DVec2::new(10.0, 0.0)), "b's start stays");
        assert_eq!(s.endpoints(b).unwrap().0, ae);
        let (_, q) = line_ends(&s, a);
        assert!(close(q, DVec2::new(6.0, 0.0)));
    }
}
