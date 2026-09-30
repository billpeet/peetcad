//! Automatic constraint inference while drawing.
//!
//! As the cursor moves, the drawing tools ask where the next point should snap to and
//! which relations that implies (coincident with an existing point, horizontal from the
//! previous point, tangent to the curve it starts from, ...). The UI shows the inferred
//! relations as glyphs next to the cursor, and adds them as real constraints when the
//! point is placed.
//!
//! # Priorities
//!
//! 1. **Point snaps** fix the position completely, and the first class with a candidate
//!    within `snap_distance` wins (nearest first, then lowest id):
//!    existing points ([`Inferred::Coincident`], including the origin), then line
//!    midpoints ([`Inferred::Midpoint`]), then circle/arc centres ([`Inferred::Center`]).
//!    A point that is only used as a centre counts as a centre, not as a point.
//! 2. **Curves and directions**, combined where both apply. The direction is the one
//!    segment direction from the anchor within `angle_tolerance_deg`: horizontal,
//!    vertical, tangent to the anchor's arc, or perpendicular/parallel to the anchor's
//!    line (the closest wins; exact ties prefer tangent, then horizontal/vertical).
//!    In order:
//!    - on a curve *and* along the direction (their intersection, if within snap distance),
//!    - on a curve *and* on an alignment guide,
//!    - on a curve ([`Inferred::OnCurve`], the closest point on the bounded curve),
//!    - along the direction *and* on an alignment guide (a corner),
//!    - along the direction,
//!    - on two alignment guides, then one.
//!
//! Alignment guides ([`Inferred::AlignedHorizontally`], [`Inferred::AlignedVertically`])
//! snap a coordinate to an existing point's but are not turned into constraints.
//!
//! The anchor's own point is never snapped to (that would make a zero-length segment),
//! and the anchor's own curve is ignored for midpoint and on-curve snaps (the new segment
//! would fold back onto it).

use peet_math::{DVec2, tolerance};

use crate::curve::Curve;
use crate::sketch::{ConstraintId, ConstraintKind, EntityId, EntityKind, Geometry, Sketch};

/// Tuning for inference. Distances are in sketch units (mm); the UI derives them from
/// pixel tolerances at the current zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InferenceSettings {
    /// How close the cursor must be to snap to a point or curve.
    pub snap_distance: f64,
    /// Angle within which a new segment snaps to horizontal/vertical/tangent/perpendicular, in degrees.
    pub angle_tolerance_deg: f64,
    /// Master switch (the UI turns inference off while a modifier key is held).
    pub enabled: bool,
}

impl Default for InferenceSettings {
    fn default() -> Self {
        Self {
            snap_distance: 1.0,
            angle_tolerance_deg: 3.0,
            enabled: true,
        }
    }
}

/// The point a new segment starts from (the previous click), if any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub pos: DVec2,
    /// The existing point entity at the anchor (the previous segment's end), if any.
    pub point: Option<EntityId>,
    /// The curve that ends at the anchor (to infer tangent/perpendicular continuation).
    pub curve: Option<EntityId>,
}

/// A relation inferred for the point being placed (and the segment from the anchor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Inferred {
    /// The point lands on an existing point.
    Coincident(EntityId),
    /// The point lands on an existing curve.
    OnCurve(EntityId),
    /// The point lands on a line's midpoint.
    Midpoint(EntityId),
    /// The point lands on a circle/arc's centre.
    Center(EntityId),
    /// The segment from the anchor is horizontal.
    Horizontal,
    /// The segment from the anchor is vertical.
    Vertical,
    /// The point is horizontally aligned with an existing point (same Y); shown as a
    /// dotted guide, not turned into a constraint.
    AlignedHorizontally(EntityId),
    /// The point is vertically aligned with an existing point (same X); a guide only.
    AlignedVertically(EntityId),
    /// The segment from the anchor continues tangent to the anchor's curve.
    Tangent(EntityId),
    /// The segment from the anchor is perpendicular to the anchor's curve (a line).
    Perpendicular(EntityId),
    /// The segment from the anchor is parallel to the anchor's curve (a line).
    Parallel(EntityId),
}

impl Inferred {
    /// Guides only help placing the point; they don't become constraints.
    pub fn is_guide(&self) -> bool {
        matches!(
            self,
            Inferred::AlignedHorizontally(_) | Inferred::AlignedVertically(_)
        )
    }
}

/// Result of inferring a point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inference {
    /// The snapped position to use instead of the raw cursor position.
    pub pos: DVec2,
    /// Relations that apply at `pos`, most important first.
    pub relations: Vec<Inferred>,
}

