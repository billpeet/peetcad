//! The numerical core of PeetCAD's constraint solvers.
//!
//! A constraint problem is a list of residual equations over a flat array of values.
//! Each equation reads a few *slots* of the array and returns its residual and the
//! gradient with respect to those slots ([`Equation`]). A [`Problem`] is a set of
//! equations and the slots that are unknown in them; it solves them by damped least
//! squares (Levenberg–Marquardt with minimum norm steps, falling back to Powell's
//! dogleg), factorising its sparse Gram matrix with a cached Cholesky ([`sparse`]).
//!
//! What the values mean is the caller's business. The sketcher (`peet-sketch`) uses two
//! slots per point and one per radius; an assembly uses six per component. Splitting a
//! system into independent clusters, choosing which slots are unknown, and explaining
//! conflicts stay with the caller too: they depend on what the equations are about.
//!
//! Residuals should be scaled so that Jacobian rows have entries of order one
//! (millimetres for lengths, radians or sines for angles): the tolerances
//! ([`TARGET`], [`CONVERGED`]) are absolute.

mod problem;
pub mod sparse;

pub use problem::{CONVERGED, Options, Outcome, Problem, SKIP, Space, TARGET, max_abs};

/// Marks "no slot" / "no entry".
pub const NONE: u32 = u32::MAX;

/// Most slots one equation can read.
pub const MAX_SLOTS: usize = 12;

/// One residual equation over the value array.
pub trait Equation {
    /// The slots the equation reads, at most [`MAX_SLOTS`]. A slot may be listed twice
    /// (two ends that share a variable); its gradients are then summed.
    fn slots(&self) -> &[u32];

    /// The residual at `vals`, writing the gradient with respect to each slot of
    /// [`Equation::slots`] into the start of `grad`, in the same order.
    fn eval(&self, vals: &[f64], grad: &mut [f64; MAX_SLOTS]) -> f64;

    /// The residual only.
    fn residual(&self, vals: &[f64]) -> f64 {
        let mut grad = [0.0; MAX_SLOTS];
        self.eval(vals, &mut grad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Σ c[k] v[slot k] - target`, or `|p - q| - target` for two points in the plane.
    enum Eq {
        Linear(Vec<u32>, Vec<f64>, f64),
        Distance([u32; 4], f64),
    }

    impl Equation for Eq {
        fn slots(&self) -> &[u32] {
            match self {
                Self::Linear(s, ..) => s,
                Self::Distance(s, _) => s,
            }
        }

        fn eval(&self, v: &[f64], g: &mut [f64; MAX_SLOTS]) -> f64 {
            match self {
                Self::Linear(s, c, target) => {
                    let mut r = -target;
                    for (k, (slot, c)) in s.iter().zip(c).enumerate() {
                        r += c * v[*slot as usize];
                        g[k] = *c;
                    }
                    r
                }
                Self::Distance(s, target) => {
                    let at = |k: usize| v[s[k] as usize];
                    let (dx, dy) = (at(2) - at(0), at(3) - at(1));
                    let d = dx.hypot(dy).max(1e-300);
                    *g = [0.0; MAX_SLOTS];
                    g[0] = -dx / d;
                    g[1] = -dy / d;
                    g[2] = dx / d;
                    g[3] = dy / d;
                    d - target
                }
            }
        }
    }

    const HARD: Options = Options {
        max_iter: 100,
        hard: true,
        dogleg: true,
    };

    #[test]
    fn a_triangle_is_solved_with_the_least_movement() {
        // Three points; the first is fixed (its slots are not unknowns). The sides are
        // to be 30, 40 and 50.
        let eqs = [
            Eq::Distance([0, 1, 2, 3], 30.0),
            Eq::Distance([2, 3, 4, 5], 40.0),
            Eq::Distance([4, 5, 0, 1], 50.0),
        ];
        let mut vals = vec![0.0, 0.0, 25.0, 1.0, 27.0, 33.0];
        let mut p = Problem::new(vec![2, 3, 4, 5], vec![0, 1, 2], &eqs, Space::Equations);
        let out = p.solve(&eqs, &mut vals, HARD);
        assert!(out.converged, "{out:?}");
        assert!(out.max_residual <= TARGET);
        let d = |a: usize, b: usize| (vals[b] - vals[a]).hypot(vals[b + 1] - vals[a + 1]);
        assert!((d(0, 2) - 30.0).abs() < 1e-9);
        assert!((d(2, 4) - 40.0).abs() < 1e-9);
        assert!((d(4, 0) - 50.0).abs() < 1e-9);
        // The fixed point stayed, and the free rotation was not used up: the second point
        // is still close to where it started.
        assert_eq!((vals[0], vals[1]), (0.0, 0.0));
        assert!((vals[3] - 1.0).abs() < 3.0, "{vals:?}");
    }

    #[test]
    fn redundant_equations_are_dropped_and_conflicts_do_not_converge() {
        // x + y = 3 twice (redundant) and x - y = 1.
        let eqs = [
            Eq::Linear(vec![0, 1], vec![1.0, 1.0], 3.0),
            Eq::Linear(vec![0, 1], vec![1.0, 1.0], 3.0),
            Eq::Linear(vec![0, 1], vec![1.0, -1.0], 1.0),
        ];
        let mut vals = vec![10.0, -4.0];
        let mut p = Problem::new(vec![0, 1], vec![0, 1, 2], &eqs, Space::Equations);
        assert!(p.solve(&eqs, &mut vals, HARD).converged);
        assert!(
            (vals[0] - 2.0).abs() < 1e-9 && (vals[1] - 1.0).abs() < 1e-9,
            "{vals:?}"
        );

        // x + y = 3 and x + y = 4 can't both hold.
        let eqs = [
            Eq::Linear(vec![0, 1], vec![1.0, 1.0], 3.0),
            Eq::Linear(vec![0, 1], vec![1.0, 1.0], 4.0),
        ];
        let mut vals = vec![0.0, 0.0];
        let mut p = Problem::new(vec![0, 1], vec![0, 1], &eqs, Space::Equations);
        let out = p.solve(&eqs, &mut vals, HARD);
        assert!(!out.converged && out.max_residual > 0.1, "{out:?}");
    }

    #[test]
    fn soft_targets_are_a_least_squares_compromise() {
        // x = 0 and x = 10, equally weighted, in variable space: x ends in the middle.
        let eqs = [
            Eq::Linear(vec![0], vec![1.0], 0.0),
            Eq::Linear(vec![0], vec![1.0], 10.0),
        ];
        let mut vals = vec![1.0];
        let mut p = Problem::new(vec![0], vec![0, 1], &eqs, Space::Variables);
        p.solve(
            &eqs,
            &mut vals,
            Options {
                max_iter: 50,
                hard: false,
                dogleg: false,
            },
        );
        assert!((vals[0] - 5.0).abs() < 1e-6, "{vals:?}");
        let mut r = Vec::new();
        p.residuals(&eqs, &vals, &mut r);
        assert!((max_abs(&r) - 5.0).abs() < 1e-6);
    }
}
