//! The geometric constraint solver.
//!
//! Variables are point coordinates and circle radii (an arc's radius is `|start - centre|`,
//! and each arc adds an implicit equation keeping its end on the same circle). Every
//! driving constraint becomes one or more residual equations (see `equations.rs` for the
//! formulation); driven dimensions only measure. Coincident points share
//! variables (aliasing), and the system is split into independent clusters that are solved
//! separately by damped least squares (the `peet-solve` crate): Levenberg–Marquardt with minimum norm
//! steps, falling back to dogleg if it stalls. Clusters that are already satisfied are
//! skipped, and during a drag only clusters touching dragged geometry (or left unsatisfied
//! by an edit) are solved.
//!
//! Dragging is two-phase, repeated a few times: (a) a minimum norm Gauss–Newton step on the
//! constraints plus low-weight "follow the cursor" equations, then (b) the constraints
//! alone are re-solved from there (a minimum norm projection back onto the constraints).
//! Steps are halved if the projection fails or the dragged geometry ends up further from
//! its target. So the constraints hold exactly, the dragged geometry gets as close to the
//! cursor as they allow, and everything else moves as little as possible.
//!
//! A dimension given a new value in a sketch that is not fully defined moves the geometry
//! the way SolidWorks does: what the most relations and dimensions hold stays put, and the
//! rest moves (`solve_holding_still`). Only failing that is the change spread by least
//! squares.
//!
//! If a cluster cannot be solved, or the "solution" collapses a curve to nothing (the least
//! squares escape of conflicting constraints), its geometry is left as it was, so a
//! conflicting edit never scrambles the sketch. [`Solver::analyze`] then explains which constraints clash
//! (`analysis.rs`) and reports degrees of freedom per entity.
//!
//! Structure that only depends on the sketch topology (aliasing, clusters, sparse
//! factorisation patterns) is cached in the [`Solver`] and rebuilt whenever the topology
//! fingerprint changes; values and sign choices are re-read on every call.

mod analysis;
mod equations;
mod structure;
#[cfg(test)]
mod tests;

use peet_math::DVec2;

use peet_solve::{CONVERGED, Equation, NONE, Options, Problem, SKIP, Space};

use self::equations::{Eq, Owner};
use self::structure::{Structure, curve_points, fingerprint};
use crate::sketch::{ConstraintId, EntityId, Geometry, Sketch};

/// Weight of the soft drag equations relative to constraints (phase one of a drag).
const SOFT_WEIGHT: f64 = 1e-3;
/// A line, circle or arc smaller than this (mm) after a solve, that wasn't before, counts
/// as collapsed and the solve is rejected.
const COLLAPSED: f64 = peet_math::tolerance::LINEAR * 10.0;
/// Iteration limit per cluster solve.
const MAX_ITER: usize = 100;
/// Drag: outer (step + projection) iterations, and step halvings per iteration.
const MAX_DRAG_STEPS: usize = 12;
const MAX_HALVINGS: usize = 6;
/// Drag: iteration budget per cluster and call, so degenerate configurations can't stall
/// the frame. The geometry is feasible after every accepted step anyway.
const DRAG_BUDGET: usize = 150;
/// Options for projecting back onto the constraints during a drag.
const PROJECT: Options = Options {
    max_iter: 30,
    hard: true,
    dogleg: false,
};
/// Options for solving the constraints alone.
const HARD: Options = Options {
    max_iter: MAX_ITER,
    hard: true,
    dogleg: true,
};
/// Cached drag problems are dropped beyond this many.
const MAX_DRAG_PROBLEMS: usize = 64;

/// A drag target for interactive editing. Dragged geometry follows the cursor as closely
/// as the constraints allow; everything else moves as little as possible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    /// Move a point towards `target`.
    Point { point: EntityId, target: DVec2 },
    /// Move a curve. For a line or arc: translate it by `target - grab`. For a circle:
    /// change its radius so it passes through `target` (the centre stays put when free to).
    /// `grab` is where the curve was picked, in sketch coordinates, at the drag start.
    Curve {
        curve: EntityId,
        grab: DVec2,
        target: DVec2,
    },
}