/// Snaps `cursor` and infers relations. `exclude` lists entities to ignore (the geometry
/// being drawn, whose points would otherwise snap to themselves). Points owned by
/// excluded curves are ignored too.
pub fn infer(
    sketch: &Sketch,
    cursor: DVec2,
    anchor: Option<Anchor>,
    settings: &InferenceSettings,
    exclude: &[EntityId],
) -> Inference {
    let mut out = Inference {
        pos: cursor,
        relations: Vec::new(),
    };
    if !settings.enabled || !cursor.is_finite() {
        return out;
    }
    let ctx = Ctx {
        sketch,
        exclude,
        anchor,
        snap: settings.snap_distance.max(0.0),
    };

    if let Some((pos, rel)) = ctx.point_snap(cursor) {
        out.pos = pos;
        out.relations.push(rel);
        return out;
    }

    let dir = anchor.and_then(|a| ctx.direction(a, cursor, settings.angle_tolerance_deg));
    let (guide_v, guide_h) = ctx.guides(cursor);
    let mut set = |pos: DVec2, rels: &[Inferred]| {
        out.pos = pos;
        out.relations.extend_from_slice(rels);
    };

    // On a curve and along the direction.
    if let Some(d) = &dir
        && let Some((c, p)) = ctx.curve_line_snap(cursor, d.origin, d.dir, d.forward_only)
    {
        set(p, &[Inferred::OnCurve(c), d.rel]);
        return out;
    }
    // On a curve and on a guide.
    let curve_guide = [
        guide_v.map(|g| (g, DVec2::Y, Inferred::AlignedVertically(g.id))),
        guide_h.map(|g| (g, DVec2::X, Inferred::AlignedHorizontally(g.id))),
    ]
    .into_iter()
    .flatten()
    .filter_map(|(g, axis, rel)| {
        let (c, p) = ctx.curve_line_snap(cursor, g.pos, axis, false)?;
        Some((p.distance(cursor), c, p, rel))
    })
    .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, c, p, rel)) = curve_guide {
        set(p, &[Inferred::OnCurve(c), rel]);
        return out;
    }
    // On a curve.
    if let Some((c, p)) = ctx.curve_snap(cursor) {
        set(p, &[Inferred::OnCurve(c)]);
        return out;
    }
    // Along the direction, possibly at a guide's crossing.
    if let Some(d) = &dir {
        let on_dir = d.origin + d.dir * (cursor - d.origin).dot(d.dir);
        let crossing = [
            guide_v.map(|g| (g, DVec2::X, Inferred::AlignedVertically(g.id))),
            guide_h.map(|g| (g, DVec2::Y, Inferred::AlignedHorizontally(g.id))),
        ]
        .into_iter()
        .flatten()
        .filter_map(|(g, normal, rel)| {
            // Solve (origin + t dir) · normal = g.pos · normal.
            let denom = d.dir.dot(normal);
            if denom.abs() < 1e-3 {
                return None; // (nearly) parallel to the guide
            }
            let t = (g.pos - d.origin).dot(normal) / denom;
            if d.forward_only && t <= 0.0 {
                return None;
            }
            let p = d.origin + d.dir * t;
            let dist = p.distance(on_dir);
            (dist <= ctx.snap && p.distance(d.origin) > tolerance::LINEAR).then_some((dist, p, rel))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
        match crossing {
            Some((_, p, rel)) => set(p, &[d.rel, rel]),
            None => set(on_dir, &[d.rel]),
        }
        return out;
    }
    // Guides alone.
    match (guide_v, guide_h) {
        (Some(v), Some(h)) => {
            let rels = if h.dist < v.dist {
                [
                    Inferred::AlignedHorizontally(h.id),
                    Inferred::AlignedVertically(v.id),
                ]
            } else {
                [
                    Inferred::AlignedVertically(v.id),
                    Inferred::AlignedHorizontally(h.id),
                ]
            };
            set(DVec2::new(v.pos.x, h.pos.y), &rels);
        }
        (Some(v), None) => set(
            DVec2::new(v.pos.x, cursor.y),
            &[Inferred::AlignedVertically(v.id)],
        ),
        (None, Some(h)) => set(
            DVec2::new(cursor.x, h.pos.y),
            &[Inferred::AlignedHorizontally(h.id)],
        ),
        (None, None) => {}
    }
    out
}

/// A direction inferred for the segment from the anchor.
#[derive(Clone, Copy, Debug)]
struct Direction {
    rel: Inferred,
    origin: DVec2,
    /// Unit direction.
    dir: DVec2,
    /// Whether only the half-line along `dir` counts (tangent/collinear continuation).
    forward_only: bool,
}

/// The existing point an alignment guide runs through.
#[derive(Clone, Copy, Debug)]
struct Guide {
    id: EntityId,
    pos: DVec2,
    /// Distance from the cursor to the guide line.
    dist: f64,
}

struct Ctx<'a> {
    sketch: &'a Sketch,
    exclude: &'a [EntityId],
    anchor: Option<Anchor>,
    snap: f64,
}

