//! 无第三方依赖的 spillover 数据结构（build.rs 与库共用）。

#[derive(Debug, Clone, PartialEq)]
pub struct SpilloverMatrix {
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    /// rows[detector][fluor]
    pub rows: Vec<Vec<f64>>,
    pub source: String,
}

pub fn identity(n: usize) -> Vec<Vec<f64>> {
    (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect()
}

pub fn determinant(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    let mut m = a.to_vec();
    let mut det = 1.0;
    for col in 0..n {
        let mut pivot = col;
        for r in col + 1..n {
            if m[r][col].abs() > m[pivot][col].abs() {
                pivot = r;
            }
        }
        if m[pivot][col].abs() < 1e-14 {
            return 0.0;
        }
        if pivot != col {
            m.swap(pivot, col);
            det = -det;
        }
        let piv = m[col][col];
        det *= piv;
        for r in col + 1..n {
            let f = m[r][col] / piv;
            for c in col..n {
                m[r][c] -= f * m[col][c];
            }
        }
    }
    det
}

pub fn invert(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let id = identity(n);
    let mut m: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = a[i].clone();
        row.extend_from_slice(&id[i]);
        m.push(row);
    }
    for col in 0..n {
        let mut pivot = col;
        for r in col + 1..n {
            if m[r][col].abs() > m[pivot][col].abs() {
                pivot = r;
            }
        }
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(pivot, col);
        let piv = m[col][col];
        for c in 0..2 * n {
            m[col][c] /= piv;
        }
        for r in 0..n {
            if r != col {
                let f = m[r][col];
                for c in 0..2 * n {
                    m[r][c] -= f * m[col][c];
                }
            }
        }
    }
    Some(m.into_iter().map(|row| row[n..].to_vec()).collect())
}