/// Outcome of a solve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolveReport {
    /// All driving constraints are satisfied (within tolerance).
    pub converged: bool,
    /// Total iterations over all clusters that needed solving.
    pub iterations: usize,
    /// Largest remaining residual (mm, or radians for angular equations).
    pub max_residual: f64,
    /// When not converged: the constraints whose equations are still unsatisfied.
    pub unsatisfied: Vec<ConstraintId>,
}

/// Constraint state of a single entity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DofStatus {
    /// Can still move: shown in blue in the UI.
    #[default]
    Under,
    /// Fully defined: shown in black/white.
    Fully,
    /// Involved in conflicting constraints: shown in red. (Redundant ones don't make
    /// geometry over-defined.)
    Over,
}

/// State of a single constraint after analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConstraintStatus {
    #[default]
    Ok,
    /// Satisfied, but implied by other constraints (it can be deleted without changing
    /// anything). Harmless numerically, but the UI shows it so the user can clean up.
    Redundant,
    /// Cannot be satisfied together with others.
    Conflicting,
}

/// A redundant or conflicting constraint and the constraints it clashes with.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnosis {
    pub constraint: ConstraintId,
    pub status: ConstraintStatus,
    /// The other constraints involved, so the UI can say "Horizontal on Line3 conflicts
    /// with Vertical on Line3 and Angle d4".
    pub involved: Vec<ConstraintId>,
}

/// Degrees of freedom and diagnostics for a whole sketch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysis {
    /// Remaining degrees of freedom of the whole sketch.
    pub dof: usize,
    /// Per entity id (index = `EntityId.0`). Dead ids hold `Under`.
    pub entities: Vec<DofStatus>,
    /// Per constraint id (index = `ConstraintId.0`).
    pub constraints: Vec<ConstraintStatus>,
    /// One entry per redundant or conflicting constraint.
    pub diagnoses: Vec<Diagnosis>,
}

