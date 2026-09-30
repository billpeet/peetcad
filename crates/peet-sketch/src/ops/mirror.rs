use std::collections::HashMap;

use peet_math::{DVec2, tolerance};

use super::OpError;
use super::fillet::PointGroups;
use crate::sketch::{ConstraintKind, EntityId, EntityKind, Geometry, Sketch};

/// Mirrors entities about `axis` (a line), creating copies tied to the originals with
/// symmetric relations (and equal radius for circles/arcs). Returns the new entities in the
/// same order as `entities`.
///
/// Relations, chosen so none is redundant:
/// - Each mirrored point gets `Symmetric(original, copy, axis)`. Points that coincide in
///   the original (the same point entity, or joined by coincident relations) are mirrored
///   once: the first copy gets the symmetric relation and the other copies are made
///   coincident with it, so the copies are connected the way the originals are.
/// - A point lying on the axis mirrors onto itself. Its copy is made coincident with the
///   original, and the original gets a point-on-curve relation to the axis (unless its
///   group is already tied to the axis), so the joined halves stay symmetric.
/// - Circles also get `Equal` radius. Arcs don't need it: symmetric centres and
///   endpoints already imply it.
///
/// Mirroring reverses orientation, so a copied arc runs counter-clockwise from the mirror
/// of the original's end to the mirror of its start. Copies keep the construction flag. A
/// free-standing point on the axis, or the axis itself, maps to itself (nothing is
/// created). Points owned by a curve that is also being mirrored map to the copy's point.
pub fn mirror(
    sketch: &mut Sketch,
    entities: &[EntityId],
    axis: EntityId,
) -> Result<Vec<EntityId>, OpError> {
    let Some(axis_entity) = sketch.entity(axis) else {
        return Err(OpError::Missing(axis));
    };
    if axis_entity.kind() != EntityKind::Line {
        return Err(OpError::Unsupported("the mirror axis must be a line"));
    }
    for &id in entities {
        if sketch.entity(id).is_none() {
            return Err(OpError::Missing(id));
        }
    }
    let axis_curve = sketch.curve(axis).expect("a line");
    let Some(dir) = axis_curve.line_direction() else {
        return Err(OpError::Unsupported("the mirror axis has zero length"));
    };
    let origin = axis_curve.start();

    let mut m = Mirrorer {
        groups: PointGroups::new(sketch),
        reps: HashMap::new(),
        copies: HashMap::new(),
        axis,
        origin,
        dir,
    };

    // Curves first, so selected points owned by a mirrored curve map to its copy's points.
    let mut result: HashMap<EntityId, EntityId> = HashMap::new();
    for &id in entities {
        if id == axis || result.contains_key(&id) {
            continue;
        }
        let entity = sketch.entity(id).expect("checked above");
        let construction = entity.construction;
        let copy = match entity.geometry {
            Geometry::Point { .. } => continue,
            Geometry::Line { start, end } => {
                let copy =
                    sketch.add_line(m.reflect(sketch.point(start)), m.reflect(sketch.point(end)));
                let (s, e) = sketch.endpoints(copy).expect("a line");
                m.relate(sketch, start, s)?;
                m.relate(sketch, end, e)?;
                copy
            }
            Geometry::Arc { center, start, end } => {
                // Orientation reverses: the copy starts at the mirror of the original's end.
                let copy = sketch.add_arc(
                    m.reflect(sketch.point(center)),
                    m.reflect(sketch.point(end)),
                    m.reflect(sketch.point(start)),
                );
                let (s, e) = sketch.endpoints(copy).expect("an arc");
                let c = sketch.center(copy).expect("an arc");
                m.relate(sketch, center, c)?;
                m.relate(sketch, end, s)?;
                m.relate(sketch, start, e)?;
                copy
            }
            Geometry::Circle { center, radius } => {
                let copy = sketch.add_circle(m.reflect(sketch.point(center)), radius);
                let c = sketch.center(copy).expect("a circle");
                m.relate(sketch, center, c)?;
                sketch.add_constraint(ConstraintKind::Equal(id, copy))?;
                copy
            }
        };
        sketch.set_construction(copy, construction);
        result.insert(id, copy);
    }

    for &id in entities {
        if id == axis || result.contains_key(&id) || sketch.kind(id) != Some(EntityKind::Point) {
            continue;
        }
        let copy = match m.copies.get(&id) {
            Some(&copy) => copy,
            None if m.on_axis(sketch.point(id)) => id,
            None => {
                let copy = sketch.add_point(m.reflect(sketch.point(id)));
                let construction = sketch.entity(id).is_some_and(|e| e.construction);
                if let Some(e) = sketch.entity_mut(copy) {
                    e.construction = construction;
                }
                m.relate(sketch, id, copy)?;
                copy
            }
        };
        result.insert(id, copy);
    }

    Ok(entities
        .iter()
        .map(|id| if *id == axis { axis } else { result[id] })
        .collect())
}

