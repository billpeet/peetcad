//! Property tests on random consistent sketches.
//!
//! Geometry is generated on an integer lattice, so many relations hold *exactly* (shared
//! points, horizontal/vertical/parallel/perpendicular lines, equal lengths, points on
//! circles, tangencies). Every relation that holds is a candidate; a random subset is
//! added, plus random dimensions set to their measured values (some driven). The sketch is
//! therefore consistent by construction, usually with redundancy. Then every free value is
//! perturbed and the solver must restore all constraints.

use super::*;

/// Small deterministic generator driven by a proptest seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn lattice(r: &mut Rng) -> DVec2 {
    DVec2::new(r.below(5) as f64 * 10.0, r.below(5) as f64 * 10.0)
}

/// Builds a random consistent sketch from a seed.
fn random_sketch(seed: u64) -> Sketch {
    let mut r = Rng(seed | 1);
    let mut s = Sketch::new();
    let mut lines = Vec::new();
    let mut circles = Vec::new(); // circles and arcs
    let n_lines = 2 + r.below(4);
    for _ in 0..n_lines {
        let a = lattice(&mut r);
        let mut b = lattice(&mut r);
        while b == a {
            b = lattice(&mut r);
        }
        lines.push(s.add_line(a, b));
    }
    for _ in 0..r.below(3) {
        circles.push(s.add_circle(lattice(&mut r), 10.0 * (1 + r.below(2)) as f64));
    }
    if r.chance(1, 2) {
        let c = lattice(&mut r);
        let rad = 10.0 * (1 + r.below(2)) as f64;
        let k = r.below(4) as f64;
        let start = c + DVec2::from_angle(k * PI / 2.0) * rad;
        let end = c + DVec2::from_angle((k + 1.0 + r.below(2) as f64) * PI / 2.0) * rad;
        circles.push(s.add_arc(c, start, end));
    }
    for _ in 0..r.below(3) {
        s.add_point(lattice(&mut r));
    }

    let points: Vec<EntityId> = s
        .entities()
        .filter(|(_, e)| e.kind() == EntityKind::Point)
        .map(|(id, _)| id)
        .collect();
    let pos = |s: &Sketch, p| s.point(p);
    let seg = |s: &Sketch, l| line_ab(s, l);
    let mut candidates: Vec<ConstraintKind> = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        for &q in &points[i + 1..] {
            let (a, b) = (pos(&s, p), pos(&s, q));
            if a == b {
                candidates.push(K::Coincident(p, q));
            } else {
                if a.y == b.y {
                    candidates.push(K::HorizontalPoints(p, q));
                }
                if a.x == b.x {
                    candidates.push(K::VerticalPoints(p, q));
                }
            }
        }
    }
    for (i, &l) in lines.iter().enumerate() {
        let (a, b) = seg(&s, l);
        let d = b - a;
        if d.y == 0.0 {
            candidates.push(K::Horizontal(l));
        }
        if d.x == 0.0 {
            candidates.push(K::Vertical(l));
        }
        for &m in &lines[i + 1..] {
            let (c, e) = seg(&s, m);
            let d2 = e - c;
            if d.perp_dot(d2) == 0.0 {
                candidates.push(K::Parallel(l, m));
            }
            if d.dot(d2) == 0.0 {
                candidates.push(K::Perpendicular(l, m));
            }
            if d.length_squared() == d2.length_squared() {
                candidates.push(K::Equal(l, m));
            }
        }
        for &p in &points {
            let pp = pos(&s, p);
            if d.perp_dot(pp - a) == 0.0 {
                candidates.push(K::PointOnCurve { point: p, curve: l });
            }
            if pp * 2.0 == a + b {
                candidates.push(K::Midpoint { point: p, line: l });
            }
        }
        for (j, &p) in points.iter().enumerate() {
            for &q in &points[j + 1..] {
                let (pa, pb) = (pos(&s, p), pos(&s, q));
                if pa != pb && d.perp_dot((pa + pb) * 0.5 - a) == 0.0 && d.dot(pb - pa) == 0.0 {
                    candidates.push(K::Symmetric {
                        a: p,
                        b: q,
                        axis: l,
                    });
                }
            }
        }
        for &c in &circles {
            let (cc, rad) = circ(&s, c);
            let cr = d.perp_dot(cc - a);
            if cr * cr == rad * rad * d.length_squared() {
                candidates.push(K::Tangent(l, c));
            }
        }
    }
    for (i, &c) in circles.iter().enumerate() {
        let (cc, rc) = circ(&s, c);
        for &p in &points {
            if pos(&s, p).distance_squared(cc) == rc * rc {
                candidates.push(K::PointOnCurve { point: p, curve: c });
            }
        }
        for &e in &circles[i + 1..] {
            let (ce, re) = circ(&s, e);
            let d2 = cc.distance_squared(ce);
            if rc == re {
                candidates.push(K::Equal(c, e));
            }
            if cc == ce {
                candidates.push(K::Concentric(c, e));
            } else if d2 == (rc + re) * (rc + re) || d2 == (rc - re) * (rc - re) {
                candidates.push(K::Tangent(c, e));
            }
        }
    }
    for k in candidates {
        if r.chance(1, 2) {
            s.add_constraint(k).unwrap();
        }
    }
    if r.chance(1, 3) {
        let p = points[r.below(points.len() as u64) as usize];
        let at = pos(&s, p);
        s.add_constraint(K::Fix { point: p, at }).unwrap();
    }
    if let Some(&p) = points
        .iter()
        .find(|&&p| p != Sketch::ORIGIN && pos(&s, p) == DVec2::ZERO)
        && r.chance(1, 2)
    {
        s.add_constraint(K::Coincident(p, Sketch::ORIGIN)).unwrap();
    }

    // Dimensions at measured values; some driven.
    let mut dims: Vec<ConstraintKind> = Vec::new();
    for _ in 0..r.below(5) {
        let l = lines[r.below(lines.len() as u64) as usize];
        let p = points[r.below(points.len() as u64) as usize];
        let q = points[r.below(points.len() as u64) as usize];
        let m = lines[r.below(lines.len() as u64) as usize];
        match r.below(7) {
            0 => dims.push(K::Length(l)),
            1 if p != q && pos(&s, p) != pos(&s, q) => dims.push(K::Distance(p, q)),
            2 if !seg_has(&s, l, p) => dims.push(K::Distance(p, l)),
            3 if p != q => dims.push(K::HorizontalDistance(p, q)),
            4 if p != q => dims.push(K::VerticalDistance(p, q)),
            5 if l != m => dims.push(K::Angle(l, m)),
            6 if !circles.is_empty() => {
                let c = circles[r.below(circles.len() as u64) as usize];
                dims.push(if r.chance(1, 2) {
                    K::Radius(c)
                } else {
                    K::Diameter(c)
                });
            }
            _ => {}
        }
    }
    for k in dims {
        let id = s.add_constraint(k).unwrap();
        if r.chance(1, 3) {
            s.constraint_mut(id)
                .unwrap()
                .dimension
                .as_mut()
                .unwrap()
                .driving = false;
        }
    }
    s
}

