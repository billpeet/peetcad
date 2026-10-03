//! Degrees of freedom and redundancy analysis.
//!
//! For each cluster, the Jacobian rows are equilibrated (scaled to unit length) and fed in
//! order (constraints by id, implicit arc equations last) to a Householder QR of `Jᵀ`
//! without pivoting. A row whose component orthogonal to the previous independent rows
//! vanishes is *dependent*: it is re-expressed as a combination of the independent rows,
//! and the constraints with non-negligible coefficients are the ones it clashes with. The
//! row is redundant if its dependency group is satisfied, conflicting otherwise. Processing
//! in id order means the constraint added last is the one reported.
//!
//! The rank gives the degrees of freedom, and the orthonormal basis `Q` of the row space
//! tells which variables are pinned down: variable `i` is fixed exactly when the unit
//! vector `e_i` lies in the row space (`|Qᵀ e_i| = 1`), i.e. has no null space component.
//!
//! A conflict does not always show up as a dependency at the current geometry (a triangle
//! with sides 3, 4 and 10 has independent rows everywhere except in the collinear limit).
//! When a cluster has unsatisfied equations that the rank test did not explain, it is
//! solved in least squares on a scratch copy: at a least squares minimum `Jᵀf = 0`, so the
//! equations with significant residuals are exactly a dependent, conflicting set.

use std::collections::{BTreeMap, BTreeSet};

use peet_math::tolerance;

use super::equations::{Eq, MAX_SLOTS, NONE, Owner};
use super::problem::{Options, Problem, Space};
use super::structure::{Structure, constraint_refs, curve_points};
use super::{Analysis, ConstraintStatus, Diagnosis, DofStatus};
use crate::sketch::{ConstraintId, EntityId, Geometry, Sketch};

/// Orthogonal remainder (of a unit row) below which a row counts as dependent.
const RANK_TOL: f64 = 1e-8;
/// Squared null space component below which a variable counts as fixed.
const NULL_TOL: f64 = 1e-10;
/// An equation is satisfied when its residual is below this (mm or radians).
const SATISFIED: f64 = tolerance::LINEAR;
/// Coefficients below this fraction of the largest are ignored when naming constraints.
const COEFF_TOL: f64 = 1e-6;

/// Result of the ordered rank-revealing factorisation of one cluster.
pub(crate) struct Rank {
    pub rank: usize,
    pub dependent: Vec<Dependent>,
    /// Per column: fixed (no null space component).
    pub fixed: Vec<bool>,
}

/// A row that depends on earlier rows.
pub(crate) struct Dependent {
    pub row: usize,
    /// Coefficients on the independent rows that reproduce it (equilibrated rows).
    pub coeffs: Vec<(usize, f64)>,
    /// The row was (numerically) zero: a structural dependency, e.g. through aliasing.
    pub zero: bool,
}

/// Ordered rank analysis of the `m × n` matrix given by sparse rows.
pub(crate) fn ordered_rank(rows: &[Vec<(usize, f64)>], n: usize) -> Rank {
    struct Reflector {
        k: usize,
        v: Vec<f64>,
        beta: f64,
    }
    let apply = |h: &Reflector, a: &mut [f64]| {
        let dot: f64 = h.v.iter().zip(&a[h.k..]).map(|(v, x)| v * x).sum();
        let s = h.beta * dot;
        for (x, v) in a[h.k..].iter_mut().zip(&h.v) {
            *x -= s * v;
        }
    };
    let mut refl: Vec<Reflector> = Vec::new();
    // R columns of independent rows (upper triangular), and which row each came from.
    let mut r_cols: Vec<Vec<f64>> = Vec::new();
    let mut indep_rows: Vec<usize> = Vec::new();
    let mut dependent = Vec::new();
    let mut a = vec![0.0; n];
    for (i, row) in rows.iter().enumerate() {
        a.fill(0.0);
        for &(c, v) in row {
            a[c] += v;
        }
        let norm = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-12 {
            dependent.push(Dependent {
                row: i,
                coeffs: Vec::new(),
                zero: true,
            });
            continue;
        }
        for x in &mut a {
            *x /= norm;
        }
        for h in &refl {
            apply(h, &mut a);
        }
        let k = refl.len();
        let sigma = a[k..].iter().map(|x| x * x).sum::<f64>().sqrt();
        if sigma <= RANK_TOL {
            // Back-substitute R c = a[..k].
            let mut c = a[..k].to_vec();
            for j in (0..k).rev() {
                c[j] /= r_cols[j][j];
                let cj = c[j];
                for (ci, rj) in c[..j].iter_mut().zip(&r_cols[j]) {
                    *ci -= rj * cj;
                }
            }
            let max = c.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            let coeffs = c
                .iter()
                .enumerate()
                .filter(|(_, x)| x.abs() > COEFF_TOL * max)
                .map(|(j, &x)| (indep_rows[j], x))
                .collect();
            dependent.push(Dependent {
                row: i,
                coeffs,
                zero: false,
            });
            continue;
        }
        let alpha = if a[k] > 0.0 { -sigma } else { sigma };
        let mut v = a[k..].to_vec();
        v[0] -= alpha;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        let mut col = a[..k].to_vec();
        col.push(alpha);
        r_cols.push(col);
        indep_rows.push(i);
        refl.push(Reflector {
            k,
            v,
            beta: 2.0 / vv,
        });
    }
    // Row space basis Q_r = H_0 … H_{r-1} [I; 0]; fixed columns have |row of Q_r| = 1.
    let r = refl.len();
    let mut q = vec![vec![0.0; n]; r];
    for (j, col) in q.iter_mut().enumerate() {
        col[j] = 1.0;
        for h in refl.iter().rev() {
            apply(h, col);
        }
    }
    let fixed = (0..n)
        .map(|i| {
            let s: f64 = q.iter().map(|col| col[i] * col[i]).sum();
            1.0 - s <= NULL_TOL
        })
        .collect();
    Rank {
        rank: r,
        dependent,
        fixed,
    }
}