impl Ctx<'_> {
    /// Points that can be snapped to: live, not excluded, not owned by excluded curves,
    /// and not the anchor's own point.
    fn points(&self) -> impl Iterator<Item = (EntityId, DVec2)> + '_ {
        self.sketch.entities().filter_map(move |(id, e)| {
            let Geometry::Point { pos } = e.geometry else {
                return None;
            };
            let skip = self.exclude.contains(&id)
                || e.owner.is_some_and(|o| self.exclude.contains(&o))
                || self.anchor.is_some_and(|a| a.point == Some(id));
            (!skip).then_some((id, pos))
        })
    }

    /// Curves (lines, circles, arcs) that can be snapped to.
    fn curves(&self, skip_anchor_curve: bool) -> impl Iterator<Item = (EntityId, Curve)> + '_ {
        let anchor_curve = self.anchor.and_then(|a| a.curve);
        self.sketch.entities().filter_map(move |(id, e)| {
            if e.kind() == EntityKind::Point
                || self.exclude.contains(&id)
                || (skip_anchor_curve && anchor_curve == Some(id))
            {
                return None;
            }
            Some((id, self.sketch.curve(id)?))
        })
    }

    /// How snapping onto point `p` should be reported: as the point itself, or as the
    /// centre of the (lowest id) circle/arc using it if it is only ever used as a centre.
    fn classify_point(&self, p: EntityId) -> Inferred {
        if p == Sketch::ORIGIN || self.sketch.entity(p).is_some_and(|e| e.locked) {
            return Inferred::Coincident(p);
        }
        let mut centre_of = None;
        for (id, e) in self.sketch.entities() {
            if self.exclude.contains(&id) {
                continue;
            }
            match e.geometry {
                Geometry::Line { start, end } | Geometry::Arc { start, end, .. }
                    if start == p || end == p =>
                {
                    return Inferred::Coincident(p);
                }
                Geometry::Circle { center, .. } | Geometry::Arc { center, .. }
                    if center == p && centre_of.is_none() =>
                {
                    centre_of = Some(id);
                }
                _ => {}
            }
        }
        match centre_of {
            Some(c) => Inferred::Center(c),
            None => Inferred::Coincident(p),
        }
    }

    /// Points, midpoints and centres within snap distance, by priority class.
    fn point_snap(&self, cursor: DVec2) -> Option<(DVec2, Inferred)> {
        // (class, distance, pos, relation); iteration is in id order, so keeping only
        // strictly better candidates breaks ties by lowest id.
        let mut best: Option<(u8, f64, DVec2, Inferred)> = None;
        let mut offer = |class: u8, pos: DVec2, rel: Inferred| {
            let dist = pos.distance(cursor);
            if dist > self.snap {
                return;
            }
            let better = match best {
                None => true,
                Some((bc, bd, ..)) => class < bc || (class == bc && dist < bd),
            };
            if better {
                best = Some((class, dist, pos, rel));
            }
        };
        for (id, pos) in self.points() {
            if pos.distance(cursor) <= self.snap {
                let rel = self.classify_point(id);
                let class = if matches!(rel, Inferred::Center(_)) {
                    2
                } else {
                    0
                };
                offer(class, pos, rel);
            }
        }
        for (id, curve) in self.curves(true) {
            if let Curve::Line { a, b } = curve
                && a.distance(b) > tolerance::LINEAR
            {
                offer(1, (a + b) * 0.5, Inferred::Midpoint(id));
            }
        }
        best.map(|(_, _, pos, rel)| (pos, rel))
    }

    /// The closest curve within snap distance, and the closest point on it.
    fn curve_snap(&self, cursor: DVec2) -> Option<(EntityId, DVec2)> {
        let mut best: Option<(f64, EntityId, DVec2)> = None;
        for (id, curve) in self.curves(true) {
            let (_, p) = curve.closest_point(cursor);
            let dist = p.distance(cursor);
            if dist <= self.snap && best.is_none_or(|(bd, ..)| dist < bd) {
                best = Some((dist, id, p));
            }
        }
        best.map(|(_, id, p)| (id, p))
    }

    /// The crossing of the line through `origin` along `dir` with a curve that is closest
    /// to the cursor, if within snap distance.
    fn curve_line_snap(
        &self,
        cursor: DVec2,
        origin: DVec2,
        dir: DVec2,
        forward_only: bool,
    ) -> Option<(EntityId, DVec2)> {
        let line = Curve::Line {
            a: origin,
            b: origin + dir,
        };
        let mut best: Option<(f64, EntityId, DVec2)> = None;
        for (id, curve) in self.curves(true) {
            // Any crossing within snap distance of the cursor means the curve is too.
            if curve.distance(cursor) > self.snap {
                continue;
            }
            for i in line.intersect_unbounded(&curve) {
                if !curve.contains_param(i.t_b)
                    || (forward_only && i.t_a <= 0.0)
                    || i.point.distance(origin) <= tolerance::LINEAR
                {
                    continue;
                }
                let dist = i.point.distance(cursor);
                if dist <= self.snap && best.is_none_or(|(bd, ..)| dist < bd) {
                    best = Some((dist, id, i.point));
                }
            }
        }
        best.map(|(_, id, p)| (id, p))
    }

    /// The nearest points whose X (vertical guide) or Y (horizontal guide) is within snap
    /// distance of the cursor's.
    fn guides(&self, cursor: DVec2) -> (Option<Guide>, Option<Guide>) {
        let mut v: Option<Guide> = None;
        let mut h: Option<Guide> = None;
        for (id, pos) in self.points() {
            let dx = (pos.x - cursor.x).abs();
            if dx <= self.snap && v.is_none_or(|g| dx < g.dist) {
                v = Some(Guide { id, pos, dist: dx });
            }
            let dy = (pos.y - cursor.y).abs();
            if dy <= self.snap && h.is_none_or(|g| dy < g.dist) {
                h = Some(Guide { id, pos, dist: dy });
            }
        }
        (v, h)
    }

    /// Outward direction of the anchor's curve at the anchor, and whether it's a line.
    fn continuation(&self, anchor: Anchor) -> Option<(EntityId, DVec2, bool)> {
        let c = anchor.curve?;
        let (start, end) = self.sketch.endpoints(c)?;
        let curve = self.sketch.curve(c)?;
        let at_end = match anchor.point {
            Some(p) if p == end => true,
            Some(p) if p == start => false,
            _ => {
                let ds = self.sketch.try_point(start)?.distance(anchor.pos);
                let de = self.sketch.try_point(end)?.distance(anchor.pos);
                if de.min(ds) > tolerance::LINEAR.max(self.snap * 1e-3) {
                    return None;
                }
                de <= ds
            }
        };
        match curve {
            Curve::Line { a, b } => {
                let d = if at_end { b - a } else { a - b };
                Some((c, d.try_normalize()?, true))
            }
            Curve::Arc { .. } => {
                let d = if at_end {
                    curve.tangent_at(1.0)
                } else {
                    -curve.tangent_at(0.0)
                };
                Some((c, d, false))
            }
            Curve::Circle { .. } => None,
        }
    }

    /// The direction from the anchor that the cursor is closest to, within tolerance.
    fn direction(&self, anchor: Anchor, cursor: DVec2, tolerance_deg: f64) -> Option<Direction> {
        let v = cursor - anchor.pos;
        let u = v.try_normalize()?;
        if v.length() <= tolerance::LINEAR {
            return None;
        }
        let tol = tolerance_deg.max(0.0).to_radians();
        // (deviation, rank, direction); lower rank wins exact ties.
        let mut best: Option<(f64, u8, Direction)> = None;
        let mut offer = |dir: DVec2, forward_only: bool, rank: u8, rel: Inferred| {
            let along = u.dot(dir);
            let dev = if forward_only {
                u.perp_dot(dir).abs().atan2(along)
            } else {
                u.perp_dot(dir).abs().atan2(along.abs())
            };
            if dev > tol {
                return;
            }
            let better = match best {
                None => true,
                Some((bd, br, _)) => dev < bd - 1e-12 || (dev <= bd + 1e-12 && rank < br),
            };
            if better {
                // Orient bidirectional directions towards the cursor.
                let dir = if along < 0.0 { -dir } else { dir };
                best = Some((
                    dev,
                    rank,
                    Direction {
                        rel,
                        origin: anchor.pos,
                        dir,
                        forward_only,
                    },
                ));
            }
        };
        if let Some((c, out, is_line)) = self.continuation(anchor) {
            if is_line {
                offer(out, true, 2, Inferred::Parallel(c));
                offer(out.perp(), false, 2, Inferred::Perpendicular(c));
            } else {
                offer(out, true, 0, Inferred::Tangent(c));
            }
        }
        offer(DVec2::X, false, 1, Inferred::Horizontal);
        offer(DVec2::Y, false, 1, Inferred::Vertical);
        best.map(|(_, _, d)| d)
    }
}

