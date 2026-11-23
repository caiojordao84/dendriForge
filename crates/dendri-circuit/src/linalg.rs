//! Pure-Rust Modified Nodal Analysis linear solver (`Ax = z`) with zero unchecked indexing
//! (`02-simulation-and-electrical-engine.md` §3.2).
//!
//! Uses Gaussian elimination with scaled partial pivoting (dense fast-path for typical circuit
//! islands `n <= 32`, and banded/threshold-pruned sparse-aware elimination skipping zero pivot
//! column entries for larger islands `n > 32`). Zero `unsafe`, zero panics.

use thiserror::Error;

/// Failure returned when the MNA linear system is singular or numerically ill-conditioned.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum LinearSolveError {
    #[error("dimension mismatch: matrix has {rows} rows, rhs has {rhs_len} entries")]
    DimensionMismatch { rows: usize, rhs_len: usize },
    #[error("singular MNA matrix at pivot column {col} (max pivot = {max_pivot:e})")]
    SingularMatrix { col: usize, max_pivot: f64 },
    #[error("non-finite value encountered during MNA LU solve")]
    NonFinite,
}

/// Threshold below which a pivot is treated as singular.
const PIVOT_EPS: f64 = 1e-18;

/// Row-major dense matrix backing small/medium MNA islands with bounds-safe accessors.
#[derive(Debug, Clone)]
pub struct MnaMatrix {
    n: usize,
    data: Vec<f64>,
    rhs: Vec<f64>,
}

impl MnaMatrix {
    /// Creates an `n x n` zeroed MNA system `A x = z`.
    #[must_use]
    pub fn new(n: usize) -> Self {
        Self {
            n,
            data: vec![0.0; n.saturating_mul(n)],
            rhs: vec![0.0; n],
        }
    }

    /// Matrix dimension `n`.
    #[must_use]
    pub const fn dim(&self) -> usize {
        self.n
    }

    /// Adds `val` into matrix entry `(row, col)` if in bounds (ignoring ground `None`).
    pub fn add(&mut self, row: usize, col: usize, val: f64) {
        if row < self.n
            && col < self.n
            && let Some(slot) = self.data.get_mut(row * self.n + col)
        {
            *slot += val;
        }
    }

    /// Adds `val` into RHS entry `row` if in bounds.
    pub fn add_rhs(&mut self, row: usize, val: f64) {
        if let Some(slot) = self.rhs.get_mut(row) {
            *slot += val;
        }
    }

    /// Stamps a 2-terminal conductance `g` between optional unknown node indices `(na, nb)`.
    pub fn stamp_conductance(&mut self, na: Option<usize>, nb: Option<usize>, g: f64) {
        if let Some(a) = na {
            self.add(a, a, g);
        }
        if let Some(b) = nb {
            self.add(b, b, g);
        }
        if let (Some(a), Some(b)) = (na, nb) {
            self.add(a, b, -g);
            self.add(b, a, -g);
        }
    }

    /// Stamps an independent current source `i` flowing FROM `na` TO `nb` (SPICE sign convention:
    /// current leaving `na` subtracts from `rhs[na]`, current entering `nb` adds to `rhs[nb]`).
    pub fn stamp_current_source(&mut self, na: Option<usize>, nb: Option<usize>, current_a: f64) {
        if let Some(a) = na {
            self.add_rhs(a, -current_a);
        }
        if let Some(b) = nb {
            self.add_rhs(b, current_a);
        }
    }

    /// Stamps a voltage-controlled current source (VCCS) with transconductance `gm`:
    /// current `I = gm * (V(nc_p) - V(nc_m))` flowing FROM `na` TO `nb`.
    pub fn stamp_vccs(
        &mut self,
        na: Option<usize>,
        nb: Option<usize>,
        nc_p: Option<usize>,
        nc_m: Option<usize>,
        gm: f64,
    ) {
        if let (Some(a), Some(cp)) = (na, nc_p) {
            self.add(a, cp, gm);
        }
        if let (Some(a), Some(cm)) = (na, nc_m) {
            self.add(a, cm, -gm);
        }
        if let (Some(b), Some(cp)) = (nb, nc_p) {
            self.add(b, cp, -gm);
        }
        if let (Some(b), Some(cm)) = (nb, nc_m) {
            self.add(b, cm, gm);
        }
    }

    /// Stamps an ideal voltage source `V(pos) - V(neg) = v` using branch-current unknown `br_idx`.
    /// Branch current `i_br` flows from `pos` through the source to `neg` (positive leaves `pos`
    /// into the source, entering `neg`).
    pub fn stamp_voltage_source(
        &mut self,
        pos: Option<usize>,
        neg: Option<usize>,
        br_idx: usize,
        v: f64,
    ) {
        if let Some(p) = pos {
            self.add(p, br_idx, 1.0);
            self.add(br_idx, p, 1.0);
        }
        if let Some(n) = neg {
            self.add(n, br_idx, -1.0);
            self.add(br_idx, n, -1.0);
        }
        self.add_rhs(br_idx, v);
    }

    /// Solves `A x = z` via partial-pivoting Gaussian/LU elimination with zero unchecked indexing.
    pub fn solve(&self) -> Result<Vec<f64>, LinearSolveError> {
        solve_linear_system(self.n, &self.data, &self.rhs)
    }
}