struct Mirrorer {
    groups: PointGroups,
    /// Coincident group root of an original point -> the copy point representing the group.
    reps: HashMap<EntityId, EntityId>,
    /// Original point -> its (first) copy.
    copies: HashMap<EntityId, EntityId>,
    axis: EntityId,
    origin: DVec2,
    dir: DVec2,
}

impl Mirrorer {
    fn reflect(&self, p: DVec2) -> DVec2 {
        let v = p - self.origin;
        self.origin + self.dir * (2.0 * v.dot(self.dir)) - v
    }

    fn on_axis(&self, p: DVec2) -> bool {
        (p - self.origin).perp_dot(self.dir).abs() <= tolerance::LINEAR
    }

    /// Ties `copy` to `original` (see [`mirror`] for the rules).
    fn relate(
        &mut self,
        sketch: &mut Sketch,
        original: EntityId,
        copy: EntityId,
    ) -> Result<(), OpError> {
        self.copies.entry(original).or_insert(copy);
        let root = self.groups.root(original);
        if let Some(&rep) = self.reps.get(&root) {
            sketch.add_constraint(ConstraintKind::Coincident(copy, rep))?;
            return Ok(());
        }
        self.reps.insert(root, copy);
        if self.on_axis(sketch.point(original)) {
            sketch.add_constraint(ConstraintKind::Coincident(copy, original))?;
            if !self.tied_to_axis(sketch, original) {
                sketch.add_constraint(ConstraintKind::PointOnCurve {
                    point: original,
                    curve: self.axis,
                })?;
            }
        } else {
            sketch.add_constraint(ConstraintKind::Symmetric {
                a: original,
                b: copy,
                axis: self.axis,
            })?;
        }
        Ok(())
    }

