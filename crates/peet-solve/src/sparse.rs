//! Sparse symmetric positive definite factorisation for Gram matrices `G = M Mᵀ + λI`.
//!
//! The solver forms `J Jᵀ + λI` (one row/column per equation) every iteration. The
//! sparsity pattern only depends on which equations read which variables, so everything structural is
//! computed once and cached in a [`Gram`]:
//!
//! - a minimum degree ordering (to keep fill low),
//! - the pattern of the Cholesky factor `L`,
//! - a flat list of `(target, a, b)` contributions to assemble `G` from Jacobian entries,
//! - a flat list of `(target, a, b)` updates that performs the numeric factorisation.
//!
//! The numeric phase is then a couple of tight loops over precomputed indices.

/// Structure and workspace for factorising `G = Σ_records r rᵀ` (with damping).
#[derive(Clone, Debug)]
pub struct Gram {
    dim: usize,
    /// `perm[new] = old`.
    perm: Vec<u32>,
    /// Column pointers of `L` (permuted indices). The diagonal is first in each column.
    col_ptr: Vec<u32>,
    /// Row index (permuted) of every stored entry of `L`.
    row_idx: Vec<u32>,
    /// Assembly: `vals[t] += entries[a] * entries[b]`.
    assembly: Vec<[u32; 3]>,
    /// Factorisation updates for column `j`: `upd[upd_ptr[j]..upd_ptr[j+1]]`, each
    /// `vals[t] -= vals[a] * vals[b]`.
    upd_ptr: Vec<u32>,
    upd: Vec<[u32; 3]>,
    /// Numeric values of `L` (and of `G` before factorising).
    vals: Vec<f64>,
    /// Scratch vector for solves.
    work: Vec<f64>,
    /// Damped diagonal before elimination, and which rows were dropped as dependent.
    diag0: Vec<f64>,
    dropped: Vec<bool>,
}

/// How the damping `μ` is added to the diagonal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damping {
    /// `μ diag(G)`: scale invariant per row (Marquardt).
    Scaled,
    /// `μ max(diag(G)) I`: damps every direction, including null space ones (Levenberg).
    Uniform,
}

/// A pivot below this fraction of its row's damped diagonal marks the row as dependent.
pub const DROP: f64 = 1e-13;