/// Points an equation refers to (for naming coincidences behind degenerate equations).
fn owner_points(sketch: &Sketch, owner: Owner) -> Vec<EntityId> {
    let mut out = Vec::new();
    let mut add = |id: EntityId| match sketch.entity(id).map(|e| &e.geometry) {
        Some(Geometry::Point { .. }) => out.push(id),
        Some(g) => out.extend_from_slice(&curve_points(g)),
        None => {}
    };
    match owner {
        Owner::Constraint(c) => {
            if let Some(con) = sketch.constraint(ConstraintId(c)) {
                let (refs, n, _) = constraint_refs(&con.kind);
                for &r in &refs[..n] {
                    add(r);
                }
            }
        }
        Owner::Arc(e) => add(EntityId(e)),
        Owner::Soft => {}
    }
    out
}

#[derive(Default)]
struct Diags {
    map: BTreeMap<u32, (ConstraintStatus, BTreeSet<u32>)>,
}

impl Diags {
    fn add(&mut self, cid: u32, status: ConstraintStatus, involved: impl IntoIterator<Item = u32>) {
        let e = self
            .map
            .entry(cid)
            .or_insert((ConstraintStatus::Redundant, BTreeSet::new()));
        if status == ConstraintStatus::Conflicting {
            e.0 = status;
        }
        e.1.extend(involved.into_iter().filter(|&c| c != cid));
    }

    /// Records a dependency found among equations. `members` are the other equations
    /// involved; `extra` are constraint ids implied by aliasing.
    fn record(&mut self, eqs: &[Eq], dep: u32, members: &[u32], extra: &[u32], satisfied: bool) {
        let status = if satisfied {
            ConstraintStatus::Redundant
        } else {
            ConstraintStatus::Conflicting
        };
        let mut involved: Vec<u32> = members
            .iter()
            .filter_map(|&e| match eqs[e as usize].owner {
                Owner::Constraint(c) => Some(c),
                _ => None,
            })
            .chain(extra.iter().copied())
            .collect();
        involved.sort_unstable();
        involved.dedup();
        match eqs[dep as usize].owner {
            Owner::Constraint(c) => self.add(c, status, involved),
            // Implicit arc equations are never reported themselves. Implied by the user's
            // constraints: harmless, ignore. Contradicted: blame the latest constraint.
            Owner::Arc(_) if !satisfied => {
                if let Some(&last) = involved.last() {
                    self.add(last, status, involved[..involved.len() - 1].iter().copied());
                }
            }
            _ => {}
        }
    }

    fn covers(&self, cid: u32) -> bool {
        self.map.contains_key(&cid) || self.map.values().any(|(_, inv)| inv.contains(&cid))
    }
}