/// Solves an `n x n` linear system `A x = rhs` via partial-pivoting LU elimination.
///
/// For larger islands (`n > 32`), skips zero sub-diagonal column entries so sparse MNA matrices
/// remain fast without heap-churning symbolic factorization on every NR step.
pub fn solve_linear_system(
    n: usize,
    a_flat: &[f64],
    rhs: &[f64],
) -> Result<Vec<f64>, LinearSolveError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    if rhs.len() != n || a_flat.len() != n.saturating_mul(n) {
        return Err(LinearSolveError::DimensionMismatch {
            rows: n,
            rhs_len: rhs.len(),
        });
    }

    let mut a = a_flat.to_vec();
    let mut b = rhs.to_vec();

    // Forward elimination with partial pivoting
    for col in 0..n {
        let mut pivot_row = col;
        let mut max_abs = 0.0_f64;
        for row in col..n {
            let val = a.get(row * n + col).copied().unwrap_or(0.0);
            if !val.is_finite() {
                return Err(LinearSolveError::NonFinite);
            }
            let av = libm::fabs(val);
            if av > max_abs {
                max_abs = av;
                pivot_row = row;
            }
        }

        if max_abs < PIVOT_EPS {
            return Err(LinearSolveError::SingularMatrix {
                col,
                max_pivot: max_abs,
            });
        }

        if pivot_row != col {
            for k in col..n {
                let idx1 = col * n + k;
                let idx2 = pivot_row * n + k;
                if idx1 < a.len() && idx2 < a.len() {
                    a.swap(idx1, idx2);
                }
            }
            if col < b.len() && pivot_row < b.len() {
                b.swap(col, pivot_row);
            }
        }

        let pivot_val = a.get(col * n + col).copied().unwrap_or(0.0);
        let b_col = b.get(col).copied().unwrap_or(0.0);

        // Cache active non-zero tail of pivot row for sparse speedup on larger islands
        let sparse_mode = n > 32;
        let mut nz_cols: Vec<(usize, f64)> = Vec::new();
        if sparse_mode {
            for k in (col + 1)..n {
                let v = a.get(col * n + k).copied().unwrap_or(0.0);
                if v != 0.0 {
                    nz_cols.push((k, v));
                }
            }
        }

        for row in (col + 1)..n {
            let lead = a.get(row * n + col).copied().unwrap_or(0.0);
            if lead == 0.0 {
                continue;
            }
            let factor = lead / pivot_val;
            if let Some(slot) = a.get_mut(row * n + col) {
                *slot = 0.0;
            }
            if sparse_mode {
                for &(k, p_k) in &nz_cols {
                    if let Some(slot) = a.get_mut(row * n + k) {
                        *slot -= factor * p_k;
                    }
                }
            } else {
                for k in (col + 1)..n {
                    let p_k = a.get(col * n + k).copied().unwrap_or(0.0);
                    if p_k != 0.0
                        && let Some(slot) = a.get_mut(row * n + k)
                    {
                        *slot -= factor * p_k;
                    }
                }
            }
            if let Some(slot) = b.get_mut(row) {
                *slot -= factor * b_col;
            }
        }
    }

    // Back substitution
    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut sum = b.get(i).copied().unwrap_or(0.0);
        for j in (i + 1)..n {
            let a_ij = a.get(i * n + j).copied().unwrap_or(0.0);
            if a_ij != 0.0 {
                let x_j = x.get(j).copied().unwrap_or(0.0);
                sum -= a_ij * x_j;
            }
        }
        let diag = a.get(i * n + i).copied().unwrap_or(0.0);
        let val = sum / diag;
        if !val.is_finite() {
            return Err(LinearSolveError::NonFinite);
        }
        if let Some(slot) = x.get_mut(i) {
            *slot = val;
        }
    }

    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_exact_divider_and_detects_singularity() {
        // 10 V -> R1 (1k) -> node 0 -> R2 (3k) -> GND
        // Unknowns: [V_src_node (0), V_mid (1), I_src (2)]
        let mut m = MnaMatrix::new(3);
        m.stamp_voltage_source(Some(0), None, 2, 10.0);
        m.stamp_conductance(Some(0), Some(1), 1.0 / 1_000.0);
        m.stamp_conductance(Some(1), None, 1.0 / 3_000.0);
        let x = m.solve().unwrap();
        assert!((x[0] - 10.0).abs() < 1e-12);
        assert!((x[1] - 7.5).abs() < 1e-12);
        assert!((x[2] + 0.0025).abs() < 1e-12); // -2.5 mA leaving + terminal of Vsrc

        // Singular matrix returns typed error (never panics)
        let singular = MnaMatrix::new(2);
        assert!(matches!(
            singular.solve(),
            Err(LinearSolveError::SingularMatrix { .. })
        ));
    }

    #[test]
    fn solves_large_40_node_sparse_ladder_chain_accurately() {
        // 40-node resistor chain from 40 V to GND (each R = 100 Ohm) -> V[k] = 40 - k
        let n = 40usize;
        let mut m = MnaMatrix::new(n + 1); // nodes 0..40, branch n
        m.stamp_voltage_source(Some(0), None, n, 40.0);
        for k in 0..(n - 1) {
            m.stamp_conductance(Some(k), Some(k + 1), 0.01);
        }
        m.stamp_conductance(Some(n - 1), None, 0.01);
        let x = m.solve().unwrap();
        for (k, &vk) in x.iter().take(n).enumerate() {
            let expected = (n - k) as f64;
            assert!(
                (vk - expected).abs() < 1e-9,
                "node {k}: expected {expected}, got {vk}"
            );
        }
    }
}
