use peet_math::{DVec2, tolerance};

use super::OpError;
use super::fillet::PointGroups;
use crate::curve::Curve;
use crate::sketch::{ConstraintKind, EntityId, EntityKind, Sketch};

/// Offsets a curve, or a connected chain of curves, by `distance` (mm). Positive offsets go
/// to the left of the chain's direction of travel (outward for a clockwise loop, inward
/// for a counter-clockwise one). Consecutive offset curves are trimmed/extended to meet,
/// and joined with coincident relations. Returns the new curves in chain order.
///
/// **Chain and direction.** The curves may be given in any order and direction; they must
/// form a single open chain or closed loop, joined end to end (same point entity,
/// coincident relation, or the same position). The direction of travel is the direction
/// of `curves[0]` (a line from start to end, an arc counter-clockwise); the chain is
/// ordered from there. A circle can only be offset on its own (its direction is
/// counter-clockwise, so a positive distance shrinks it).
///
/// **Joints.** Where the original curves meet smoothly, the offsets still meet; elsewhere
/// they are extended or trimmed to the intersection of their carriers nearest the original
/// joint (a sharp corner, even around a convex corner).
///
/// **Relations**, none redundant: coincident at each joint; each offset line parallel to
/// its original, each arc/circle concentric with its original; tangent (line–arc or
/// arc–arc) where the original joint is smooth, except one in a closed loop whose joints
/// are all smooth (there the last tangency is implied by the others). One driving
/// `Distance` dimension from the first offset line's start point to its original line
/// records the offset distance. There is no "equal offset" relation in the model, so
/// pieces separated by sharp corners keep their own offset freedom.
///
/// Errors: [`OpError::TooLarge`] when an arc or circle would reach zero radius, or an
/// offset piece would collapse or reverse; [`OpError::NoIntersection`] when two
/// consecutive offset pieces no longer meet; [`OpError::NotConnected`] when the curves
/// don't form a single chain; [`OpError::Unsupported`] for a zero distance, non-curves or
/// splines (the offset of a spline is not a spline).
pub fn offset(
    sketch: &mut Sketch,
    curves: &[EntityId],
    distance: f64,
) -> Result<Vec<EntityId>, OpError> {
    if distance.abs() <= tolerance::LINEAR {
        return Err(OpError::Unsupported("an offset distance of zero"));
    }
    let mut ids: Vec<EntityId> = Vec::new();
    for &id in curves {
        let kind = sketch.kind(id).ok_or(OpError::Missing(id))?;
        if !kind.is_curve() {
            return Err(OpError::Unsupported("offsetting points"));
        }
        if kind == EntityKind::Spline {
            return Err(OpError::Unsupported(
                "offsetting a spline. Draw the offset curve as a spline of its own, or offset \
                 the lines and arcs without it",
            ));
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let Some(&first) = ids.first() else {
        return Ok(Vec::new());
    };
    if ids
        .iter()
        .any(|&id| sketch.kind(id) == Some(EntityKind::Circle))
    {
        if ids.len() > 1 {
            return Err(OpError::NotConnected);
        }
        return offset_circle(sketch, first, distance);
    }

    let (chain, closed) = order_chain(sketch, &ids)?;
    let segs: Vec<Seg> = chain
        .iter()
        .map(|&(id, reversed)| Seg::new(sketch, id, reversed, distance))
        .collect::<Result<_, _>>()?;
    let n = segs.len();

    // Joint k is between seg k and seg k + 1 (the last one closes a loop).
    let joint_count = if closed { n } else { n - 1 };
    let mut starts: Vec<DVec2> = segs.iter().map(|s| s.naive_start).collect();
    let mut ends: Vec<DVec2> = segs.iter().map(|s| s.naive_end).collect();
    let mut smooth = vec![false; joint_count];
    for k in 0..joint_count {
        let (a, b) = (&segs[k], &segs[(k + 1) % n]);
        let p = if a.naive_end.distance(b.naive_start) <= tolerance::LINEAR {
            smooth[k] = true;
            (a.naive_end + b.naive_start) * 0.5
        } else {
            let joint = a.orig.end_travel();
            a.carrier
                .intersect_unbounded(&b.carrier)
                .into_iter()
                .map(|i| i.point)
                .min_by(|p, q| p.distance(joint).total_cmp(&q.distance(joint)))
                .ok_or(OpError::NoIntersection)?
        };
        ends[k] = p;
        starts[(k + 1) % n] = p;
    }

    // Check every piece before creating anything, so a failure leaves the sketch as is.
    for (k, seg) in segs.iter().enumerate() {
        seg.check(starts[k], ends[k])?;
    }

    // Create the offset curves.
    let mut new_ids = Vec::with_capacity(n);
    for (k, seg) in segs.iter().enumerate() {
        let id = seg.create(sketch, starts[k], ends[k]);
        let construction = sketch.entity(seg.id).is_some_and(|e| e.construction);
        sketch.set_construction(id, construction);
        new_ids.push(id);
    }

    // Relations.
    for (k, seg) in segs.iter().enumerate() {
        let kind = if seg.is_line() {
            ConstraintKind::Parallel(new_ids[k], seg.id)
        } else {
            ConstraintKind::Concentric(new_ids[k], seg.id)
        };
        sketch.add_constraint(kind)?;
    }
    let all_smooth = closed && smooth.iter().all(|&s| s);
    let mut skipped_closing_tangent = !all_smooth;
    for k in (0..joint_count).rev() {
        let (a, b) = (k, (k + 1) % n);
        let end = travel_end(sketch, new_ids[a], segs[a].reversed);
        let start = travel_start(sketch, new_ids[b], segs[b].reversed);
        sketch.add_constraint(ConstraintKind::Coincident(end, start))?;
        if smooth[k] && !(segs[a].is_line() && segs[b].is_line()) {
            if skipped_closing_tangent {
                sketch.add_constraint(ConstraintKind::Tangent(new_ids[a], new_ids[b]))?;
            } else {
                skipped_closing_tangent = true;
            }
        }
    }
    if let Some(k) = segs.iter().position(Seg::is_line) {
        let (start, _) = sketch.endpoints(new_ids[k]).expect("a line");
        sketch.add_dimension(ConstraintKind::Distance(start, segs[k].id), distance.abs())?;
    }
    Ok(new_ids)
}

fn offset_circle(
    sketch: &mut Sketch,
    id: EntityId,
    distance: f64,
) -> Result<Vec<EntityId>, OpError> {
    let Some(Curve::Circle { center, radius }) = sketch.curve(id) else {
        unreachable!("checked to be a circle");
    };
    let r = radius - distance;
    if r <= tolerance::LINEAR {
        return Err(OpError::TooLarge);
    }
    let new = sketch.add_circle(center, r);
    let construction = sketch.entity(id).is_some_and(|e| e.construction);
    sketch.set_construction(new, construction);
    sketch.add_constraint(ConstraintKind::Concentric(new, id))?;
    Ok(vec![new])
}

fn travel_start(sketch: &Sketch, curve: EntityId, reversed: bool) -> EntityId {
    let (s, e) = sketch.endpoints(curve).expect("a line or arc");
    if reversed { e } else { s }
}

fn travel_end(sketch: &Sketch, curve: EntityId, reversed: bool) -> EntityId {
    let (s, e) = sketch.endpoints(curve).expect("a line or arc");
    if reversed { s } else { e }
}

/// A line or arc with its direction of travel in the chain.
#[derive(Clone)]
struct Travel {
    curve: Curve,
    reversed: bool,
}

impl Travel {
    fn start_travel(&self) -> DVec2 {
        if self.reversed {
            self.curve.end()
        } else {
            self.curve.start()
        }
    }

    fn end_travel(&self) -> DVec2 {
        if self.reversed {
            self.curve.start()
        } else {
            self.curve.end()
        }
    }
}

/// One chain element and its offset carrier.
struct Seg {
    id: EntityId,
    reversed: bool,
    orig: Travel,
    /// The offset curve's carrier: an (unbounded) line, or the offset circle.
    carrier: Curve,
    naive_start: DVec2,
    naive_end: DVec2,
}

impl Seg {
    fn new(sketch: &Sketch, id: EntityId, reversed: bool, distance: f64) -> Result<Self, OpError> {
        let curve = sketch.curve(id).ok_or(OpError::Missing(id))?;
        let orig = Travel {
            curve: curve.clone(),
            reversed,
        };
        let (a, b) = (orig.start_travel(), orig.end_travel());
        match curve {
            Curve::Line { .. } => {
                let Some(dir) = (b - a).try_normalize() else {
                    return Err(OpError::TooLarge);
                };
                let shift = dir.perp() * distance;
                Ok(Self {
                    id,
                    reversed,
                    orig,
                    carrier: Curve::Line {
                        a: a + shift,
                        b: b + shift,
                    },
                    naive_start: a + shift,
                    naive_end: b + shift,
                })
            }
            Curve::Arc { center, radius, .. } => {
                // Left of a counter-clockwise arc is towards the centre.
                let r = if reversed {
                    radius + distance
                } else {
                    radius - distance
                };
                if r <= tolerance::LINEAR {
                    return Err(OpError::TooLarge);
                }
                let on = |p: DVec2| center + (p - center).normalize_or(DVec2::X) * r;
                Ok(Self {
                    id,
                    reversed,
                    orig,
                    carrier: Curve::Circle { center, radius: r },
                    naive_start: on(a),
                    naive_end: on(b),
                })
            }
            Curve::Circle { .. } | Curve::Spline(_) => Err(OpError::NotConnected),
        }
    }

    fn is_line(&self) -> bool {
        matches!(self.orig.curve, Curve::Line { .. })
    }

    /// The offset piece's own start and end (original orientation) from the joints
    /// `start` and `end` (travel order).
    fn native(&self, start: DVec2, end: DVec2) -> (DVec2, DVec2) {
        if self.reversed {
            (end, start)
        } else {
            (start, end)
        }
    }

    /// Fails with [`OpError::TooLarge`] if the piece between the joints collapsed: a line
    /// that reversed (or shrank to nothing), or an arc whose sweep flipped.
    fn check(&self, start: DVec2, end: DVec2) -> Result<(), OpError> {
        let (s, e) = self.native(start, end);
        let ok = match (&self.orig.curve, &self.carrier) {
            (Curve::Line { a, b }, _) => (e - s).dot(*b - *a) > tolerance::LINEAR * a.distance(*b),
            (Curve::Arc { sweep, .. }, Curve::Circle { center, .. }) => {
                match Curve::arc_from_points(*center, s, e) {
                    Curve::Arc { sweep: new, .. } => (new - sweep).abs() <= std::f64::consts::PI,
                    _ => false,
                }
            }
            _ => false,
        };
        if ok { Ok(()) } else { Err(OpError::TooLarge) }
    }

    /// Adds the offset curve between the joints `start` and `end` (in travel order),
    /// keeping the original's orientation.
    fn create(&self, sketch: &mut Sketch, start: DVec2, end: DVec2) -> EntityId {
        let (s, e) = self.native(start, end);
        match self.carrier {
            Curve::Circle { center, .. } => sketch.add_arc(center, s, e),
            _ => sketch.add_line(s, e),
        }
    }
}

/// Orders connected lines/arcs into a chain following `ids[0]`'s direction. Returns
/// `(curve, reversed)` pairs and whether the chain is closed.
fn order_chain(
    sketch: &Sketch,
    ids: &[EntityId],
) -> Result<(Vec<(EntityId, bool)>, bool), OpError> {
    let groups = PointGroups::new(sketch);
    // End slots: (curve index, is_end, point id, position).
    let slots: Vec<(usize, bool, EntityId, DVec2)> = ids
        .iter()
        .enumerate()
        .flat_map(|(i, &id)| {
            let (s, e) = sketch.endpoints(id).expect("a line or arc");
            [
                (i, false, s, sketch.point(s)),
                (i, true, e, sketch.point(e)),
            ]
        })
        .collect();
    let joined = |x: &(usize, bool, EntityId, DVec2), y: &(usize, bool, EntityId, DVec2)| {
        x.0 != y.0 && (groups.same(x.2, y.2) || x.3.distance(y.3) <= tolerance::LINEAR)
    };
    // Neighbour of each slot: (curve index, joined at its end?).
    let mut next: Vec<Option<(usize, bool)>> = vec![None; slots.len()];
    for (k, x) in slots.iter().enumerate() {
        let mut found = slots.iter().filter(|y| joined(x, y));
        if let Some(y) = found.next() {
            if found.next().is_some() {
                return Err(OpError::NotConnected); // a branch
            }
            next[k] = Some((y.0, y.1));
        }
    }
    let slot = |curve: usize, at_end: bool| 2 * curve + usize::from(at_end);

    // Walk forward from ids[0]'s end.
    let mut chain = vec![(0usize, false)];
    let mut closed = false;
    let mut cur = (0usize, false);
    loop {
        let exit = slot(cur.0, !cur.1);
        let Some((c, at_end)) = next[exit] else { break };
        if c == 0 {
            closed = true;
            break;
        }
        // Entering at its end means travelling it backwards.
        cur = (c, at_end);
        if chain.iter().any(|&(i, _)| i == c) {
            return Err(OpError::NotConnected);
        }
        chain.push(cur);
    }
    if !closed {
        // Walk backward from ids[0]'s start, prepending.
        let mut cur = (0usize, false);
        loop {
            let entry = slot(cur.0, cur.1);
            let Some((c, at_end)) = next[entry] else {
                break;
            };
            // Leaving it at its start means travelling it backwards.
            cur = (c, !at_end);
            if chain.iter().any(|&(i, _)| i == c) {
                return Err(OpError::NotConnected);
            }
            chain.insert(0, cur);
        }
    }
    if chain.len() != ids.len() {
        return Err(OpError::NotConnected);
    }
    Ok((
        chain.into_iter().map(|(i, r)| (ids[i], r)).collect(),
        closed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::testing::{assert_satisfied, close, count};
    use crate::shapes::{rectangle, slot};

    fn line_pts(s: &Sketch, id: EntityId) -> (DVec2, DVec2) {
        let c = s.curve(id).unwrap();
        (c.start(), c.end())
    }

    #[test]
    fn single_line_both_signs() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(4.0, 0.0));
        let up = offset(&mut s, &[l], 1.5).unwrap();
        assert_eq!(
            line_pts(&s, up[0]),
            (DVec2::new(0.0, 1.5), DVec2::new(4.0, 1.5))
        );
        let down = offset(&mut s, &[l], -2.0).unwrap();
        assert_eq!(
            line_pts(&s, down[0]),
            (DVec2::new(0.0, -2.0), DVec2::new(4.0, -2.0))
        );
        assert_eq!(count(&s, "Parallel"), 2);
        assert_eq!(count(&s, "Distance"), 2);
        assert_satisfied(&s);
        assert_eq!(
            offset(&mut s, &[l], 0.0),
            Err(OpError::Unsupported("an offset distance of zero"))
        );
    }

    #[test]
    fn open_l_chain_in_any_order() {
        let mut s = Sketch::new();
        // (0,0) -> (4,0) -> (4,3), given out of order and with the second line reversed.
        let a = s.add_line(DVec2::ZERO, DVec2::new(4.0, 0.0));
        let b = s.add_line(DVec2::new(4.0, 3.0), DVec2::new(4.0, 0.0));
        let out = offset(&mut s, &[a, b], 1.0).unwrap();
        // Travel follows `a`: left of +X is +Y, left of +Y (b travelled upwards) is -X.
        assert_eq!(out.len(), 2);
        assert!(close(line_pts(&s, out[0]).0, DVec2::new(0.0, 1.0)));
        assert!(close(line_pts(&s, out[0]).1, DVec2::new(3.0, 1.0)));
        // b's copy keeps b's orientation (downwards).
        assert!(close(line_pts(&s, out[1]).0, DVec2::new(3.0, 3.0)));
        assert!(close(line_pts(&s, out[1]).1, DVec2::new(3.0, 1.0)));
        assert_eq!(count(&s, "Coincident"), 1);
        assert_satisfied(&s);

        // Starting from b reverses the travel direction, so the sign flips.
        let out = offset(&mut s, &[b, a], 1.0).unwrap();
        assert!(close(line_pts(&s, out[0]).1, DVec2::new(5.0, -1.0)));
        assert!(close(line_pts(&s, out[1]).0, DVec2::new(0.0, -1.0)));
        assert_satisfied(&s);
    }

    #[test]
    fn closed_rectangle_inward_and_outward() {
        let mut s = Sketch::new();
        let rect = rectangle(&mut s, DVec2::ZERO, DVec2::new(10.0, 6.0));
        // Counter-clockwise loop: positive is inward.
        let inner = offset(&mut s, &rect.curves, 1.0).unwrap();
        let corners: Vec<DVec2> = inner.iter().map(|&l| line_pts(&s, l).0).collect();
        assert!(close(corners[0], DVec2::new(1.0, 1.0)));
        assert!(close(corners[2], DVec2::new(9.0, 5.0)));
        let outer = offset(
            &mut s,
            &[
                rect.curves[2],
                rect.curves[0],
                rect.curves[3],
                rect.curves[1],
            ],
            -2.0,
        )
        .unwrap();
        let (a, b) = line_pts(&s, outer[0]);
        assert!(close(a, DVec2::new(12.0, 8.0)) && close(b, DVec2::new(-2.0, 8.0)));
        assert_eq!(count(&s, "Coincident"), 12);
        assert_eq!(count(&s, "Parallel"), 8);
        assert_eq!(count(&s, "Tangent"), 0);
        assert_satisfied(&s);
        assert_eq!(offset(&mut s, &rect.curves, 3.5), Err(OpError::TooLarge));
    }

    #[test]
    fn arc_and_circle() {
        let mut s = Sketch::new();
        let arc = s.add_arc(DVec2::ZERO, DVec2::new(2.0, 0.0), DVec2::new(0.0, 2.0));
        let out = offset(&mut s, &[arc], 0.5).unwrap();
        let c = s.curve(out[0]).unwrap();
        assert_eq!(c.radius(), Some(1.5));
        assert!(close(c.end(), DVec2::new(0.0, 1.5)));
        assert_eq!(offset(&mut s, &[arc], 2.0), Err(OpError::TooLarge));
        let circle = s.add_circle(DVec2::ONE, 1.0);
        let out = offset(&mut s, &[circle], -1.0).unwrap();
        assert_eq!(s.curve(out[0]).unwrap().radius(), Some(2.0));
        assert_eq!(offset(&mut s, &[circle], 1.0), Err(OpError::TooLarge));
        assert_eq!(
            offset(&mut s, &[circle, arc], 0.1),
            Err(OpError::NotConnected)
        );
        assert_eq!(count(&s, "Concentric"), 2);
        assert_satisfied(&s);
    }

    #[test]
    fn slot_loop_keeps_tangency() {
        let mut s = Sketch::new();
        let shape = slot(&mut s, DVec2::ZERO, DVec2::new(10.0, 0.0), 2.0);
        let before = count(&s, "Tangent");
        let loop_curves = &shape.curves[..4];
        let outer = offset(
            &mut s,
            &[
                loop_curves[3],
                loop_curves[1],
                loop_curves[0],
                loop_curves[2],
            ],
            1.0,
        )
        .unwrap();
        // Travel follows arc1 counter-clockwise: a positive distance goes inward.
        let radii: Vec<f64> = outer
            .iter()
            .filter_map(|&c| s.curve(c).unwrap().radius())
            .collect();
        assert_eq!(radii, vec![1.0, 1.0]);
        // All 4 joints smooth in a closed loop: 3 tangent relations (the 4th is implied).
        assert_eq!(count(&s, "Tangent") - before, 3);
        let bigger = offset(&mut s, loop_curves, -1.0).unwrap();
        let (a, b) = line_pts(&s, bigger[0]);
        assert!(close(a, DVec2::new(0.0, -3.0)) && close(b, DVec2::new(10.0, -3.0)));
        assert_satisfied(&s);
        assert_eq!(offset(&mut s, loop_curves, 2.0), Err(OpError::TooLarge));
    }

    #[test]
    fn open_chain_with_tangent_arc() {
        let mut s = Sketch::new();
        // Line (0,0)->(5,0), then a CCW quarter arc around (5,2) up to (7,2).
        let l = s.add_line(DVec2::ZERO, DVec2::new(5.0, 0.0));
        let arc = s.add_arc(
            DVec2::new(5.0, 2.0),
            DVec2::new(5.0, 0.0),
            DVec2::new(7.0, 2.0),
        );
        let (_, le) = s.endpoints(l).unwrap();
        let (as_, _) = s.endpoints(arc).unwrap();
        s.add_constraint(ConstraintKind::Coincident(le, as_))
            .unwrap();
        let out = offset(&mut s, &[arc, l], -0.5).unwrap();
        assert!(close(line_pts(&s, out[0]).1, DVec2::new(5.0, -0.5)));
        assert_eq!(s.curve(out[1]).unwrap().radius(), Some(2.5));
        assert_eq!(count(&s, "Tangent"), 1);
        assert_satisfied(&s);
    }

    #[test]
    fn branches_and_gaps_are_not_chains() {
        let mut s = Sketch::new();
        let a = s.add_line(DVec2::ZERO, DVec2::X);
        let b = s.add_line(DVec2::X, DVec2::ONE);
        let c = s.add_line(DVec2::X, DVec2::new(2.0, 0.0));
        let far = s.add_line(DVec2::new(5.0, 5.0), DVec2::new(6.0, 5.0));
        assert_eq!(offset(&mut s, &[a, b, c], 0.1), Err(OpError::NotConnected));
        assert_eq!(offset(&mut s, &[a, far], 0.1), Err(OpError::NotConnected));
    }
}
