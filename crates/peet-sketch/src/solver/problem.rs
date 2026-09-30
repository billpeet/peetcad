//! One cluster's nonlinear least squares problem, solved by Levenberg–Marquardt with a
//! dogleg trust-region fallback.
//!
//! **Minimum norm steps.** Constraint solves work in equation space,
//! `dx = Jᵀ (J Jᵀ + μD)⁻¹ (-f)` with `D = diag(J Jᵀ)` (Marquardt scaling, which amounts to
//! equilibrating the rows). As `μ → 0` this is the minimum norm Gauss–Newton step, and
//! since `dx` is always in the row space of `J`, geometry the equations leave free does
//! not drift at all: under-constrained sketches move as little as possible.
//!
//! Redundant equations make `J Jᵀ` singular, and away from the solution their
//! linearisations are inconsistent (to second order), which would blow the step up. The
//! sparse Cholesky therefore drops rows whose pivot vanishes: the step then solves an
//! independent subset of the equations, which is still exact for a consistent system.
//!
//! Drag steps (constraints plus weighted soft targets, usually over-determined in the
//! directions the constraints fix) work in variable space instead,
//! `(JᵀJ + μ max(D) I) dx = -Jᵀf`, whose right-hand side is always consistent: that gives the
//! true weighted least squares compromise between constraints and targets.
//!
//! The Jacobian is sparse (at most eight entries per row), so the Gram matrix is factorised
//! by the cached sparse Cholesky in [`super::sparse`].

use std::collections::HashMap;

use super::equations::{Eq, MAX_SLOTS, NONE};
use super::sparse::{Damping, Gram};

/// Stop iterating once every residual is below this (mm or radians).
pub(crate) const TARGET: f64 = 1e-11;
/// A solve counts as converged (and its result is kept) when every residual is within the
/// model tolerance. Regular configurations converge quadratically to [`TARGET`]; this only
/// matters for singular ones (for example constraints that meet tangentially), where
/// Newton-type methods converge slowly and stall somewhere below it.
pub(crate) const CONVERGED: f64 = peet_math::tolerance::LINEAR;
/// Clusters whose residuals are all below this are considered solved and left alone.
/// Anything that does get solved goes on to [`TARGET`]; but re-solving a singular cluster
/// that stalled within tolerance would only nudge its (then poorly determined) geometry.
pub(crate) const SKIP: f64 = CONVERGED;
/// Below this, rejected steps are rounding noise: stop instead of raising the damping.
const ROUNDING: f64 = 1e-10;
/// Iterations over which LM (and dogleg) must at least halve the cost to keep going.
const STALL_WINDOW: usize = 10;
/// Damping floor (relative to each diagonal entry) in equation space. Far below
/// [`super::sparse::DROP`], so dependent rows are recognised and dropped.
const MU_FLOOR_EQ: f64 = 1e-15;
/// Damping floor in variable space. Above `DROP`, so no variable is ever dropped; it keeps
/// the null space drift of under-determined drag steps around `ε/μ ≈ 1e-7` of the step.
const MU_FLOOR_VAR: f64 = 1e-9;
/// Initial damping for a large starting residual (an edit): small enough that steps are
/// Gauss–Newton in all well-conditioned directions, but it keeps the first steps from
/// shooting along nearly singular directions into a far-away (often degenerate) solution.
/// Near a solution (drag projections, polishing) the damping starts much lower, down to
/// [`MU_START_MIN`], so convergence is quadratic from the first step.
const MU_START: f64 = 1e-4;
const MU_START_MIN: f64 = 1e-10;
/// Residual (mm) at and above which the full [`MU_START`] is used.
const MU_START_RESIDUAL: f64 = 1e-2;