impl Gram {
    /// Builds the structure for a Gram matrix of dimension `dim`. Each record lists
    /// `(index, entry)` pairs: the record contributes `entry_a * entry_b` to `G[ia, ib]`
    /// for every pair of its elements, where `entry` indexes the value array passed to
    /// [`Gram::assemble`].
    pub fn new(dim: usize, records: &[Vec<(u32, u32)>]) -> Self {
        // Adjacency of G.
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); dim];
        for rec in records {
            for &(i, _) in rec {
                for &(j, _) in rec {
                    if i != j {
                        adj[i as usize].push(j);
                    }
                }
            }
        }
        for a in &mut adj {
            a.sort_unstable();
            a.dedup();
        }

        // Minimum degree ordering on the explicit elimination graph. Records the pattern
        // of each column of L (original indices) as it goes.
        let mut eliminated = vec![false; dim];
        let mut order = Vec::with_capacity(dim);
        let mut col_pattern: Vec<Vec<u32>> = vec![Vec::new(); dim];
        let mut merged = Vec::new();
        for _ in 0..dim {
            let mut best = usize::MAX;
            let mut best_deg = usize::MAX;
            for (v, a) in adj.iter().enumerate() {
                if !eliminated[v] && a.len() < best_deg {
                    best = v;
                    best_deg = a.len();
                }
            }
            let v = best;
            eliminated[v] = true;
            order.push(v as u32);
            let nbrs = std::mem::take(&mut adj[v]);
            for &u in &nbrs {
                let au = &mut adj[u as usize];
                // au := (au ∪ nbrs) \ {u, v}
                merged.clear();
                let (mut i, mut j) = (0, 0);
                while i < au.len() || j < nbrs.len() {
                    let x = match (au.get(i), nbrs.get(j)) {
                        (Some(&a), Some(&b)) if a == b => {
                            i += 1;
                            j += 1;
                            a
                        }
                        (Some(&a), Some(&b)) if a < b => {
                            i += 1;
                            a
                        }
                        (Some(_), Some(&b)) => {
                            j += 1;
                            b
                        }
                        (Some(&a), None) => {
                            i += 1;
                            a
                        }
                        (None, Some(&b)) => {
                            j += 1;
                            b
                        }
                        (None, None) => unreachable!(),
                    };
                    if x != u && x as usize != v {
                        merged.push(x);
                    }
                }
                std::mem::swap(au, &mut merged);
            }
            col_pattern[v] = nbrs;
        }

        let mut inv = vec![0u32; dim];
        for (new, &old) in order.iter().enumerate() {
            inv[old as usize] = new as u32;
        }

        // Pattern of L in permuted indices, diagonal first then sorted rows.
        let mut col_ptr = Vec::with_capacity(dim + 1);
        let mut row_idx = Vec::new();
        col_ptr.push(0u32);
        for &old in &order {
            let j = inv[old as usize];
            row_idx.push(j);
            let start = row_idx.len();
            row_idx.extend(col_pattern[old as usize].iter().map(|&o| inv[o as usize]));
            row_idx[start..].sort_unstable();
            col_ptr.push(row_idx.len() as u32);
        }

        let find = |col: u32, row: u32| -> u32 {
            let (s, e) = (
                col_ptr[col as usize] as usize,
                col_ptr[col as usize + 1] as usize,
            );
            if row == col {
                return s as u32;
            }
            let off = row_idx[s + 1..e]
                .binary_search(&row)
                .expect("fill pattern is closed under elimination");
            (s + 1 + off) as u32
        };

        // Assembly contributions (lower triangle).
        let mut assembly = Vec::new();
        for rec in records {
            for (x, &(i, ea)) in rec.iter().enumerate() {
                for &(j, eb) in &rec[..=x] {
                    let (pi, pj) = (inv[i as usize], inv[j as usize]);
                    let (row, col) = if pi >= pj { (pi, pj) } else { (pj, pi) };
                    assembly.push([find(col, row), ea, eb]);
                }
            }
        }
        assembly.sort_unstable_by_key(|a| a[0]);

        // Right-looking update lists.
        let mut upd_ptr = Vec::with_capacity(dim + 1);
        let mut upd = Vec::new();
        upd_ptr.push(0u32);
        for j in 0..dim {
            let (s, e) = (col_ptr[j] as usize + 1, col_ptr[j + 1] as usize);
            for a in s..e {
                for b in s..=a {
                    // L[ra, rb] -= L[ra, j] * L[rb, j], stored in column rb.
                    let (ra, rb) = (row_idx[a], row_idx[b]);
                    upd.push([find(rb, ra), a as u32, b as u32]);
                }
            }
            upd_ptr.push(upd.len() as u32);
        }

        let nnz = row_idx.len();
        Self {
            dim,
            perm: order,
            col_ptr,
            row_idx,
            assembly,
            upd_ptr,
            upd,
            vals: vec![0.0; nnz],
            work: vec![0.0; dim],
            diag0: vec![0.0; dim],
            dropped: vec![false; dim],
        }
    }

    /// Assembles `G + μ diag(G)` ([`Damping::Scaled`]) or `G + μ max(diag(G)) I`
    /// ([`Damping::Uniform`]) from the entry values and factorises it in place.
    ///
    /// Rows that are numerically dependent on earlier ones (pivot below [`DROP`] of the
    /// row's damped diagonal) are *dropped*: their column of `L` is cleared and [`solve`]
    /// returns 0 for them. For a Gram matrix `J Jᵀ` this solves for an independent subset
    /// of the equations, which keeps steps finite when redundant equations are
    /// inconsistent to second order away from the solution. Returns false only if the
    /// values are not finite.
    ///
    /// [`solve`]: Gram::solve
    pub fn factor(&mut self, entries: &[f64], mu: f64, damping: Damping) -> bool {
        self.vals.fill(0.0);
        for &[t, a, b] in &self.assembly {
            self.vals[t as usize] += entries[a as usize] * entries[b as usize];
        }
        let add = match damping {
            Damping::Scaled => 0.0,
            Damping::Uniform => {
                let max = (0..self.dim)
                    .map(|j| self.vals[self.col_ptr[j] as usize])
                    .fold(0.0, f64::max);
                mu * max
            }
        };
        for j in 0..self.dim {
            let s = self.col_ptr[j] as usize;
            self.vals[s] = match damping {
                Damping::Scaled => self.vals[s] * (1.0 + mu),
                Damping::Uniform => self.vals[s] + add,
            };
            self.diag0[j] = self.vals[s];
        }
        for j in 0..self.dim {
            let (s, e) = (self.col_ptr[j] as usize, self.col_ptr[j + 1] as usize);
            let d = self.vals[s];
            if !d.is_finite() {
                return false;
            }
            let (us, ue) = (self.upd_ptr[j] as usize, self.upd_ptr[j + 1] as usize);
            if d <= DROP * self.diag0[j] || d <= 0.0 {
                self.dropped[j] = true;
                self.vals[s] = 1.0;
                self.vals[s + 1..e].fill(0.0);
                continue;
            }
            self.dropped[j] = false;
            let d = d.sqrt();
            self.vals[s] = d;
            let inv = 1.0 / d;
            for v in &mut self.vals[s + 1..e] {
                *v *= inv;
            }
            for &[t, a, b] in &self.upd[us..ue] {
                self.vals[t as usize] -= self.vals[a as usize] * self.vals[b as usize];
            }
        }
        true
    }

    /// Solves with the last factorisation, in place. Dropped rows get 0.
    pub fn solve(&mut self, b: &mut [f64]) {
        let n = self.dim;
        for (new, &old) in self.perm.iter().enumerate() {
            self.work[new] = b[old as usize];
        }
        for j in 0..n {
            if self.dropped[j] {
                self.work[j] = 0.0;
                continue;
            }
            let (s, e) = (self.col_ptr[j] as usize, self.col_ptr[j + 1] as usize);
            let y = self.work[j] / self.vals[s];
            self.work[j] = y;
            for p in s + 1..e {
                self.work[self.row_idx[p] as usize] -= self.vals[p] * y;
            }
        }
        for j in (0..n).rev() {
            if self.dropped[j] {
                self.work[j] = 0.0;
                continue;
            }
            let (s, e) = (self.col_ptr[j] as usize, self.col_ptr[j + 1] as usize);
            let mut y = self.work[j];
            for p in s + 1..e {
                y -= self.vals[p] * self.work[self.row_idx[p] as usize];
            }
            self.work[j] = y / self.vals[s];
        }
        for (new, &old) in self.perm.iter().enumerate() {
            b[old as usize] = self.work[new];
        }
    }

    /// Number of rows dropped as dependent by the last factorisation.
    #[cfg(test)]
    pub fn dropped(&self) -> usize {
        self.dropped.iter().filter(|&&d| d).count()
    }

    /// Number of stored entries in `L` (for diagnostics and tests).
    #[cfg(test)]
    pub fn nnz(&self) -> usize {
        self.row_idx.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dense reference: G = Σ r rᵀ, diagonal scaled by 1 + λ.
    fn dense(dim: usize, records: &[Vec<(u32, u32)>], entries: &[f64], lambda: f64) -> Vec<f64> {
        let mut g = vec![0.0; dim * dim];
        for rec in records {
            for &(i, a) in rec {
                for &(j, b) in rec {
                    g[i as usize * dim + j as usize] += entries[a as usize] * entries[b as usize];
                }
            }
        }
        for i in 0..dim {
            g[i * dim + i] *= 1.0 + lambda;
        }
        g
    }

    #[test]
    fn solves_random_sparse_systems() {
        let mut seed = 12345u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for dim in [1usize, 2, 5, 17, 40] {
            let mut records = Vec::new();
            let mut entries = Vec::new();
            for _ in 0..dim * 2 {
                let k = 1 + (rnd() * 3.0) as usize;
                let mut rec: Vec<(u32, u32)> = Vec::new();
                for _ in 0..k {
                    let i = (rnd() * dim as f64) as u32 % dim as u32;
                    if rec.iter().all(|r| r.0 != i) {
                        rec.push((i, entries.len() as u32));
                        entries.push(rnd() * 2.0 - 1.0);
                    }
                }
                records.push(rec);
            }
            let mu = 1e-3;
            let mut gram = Gram::new(dim, &records);
            assert!(gram.factor(&entries, mu, Damping::Scaled));
            let b: Vec<f64> = (0..dim).map(|_| rnd()).collect();
            let mut x = b.clone();
            gram.solve(&mut x);
            let g = dense(dim, &records, &entries, mu);
            for i in 0..dim {
                let gx: f64 = (0..dim).map(|j| g[i * dim + j] * x[j]).sum();
                assert!((gx - b[i]).abs() < 1e-8, "dim {dim}: {gx} vs {}", b[i]);
            }
            assert!(gram.nnz() >= dim);
        }
    }

    #[test]
    fn chain_has_no_fill() {
        // Records coupling i and i+1: tridiagonal G, L should have 2n-1 entries.
        let n = 50;
        let records: Vec<Vec<(u32, u32)>> = (0..n - 1)
            .map(|i| vec![(i as u32, 0), (i as u32 + 1, 0)])
            .collect();
        let g = Gram::new(n, &records);
        assert_eq!(g.nnz(), 2 * n - 1);
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    #[test]
    fn dependent_rows_are_dropped() {
        // Three rows over two columns (as records per column): r0 = (1, 0), r1 = (0, 1),
        // r2 = (1, 1) = r0 + r1. Gram of the rows is singular; the last row is dropped.
        let records = vec![vec![(0, 0), (2, 1)], vec![(1, 2), (2, 3)]];
        let entries = [1.0, 1.0, 1.0, 1.0];
        let mut g = Gram::new(3, &records);
        assert!(g.factor(&entries, 1e-14, Damping::Scaled));
        assert_eq!(g.dropped(), 1);
        // An inconsistent right-hand side still gives a finite, sensible answer.
        let mut b = [1.0, 1.0, 5.0];
        g.solve(&mut b);
        assert!(b.iter().all(|x| x.is_finite() && x.abs() < 10.0), "{b:?}");
    }
}