    /// Whether a relation already keeps `point` (or a point coincident with it) on the axis.
    fn tied_to_axis(&self, sketch: &Sketch, point: EntityId) -> bool {
        let (a, b) = sketch.endpoints(self.axis).expect("a line");
        let members = self.groups.members(point);
        members.iter().any(|&p| {
            p == a
                || p == b
                || sketch.constraints().any(|(_, c)| match c.kind {
                    ConstraintKind::PointOnCurve { point, curve } => {
                        point == p && curve == self.axis
                    }
                    ConstraintKind::Midpoint { point, line } => point == p && line == self.axis,
                    _ => false,
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Curve;
    use crate::shapes::testing::{assert_satisfied, close, count};

    fn y_axis(s: &mut Sketch) -> EntityId {
        let a = s.add_line(DVec2::new(0.0, -10.0), DVec2::new(0.0, 10.0));
        s.set_construction(a, true);
        a
    }

    #[test]
    fn mirrors_a_line_with_symmetric_points() {
        let mut s = Sketch::new();
        let axis = y_axis(&mut s);
        let l = s.add_line(DVec2::new(1.0, 1.0), DVec2::new(3.0, 2.0));
        let out = mirror(&mut s, &[l], axis).unwrap();
        assert_eq!(out.len(), 1);
        let c = s.curve(out[0]).unwrap();
        assert!(close(c.start(), DVec2::new(-1.0, 1.0)));
        assert!(close(c.end(), DVec2::new(-3.0, 2.0)));
        assert_eq!(count(&s, "Symmetric"), 2);
        assert!(!s.entity(out[0]).unwrap().construction);
        assert_satisfied(&s);
    }

    #[test]
    fn mirrors_arcs_with_reversed_orientation() {
        let mut s = Sketch::new();
        let axis = y_axis(&mut s);
        // Quarter arc from (3, 0) to (2, 1) around (2, 0).
        let arc = s.add_arc(
            DVec2::new(2.0, 0.0),
            DVec2::new(3.0, 0.0),
            DVec2::new(2.0, 1.0),
        );
        let out = mirror(&mut s, &[arc], axis).unwrap();
        let Curve::Arc { center, sweep, .. } = s.curve(out[0]).unwrap() else {
            panic!("expected an arc")
        };
        assert!(close(center, DVec2::new(-2.0, 0.0)));
        assert!(
            (sweep - std::f64::consts::FRAC_PI_2).abs() < 1e-12,
            "short way round"
        );
        let c = s.curve(out[0]).unwrap();
        assert!(close(c.start(), DVec2::new(-2.0, 1.0)));
        assert!(close(c.end(), DVec2::new(-3.0, 0.0)));
        assert_eq!(count(&s, "Symmetric"), 3);
        assert_eq!(count(&s, "Equal"), 0);
        assert_satisfied(&s);
    }

    #[test]
    fn mirrors_circles_with_equal_radius() {
        let mut s = Sketch::new();
        // A slanted axis.
        let axis = s.add_line(DVec2::ZERO, DVec2::new(1.0, 1.0));
        let c = s.add_circle(DVec2::new(2.0, 0.0), 0.5);
        s.set_construction(c, true);
        let out = mirror(&mut s, &[c], axis).unwrap();
        let copy = s.curve(out[0]).unwrap();
        assert!(close(copy.center().unwrap(), DVec2::new(0.0, 2.0)));
        assert_eq!(copy.radius(), Some(0.5));
        assert!(s.entity(out[0]).unwrap().construction);
        assert_eq!(count(&s, "Equal"), 1);
        assert_eq!(count(&s, "Symmetric"), 1);
        assert_satisfied(&s);
    }

    #[test]
    fn connected_chain_stays_connected() {
        let mut s = Sketch::new();
        let axis = y_axis(&mut s);
        // An L from the axis: (0, 0) -> (2, 0) -> (2, 3), joined by a coincident relation.
        let a = s.add_line(DVec2::new(0.0, 0.0), DVec2::new(2.0, 0.0));
        let b = s.add_line(DVec2::new(2.0, 0.0), DVec2::new(2.0, 3.0));
        let (_, ae) = s.endpoints(a).unwrap();
        let (bs, _) = s.endpoints(b).unwrap();
        s.add_constraint(ConstraintKind::Coincident(ae, bs))
            .unwrap();
        let before = count(&s, "Coincident");
        let out = mirror(&mut s, &[a, b], axis).unwrap();
        // Joint: one symmetric + one coincident between the copies. On-axis start: coincident
        // with the original, plus point-on-axis. Far end of b: symmetric.
        assert_eq!(count(&s, "Symmetric"), 2);
        assert_eq!(count(&s, "Coincident") - before, 2);
        assert_eq!(count(&s, "Point on curve"), 1);
        let (_, ce) = s.endpoints(out[0]).unwrap();
        let (ds, _) = s.endpoints(out[1]).unwrap();
        assert!(
            s.constraints()
                .any(|(_, c)| c.kind == ConstraintKind::Coincident(ds, ce))
        );
        assert!(close(s.curve(out[1]).unwrap().end(), DVec2::new(-2.0, 3.0)));
        assert_satisfied(&s);
    }

    #[test]
    fn point_on_axis_already_tied_gets_no_extra_relation() {
        let mut s = Sketch::new();
        let axis = y_axis(&mut s);
        let (axis_start, _) = s.endpoints(axis).unwrap();
        let l = s.add_line(DVec2::new(0.0, -10.0), DVec2::new(4.0, -8.0));
        let (ls, _) = s.endpoints(l).unwrap();
        s.add_constraint(ConstraintKind::Coincident(ls, axis_start))
            .unwrap();
        mirror(&mut s, &[l], axis).unwrap();
        assert_eq!(count(&s, "Point on curve"), 0);
        assert_satisfied(&s);
    }

    #[test]
    fn shared_point_entities_and_points() {
        let mut s = Sketch::new();
        let axis = y_axis(&mut s);
        let a = s.add_line(DVec2::new(1.0, 0.0), DVec2::new(2.0, 0.0));
        let b = s.add_line(DVec2::new(2.0, 0.0), DVec2::new(2.0, 1.0));
        let (_, ae) = s.endpoints(a).unwrap();
        let (bs, _) = s.endpoints(b).unwrap();
        assert!(s.replace_curve_point(b, bs, ae));
        s.remove_entity(bs).unwrap();
        let free = s.add_point(DVec2::new(5.0, 5.0));
        let on_axis = s.add_point(DVec2::new(0.0, 5.0));
        let out = mirror(&mut s, &[free, a, b, on_axis, ae, axis], axis).unwrap();
        assert_eq!(out.len(), 6);
        assert!(close(s.point(out[0]), DVec2::new(-5.0, 5.0)));
        assert_eq!(out[3], on_axis, "points on the axis map to themselves");
        assert_eq!(
            out[4],
            s.endpoints(out[1]).unwrap().1,
            "owned point maps to the copy's"
        );
        assert_eq!(out[5], axis);
        // Copies don't share point entities.
        let (_, ce) = s.endpoints(out[1]).unwrap();
        let (ds, _) = s.endpoints(out[2]).unwrap();
        assert_ne!(ce, ds);
        // 2 (line a) + 1 (far end of b) + 1 (free point) symmetric; one coincident joint.
        assert_eq!(count(&s, "Symmetric"), 4);
        assert_eq!(count(&s, "Coincident"), 1);
        assert_satisfied(&s);
    }

    #[test]
    fn bad_axis() {
        let mut s = Sketch::new();
        let c = s.add_circle(DVec2::ZERO, 1.0);
        let l = s.add_line(DVec2::ONE, DVec2::ONE);
        assert!(matches!(
            mirror(&mut s, &[l], c),
            Err(OpError::Unsupported(_))
        ));
        assert!(matches!(
            mirror(&mut s, &[c], l),
            Err(OpError::Unsupported(_))
        ));
        assert_eq!(
            mirror(&mut s, &[c], EntityId(99)),
            Err(OpError::Missing(EntityId(99)))
        );
    }
}