/// Whether two constraint kinds state the same relation (symmetric relations compare
/// their entities in either order).
fn same_relation(a: &ConstraintKind, b: &ConstraintKind) -> bool {
    use ConstraintKind::*;
    match (a, b) {
        (Coincident(a1, a2), Coincident(b1, b2))
        | (Parallel(a1, a2), Parallel(b1, b2))
        | (Perpendicular(a1, a2), Perpendicular(b1, b2))
        | (Tangent(a1, a2), Tangent(b1, b2)) => (a1 == b1 && a2 == b2) || (a1 == b2 && a2 == b1),
        _ => a == b,
    }
}

/// Turns inferred relations into constraints on newly drawn geometry: `point` is the new
/// point placed at the inferred position, and `segment` the new line/arc running from the
/// anchor to it (for horizontal/vertical/tangent/...). Guides are skipped, and so is
/// anything that doesn't fit the entities (for example horizontal on an arc segment) or
/// already exists. Returns the constraints added.
pub fn apply(
    sketch: &mut Sketch,
    relations: &[Inferred],
    point: EntityId,
    segment: Option<EntityId>,
) -> Vec<ConstraintId> {
    let uses_point = |sketch: &Sketch, curve: EntityId| {
        sketch
            .entity(curve)
            .is_some_and(|e| e.geometry.points().contains(&point))
    };
    let mut added = Vec::new();
    for rel in relations {
        let kind = match *rel {
            Inferred::Coincident(p) => (p != point).then_some(ConstraintKind::Coincident(point, p)),
            Inferred::OnCurve(c) => {
                (!uses_point(sketch, c)).then_some(ConstraintKind::PointOnCurve { point, curve: c })
            }
            Inferred::Midpoint(l) => {
                (!uses_point(sketch, l)).then_some(ConstraintKind::Midpoint { point, line: l })
            }
            Inferred::Center(c) => sketch
                .center(c)
                .filter(|centre| *centre != point)
                .map(|centre| ConstraintKind::Coincident(point, centre)),
            Inferred::Horizontal => segment.map(ConstraintKind::Horizontal),
            Inferred::Vertical => segment.map(ConstraintKind::Vertical),
            Inferred::Tangent(c) => segment
                .filter(|s| *s != c)
                .map(|s| ConstraintKind::Tangent(s, c)),
            Inferred::Perpendicular(l) => segment
                .filter(|s| *s != l)
                .map(|s| ConstraintKind::Perpendicular(s, l)),
            Inferred::Parallel(l) => segment
                .filter(|s| *s != l)
                .map(|s| ConstraintKind::Parallel(s, l)),
            Inferred::AlignedHorizontally(_) | Inferred::AlignedVertically(_) => None,
        };
        let Some(kind) = kind else { continue };
        if sketch.validate(&kind).is_err()
            || sketch
                .constraints()
                .any(|(_, c)| same_relation(&c.kind, &kind))
        {
            continue;
        }
        if let Ok(id) = sketch.add_constraint(kind) {
            added.push(id);
        }
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> DVec2 {
        DVec2::new(x, y)
    }

    fn settings() -> InferenceSettings {
        InferenceSettings::default()
    }

    fn near(a: DVec2, b: DVec2) -> bool {
        a.distance(b) < 1e-9
    }

    #[test]
    fn disabled_returns_cursor() {
        let mut s = Sketch::new();
        s.add_line(v(0.0, 0.0), v(10.0, 0.0));
        let st = InferenceSettings {
            enabled: false,
            ..settings()
        };
        let r = infer(&s, v(0.2, 0.1), None, &st, &[]);
        assert_eq!(r.pos, v(0.2, 0.1));
        assert!(r.relations.is_empty());
    }

    #[test]
    fn nothing_nearby() {
        let mut s = Sketch::new();
        s.add_line(v(0.0, 0.0), v(10.0, 0.0));
        let r = infer(&s, v(5.0, 20.0), None, &settings(), &[]);
        assert_eq!(r.pos, v(5.0, 20.0));
        assert!(r.relations.is_empty());
    }

    #[test]
    fn snaps_to_origin_and_endpoints() {
        let mut s = Sketch::new();
        let r = infer(&s, v(0.3, -0.2), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Coincident(Sketch::ORIGIN)]);
        assert_eq!(r.pos, DVec2::ZERO);

        let l = s.add_line(v(10.0, 0.0), v(20.0, 5.0));
        let (a, b) = s.endpoints(l).unwrap();
        let r = infer(&s, v(19.5, 5.5), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Coincident(b)]);
        assert_eq!(r.pos, v(20.0, 5.0));
        let _ = a;
    }

    #[test]
    fn endpoint_beats_midpoint_even_when_farther() {
        let mut s = Sketch::new();
        // Midpoint at (1, 0); endpoints at (0, 0) and (2, 0).
        let l = s.add_line(v(0.0, 5.0), v(2.0, 5.0));
        let (a, _) = s.endpoints(l).unwrap();
        let r = infer(&s, v(0.8, 5.0), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Coincident(a)]);
        // Only the midpoint in range.
        let l2 = s.add_line(v(0.0, 20.0), v(10.0, 20.0));
        let r = infer(&s, v(5.3, 20.4), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Midpoint(l2)]);
        assert_eq!(r.pos, v(5.0, 20.0));
    }

    #[test]
    fn nearest_point_then_lowest_id() {
        let mut s = Sketch::new();
        let p1 = s.add_point(v(10.0, 10.0));
        let p2 = s.add_point(v(10.0, 10.0));
        let p3 = s.add_point(v(10.5, 10.0));
        let r = infer(&s, v(10.1, 10.0), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Coincident(p1)]);
        let r = infer(&s, v(10.4, 10.0), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Coincident(p3)]);
        let _ = p2;
    }

    #[test]
    fn centre_and_on_curve() {
        let mut s = Sketch::new();
        let c = s.add_circle(v(20.0, 0.0), 5.0);
        let r = infer(&s, v(20.3, 0.3), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Center(c)]);
        assert_eq!(r.pos, v(20.0, 0.0));
        // Above the centre: the quadrant point, on the circle and aligned with the centre.
        let centre = s.center(c).unwrap();
        let r = infer(&s, v(20.3, 5.4), None, &settings(), &[]);
        assert_eq!(
            r.relations,
            vec![Inferred::OnCurve(c), Inferred::AlignedVertically(centre)]
        );
        assert!(near(r.pos, v(20.0, 5.0)));
        // Elsewhere: just on the circle, at the closest point.
        let dir = DVec2::from_angle(40f64.to_radians());
        let r = infer(&s, v(20.0, 0.0) + dir * 5.4, None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::OnCurve(c)]);
        assert!(near(r.pos, v(20.0, 0.0) + dir * 5.0));
    }

    #[test]
    fn on_bounded_curve_only() {
        let mut s = Sketch::new();
        let l = s.add_line(v(0.0, 10.0), v(10.0, 10.0));
        let r = infer(&s, v(3.0, 10.5), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::OnCurve(l)]);
        assert!(near(r.pos, v(3.0, 10.0)));
        // Beyond the end of the segment (but on its infinite line): nothing.
        let r = infer(&s, v(14.0, 10.5), None, &settings(), &[]);
        assert!(!r.relations.contains(&Inferred::OnCurve(l)));
    }

    #[test]
    fn horizontal_within_tolerance_only() {
        let s = Sketch::new();
        let anchor = Some(Anchor {
            pos: v(0.0, 0.0),
            point: None,
            curve: None,
        });
        // 2° off horizontal: snaps.
        let a = 2f64.to_radians();
        let cursor = v(a.cos() * 50.0, a.sin() * 50.0);
        let r = infer(&s, cursor, anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Horizontal]);
        assert!(near(r.pos, v(cursor.x, 0.0)));
        // Leftwards works too.
        let r = infer(&s, v(-cursor.x, cursor.y), anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Horizontal]);
        // 5° off: no inference.
        let a = 5f64.to_radians();
        let cursor = v(a.cos() * 50.0, a.sin() * 50.0);
        let r = infer(&s, cursor, anchor, &settings(), &[]);
        assert!(r.relations.is_empty());
        assert_eq!(r.pos, cursor);
        // Vertical.
        let r = infer(&s, v(1.0, -40.0), anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Vertical]);
        assert!(near(r.pos, v(0.0, -40.0)));
    }

    #[test]
    fn tangent_continuation_from_arc_end() {
        let mut s = Sketch::new();
        // Quarter arc CCW from (10, 0) to (0, 10) around the origin-ish centre (0, 0),
        // offset upwards so nothing else interferes. Tangent at the end points -X.
        let arc = s.add_arc(v(0.0, 50.0), v(10.0, 50.0), v(0.0, 60.0));
        let (_, end) = s.endpoints(arc).unwrap();
        let anchor = Some(Anchor {
            pos: v(0.0, 60.0),
            point: Some(end),
            curve: Some(arc),
        });
        // Tangent there is exactly horizontal: tangent wins the tie with horizontal.
        let r = infer(&s, v(-30.0, 61.0), anchor, &settings(), &[arc]);
        assert_eq!(r.relations, vec![Inferred::Tangent(arc)]);
        assert!(near(r.pos, v(-30.0, 60.0)));
        // Backwards (into the arc) is not tangent continuation, but still horizontal.
        let r = infer(&s, v(30.0, 60.5), anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Horizontal]);
    }

    #[test]
    fn tangent_continuation_diagonal() {
        let mut s = Sketch::new();
        // Arc from angle 0 to 45° around (100, 100), radius 10. End tangent at 135°.
        let c = v(100.0, 100.0);
        let e = c + DVec2::from_angle(45f64.to_radians()) * 10.0;
        let arc = s.add_arc(c, c + v(10.0, 0.0), e);
        let t = DVec2::from_angle(135f64.to_radians());
        let off = DVec2::from_angle(136f64.to_radians());
        let anchor = Some(Anchor {
            pos: e,
            point: None, // found by position
            curve: Some(arc),
        });
        let r = infer(&s, e + off * 20.0, anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Tangent(arc)]);
        let expected = e + t * (off * 20.0).dot(t);
        assert!(near(r.pos, expected));
    }

    #[test]
    fn perpendicular_and_parallel_to_anchor_line() {
        let mut s = Sketch::new();
        let d = DVec2::from_angle(30f64.to_radians());
        let l = s.add_line(v(50.0, 50.0), v(50.0, 50.0) + d * 10.0);
        let (_, end) = s.endpoints(l).unwrap();
        let anchor = Some(Anchor {
            pos: s.point(end),
            point: Some(end),
            curve: Some(l),
        });
        let p = s.point(end);
        let r = infer(&s, p + d.perp() * 15.0 + d * 0.3, anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Perpendicular(l)]);
        assert!(near(r.pos, p + d.perp() * 15.0));
        let r = infer(&s, p + d * 15.0 + d.perp() * 0.3, anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Parallel(l)]);
        assert!(near(r.pos, p + d * 15.0));
    }

    #[test]
    fn horizontal_line_prefers_horizontal_over_parallel() {
        let mut s = Sketch::new();
        let l = s.add_line(v(20.0, 20.0), v(30.0, 20.0));
        let (_, end) = s.endpoints(l).unwrap();
        let anchor = Some(Anchor {
            pos: v(30.0, 20.0),
            point: Some(end),
            curve: Some(l),
        });
        let r = infer(&s, v(45.0, 20.2), anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Horizontal]);
        let r = infer(&s, v(30.2, 40.0), anchor, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::Vertical]);
    }

    #[test]
    fn alignment_guides() {
        let mut s = Sketch::new();
        let p = s.add_point(v(10.0, 30.0));
        let q = s.add_point(v(40.0, 5.0));
        let r = infer(&s, v(10.4, 17.0), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::AlignedVertically(p)]);
        assert_eq!(r.pos, v(10.0, 17.0));
        let r = infer(&s, v(25.0, 5.3), None, &settings(), &[]);
        assert_eq!(r.relations, vec![Inferred::AlignedHorizontally(q)]);
        assert_eq!(r.pos, v(25.0, 5.0));
        // Both: the corner, nearest guide first.
        let r = infer(&s, v(10.2, 5.6), None, &settings(), &[]);
        assert_eq!(
            r.relations,
            vec![
                Inferred::AlignedVertically(p),
                Inferred::AlignedHorizontally(q)
            ]
        );
        assert_eq!(r.pos, v(10.0, 5.0));
    }

    #[test]
    fn horizontal_plus_vertical_guide_gives_corner() {
        let mut s = Sketch::new();
        let p = s.add_point(v(30.0, 40.0));
        let anchor = Some(Anchor {
            pos: v(0.0, 10.0),
            point: None,
            curve: None,
        });
        let r = infer(&s, v(30.5, 11.0), anchor, &settings(), &[]);
        assert_eq!(
            r.relations,
            vec![Inferred::Horizontal, Inferred::AlignedVertically(p)]
        );
        assert!(near(r.pos, v(30.0, 10.0)));
    }

    #[test]
    fn curve_and_direction_intersection() {
        let mut s = Sketch::new();
        let c = s.add_circle(v(50.0, 0.0), 10.0);
        let anchor = Some(Anchor {
            pos: v(0.0, 5.0),
            point: None,
            curve: None,
        });
        // Near the circle at its crossing with y = 5, slightly off horizontal.
        let x = 50.0 - (100.0f64 - 25.0).sqrt();
        let r = infer(&s, v(x + 0.3, 5.6), anchor, &settings(), &[]);
        assert_eq!(
            r.relations,
            vec![Inferred::OnCurve(c), Inferred::Horizontal]
        );
        assert!(near(r.pos, v(x, 5.0)));
    }

    #[test]
    fn exclude_list() {
        let mut s = Sketch::new();
        let l = s.add_line(v(10.0, 10.0), v(20.0, 10.0));
        let (_, b) = s.endpoints(l).unwrap();
        let r = infer(&s, v(20.2, 10.1), None, &settings(), &[l]);
        assert!(!r.relations.contains(&Inferred::Coincident(b)));
        assert!(
            r.relations
                .iter()
                .all(|r| !matches!(r, Inferred::OnCurve(_)))
        );
        let r = infer(&s, v(20.2, 10.1), None, &settings(), &[b]);
        assert_eq!(r.relations, vec![Inferred::OnCurve(l)]);
    }

    #[test]
    fn anchor_point_is_not_a_snap_target() {
        let mut s = Sketch::new();
        let l = s.add_line(v(10.0, 10.0), v(20.0, 10.0));
        let (_, b) = s.endpoints(l).unwrap();
        let anchor = Some(Anchor {
            pos: v(20.0, 10.0),
            point: Some(b),
            curve: Some(l),
        });
        let r = infer(&s, v(20.3, 10.1), anchor, &settings(), &[]);
        assert!(!r.relations.contains(&Inferred::Coincident(b)));
    }

    #[test]
    fn apply_adds_constraints_without_duplicates() {
        let mut s = Sketch::new();
        let target = s.add_line(v(0.0, 50.0), v(10.0, 50.0));
        let (ta, _) = s.endpoints(target).unwrap();
        let seg = s.add_line(v(0.0, 0.0), v(0.0, 50.0));
        let (_, p) = s.endpoints(seg).unwrap();
        let rels = [
            Inferred::Coincident(ta),
            Inferred::Vertical,
            Inferred::AlignedHorizontally(ta),
        ];
        let added = apply(&mut s, &rels, p, Some(seg));
        assert_eq!(added.len(), 2);
        assert_eq!(
            s.constraint(added[0]).unwrap().kind,
            ConstraintKind::Coincident(p, ta)
        );
        assert_eq!(
            s.constraint(added[1]).unwrap().kind,
            ConstraintKind::Vertical(seg)
        );
        // Again: nothing new.
        assert!(apply(&mut s, &rels, p, Some(seg)).is_empty());
        // The reversed coincident counts as a duplicate too.
        assert!(apply(&mut s, &[Inferred::Coincident(p)], ta, None).is_empty());
    }

    #[test]
    fn apply_maps_each_relation() {
        let mut s = Sketch::new();
        let arc = s.add_arc(v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0));
        let circle = s.add_circle(v(40.0, 0.0), 3.0);
        let other = s.add_line(v(0.0, -20.0), v(10.0, -20.0));
        let seg = s.add_line(v(0.0, 10.0), v(-20.0, 10.0));
        let (_, p) = s.endpoints(seg).unwrap();
        let centre = s.center(circle).unwrap();
        let added = apply(
            &mut s,
            &[
                Inferred::OnCurve(other),
                Inferred::Midpoint(other),
                Inferred::Center(circle),
                Inferred::Tangent(arc),
                Inferred::Parallel(other),
                Inferred::Perpendicular(other),
                Inferred::Horizontal,
                Inferred::AlignedVertically(centre),
            ],
            p,
            Some(seg),
        );
        let kinds: Vec<ConstraintKind> = added
            .iter()
            .map(|c| s.constraint(*c).unwrap().kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ConstraintKind::PointOnCurve {
                    point: p,
                    curve: other
                },
                ConstraintKind::Midpoint {
                    point: p,
                    line: other
                },
                ConstraintKind::Coincident(p, centre),
                ConstraintKind::Tangent(seg, arc),
                ConstraintKind::Parallel(seg, other),
                ConstraintKind::Perpendicular(seg, other),
                ConstraintKind::Horizontal(seg),
            ]
        );
    }

    #[test]
    fn apply_skips_what_does_not_fit() {
        let mut s = Sketch::new();
        let arc_seg = s.add_arc(v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0));
        let (_, p) = s.endpoints(arc_seg).unwrap();
        let line = s.add_line(v(0.0, -20.0), v(10.0, -20.0));
        let added = apply(
            &mut s,
            &[
                Inferred::Horizontal,
                Inferred::Vertical,
                Inferred::Perpendicular(line),
                Inferred::Coincident(p),
                Inferred::OnCurve(arc_seg),
                Inferred::Tangent(arc_seg),
            ],
            p,
            Some(arc_seg),
        );
        assert!(added.is_empty());
        // No segment: segment relations are skipped.
        assert!(apply(&mut s, &[Inferred::Horizontal], p, None).is_empty());
    }

    #[test]
    fn apply_center_on_arc() {
        let mut s = Sketch::new();
        let arc = s.add_arc(v(5.0, 5.0), v(10.0, 5.0), v(5.0, 10.0));
        let p = s.add_point(v(5.0, 5.0));
        let added = apply(&mut s, &[Inferred::Center(arc)], p, None);
        assert_eq!(
            s.constraint(added[0]).unwrap().kind,
            ConstraintKind::Coincident(p, s.center(arc).unwrap())
        );
    }
}
