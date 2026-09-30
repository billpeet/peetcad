//! Realistic test and benchmark sketches.
//!
//! Shared by the solver tests and `benches/solver.rs` (included with `#[path]`), so it only
//! uses the public API and expects the parent module to have `Sketch`, `EntityId`,
//! `ConstraintKind`, `DVec2` and `shapes` in scope.

#![allow(dead_code)]

use super::*;

fn p2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

macro_rules! rel {
    ($s:expr, $k:expr) => {{
        let k = $k;
        $s.add_constraint(k).expect("valid relation");
    }};
}

/// Adds a measured dimension if `dims`.
macro_rules! measured {
    ($s:expr, $k:expr, $dims:expr) => {{
        let k = $k;
        if $dims {
            $s.add_constraint(k).expect("valid dimension");
        }
    }};
}

fn ends(s: &Sketch, l: EntityId) -> (EntityId, EntityId) {
    s.endpoints(l).expect("line or arc")
}

/// Axis-aligned 30 × 20 rectangle with its lower-left corner on the origin, fully
/// dimensioned. Returns the sketch and its lines (bottom, right, top, left).
pub fn rectangle_fully_defined() -> (Sketch, [EntityId; 4]) {
    let mut s = Sketch::new();
    let shape = shapes::rectangle(&mut s, p2(0.0, 0.0), p2(30.0, 20.0));
    let l = [
        shape.curves[0],
        shape.curves[1],
        shape.curves[2],
        shape.curves[3],
    ];
    rel!(
        s,
        ConstraintKind::Coincident(ends(&s, l[0]).0, Sketch::ORIGIN)
    );
    s.add_dimension(ConstraintKind::Length(l[0]), 30.0).unwrap();
    s.add_dimension(ConstraintKind::Length(l[1]), 20.0).unwrap();
    (s, l)
}

/// A realistic bracket profile: an L-shaped outline with a rounded end, 12 holes, three
/// slots and a hexagonal cut-out: 40 curves (about 150 entities). With `dims` it is fully
/// defined (0 DOF); without, only the relations are present.
pub fn bracket(dims: bool) -> Sketch {
    let mut s = Sketch::new();
    // Outline: V0 (origin) → V1 → V2 → V3 → V4, arc V4 → V5 around (10, 80), V5 → V0.
    let v = [
        p2(0.0, 0.0),
        p2(100.0, 0.0),
        p2(100.0, 20.0),
        p2(20.0, 20.0),
        p2(20.0, 80.0),
        p2(0.0, 80.0),
    ];
    let bottom = s.add_line(v[0], v[1]);
    let right = s.add_line(v[1], v[2]);
    let step = s.add_line(v[2], v[3]);
    let inner = s.add_line(v[3], v[4]);
    let top = s.add_arc(p2(10.0, 80.0), v[4], v[5]);
    let left = s.add_line(v[5], v[0]);
    let loop_curves = [bottom, right, step, inner, top, left];
    for i in 0..6 {
        let a = ends(&s, loop_curves[i]).1;
        let b = ends(&s, loop_curves[(i + 1) % 6]).0;
        rel!(s, ConstraintKind::Coincident(a, b));
    }
    rel!(
        s,
        ConstraintKind::Coincident(ends(&s, bottom).0, Sketch::ORIGIN)
    );
    rel!(s, ConstraintKind::Horizontal(bottom));
    rel!(s, ConstraintKind::Vertical(right));
    rel!(s, ConstraintKind::Horizontal(step));
    rel!(s, ConstraintKind::Vertical(inner));
    rel!(s, ConstraintKind::Vertical(left));
    rel!(s, ConstraintKind::Tangent(inner, top));
    rel!(s, ConstraintKind::Tangent(top, left));
    measured!(s, ConstraintKind::Length(bottom), dims);
    measured!(s, ConstraintKind::Length(right), dims);
    measured!(s, ConstraintKind::Length(left), dims);
    measured!(s, ConstraintKind::Radius(top), dims);

    // Holes: a row along the base and a column up the leg, all equal to the first.
    let mut first: Option<EntityId> = None;
    let mut holes = Vec::new();
    for i in 0..7 {
        holes.push(p2(30.0 + 10.0 * i as f64, 10.0));
    }
    for i in 0..5 {
        holes.push(p2(10.0, 25.0 + 10.0 * i as f64));
    }
    for c in holes {
        let h = s.add_circle(c, 2.5);
        let centre = s.center(h).unwrap();
        measured!(
            s,
            ConstraintKind::HorizontalDistance(Sketch::ORIGIN, centre),
            dims
        );
        measured!(
            s,
            ConstraintKind::VerticalDistance(Sketch::ORIGIN, centre),
            dims
        );
        match first {
            None => {
                measured!(s, ConstraintKind::Diameter(h), dims);
                first = Some(h);
            }
            Some(f) => rel!(s, ConstraintKind::Equal(f, h)),
        }
    }

    // Slots.
    for k in 0..3 {
        let c1 = p2(35.0 + 20.0 * k as f64, 4.0);
        let c2 = c1 + p2(10.0, 0.0);
        let slot = shapes::slot(&mut s, c1, c2, 1.5);
        let centreline = slot.curves[4];
        let arc = slot.curves[3];
        let (cs, _) = ends(&s, centreline);
        rel!(s, ConstraintKind::Horizontal(centreline));
        measured!(
            s,
            ConstraintKind::HorizontalDistance(Sketch::ORIGIN, cs),
            dims
        );
        measured!(
            s,
            ConstraintKind::VerticalDistance(Sketch::ORIGIN, cs),
            dims
        );
        measured!(s, ConstraintKind::Length(centreline), dims);
        measured!(s, ConstraintKind::Radius(arc), dims);
    }

    // Hexagonal cut-out.
    let hex = shapes::polygon(&mut s, p2(10.0, 70.0), p2(14.0, 70.0), 6);
    let circle = *hex.curves.last().unwrap();
    let centre = s.center(circle).unwrap();
    rel!(s, ConstraintKind::Horizontal(hex.curves[1]));
    measured!(
        s,
        ConstraintKind::HorizontalDistance(Sketch::ORIGIN, centre),
        dims
    );
    measured!(
        s,
        ConstraintKind::VerticalDistance(Sketch::ORIGIN, centre),
        dims
    );
    measured!(s, ConstraintKind::Radius(circle), dims);
    s
}