impl Analysis {
    pub fn entity_status(&self, id: EntityId) -> DofStatus {
        self.entities
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn constraint_status(&self, id: ConstraintId) -> ConstraintStatus {
        self.constraints
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    /// True if nothing can move and nothing conflicts. Redundant constraints don't
    /// count against it.
    pub fn is_fully_defined(&self) -> bool {
        self.dof == 0 && !self.is_over_defined()
    }

    /// True if any constraint conflicts with others. As in SolidWorks, a constraint
    /// that is redundant (implied by the others, and satisfied) does not over-define the
    /// sketch: it is accepted, adds nothing, and is listed in [`Analysis::diagnoses`] so
    /// it can be cleaned up.
    pub fn is_over_defined(&self) -> bool {
        self.conflicts() > 0
    }

    /// How many constraints conflict with others.
    pub fn conflicts(&self) -> usize {
        self.diagnoses
            .iter()
            .filter(|d| d.status == ConstraintStatus::Conflicting)
            .count()
    }
}

/// The constraint solver. Keep one per sketch being edited: it caches the equation
/// structure between calls, so repeated drag solves on an unchanged topology are cheap.
#[derive(Debug, Default)]
pub struct Solver {
    structure: Option<Structure>,
    vals: Vec<f64>,
    eqs: Vec<Eq>,
    /// Positions of dragged curves' points when their drag started, keyed by the curve
    /// and the grab position.
    drag_starts: Vec<DragStart>,
}

#[derive(Debug)]
struct DragStart {
    curve: EntityId,
    grab: [u64; 2],
    points: Vec<(EntityId, DVec2)>,
}

fn grab_key(grab: DVec2) -> [u64; 2] {
    [grab.x.to_bits(), grab.y.to_bits()]
}

impl Solver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the geometry to satisfy all driving constraints, changing it as little as
    /// possible. Refreshes driven dimension values afterwards.
    pub fn solve(&mut self, sketch: &mut Sketch) -> SolveReport {
        self.drag_starts.clear();
        self.run(sketch, &[], None)
    }

    /// Solves after reference geometry moves, retaining side and tangency choices from
    /// the previous solution. Both sketches must have identical entity topology.
    pub fn solve_with_previous(&mut self, sketch: &mut Sketch, previous: &Sketch) -> SolveReport {
        self.drag_starts.clear();
        self.run(sketch, &[], Some(previous))
    }

    /// Solves with the drags as additional soft targets: constraints always win, and the
    /// dragged geometry gets as close to its targets as they allow.
    pub fn solve_drag(&mut self, sketch: &mut Sketch, drags: &[Drag]) -> SolveReport {
        self.run(sketch, drags, None)
    }

    /// Degrees of freedom per entity and diagnostics for redundant/conflicting constraints,
    /// evaluated at the sketch's current geometry (call after [`Solver::solve`]).
    pub fn analyze(&mut self, sketch: &Sketch) -> Analysis {
        self.drag_starts.clear();
        self.prepare(sketch);
        let st = self.structure.as_mut().expect("prepared");
        analysis::analyze(st, sketch, &self.vals, &self.eqs)
    }

    /// Refreshes the cached structure if the topology changed, loads the current values
    /// and regenerates the equations (sign and branch choices come from the geometry now).
    fn prepare(&mut self, sketch: &Sketch) {
        let fp = fingerprint(sketch);
        if self.structure.as_ref().is_none_or(|s| s.fingerprint != fp) {
            self.structure = Some(Structure::build(sketch));
            self.drag_starts.clear();
        }
        let st = self.structure.as_ref().expect("just built");
        st.load(sketch, &mut self.vals);
        equations::generate(sketch, &st.map, &self.vals, &mut self.eqs);
        debug_assert_eq!(self.eqs.len(), st.n_hard);
    }

    /// Appends the soft equations for `drags`.
    fn soft_equations(&mut self, sketch: &Sketch, drags: &[Drag]) {
        let st = self.structure.as_ref().expect("prepared");
        let vals = &self.vals;
        let pos = |slot: u32| DVec2::new(vals[slot as usize], vals[slot as usize + 1]);
        self.drag_starts.retain(|d| {
            drags.iter().any(|g| {
                matches!(*g, Drag::Curve { curve, grab, .. }
                    if curve == d.curve && d.grab == grab_key(grab))
            })
        });
        let mut soft = Vec::new();
        let target_point = |slot: u32, t: DVec2, soft: &mut Vec<Eq>| {
            if slot != NONE && (slot as usize) < st.n_vars {
                let w = SOFT_WEIGHT;
                soft.push(Eq::linear(&[(slot, w)], w * t.x, Owner::Soft));
                soft.push(Eq::linear(&[(slot + 1, w)], w * t.y, Owner::Soft));
            }
        };
        let point_slot = |id: EntityId| st.map.point.get(id.0 as usize).copied().unwrap_or(NONE);
        for drag in drags {
            match *drag {
                Drag::Point { point, target } => target_point(point_slot(point), target, &mut soft),
                Drag::Curve {
                    curve,
                    grab,
                    target,
                } => {
                    let Some(e) = sketch.entity(curve) else {
                        continue;
                    };
                    match e.geometry {
                        Geometry::Point { .. } => {
                            target_point(point_slot(curve), target, &mut soft);
                        }
                        Geometry::Circle { center, .. } => {
                            let (c, r) = (point_slot(center), st.map.radius[curve.0 as usize]);
                            if c != NONE && r != NONE && (r as usize) < st.n_vars {
                                let want = target.distance(pos(c));
                                soft.push(Eq::linear(
                                    &[(r, SOFT_WEIGHT)],
                                    SOFT_WEIGHT * want,
                                    Owner::Soft,
                                ));
                            }
                        }
                        ref g => {
                            let key = grab_key(grab);
                            let idx = match self
                                .drag_starts
                                .iter()
                                .position(|d| d.curve == curve && d.grab == key)
                            {
                                Some(i) => i,
                                None => {
                                    let points = curve_points(g)
                                        .iter()
                                        .filter(|&&p| point_slot(p) != NONE)
                                        .map(|&p| (p, pos(point_slot(p))))
                                        .collect();
                                    self.drag_starts.push(DragStart {
                                        curve,
                                        grab: key,
                                        points,
                                    });
                                    self.drag_starts.len() - 1
                                }
                            };
                            let delta = target - grab;
                            for &(p, start) in &self.drag_starts[idx].points {
                                target_point(point_slot(p), start + delta, &mut soft);
                            }
                        }
                    }
                }
            }
        }
        self.eqs.extend(soft);
    }

    fn run(
        &mut self,
        sketch: &mut Sketch,
        drags: &[Drag],
        previous: Option<&Sketch>,
    ) -> SolveReport {
        self.prepare(sketch);
        if let Some(previous) = previous {
            let st = self.structure.as_ref().expect("prepared");
            let mut branch_values = Vec::new();
            st.load(previous, &mut branch_values);
            equations::generate(sketch, &st.map, &branch_values, &mut self.eqs);
        }
        if !drags.is_empty() {
            self.soft_equations(sketch, drags);
        }
        let Self {
            structure,
            vals,
            eqs,
            ..
        } = self;
        let st = structure.as_mut().expect("prepared");
        let n_hard = st.n_hard;

        // Soft equations grouped by cluster (each soft equation has a single variable).
        let mut soft_by_cluster: Vec<(u32, Vec<u32>)> = Vec::new();
        for (k, eq) in eqs[n_hard..].iter().enumerate() {
            let ci = st.var_cluster[eq.slots[0] as usize];
            let idx = (n_hard + k) as u32;
            match soft_by_cluster.iter_mut().find(|(c, _)| *c == ci) {
                Some((_, v)) => v.push(idx),
                None => soft_by_cluster.push((ci, vec![idx])),
            }
        }
        if st.drag_problems.len() > MAX_DRAG_PROBLEMS {
            st.drag_problems.clear();
        }

        // The geometry each variable is part of: a point's two coordinates go together.
        // And which way each moves a point: 0 along x, 1 along y, 2 neither (a radius).
        let mut group: Vec<u32> = (0..st.n_vars as u32).collect();
        let mut axis = vec![2u8; st.n_vars];
        for class in &st.classes {
            if class.slot != NONE && (class.slot as usize + 1) < st.n_vars {
                group[class.slot as usize + 1] = class.slot;
                axis[class.slot as usize] = 0;
                axis[class.slot as usize + 1] = 1;
            }
        }
        // How many equations each point (or radius) takes part in, over the whole sketch.
        let mut held_by = vec![0usize; st.n_vars];
        for eq in &eqs[..n_hard] {
            let mut seen: Vec<u32> = Vec::new();
            for &s in &eq.slots {
                if s != NONE && (s as usize) < st.n_vars && !seen.contains(&group[s as usize]) {
                    seen.push(group[s as usize]);
                    held_by[group[s as usize] as usize] += 1;
                }
            }
        }
        // Which equations are dimensions'.
        let dimensions: Vec<bool> = eqs[..n_hard]
            .iter()
            .map(|eq| match eq.owner {
                Owner::Constraint(c) => sketch
                    .constraint(ConstraintId(c))
                    .is_some_and(|c| c.dimension.is_some()),
                _ => false,
            })
            .collect();
        let mut keep = vec![false; st.n_vars];
        let mut iterations = 0;
        let mut saved = Vec::new();
        let initial = vals.clone();
        for ci in 0..st.clusters.len() {
            let soft = soft_by_cluster
                .iter()
                .find(|(c, _)| *c == ci as u32)
                .map(|(_, v)| v.as_slice())
                .unwrap_or(&[]);
            let cluster = &mut st.clusters[ci];
            let hard_ok = cluster
                .eqs
                .iter()
                .all(|&e| eqs[e as usize].residual(vals).abs() <= SKIP);
            let soft_ok = soft
                .iter()
                .all(|&e| eqs[e as usize].residual(vals).abs() <= peet_solve::TARGET * SOFT_WEIGHT);
            if hard_ok && soft_ok {
                continue;
            }
            saved.clear();
            saved.extend(cluster.vars.iter().map(|&v| vals[v as usize]));
            let problem = cluster.problem.get_or_insert_with(|| {
                Problem::new(
                    cluster.vars.clone(),
                    cluster.eqs.clone(),
                    eqs,
                    Space::Equations,
                )
            });
            let converged = if soft.is_empty() {
                let still = solve_holding_still(
                    &cluster.vars,
                    &cluster.eqs,
                    &group,
                    &axis,
                    &held_by,
                    &dimensions,
                    |vals| {
                        st.sizes.iter().any(|size| {
                            size.eval(vals) < COLLAPSED && size.eval(&initial) >= 100.0 * COLLAPSED
                        })
                    },
                    eqs,
                    vals,
                );
                iterations += still.1;
                still.0 || {
                    let outcome = problem.solve(eqs, vals, HARD);
                    iterations += outcome.iterations;
                    outcome.converged
                }
            } else {
                let mut key = Vec::with_capacity(soft.len() * 2);
                for &e in soft {
                    key.push(e);
                    key.push(eqs[e as usize].slots[0]);
                }
                let drag = st.drag_problems.entry((ci as u32, key)).or_insert_with(|| {
                    let mut all = cluster.eqs.clone();
                    all.extend_from_slice(soft);
                    Problem::new(cluster.vars.clone(), all, eqs, Space::Variables)
                });
                let (converged, its) = drag_cluster(drag, problem, eqs, vals, soft, hard_ok);
                iterations += its;
                converged
            };
            if !converged {
                for (&v, &x) in cluster.vars.iter().zip(&saved) {
                    vals[v as usize] = x;
                    keep[v as usize] = true;
                }
            }
        }

        // A "solution" that collapses a curve is the least squares escape of conflicting
        // scale-dependent constraints (horizontal + vertical + an angle shrink both lines
        // to nothing), not what the user meant: reject it.
        for size in &st.sizes {
            if size.eval(vals) < COLLAPSED && size.eval(&initial) >= 100.0 * COLLAPSED {
                let (slots, n) = size.slots();
                for &slot in &slots[..n] {
                    if (slot as usize) < st.n_vars {
                        let ci = st.var_cluster[slot as usize] as usize;
                        for &v in &st.clusters[ci].vars {
                            vals[v as usize] = initial[v as usize];
                            keep[v as usize] = true;
                        }
                    }
                }
            }
        }

        st.store(sketch, vals, &keep);
        sketch.update_driven_dimensions();

        let mut report = SolveReport {
            iterations,
            ..SolveReport::default()
        };
        let mut unsatisfied: Vec<u32> = Vec::new();
        for eq in &eqs[..n_hard] {
            let r = eq.residual(vals).abs();
            let r = if r.is_nan() { f64::INFINITY } else { r };
            report.max_residual = report.max_residual.max(r);
            if r > CONVERGED
                && let Owner::Constraint(c) = eq.owner
            {
                unsatisfied.push(c);
            }
        }
        unsatisfied.sort_unstable();
        unsatisfied.dedup();
        report.converged = report.max_residual <= CONVERGED;
        report.unsatisfied = unsatisfied.into_iter().map(ConstraintId).collect();
        report
    }
}

/// Solves a cluster that a changed value left unsatisfied the way SolidWorks does: the
/// geometry held by the most constraints stays where it is and the rest moves. A
/// rectangle with its width and height dimensioned from one corner keeps that corner, and
/// a new width moves the side across from it, not both sides half way.
///
/// First the whole cluster is moved, if that does it (`translate`). Otherwise points
/// (and radii) are ranked by how many of the sketch's equations they take part in
/// (`held_by`, by the first slot of each, which `group` gives). Those of the top rank are held still and the rest solved for; if that works, the
/// next rank is held as well, and so on while it still works. Returns whether any of it
/// worked (the values are then the last solution found; otherwise they are as they were)
/// and the iterations used. A fully defined cluster comes out where it would anyway.
#[allow(clippy::too_many_arguments)]
fn solve_holding_still(
    vars: &[u32],
    cluster_eqs: &[u32],
    group: &[u32],
    axis: &[u8],
    held_by: &[usize],
    dimensions: &[bool],
    collapsed: impl Fn(&[f64]) -> bool,
    eqs: &[Eq],
    vals: &mut [f64],
) -> (bool, usize) {
    // Only where what is unsatisfied is dimensions, given new values: geometry moved out
    // of its relations is put back the least distance, as before.
    let changed = |e: &u32| eqs[*e as usize].residual(vals).abs() > SKIP;
    if !cluster_eqs
        .iter()
        .filter(|e| changed(e))
        .all(|&e| dimensions[e as usize])
    {
        return (false, 0);
    }
    let start: Vec<f64> = vars.iter().map(|&v| vals[v as usize]).collect();
    if translate(vars, cluster_eqs, axis, eqs, vals, &start) {
        return (true, 0);
    }
    let score = |v: u32| held_by[group[v as usize] as usize];
    let mut ranks: Vec<usize> = vars.iter().map(|&v| score(v)).collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));
    ranks.dedup();
    let mut found: Option<Vec<f64>> = None;
    let mut iterations = 0;
    // Holding every rank still leaves nothing to solve for.
    for &rank in ranks.iter().take(ranks.len().saturating_sub(1)) {
        for (&v, &x) in vars.iter().zip(&start) {
            vals[v as usize] = x;
        }
        let free: Vec<u32> = vars.iter().copied().filter(|&v| score(v) < rank).collect();
        let free_set: std::collections::HashSet<u32> = free.iter().copied().collect();
        let (moving, held): (Vec<u32>, Vec<u32>) = cluster_eqs
            .iter()
            .partition(|&&e| eqs[e as usize].slots.iter().any(|s| free_set.contains(s)));
        if held
            .iter()
            .any(|&e| eqs[e as usize].residual(vals).abs() > SKIP)
        {
            break;
        }
        let mut problem = Problem::new(free, moving, eqs, Space::Equations);
        let outcome = problem.solve(eqs, vals, HARD);
        iterations += outcome.iterations;
        if !outcome.converged || collapsed(vals) {
            break;
        }
        found = Some(vars.iter().map(|&v| vals[v as usize]).collect());
    }
    let solved = found.is_some();
    for (&v, &x) in vars.iter().zip(found.as_ref().unwrap_or(&start)) {
        vals[v as usize] = x;
    }
    (solved, iterations)
}

