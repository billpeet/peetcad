//! The topology-dependent part of the solver: variables, aliasing and clusters.
//!
//! Everything here depends only on *which* entities and constraints exist (not on
//! coordinates or dimension values), so it is cached between solves and rebuilt when the
//! sketch's topology fingerprint changes.
//!
//! **Aliasing.** Coincident points (and the centres of concentric circles) are merged with
//! a union-find into one *class* sharing a single pair of variables, which removes two
//! variables and two equations per coincidence. The merges form a spanning forest; a
//! coincidence that closes a cycle is redundant by construction, and the forest paths let
//! the analysis name the coincidences involved.
//!
//! **Clusters.** Variables that share an equation are merged with a second union-find;
//! each connected component is solved independently.

use std::collections::HashMap;

use peet_math::DVec2;

use super::equations::{self, NONE, SlotMap};
use super::problem::Problem;
use crate::sketch::{ConstraintKind, Entity, EntityId, Geometry, Sketch};

/// A set of points merged by coincidence, sharing two slots.
#[derive(Clone, Debug)]
pub(crate) struct Class {
    pub members: Vec<EntityId>,
    /// A locked member makes the class constant.
    pub locked: Option<EntityId>,
    /// x slot (y = slot + 1).
    pub slot: u32,
}

#[derive(Debug)]
pub(crate) struct Cluster {
    /// Variable slots, ascending.
    pub vars: Vec<u32>,
    /// Hard equation indices, ascending.
    pub eqs: Vec<u32>,
    /// Lazily built solver structure for the hard equations.
    pub problem: Option<Problem>,
}

#[derive(Debug)]
pub(crate) struct Structure {
    pub fingerprint: u64,
    pub map: SlotMap,
    pub n_vars: usize,
    pub n_slots: usize,
    pub classes: Vec<Class>,
    /// Class index of each point entity, `NONE` otherwise.
    pub class_of: Vec<u32>,
    /// Circle radius slots: (entity, slot). Arc radii are implicit.
    pub radii: Vec<(EntityId, u32)>,
    /// Coincidence forest: per entity id, (neighbour, constraint id).
    alias_adj: Vec<Vec<(u32, u32)>>,
    /// Coincident/concentric constraints that closed a cycle: (constraint, a, b).
    pub alias_cycles: Vec<(u32, EntityId, EntityId)>,
    pub clusters: Vec<Cluster>,
    /// Cluster index of each variable slot.
    pub var_cluster: Vec<u32>,
    /// Hard equations with no variables at all (only constants).
    pub const_eqs: Vec<u32>,
    pub n_hard: usize,
    /// Drag problems (hard + soft equations), keyed by cluster and soft equation layout.
    pub drag_problems: HashMap<(u32, Vec<u32>), Problem>,
    /// The size of every curve, for the collapse check.
    pub sizes: Vec<Size>,
}

/// How to measure a curve's size from the values.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Size {
    /// Distance between two points (line length, arc radius): their x slots.
    Line(u32, u32),
    /// Circle radius slot.
    Radius(u32),
}

impl Size {
    pub fn eval(self, vals: &[f64]) -> f64 {
        match self {
            Size::Line(a, b) => {
                let (a, b) = (a as usize, b as usize);
                (vals[b] - vals[a]).hypot(vals[b + 1] - vals[a + 1])
            }
            Size::Radius(r) => vals[r as usize].abs(),
        }
    }

    /// The variable slots the size depends on.
    pub fn slots(self) -> ([u32; 4], usize) {
        match self {
            Size::Line(a, b) => ([a, a + 1, b, b + 1], 4),
            Size::Radius(r) => ([r, 0, 0, 0], 1),
        }
    }
}

/// The points a curve is defined by, without allocating.
pub(crate) fn curve_points(g: &Geometry) -> ([EntityId; 3], usize) {
    let z = EntityId(0);
    match *g {
        Geometry::Point { .. } => ([z; 3], 0),
        Geometry::Line { start, end } => ([start, end, z], 2),
        Geometry::Circle { center, .. } => ([center, z, z], 1),
        Geometry::Arc { center, start, end } => ([center, start, end], 3),
    }
}