/// A polyline of `n` lines joined end to start, starting at the origin, each with a length
/// dimension: `n` degrees of freedom (the joint angles).
pub fn chain(n: usize) -> Sketch {
    let mut s = Sketch::new();
    let mut prev: Option<EntityId> = None;
    let mut at = p2(0.0, 0.0);
    for i in 0..n {
        let angle = (i as f64 * 0.7).sin() * 0.8;
        let next = at + DVec2::from_angle(angle) * 5.0;
        let l = s.add_line(at, next);
        match prev {
            None => rel!(s, ConstraintKind::Coincident(ends(&s, l).0, Sketch::ORIGIN)),
            Some(p) => rel!(s, ConstraintKind::Coincident(ends(&s, p).1, ends(&s, l).0)),
        }
        s.add_constraint(ConstraintKind::Length(l)).unwrap();
        prev = Some(l);
        at = next;
    }
    s
}

/// An `n × n` grid of points joined by horizontal and vertical lines (`2n(n-1)` lines).
/// With `dims`, the bottom row and left column lines have lengths and the corner sits on
/// the origin, so the grid is fully defined; without, it has `2n` freedoms.
pub fn grid(n: usize, dims: bool) -> (Sketch, Vec<EntityId>) {
    let mut s = Sketch::new();
    let at = |i: usize, j: usize| p2(i as f64 * 10.0, j as f64 * 10.0);
    // Point entity at each node: the start of the line leaving it (or the end of one
    // arriving), joined by coincident relations.
    let mut node: Vec<Option<EntityId>> = vec![None; n * n];
    let mut lines = Vec::new();
    let join = |s: &mut Sketch, node: &mut Vec<Option<EntityId>>, i: usize, j: usize, p| match node
        [j * n + i]
    {
        None => node[j * n + i] = Some(p),
        Some(q) => rel!(s, ConstraintKind::Coincident(q, p)),
    };
    for j in 0..n {
        for i in 0..n {
            if i + 1 < n {
                let l = s.add_line(at(i, j), at(i + 1, j));
                rel!(s, ConstraintKind::Horizontal(l));
                let (a, b) = ends(&s, l);
                join(&mut s, &mut node, i, j, a);
                join(&mut s, &mut node, i + 1, j, b);
                if dims && j == 0 {
                    s.add_constraint(ConstraintKind::Length(l)).unwrap();
                }
                lines.push(l);
            }
            if j + 1 < n {
                let l = s.add_line(at(i, j), at(i, j + 1));
                rel!(s, ConstraintKind::Vertical(l));
                let (a, b) = ends(&s, l);
                join(&mut s, &mut node, i, j, a);
                join(&mut s, &mut node, i, j + 1, b);
                if dims && i == 0 {
                    s.add_constraint(ConstraintKind::Length(l)).unwrap();
                }
                lines.push(l);
            }
        }
    }
    if dims {
        rel!(
            s,
            ConstraintKind::Coincident(node[0].unwrap(), Sketch::ORIGIN)
        );
    }
    (s, lines)
}

/// An `n × n` grid of lines like [`grid`], but held only by a length on every line (no
/// horizontal/vertical relations), one corner on the origin and the first line
/// horizontal: a flexible truss where dragging a corner moves most of the sketch.
pub fn truss(n: usize) -> (Sketch, Vec<EntityId>) {
    let (mut s, lines) = grid(n, false);
    let relations: Vec<_> = s
        .constraints()
        .filter(|(_, c)| {
            matches!(
                c.kind,
                ConstraintKind::Horizontal(_) | ConstraintKind::Vertical(_)
            )
        })
        .map(|(id, _)| id)
        .collect();
    for id in relations {
        s.remove_constraint(id);
    }
    for &l in &lines {
        s.add_constraint(ConstraintKind::Length(l)).unwrap();
    }
    let (a, _) = ends(&s, lines[0]);
    rel!(s, ConstraintKind::Coincident(a, Sketch::ORIGIN));
    rel!(s, ConstraintKind::Horizontal(lines[0]));
    (s, lines)
}
