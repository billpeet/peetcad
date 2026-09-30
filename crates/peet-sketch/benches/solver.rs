//! Solver timings: `cargo bench -p peet-sketch --bench solver`.
//!
//! A plain timing harness (no benchmark framework dependency). Each scenario runs a drag
//! of a few hundred mouse-move frames and reports the mean and worst `solve_drag` time,
//! plus full `solve` and `analyze` timings. Budget: < 2 ms per drag solve.

use std::hint::black_box;
use std::time::{Duration, Instant};

use peet_math::DVec2;
use peet_sketch::{ConstraintKind, Drag, EntityId, EntityKind, Sketch, Solver, shapes};

#[path = "../src/solver/tests/fixtures.rs"]
mod fixtures;

use fixtures::{bracket, chain, grid, truss};

const FRAMES: usize = 400;

struct Stats {
    mean: Duration,
    max: Duration,
}

fn time_frames(mut f: impl FnMut(usize)) -> Stats {
    // Warm up (builds and caches the structure).
    f(0);
    let mut total = Duration::ZERO;
    let mut max = Duration::ZERO;
    for k in 1..=FRAMES {
        let t = Instant::now();
        f(k);
        let d = t.elapsed();
        total += d;
        max = max.max(d);
    }
    Stats {
        mean: total / FRAMES as u32,
        max,
    }
}

fn count(s: &Sketch) -> (usize, usize, usize) {
    let curves = s
        .entities()
        .filter(|(_, e)| e.kind() != EntityKind::Point)
        .count();
    (s.entities().count(), curves, s.constraints().count())
}

fn drag_bench(name: &str, mut s: Sketch, point: EntityId, path: impl Fn(usize) -> DVec2) {
    let (entities, curves, constraints) = count(&s);
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged, "{name}: initial solve");
    let mut worst_residual = 0.0f64;
    let mut failures = 0;
    let stats = time_frames(|k| {
        let r = solver.solve_drag(
            &mut s,
            &[Drag::Point {
                point,
                target: path(k),
            }],
        );
        worst_residual = worst_residual.max(r.max_residual);
        failures += usize::from(!r.converged);
        black_box(&r);
    });
    println!(
        "{name:<34} {entities:>4} entities {curves:>4} curves {constraints:>4} constraints | \
         drag mean {:>8.3} ms  max {:>8.3} ms | worst residual {worst_residual:.1e} failures {failures}",
        stats.mean.as_secs_f64() * 1e3,
        stats.max.as_secs_f64() * 1e3,
    );
}

fn solve_and_analyze_bench(name: &str, s: &Sketch) {
    // Full solve from a perturbed state with a fresh solver (includes building the
    // structure), then analysis.
    let mut perturbed = s.clone();
    let ids: Vec<EntityId> = perturbed
        .entities()
        .filter(|(_, e)| e.kind() == EntityKind::Point)
        .map(|(id, _)| id)
        .collect();
    for (i, id) in ids.iter().enumerate() {
        let p = perturbed.point(*id);
        perturbed.set_point(
            *id,
            p + DVec2::new((i as f64).sin(), (i as f64).cos()) * 0.05,
        );
    }
    let reps = 20;
    let t = Instant::now();
    let mut iterations = 0;
    for _ in 0..reps {
        let mut s2 = perturbed.clone();
        let r = Solver::new().solve(&mut s2);
        assert!(r.converged, "{name}: {r:?}");
        iterations = r.iterations;
    }
    let solve = t.elapsed() / reps;
    let mut solver = Solver::new();
    let mut solved = perturbed.clone();
    solver.solve(&mut solved);
    let t = Instant::now();
    let mut dof = 0;
    for _ in 0..reps {
        dof = black_box(solver.analyze(&solved)).dof;
    }
    let analyze = t.elapsed() / reps;
    println!(
        "{name:<34} cold solve {:>8.3} ms ({iterations} it) | analyze {:>8.3} ms (dof {dof})",
        solve.as_secs_f64() * 1e3,
        analyze.as_secs_f64() * 1e3,
    );
}

/// A point of the sketch that is not the origin and is used by a curve of `kind`.
fn some_point(s: &Sketch, index: usize) -> EntityId {
    s.entities()
        .filter(|(id, e)| e.kind() == EntityKind::Point && *id != Sketch::ORIGIN)
        .nth(index)
        .map(|(id, _)| id)
        .expect("enough points")
}