/// Moves the whole cluster, unchanged in shape, as far as its dimensions say, where that
/// satisfies them: SolidWorks moves a profile dimensioned to the model as a whole
/// (Escutcheon Concealed's lock cut, 730 mm from the edge in one configuration and 55 mm
/// in the other). Points move by one offset; radii stay. Returns whether it worked; if
/// not, the values are as they were.
fn translate(
    vars: &[u32],
    cluster_eqs: &[u32],
    axis: &[u8],
    eqs: &[Eq],
    vals: &mut [f64],
    start: &[f64],
) -> bool {
    let place = |t: [f64; 2], vals: &mut [f64]| {
        for (&v, &x) in vars.iter().zip(start) {
            let a = axis[v as usize];
            vals[v as usize] = if a < 2 { x + t[a as usize] } else { x };
        }
    };
    let residuals = |vals: &[f64]| -> Vec<f64> {
        cluster_eqs
            .iter()
            .map(|&e| eqs[e as usize].residual(vals))
            .collect()
    };
    let mut t = [0.0f64; 2];
    let mut done = false;
    for _ in 0..20 {
        place(t, vals);
        let r = residuals(vals);
        if r.iter().all(|x| x.abs() <= peet_solve::TARGET) {
            done = true;
            break;
        }
        // Gauss-Newton on the two offsets, the derivatives by differences
        let mut columns = [vec![0.0; r.len()], vec![0.0; r.len()]];
        for (k, column) in columns.iter_mut().enumerate() {
            let mut moved = t;
            moved[k] += 1e-6;
            place(moved, vals);
            for (c, (now, was)) in column.iter_mut().zip(residuals(vals).iter().zip(&r)) {
                *c = (now - was) / 1e-6;
            }
        }
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
        let (a, b, c) = (
            dot(&columns[0], &columns[0]) + 1e-12,
            dot(&columns[0], &columns[1]),
            dot(&columns[1], &columns[1]) + 1e-12,
        );
        let (g0, g1) = (dot(&columns[0], &r), dot(&columns[1], &r));
        let det = a * c - b * b;
        if det.abs() < 1e-24 {
            break;
        }
        t[0] -= (c * g0 - b * g1) / det;
        t[1] -= (a * g1 - b * g0) / det;
    }
    if !done {
        place(t, vals);
        done = residuals(vals).iter().all(|x| x.abs() <= CONVERGED);
    }
    if !done {
        place([0.0, 0.0], vals);
    }
    done
}