/// Which Gram matrix a problem factorises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Space {
    /// `J Jᵀ` (one row per equation): minimum norm steps.
    Equations,
    /// `JᵀJ` (one row per variable): weighted least squares steps.
    Variables,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Options {
    pub max_iter: usize,
    /// Hard equations only: stop at [`TARGET`] and report convergence. Otherwise (soft
    /// drag targets present) iterate to a least squares minimum.
    pub hard: bool,
    /// Fall back to dogleg if LM does not converge.
    pub dogleg: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Outcome {
    pub iterations: usize,
    pub converged: bool,
    pub max_residual: f64,
}

#[derive(Debug)]
pub(crate) struct Problem {
    /// Variable slots (local index = position).
    pub vars: Vec<u32>,
    /// Equation indices (row = position).
    pub eqs: Vec<u32>,
    row_ptr: Vec<u32>,
    /// Local column of each Jacobian entry.
    cols: Vec<u32>,
    /// Per row: the Jacobian entry each equation slot contributes to, or `NONE`.
    slot_entry: Vec<[u32; MAX_SLOTS]>,
    gram: Gram,
    space: Space,
    jv: Vec<f64>,
    f: Vec<f64>,
    f_new: Vec<f64>,
    y: Vec<f64>,
    dx: Vec<f64>,
    g: Vec<f64>,
    tmp: Vec<f64>,
    x_save: Vec<f64>,
}

impl Problem {
    pub fn new(vars: Vec<u32>, eqs: Vec<u32>, all: &[Eq], space: Space) -> Self {
        let local: HashMap<u32, u32> = vars
            .iter()
            .enumerate()
            .map(|(i, &v)| (v, i as u32))
            .collect();
        let mut row_ptr = vec![0u32];
        let mut cols: Vec<u32> = Vec::new();
        let mut slot_entry = Vec::with_capacity(eqs.len());
        for &e in &eqs {
            let eq = &all[e as usize];
            let start = cols.len();
            let mut map = [NONE; MAX_SLOTS];
            for (k, s) in eq.slots().iter().enumerate() {
                if let Some(&c) = local.get(s) {
                    let pos = match cols[start..].iter().position(|&x| x == c) {
                        Some(p) => start + p,
                        None => {
                            cols.push(c);
                            cols.len() - 1
                        }
                    };
                    map[k] = pos as u32;
                }
            }
            slot_entry.push(map);
            row_ptr.push(cols.len() as u32);
        }
        let gram = match space {
            Space::Equations => {
                // J Jᵀ: one record per variable listing (row, entry).
                let mut records: Vec<Vec<(u32, u32)>> = vec![Vec::new(); vars.len()];
                for r in 0..eqs.len() {
                    for e in row_ptr[r]..row_ptr[r + 1] {
                        records[cols[e as usize] as usize].push((r as u32, e));
                    }
                }
                Gram::new(eqs.len(), &records)
            }
            Space::Variables => {
                // JᵀJ: one record per row listing (column, entry).
                let records: Vec<Vec<(u32, u32)>> = (0..eqs.len())
                    .map(|r| {
                        (row_ptr[r]..row_ptr[r + 1])
                            .map(|e| (cols[e as usize], e))
                            .collect()
                    })
                    .collect();
                Gram::new(vars.len(), &records)
            }
        };
        let (m, n, nnz) = (eqs.len(), vars.len(), cols.len());
        Self {
            vars,
            eqs,
            row_ptr,
            cols,
            slot_entry,
            gram,
            space,
            jv: vec![0.0; nnz],
            f: vec![0.0; m],
            f_new: vec![0.0; m],
            y: vec![0.0; m],
            dx: vec![0.0; n],
            g: vec![0.0; n],
            tmp: vec![0.0; m],
            x_save: vec![0.0; n],
        }
    }

    /// Residuals and Jacobian at `vals`.
    fn eval_jac(&mut self, all: &[Eq], vals: &[f64]) {
        let mut g = [0.0; MAX_SLOTS];
        self.jv.fill(0.0);
        for (r, &e) in self.eqs.iter().enumerate() {
            let eq = &all[e as usize];
            self.f[r] = eq.eval(vals, &mut g);
            let map = &self.slot_entry[r];
            for k in 0..eq.n as usize {
                if map[k] != NONE {
                    self.jv[map[k] as usize] += g[k];
                }
            }
        }
    }

    fn eval_f(eqs: &[u32], all: &[Eq], vals: &[f64], out: &mut [f64]) {
        for (r, &e) in eqs.iter().enumerate() {
            out[r] = all[e as usize].residual(vals);
        }
    }

    /// Residuals at `vals` (rows in problem order).
    pub fn residuals(&self, all: &[Eq], vals: &[f64], out: &mut Vec<f64>) {
        out.resize(self.eqs.len(), 0.0);
        Self::eval_f(&self.eqs, all, vals, out);
    }

    /// `out = Jᵀ y`.
    fn jt(&self, y: &[f64], out: &mut [f64]) {
        out.fill(0.0);
        for (r, &yr) in y.iter().enumerate().take(self.eqs.len()) {
            for e in self.row_ptr[r] as usize..self.row_ptr[r + 1] as usize {
                out[self.cols[e] as usize] += self.jv[e] * yr;
            }
        }
    }

    /// `out = J v`.
    fn jmul(&self, v: &[f64], out: &mut [f64]) {
        for (r, o) in out.iter_mut().enumerate().take(self.eqs.len()) {
            let mut s = 0.0;
            for e in self.row_ptr[r] as usize..self.row_ptr[r + 1] as usize {
                s += self.jv[e] * v[self.cols[e] as usize];
            }
            *o = s;
        }
    }

    fn mu_floor(&self) -> f64 {
        match self.space {
            Space::Equations => MU_FLOOR_EQ,
            Space::Variables => MU_FLOOR_VAR,
        }
    }

    /// Damped Gauss–Newton step for damping `mu`, into `self.dx`.
    fn step(&mut self, mu: f64) -> bool {
        let damping = match self.space {
            Space::Equations => Damping::Scaled,
            Space::Variables => Damping::Uniform,
        };
        if !self.gram.factor(&self.jv, mu, damping) {
            return false;
        }
        match self.space {
            Space::Equations => {
                // dx = Jᵀ (J Jᵀ + μD)⁻¹ (-f)
                for (y, f) in self.y.iter_mut().zip(&self.f) {
                    *y = -f;
                }
                self.gram.solve(&mut self.y);
                let y = std::mem::take(&mut self.y);
                let mut dx = std::mem::take(&mut self.dx);
                self.jt(&y, &mut dx);
                self.y = y;
                self.dx = dx;
            }
            Space::Variables => {
                // (JᵀJ + μ max(D) I) dx = -Jᵀf
                let f = std::mem::take(&mut self.f);
                let mut dx = std::mem::take(&mut self.dx);
                self.jt(&f, &mut dx);
                for d in &mut dx {
                    *d = -*d;
                }
                self.gram.solve(&mut dx);
                self.f = f;
                self.dx = dx;
            }
        }
        self.dx.iter().all(|x| x.is_finite())
    }

    fn save_x(&mut self, vals: &[f64]) {
        for (s, &v) in self.x_save.iter_mut().zip(&self.vars) {
            *s = vals[v as usize];
        }
    }

    fn restore_x(&self, vals: &mut [f64]) {
        for (s, &v) in self.x_save.iter().zip(&self.vars) {
            vals[v as usize] = *s;
        }
    }

    fn apply(&self, vals: &mut [f64], dx: &[f64]) {
        for (d, &v) in dx.iter().zip(&self.vars) {
            vals[v as usize] += d;
        }
    }

    fn x_scale(&self, vals: &[f64]) -> f64 {
        self.vars
            .iter()
            .map(|&v| vals[v as usize].abs())
            .fold(1.0, f64::max)
    }

    /// `½|f + J dx|²` with the current `f`, `J` and `self.dx`.
    fn model_cost(&mut self) -> f64 {
        let dx = std::mem::take(&mut self.dx);
        let mut tmp = std::mem::take(&mut self.tmp);
        self.jmul(&dx, &mut tmp);
        let c = tmp
            .iter()
            .zip(&self.f)
            .map(|(j, f)| (j + f) * (j + f))
            .sum::<f64>()
            * 0.5;
        self.dx = dx;
        self.tmp = tmp;
        c
    }

    /// The minimum norm Gauss–Newton step at `vals` (lightly damped), written to `dx`
    /// (one entry per problem variable). Returns false if no finite step was found.
    pub fn gauss_newton_step(&mut self, all: &[Eq], vals: &[f64], dx: &mut Vec<f64>) -> bool {
        dx.clear();
        dx.resize(self.vars.len(), 0.0);
        if self.eqs.is_empty() {
            return true;
        }
        self.eval_jac(all, vals);
        let mut mu = self.mu_floor();
        for _ in 0..8 {
            if self.step(mu) {
                dx.copy_from_slice(&self.dx);
                return true;
            }
            mu *= 100.0;
        }
        false
    }

    /// Solves the problem, updating the variables in `vals` in place.
    pub fn solve(&mut self, all: &[Eq], vals: &mut [f64], opts: Options) -> Outcome {
        let mut out = Outcome::default();
        if self.eqs.is_empty() {
            out.converged = true;
            return out;
        }
        self.eval_jac(all, vals);
        let mut cost = half_sq(&self.f);
        if opts.hard && max_abs(&self.f) <= TARGET {
            out.converged = true;
            out.max_residual = max_abs(&self.f);
            return out;
        }
        let r0 = max_abs(&self.f) / MU_START_RESIDUAL;
        let mut lambda = (MU_START * r0 * r0).clamp(MU_START_MIN, MU_START);
        let mut nu = 2.0;
        let mut done = false;
        // Stall detection: the cost must halve every STALL_WINDOW iterations.
        let mut checkpoint = (0usize, cost);
        while out.iterations < opts.max_iter {
            out.iterations += 1;
            if out.iterations - checkpoint.0 >= STALL_WINDOW {
                if cost > 0.5 * checkpoint.1 {
                    break;
                }
                checkpoint = (out.iterations, cost);
            }
            lambda = lambda.max(self.mu_floor());
            if !self.step(lambda) {
                lambda *= 10.0;
                continue;
            }
            let dx_max = max_abs(&self.dx);
            if dx_max <= 1e-15 * self.x_scale(vals) {
                break;
            }
            let pred = cost - self.model_cost();
            self.save_x(vals);
            let dx = std::mem::take(&mut self.dx);
            self.apply(vals, &dx);
            self.dx = dx;
            Self::eval_f(&self.eqs, all, vals, &mut self.f_new);
            let new_cost = half_sq(&self.f_new);
            let rho = if pred > 0.0 {
                (cost - new_cost) / pred
            } else {
                -1.0
            };
            // Already converged and the step barely helps (degenerate configurations
            // converge only linearly): stop rather than nudge the geometry for nothing.
            if opts.hard && max_abs(&self.f) <= CONVERGED && new_cost > 0.9 * cost {
                self.restore_x(vals);
                break;
            }
            if new_cost < cost && rho > 1e-4 {
                let old = cost;
                cost = new_cost;
                self.eval_jac(all, vals);
                lambda *= (1.0 - (2.0 * rho - 1.0).powi(3)).max(1.0 / 3.0);
                nu = 2.0;
                if opts.hard && max_abs(&self.f) <= TARGET {
                    done = true;
                    break;
                }
                if !opts.hard && old - cost <= 1e-14 * old.max(1e-300) {
                    break;
                }
            } else {
                self.restore_x(vals);
                // Down at rounding level: don't fight it.
                if opts.hard && max_abs(&self.f) <= ROUNDING {
                    break;
                }
                lambda *= nu;
                nu *= 2.0;
                if nu > 1e12 {
                    break;
                }
            }
        }
        if opts.hard && !done && opts.dogleg {
            out.iterations += self.dogleg(all, vals, opts.max_iter);
        }
        self.eval_jac(all, vals);
        out.max_residual = max_abs(&self.f);
        out.converged = out.max_residual <= CONVERGED;
        out
    }

    /// Powell's dogleg trust-region method, from the current point. Returns iterations.
    fn dogleg(&mut self, all: &[Eq], vals: &mut [f64], max_iter: usize) -> usize {
        self.eval_jac(all, vals);
        let mut cost = half_sq(&self.f);
        let mut delta = 0.1 * self.x_scale(vals);
        let mut it = 0;
        let mut checkpoint = (0usize, cost);
        while it < max_iter && max_abs(&self.f) > TARGET {
            it += 1;
            if it - checkpoint.0 >= STALL_WINDOW {
                if cost > 0.5 * checkpoint.1 {
                    break;
                }
                checkpoint = (it, cost);
            }
            // Steepest descent: g = Jᵀf, Cauchy step length α = |g|² / |Jg|².
            let f = std::mem::take(&mut self.f);
            let mut g = std::mem::take(&mut self.g);
            self.jt(&f, &mut g);
            self.f = f;
            let mut tmp = std::mem::take(&mut self.tmp);
            self.jmul(&g, &mut tmp);
            let g2: f64 = g.iter().map(|x| x * x).sum();
            let jg2: f64 = tmp.iter().map(|x| x * x).sum();
            self.tmp = tmp;
            if g2 == 0.0 || jg2 == 0.0 {
                self.g = g;
                break;
            }
            let alpha = g2 / jg2;
            // Gauss–Newton (minimum norm) step.
            let floor = self.mu_floor();
            let mut have_gn = self.step(floor);
            if !have_gn {
                have_gn = self.step(floor * 1e4);
            }
            let gn_norm = if have_gn {
                norm(&self.dx)
            } else {
                f64::INFINITY
            };
            if gn_norm > delta {
                let g_norm = g2.sqrt();
                if alpha * g_norm >= delta || !have_gn {
                    for (d, gi) in self.dx.iter_mut().zip(&g) {
                        *d = -gi * delta / g_norm;
                    }
                } else {
                    // Point on the segment from the Cauchy point to the GN point at |h| = Δ.
                    let a: Vec<f64> = g.iter().map(|gi| -alpha * gi).collect();
                    let b: Vec<f64> = self.dx.iter().zip(&a).map(|(n, a)| n - a).collect();
                    let bb: f64 = b.iter().map(|x| x * x).sum();
                    let ab: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
                    let aa: f64 = a.iter().map(|x| x * x).sum();
                    let c = aa - delta * delta;
                    let beta = if bb > 0.0 {
                        (-ab + (ab * ab - bb * c).max(0.0).sqrt()) / bb
                    } else {
                        0.0
                    };
                    for ((d, ai), bi) in self.dx.iter_mut().zip(&a).zip(&b) {
                        *d = ai + beta * bi;
                    }
                }
            }
            self.g = g;
            let h_norm = norm(&self.dx);
            let pred = cost - self.model_cost();
            self.save_x(vals);
            let dx = std::mem::take(&mut self.dx);
            self.apply(vals, &dx);
            self.dx = dx;
            Self::eval_f(&self.eqs, all, vals, &mut self.f_new);
            let new_cost = half_sq(&self.f_new);
            let rho = if pred > 0.0 {
                (cost - new_cost) / pred
            } else {
                -1.0
            };
            if new_cost < cost && rho > 0.0 {
                cost = new_cost;
                self.eval_jac(all, vals);
            } else {
                self.restore_x(vals);
            }
            if rho > 0.75 {
                delta = delta.max(3.0 * h_norm);
            } else if rho < 0.25 {
                delta = 0.5 * h_norm.min(delta);
            }
            if delta <= 1e-15 * self.x_scale(vals) {
                break;
            }
        }
        it
    }
}

fn half_sq(f: &[f64]) -> f64 {
    0.5 * f.iter().map(|x| x * x).sum::<f64>()
}

pub(crate) fn max_abs(f: &[f64]) -> f64 {
    f.iter().fold(0.0, |m, x| {
        if x.is_nan() {
            f64::INFINITY
        } else {
            m.max(x.abs())
        }
    })
}

fn norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}