/// The entities a constraint refers to, plus a tag for the kind, without allocating.
pub(crate) fn constraint_refs(kind: &ConstraintKind) -> ([EntityId; 3], usize, u8) {
    use ConstraintKind::*;
    let z = EntityId(0);
    match *kind {
        Coincident(a, b) => ([a, b, z], 2, 0),
        PointOnCurve { point, curve } => ([point, curve, z], 2, 1),
        Horizontal(a) => ([a, z, z], 1, 2),
        Vertical(a) => ([a, z, z], 1, 3),
        HorizontalPoints(a, b) => ([a, b, z], 2, 4),
        VerticalPoints(a, b) => ([a, b, z], 2, 5),
        Parallel(a, b) => ([a, b, z], 2, 6),
        Perpendicular(a, b) => ([a, b, z], 2, 7),
        Tangent(a, b) => ([a, b, z], 2, 8),
        Equal(a, b) => ([a, b, z], 2, 9),
        Concentric(a, b) => ([a, b, z], 2, 10),
        Midpoint { point, line } => ([point, line, z], 2, 11),
        Symmetric { a, b, axis } => ([a, b, axis], 3, 12),
        Fix { point, .. } => ([point, z, z], 1, 13),
        Distance(a, b) => ([a, b, z], 2, 14),
        Length(a) => ([a, z, z], 1, 15),
        HorizontalDistance(a, b) => ([a, b, z], 2, 16),
        VerticalDistance(a, b) => ([a, b, z], 2, 17),
        Radius(a) => ([a, z, z], 1, 18),
        Diameter(a) => ([a, z, z], 1, 19),
        Angle(a, b) => ([a, b, z], 2, 20),
    }
}

#[inline]
fn mix(h: u64, x: u64) -> u64 {
    (h.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95)
}

/// Hash of everything the cached structure depends on.
pub(crate) fn fingerprint(sketch: &Sketch) -> u64 {
    let mut h = mix(0, sketch.entity_capacity() as u64);
    h = mix(h, sketch.constraint_capacity() as u64);
    for (id, e) in sketch.entities() {
        let tag = match e.geometry {
            Geometry::Point { .. } => 1,
            Geometry::Line { .. } => 2,
            Geometry::Circle { .. } => 3,
            Geometry::Arc { .. } => 4,
        };
        h = mix(h, (u64::from(id.0) << 8) | (tag << 1) | u64::from(e.locked));
        let (pts, n) = curve_points(&e.geometry);
        for p in &pts[..n] {
            h = mix(h, u64::from(p.0));
        }
    }
    for (id, c) in sketch.constraints() {
        let (refs, n, tag) = constraint_refs(&c.kind);
        h = mix(
            h,
            (u64::from(id.0) << 16) | (u64::from(tag) << 1) | u64::from(c.is_driving()),
        );
        for r in &refs[..n] {
            h = mix(h, u64::from(r.0) | (1 << 40));
        }
    }
    h
}

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