fn seg_has(s: &Sketch, l: EntityId, p: EntityId) -> bool {
    let (a, b) = s.endpoints(l).unwrap();
    a == p || b == p
}

/// Moves every unlocked point and circle radius by a small random amount.
fn perturb(s: &mut Sketch, seed: u64, size: f64) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let ids: Vec<(EntityId, EntityKind)> = s.entities().map(|(id, e)| (id, e.kind())).collect();
    for (id, kind) in ids {
        match kind {
            EntityKind::Point => {
                let p = s.point(id);
                let d = DVec2::new(r.unit() - 0.5, r.unit() - 0.5) * size;
                s.set_point(id, p + d);
            }
            EntityKind::Circle => {
                let rad = circ(s, id).1;
                s.set_radius(id, rad + (r.unit() - 0.5) * size);
            }
            _ => {}
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 200,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_consistent_sketches_solve(seed in any::<u64>(), pseed in any::<u64>()) {
        let mut s = random_sketch(seed);
        let mut solver = Solver::new();

        // Consistent by construction: nothing to do.
        let r0 = solver.solve(&mut s);
        prop_assert!(r0.converged, "exact sketch not converged: {:?}", r0);
        prop_assert!(max_violation(&s) < TOL);

        perturb(&mut s, pseed, 0.1);
        let r = solver.solve(&mut s);
        prop_assert!(r.converged, "perturbed sketch not converged: {:?}", r);
        // Lattice sketches are often singular (constraints meeting tangentially), where
        // convergence is slow and stops within the model tolerance rather than ~1e-11.
        let m = max_violation(&s);
        prop_assert!(m < 2.0 * CONVERGED, "violation {}", m);

        // Stable: a second solve changes nothing.
        let before = s.clone();
        let r2 = solver.solve(&mut s);
        prop_assert!(r2.converged);
        prop_assert!(r2.iterations == 0, "{:?} after {:?}", r2, r);
        prop_assert_eq!(&s, &before);

        // A satisfied sketch never reports conflicts, and a fresh solver agrees.
        let a = solver.analyze(&s);
        prop_assert!(a.diagnoses.iter().all(|d| d.status == ConstraintStatus::Redundant),
            "{:?}", a.diagnoses);
        prop_assert_eq!(&a, &Solver::new().analyze(&s));
    }

    #[test]
    fn random_drags_keep_constraints(seed in any::<u64>(), pseed in any::<u64>(),
                                     tx in -60.0..60.0f64, ty in -60.0..60.0f64) {
        let mut s = random_sketch(seed);
        let mut solver = Solver::new();
        prop_assert!(solver.solve(&mut s).converged);
        let points: Vec<EntityId> = s
            .entities()
            .filter(|(id, e)| e.kind() == EntityKind::Point && *id != Sketch::ORIGIN)
            .map(|(id, _)| id)
            .collect();
        let p = points[(pseed % points.len() as u64) as usize];
        let start = s.point(p);
        let target = DVec2::new(tx, ty);
        // Drag in small steps like mouse moves.
        for k in 1..=10 {
            let t = start + (target - start) * (k as f64 / 10.0);
            let r = solver.solve_drag(&mut s, &[Drag::Point { point: p, target: t }]);
            prop_assert!(r.converged, "{:?}", r);
            // Random drags pass through degenerate configurations (zero length lines
            // in angle dimensions, …) where convergence is only linear; the solver then
            // stops at its convergence threshold instead of ~1e-11.
            let m = max_violation(&s);
            prop_assert!(m < 5.0 * CONVERGED, "violation {} at step {}", m, k);
        }
    }

    #[test]
    fn free_dof_matches_a_fresh_count(seed in any::<u64>()) {
        // DOF equals variables minus the rank found; cross-check it against removing all
        // constraints one by one never *increasing* rank.
        let s = random_sketch(seed);
        let a = Solver::new().analyze(&s);
        let mut bare = s.clone();
        let ids: Vec<ConstraintId> = bare.constraints().map(|(id, _)| id).collect();
        for id in ids {
            bare.remove_constraint(id);
        }
        let free = Solver::new().analyze(&bare);
        prop_assert!(a.dof <= free.dof);
        prop_assert!(free.diagnoses.is_empty());
    }
}