/// Full analysis at the current values.
pub(crate) fn analyze(st: &mut Structure, sketch: &Sketch, vals: &[f64], eqs: &[Eq]) -> Analysis {
    let mut diags = Diags::default();
    let mut fixed_var = vec![false; st.n_vars];
    let mut dof = 0usize;

    for &(cid, a, b) in &st.alias_cycles {
        diags.add(cid, ConstraintStatus::Redundant, st.alias_path(a, b));
    }

    let residual = |e: u32| eqs[e as usize].residual(vals);
    let mut local = vec![NONE; st.n_vars];
    let mut g = [0.0; MAX_SLOTS];
    for ci in 0..st.clusters.len() {
        let cluster = &st.clusters[ci];
        for (i, &v) in cluster.vars.iter().enumerate() {
            local[v as usize] = i as u32;
        }
        let rows: Vec<Vec<(usize, f64)>> = cluster
            .eqs
            .iter()
            .map(|&e| {
                let eq = &eqs[e as usize];
                eq.eval(vals, &mut g);
                eq.slots()
                    .iter()
                    .zip(&g)
                    .filter(|(s, _)| (**s as usize) < st.n_vars)
                    .map(|(&s, &d)| (local[s as usize] as usize, d))
                    .collect()
            })
            .collect();
        let rank = ordered_rank(&rows, cluster.vars.len());
        dof += cluster.vars.len() - rank.rank;
        for (i, &v) in cluster.vars.iter().enumerate() {
            fixed_var[v as usize] = rank.fixed[i];
        }
        for d in &rank.dependent {
            let dep = cluster.eqs[d.row];
            let members: Vec<u32> = d.coeffs.iter().map(|&(r, _)| cluster.eqs[r]).collect();
            let extra = if d.zero {
                st.alias_involvement(&owner_points(sketch, eqs[dep as usize].owner))
            } else {
                Vec::new()
            };
            let satisfied = std::iter::once(dep)
                .chain(members.iter().copied())
                .all(|e| residual(e).abs() <= SATISFIED);
            diags.record(eqs, dep, &members, &extra, satisfied);
        }

        // Unsatisfied equations the rank test did not explain: least squares fallback.
        let unexplained = cluster.eqs.iter().any(|&e| {
            residual(e).abs() > SATISFIED
                && match eqs[e as usize].owner {
                    Owner::Constraint(c) => !diags.covers(c),
                    _ => true,
                }
        });
        if unexplained {
            let cluster = &mut st.clusters[ci];
            let problem = cluster.problem.get_or_insert_with(|| {
                Problem::new(
                    cluster.vars.clone(),
                    cluster.eqs.clone(),
                    eqs,
                    Space::Equations,
                )
            });
            let mut scratch = vals.to_vec();
            problem.solve(
                eqs,
                &mut scratch,
                Options {
                    max_iter: 200,
                    hard: true,
                    dogleg: false,
                },
            );
            let mut f = Vec::new();
            problem.residuals(eqs, &scratch, &mut f);
            let max = f.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            if max > SATISFIED {
                let set: Vec<u32> = problem
                    .eqs
                    .iter()
                    .zip(&f)
                    .filter(|(_, r)| r.abs() > 1e-3 * max)
                    .map(|(&e, _)| e)
                    .collect();
                // Blame the latest user constraint in the set.
                let latest = set
                    .iter()
                    .copied()
                    .filter(|&e| matches!(eqs[e as usize].owner, Owner::Constraint(_)))
                    .max_by_key(|&e| match eqs[e as usize].owner {
                        Owner::Constraint(c) => c,
                        _ => 0,
                    });
                if let Some(dep) = latest {
                    let others: Vec<u32> = set.iter().copied().filter(|&e| e != dep).collect();
                    diags.record(eqs, dep, &others, &[], false);
                }
            }
        }
    }

    // Equations with no variables at all: constant, so always dependent.
    for &e in &st.const_eqs {
        let extra = st.alias_involvement(&owner_points(sketch, eqs[e as usize].owner));
        let satisfied = residual(e).abs() <= SATISFIED;
        diags.record(eqs, e, &[], &extra, satisfied);
    }

    // ---- Results ----
    let mut constraints = vec![ConstraintStatus::Ok; sketch.constraint_capacity()];
    let mut diagnoses = Vec::new();
    let mut over = vec![false; sketch.entity_capacity()];
    let mark_over = |sketch: &Sketch, cid: u32, over: &mut Vec<bool>| {
        if let Some(con) = sketch.constraint(ConstraintId(cid)) {
            let (refs, n, _) = constraint_refs(&con.kind);
            for &r in &refs[..n] {
                over[r.0 as usize] = true;
                if let Some(e) = sketch.entity(r) {
                    for p in curve_points(&e.geometry).iter() {
                        over[p.0 as usize] = true;
                    }
                }
            }
        }
    };
    for (&cid, (status, involved)) in &diags.map {
        constraints[cid as usize] = *status;
        mark_over(sketch, cid, &mut over);
        for &i in involved {
            mark_over(sketch, i, &mut over);
        }
        diagnoses.push(Diagnosis {
            constraint: ConstraintId(cid),
            status: *status,
            involved: involved.iter().map(|&c| ConstraintId(c)).collect(),
        });
    }

    let point_fixed = |id: EntityId| -> bool {
        let ci = st.class_of[id.0 as usize];
        if ci == NONE {
            return false;
        }
        let s = st.classes[ci as usize].slot as usize;
        s >= st.n_vars || (fixed_var[s] && fixed_var[s + 1])
    };
    let mut entities = vec![DofStatus::Under; sketch.entity_capacity()];
    for (id, e) in sketch.entities() {
        let i = id.0 as usize;
        entities[i] = if over[i] {
            DofStatus::Over
        } else {
            let fully = match e.geometry {
                Geometry::Point { .. } => point_fixed(id),
                ref g => {
                    let pts = curve_points(g);
                    let r = st.map.radius[i];
                    pts.iter().all(|&p| point_fixed(p))
                        && (r == NONE || (r as usize) >= st.n_vars || fixed_var[r as usize])
                        && !pts.is_empty()
                }
            };
            if fully {
                DofStatus::Fully
            } else {
                DofStatus::Under
            }
        };
    }

    Analysis {
        dof,
        entities,
        constraints,
        diagnoses,
    }
}