/// Drags one cluster: alternates a linearised step towards the soft targets (phase a) with
/// a projection back onto the constraints (phase b), halving steps that don't help.
/// Returns whether the constraints are satisfied at the end, and the iterations used.
fn drag_cluster(
    drag: &mut Problem,
    hard: &mut Problem,
    eqs: &[Eq],
    vals: &mut [f64],
    soft: &[u32],
    mut feasible: bool,
) -> (bool, usize) {
    let soft_cost = |vals: &[f64]| -> f64 {
        soft.iter()
            .map(|&e| eqs[e as usize].residual(vals).powi(2))
            .sum()
    };
    let mut iterations = 0;
    let mut cost = soft_cost(vals);
    let mut dx = Vec::new();
    let mut x0 = Vec::new();
    let mut best_x = Vec::new();
    // Targets met to within TARGET (in mm, before weighting): done.
    let done = (peet_solve::TARGET * SOFT_WEIGHT).powi(2) * soft.len() as f64;
    for _ in 0..MAX_DRAG_STEPS {
        if iterations > DRAG_BUDGET || (feasible && cost <= done) {
            break;
        }
        iterations += 1;
        if !drag.gauss_newton_step(eqs, vals, &mut dx) {
            break;
        }
        let scale = drag
            .vars
            .iter()
            .map(|&v| vals[v as usize].abs())
            .fold(1.0, f64::max);
        let step = peet_solve::max_abs(&dx);
        if step <= 1e-13 * scale {
            break;
        }
        x0.clear();
        x0.extend(drag.vars.iter().map(|&v| vals[v as usize]));
        // Try t = 1, 1/2, 1/4, … and keep the best feasible result: on curved constraints
        // the full step can overshoot the closest point. With samples at 0, 1/2 and 1, also
        // try the minimum of the parabola through them.
        let mut best: Option<(f64, f64)> = None; // (t, cost)
        let mut samples = [f64::NAN; 2]; // cost at t = 1 and t = 1/2
        let mut project = |t: f64, vals: &mut [f64], iterations: &mut usize| -> Option<f64> {
            for ((&v, x), d) in drag.vars.iter().zip(&x0).zip(&dx) {
                vals[v as usize] = x + t * d;
            }
            let out = hard.solve(eqs, vals, if feasible { PROJECT } else { HARD });
            *iterations += out.iterations;
            out.converged.then(|| soft_cost(vals))
        };
        let mut t = 1.0;
        for k in 0..MAX_HALVINGS {
            if let Some(c) = project(t, vals, &mut iterations) {
                if let Some(sample) = samples.get_mut(k) {
                    *sample = c;
                }
                if best.is_some_and(|(_, b)| c >= b) {
                    break;
                }
                best = Some((t, c));
                best_x.clear();
                best_x.extend(drag.vars.iter().map(|&v| vals[v as usize]));
                if !feasible || c <= 0.25 * cost {
                    break;
                }
            }
            t *= 0.5;
        }
        if feasible && samples.iter().all(|c| c.is_finite()) {
            let (c0, c1, ch) = (cost, samples[0], samples[1]);
            let a = 2.0 * (c1 - 2.0 * ch + c0);
            let b = c1 - c0 - a;
            let t_min = -b / (2.0 * a);
            if a > 0.0
                && t_min > 0.02
                && t_min < 1.0
                && (t_min - 0.5).abs() > 0.02
                && let Some(c) = project(t_min, vals, &mut iterations)
                && best.is_none_or(|(_, b)| c < b)
            {
                best = Some((t_min, c));
                best_x.clear();
                best_x.extend(drag.vars.iter().map(|&v| vals[v as usize]));
            }
        }
        let accepted = best.filter(|&(_, c)| !feasible || c < cost);
        let x_final = if accepted.is_some() { &best_x } else { &x0 };
        for (&v, x) in drag.vars.iter().zip(x_final) {
            vals[v as usize] = *x;
        }
        let Some((t, _)) = accepted else {
            break;
        };
        feasible = true;
        let old = cost;
        cost = soft_cost(vals);
        if old - cost <= 1e-12 * old || t * step <= 1e-12 * scale {
            break;
        }
    }
    (feasible, iterations)
}