impl Structure {
    pub fn build(sketch: &Sketch) -> Self {
        let cap = sketch.entity_capacity();
        let is_point = |id: EntityId| matches!(sketch.entity(id), Some(e) if matches!(e.geometry, Geometry::Point { .. }));

        // Curves whose defining points are all live points.
        let mut curve_ok = vec![false; cap];
        for (id, e) in sketch.entities() {
            let (pts, n) = curve_points(&e.geometry);
            curve_ok[id.0 as usize] = n == 0 || pts[..n].iter().all(|&p| is_point(p));
        }

        // Which constraints turn into anything at all.
        let mut valid = vec![false; sketch.constraint_capacity()];
        for (cid, c) in sketch.constraints() {
            let (refs, n, _) = constraint_refs(&c.kind);
            valid[cid.0 as usize] = c.is_driving()
                && sketch.validate(&c.kind).is_ok()
                && refs[..n].iter().all(|r| curve_ok[r.0 as usize]);
        }

        // ---- Aliasing ----
        let mut parent: Vec<u32> = (0..cap as u32).collect();
        let mut locked_root: Vec<Option<EntityId>> = vec![None; cap];
        for (id, e) in sketch.entities() {
            if e.locked && is_point(id) {
                locked_root[id.0 as usize] = Some(id);
            }
        }
        let mut alias_adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); cap];
        let mut alias_cycles = Vec::new();
        let mut emit = valid.clone();
        for (cid, c) in sketch.constraints() {
            if !valid[cid.0 as usize] {
                continue;
            }
            let (a, b) = match c.kind {
                ConstraintKind::Coincident(a, b) => (a, b),
                ConstraintKind::Concentric(a, b) => match (sketch.center(a), sketch.center(b)) {
                    (Some(a), Some(b)) => (a, b),
                    _ => continue,
                },
                _ => continue,
            };
            let (ra, rb) = (find(&mut parent, a.0), find(&mut parent, b.0));
            if ra == rb {
                alias_cycles.push((cid.0, a, b));
                emit[cid.0 as usize] = false;
                continue;
            }
            let (la, lb) = (locked_root[ra as usize], locked_root[rb as usize]);
            if la.is_some() && lb.is_some() {
                // Two different constants: keep the equations (they are constant).
                continue;
            }
            parent[rb as usize] = ra;
            locked_root[ra as usize] = la.or(lb);
            alias_adj[a.0 as usize].push((b.0, cid.0));
            alias_adj[b.0 as usize].push((a.0, cid.0));
            emit[cid.0 as usize] = false;
        }

        // ---- Classes and slots ----
        let mut class_of = vec![NONE; cap];
        let mut classes: Vec<Class> = Vec::new();
        let mut root_class: HashMap<u32, u32> = HashMap::new();
        for (id, _) in sketch.entities() {
            if !is_point(id) {
                continue;
            }
            let r = find(&mut parent, id.0);
            let ci = *root_class.entry(r).or_insert_with(|| {
                classes.push(Class {
                    members: Vec::new(),
                    locked: locked_root[r as usize],
                    slot: NONE,
                });
                (classes.len() - 1) as u32
            });
            classes[ci as usize].members.push(id);
            class_of[id.0 as usize] = ci;
        }
        let mut next = 0u32;
        for c in classes.iter_mut().filter(|c| c.locked.is_none()) {
            c.slot = next;
            next += 2;
        }
        let mut radius = vec![NONE; cap];
        let mut radii = Vec::new();
        for (id, e) in sketch.entities() {
            if !curve_ok[id.0 as usize] {
                continue;
            }
            if let Geometry::Circle { .. } = e.geometry {
                radius[id.0 as usize] = next;
                radii.push((id, next));
                next += 1;
            }
        }
        let n_vars = next as usize;
        for c in classes.iter_mut().filter(|c| c.locked.is_some()) {
            c.slot = next;
            next += 2;
        }
        let n_slots = next as usize;
        let mut point = vec![NONE; cap];
        for (i, &ci) in class_of.iter().enumerate() {
            if ci != NONE {
                point[i] = classes[ci as usize].slot;
            }
        }

        // Tangency contact points: classes known to lie on both curves.
        let mut on_curve: HashMap<u32, Vec<u32>> = HashMap::new();
        for (id, e) in sketch.entities() {
            let ends = match e.geometry {
                Geometry::Line { start, end } | Geometry::Arc { start, end, .. } => [start, end],
                _ => continue,
            };
            if curve_ok[id.0 as usize] {
                on_curve
                    .entry(id.0)
                    .or_default()
                    .extend(ends.iter().map(|p| class_of[p.0 as usize]));
            }
        }
        for (cid, c) in sketch.constraints() {
            if let ConstraintKind::PointOnCurve { point, curve } = c.kind
                && valid[cid.0 as usize]
            {
                on_curve
                    .entry(curve.0)
                    .or_default()
                    .push(class_of[point.0 as usize]);
            }
        }
        let mut contact = vec![NONE; sketch.constraint_capacity()];
        for (cid, c) in sketch.constraints() {
            if let ConstraintKind::Tangent(a, b) = c.kind
                && valid[cid.0 as usize]
                && let (Some(pa), Some(pb)) = (on_curve.get(&a.0), on_curve.get(&b.0))
                && let Some(&ci) = pa.iter().find(|ci| pb.contains(ci))
            {
                contact[cid.0 as usize] = classes[ci as usize].slot;
            }
        }

        let map = SlotMap {
            point,
            radius,
            emit,
            contact,
        };
        let mut st = Self {
            fingerprint: fingerprint(sketch),
            map,
            n_vars,
            n_slots,
            classes,
            class_of,
            radii,
            alias_adj,
            alias_cycles,
            clusters: Vec::new(),
            var_cluster: Vec::new(),
            const_eqs: Vec::new(),
            n_hard: 0,
            drag_problems: HashMap::new(),
            sizes: Vec::new(),
        };
        for (id, e) in sketch.entities() {
            if !curve_ok[id.0 as usize] {
                continue;
            }
            match e.geometry {
                Geometry::Line { start, end } => st.sizes.push(Size::Line(
                    st.map.point[start.0 as usize],
                    st.map.point[end.0 as usize],
                )),
                Geometry::Circle { .. } => {
                    st.sizes.push(Size::Radius(st.map.radius[id.0 as usize]))
                }
                Geometry::Arc { center, start, .. } => st.sizes.push(Size::Line(
                    st.map.point[center.0 as usize],
                    st.map.point[start.0 as usize],
                )),
                Geometry::Point { .. } => {}
            }
        }

        // ---- Clusters ----
        let mut vals = Vec::new();
        st.load(sketch, &mut vals);
        let mut eqs = Vec::new();
        equations::generate(sketch, &st.map, &vals, &mut eqs);
        st.n_hard = eqs.len();
        let mut vp: Vec<u32> = (0..n_vars as u32).collect();
        let mut eq_root = vec![NONE; eqs.len()];
        for (i, eq) in eqs.iter().enumerate() {
            let mut first = NONE;
            for &s in eq.slots() {
                if (s as usize) < n_vars {
                    if first == NONE {
                        first = s;
                    } else {
                        let (a, b) = (find(&mut vp, first), find(&mut vp, s));
                        if a != b {
                            vp[b as usize] = a;
                        }
                    }
                }
            }
            eq_root[i] = first;
        }
        let mut cluster_of_root = vec![NONE; n_vars];
        let mut var_cluster = vec![NONE; n_vars];
        for v in 0..n_vars as u32 {
            let r = find(&mut vp, v);
            if cluster_of_root[r as usize] == NONE {
                cluster_of_root[r as usize] = st.clusters.len() as u32;
                st.clusters.push(Cluster {
                    vars: Vec::new(),
                    eqs: Vec::new(),
                    problem: None,
                });
            }
            let ci = cluster_of_root[r as usize];
            var_cluster[v as usize] = ci;
            st.clusters[ci as usize].vars.push(v);
        }
        for (i, &first) in eq_root.iter().enumerate() {
            if first == NONE {
                st.const_eqs.push(i as u32);
            } else {
                let ci = var_cluster[first as usize];
                st.clusters[ci as usize].eqs.push(i as u32);
            }
        }
        st.var_cluster = var_cluster;
        st
    }

    /// Reads the current values into `vals`. Classes whose members disagree (a fresh
    /// coincidence) start at the members' average, or at the locked member's position.
    pub fn load(&self, sketch: &Sketch, vals: &mut Vec<f64>) {
        vals.clear();
        vals.resize(self.n_slots, 0.0);
        for c in &self.classes {
            let p = match c.locked {
                Some(l) => sketch.point(l),
                None => {
                    let first = sketch.point(c.members[0]);
                    if c.members[1..].iter().all(|&m| sketch.point(m) == first) {
                        first
                    } else {
                        let sum: DVec2 = c.members.iter().map(|&m| sketch.point(m)).sum();
                        sum / c.members.len() as f64
                    }
                }
            };
            vals[c.slot as usize] = p.x;
            vals[c.slot as usize + 1] = p.y;
        }
        for &(id, slot) in &self.radii {
            if let Some(Entity {
                geometry: Geometry::Circle { radius, .. },
                ..
            }) = sketch.entity(id)
            {
                vals[slot as usize] = *radius;
            }
        }
    }

    /// Writes solved values back into the sketch. Variables flagged in `keep` (failed
    /// clusters) keep the sketch's current values.
    pub fn store(&self, sketch: &mut Sketch, vals: &[f64], keep: &[bool]) {
        for c in &self.classes {
            let (sx, sy) = (c.slot as usize, c.slot as usize + 1);
            // Locked classes are constants: their members snap to the locked point.
            let keep_x = sx < self.n_vars && keep[sx];
            let keep_y = sy < self.n_vars && keep[sy];
            for &m in &c.members {
                if Some(m) == c.locked {
                    continue;
                }
                let cur = sketch.point(m);
                let new = DVec2::new(
                    if keep_x { cur.x } else { vals[sx] },
                    if keep_y { cur.y } else { vals[sy] },
                );
                if new != cur {
                    sketch.set_point(m, new);
                }
            }
        }
        for &(id, slot) in &self.radii {
            if !keep[slot as usize] {
                sketch.set_radius(id, vals[slot as usize].abs());
            }
        }
    }

    /// The coincident/concentric constraints on the forest path between two points.
    pub fn alias_path(&self, a: EntityId, b: EntityId) -> Vec<u32> {
        if a == b {
            return Vec::new();
        }
        let n = self.alias_adj.len();
        let mut prev: Vec<(u32, u32)> = vec![(NONE, NONE); n];
        let mut queue = std::collections::VecDeque::new();
        prev[a.0 as usize] = (a.0, NONE);
        queue.push_back(a.0);
        while let Some(x) = queue.pop_front() {
            if x == b.0 {
                break;
            }
            for &(y, cid) in &self.alias_adj[x as usize] {
                if prev[y as usize].0 == NONE {
                    prev[y as usize] = (x, cid);
                    queue.push_back(y);
                }
            }
        }
        let mut path = Vec::new();
        if prev[b.0 as usize].0 == NONE {
            return path;
        }
        let mut x = b.0;
        while x != a.0 {
            let (p, cid) = prev[x as usize];
            path.push(cid);
            x = p;
        }
        path
    }

    /// Coincidences that make the equation structurally degenerate: forest paths between
    /// aliased points it refers to, and from its points to a locked class member.
    pub fn alias_involvement(&self, points: &[EntityId]) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, &p) in points.iter().enumerate() {
            let ci = self.class_of[p.0 as usize];
            if ci == NONE {
                continue;
            }
            if let Some(l) = self.classes[ci as usize].locked {
                out.extend(self.alias_path(p, l));
            }
            for &q in &points[i + 1..] {
                if q != p && self.class_of[q.0 as usize] == ci {
                    out.extend(self.alias_path(p, q));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}