fn circle_path(center: DVec2, radius: f64, step: f64) -> impl Fn(usize) -> DVec2 {
    move |k| center + DVec2::from_angle(k as f64 * step) * radius - DVec2::new(radius, 0.0)
}

fn main() {
    println!("solver benchmark ({FRAMES} drag frames per scenario)");

    // Fully defined bracket: every drag frame solves and projects back (nothing moves).
    let s = bracket(true);
    let p = some_point(&s, 1);
    let at = s.point(p);
    drag_bench(
        "bracket, fully defined",
        s.clone(),
        p,
        circle_path(at, 5.0, 0.05),
    );
    solve_and_analyze_bench("bracket, fully defined", &s);

    // The same bracket without dimensions: dragging an outline corner moves the outline.
    let s = bracket(false);
    let (_, corner) = s.endpoints(EntityId(1)).unwrap();
    let at = s.point(corner);
    drag_bench(
        "bracket, relations only",
        s.clone(),
        corner,
        circle_path(at, 5.0, 0.05),
    );
    solve_and_analyze_bench("bracket, relations only", &s);

    // A chain of 120 lines with lengths: dragging the free end swings the whole chain.
    let s = chain(120);
    let last = s
        .entities()
        .filter(|(_, e)| e.kind() == EntityKind::Line)
        .last()
        .map(|(id, _)| s.endpoints(id).unwrap().1)
        .unwrap();
    let at = s.point(last);
    drag_bench(
        "chain of 120 lines",
        s.clone(),
        last,
        circle_path(at, 40.0, 0.02),
    );
    solve_and_analyze_bench("chain of 120 lines", &s);

    // 8 × 8 grid (112 lines), under-constrained and fully defined.
    let (s, lines) = grid(8, false);
    let (_, corner) = s.endpoints(*lines.last().unwrap()).unwrap();
    let at = s.point(corner);
    drag_bench(
        "grid 8x8 (112 lines), free",
        s.clone(),
        corner,
        circle_path(at, 10.0, 0.05),
    );
    solve_and_analyze_bench("grid 8x8 (112 lines), free", &s);
    let (s, lines) = grid(8, true);
    let (_, corner) = s.endpoints(*lines.last().unwrap()).unwrap();
    let at = s.point(corner);
    drag_bench(
        "grid 8x8 (112 lines), defined",
        s.clone(),
        corner,
        circle_path(at, 10.0, 0.05),
    );
    solve_and_analyze_bench("grid 8x8 (112 lines), defined", &s);

    // Flexible 10 × 10 truss (180 lines, lengths only): one big connected cluster.
    let (s, lines) = truss(10);
    let (_, corner) = s.endpoints(*lines.last().unwrap()).unwrap();
    let at = s.point(corner);
    drag_bench(
        "truss 10x10 (180 lines)",
        s.clone(),
        corner,
        circle_path(at, 8.0, 0.05),
    );
    solve_and_analyze_bench("truss 10x10 (180 lines)", &s);
    let (s, lines) = truss(14);
    let (_, corner) = s.endpoints(*lines.last().unwrap()).unwrap();
    let at = s.point(corner);
    drag_bench(
        "truss 14x14 (364 lines)",
        s.clone(),
        corner,
        circle_path(at, 8.0, 0.05),
    );
    solve_and_analyze_bench("truss 14x14 (364 lines)", &s);

    // Many independent rectangles: clustering means only the dragged one is solved.
    let mut s = Sketch::new();
    for i in 0..30 {
        let o = DVec2::new((i % 6) as f64 * 40.0, (i / 6) as f64 * 30.0);
        shapes::rectangle(&mut s, o, o + DVec2::new(30.0, 20.0));
    }
    let l = s
        .entities()
        .find(|(_, e)| e.kind() == EntityKind::Line)
        .unwrap()
        .0;
    let (p, _) = s.endpoints(l).unwrap();
    s.add_constraint(ConstraintKind::Coincident(p, Sketch::ORIGIN))
        .unwrap();
    let corner = some_point(&s, 5);
    let at = s.point(corner);
    drag_bench(
        "30 rectangles (120 lines)",
        s.clone(),
        corner,
        circle_path(at, 5.0, 0.05),
    );
    solve_and_analyze_bench("30 rectangles (120 lines)", &s);
}
