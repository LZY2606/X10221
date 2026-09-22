//! Spillover：关键字解析、条件数与求逆候选（serde 序列化层）。

use super::types::{determinant, identity, invert, SpilloverMatrix as CoreMatrix};
use serde::{Deserialize, Serialize};

/// 库内使用的 spillover 矩阵（带 serde）；与无 serde 核心结构互转。
pub type SpilloverMatrix = SpillMatrix;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpillMatrix {
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub source: String,
}

impl SpillMatrix {
    fn from_core(c: &CoreMatrix) -> Self {
        SpillMatrix {
            detectors: c.detectors.clone(),
            fluorochromes: c.fluorochromes.clone(),
            rows: c.rows.clone(),
            source: c.source.clone(),
        }
    }
    fn to_core(&self) -> CoreMatrix {
        CoreMatrix {
            detectors: self.detectors.clone(),
            fluorochromes: self.fluorochromes.clone(),
            rows: self.rows.clone(),
            source: self.source.clone(),
        }
    }
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn from_value(v: serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct SpillDiag {
    pub code: String,
    pub detail: String,
}

/// 条件数阈值：超过则视为近奇异（1-范数估计）。
pub const NEAR_SINGULAR_COND: f64 = 50.0;

impl SpillMatrix {
    /// 解析 FCS $SPILLOVER 关键字。无法转换的系数保留原表示并附诊断。
    pub fn parse_keyword(value: &str, source: String) -> Result<Self, SpillDiag> {
        let fields: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
        if fields.is_empty() {
            return Err(SpillDiag {
                code: "SPILL_EMPTY".into(),
                detail: "$SPILLOVER 为空".into(),
            });
        }
        let n: usize = fields[0].parse().map_err(|_| SpillDiag {
            code: "SPILL_N_PARSE".into(),
            detail: format!("$SPILLOVER 维度无法解析：{}", fields[0]),
        })?;
        if fields.len() < 1 + n + n * n {
            return Err(SpillDiag {
                code: "SPILL_FIELDS".into(),
                detail: format!("$SPILLOVER 需要 {n} 个标签与 {} 个系数，字段不足", n * n),
            });
        }
        let fluor: Vec<String> = fields[1..1 + n].iter().map(|s| s.to_string()).collect();
        let mut rows = Vec::with_capacity(n);
        for r in 0..n {
            let mut row = Vec::with_capacity(n);
            for c in 0..n {
                let raw = fields[1 + n + r * n + c];
                let v: f64 = raw.parse().map_err(|_| SpillDiag {
                    code: "SPILL_COEFF_PARSE".into(),
                    detail: format!(
                        "系数[{r}][{c}]='{raw}' 无法转换，原值保留在 $SPILLOVER 原始表示中"
                    ),
                })?;
                if !v.is_finite() {
                    return Err(SpillDiag {
                        code: "SPILL_COEFF_NONFINITE".into(),
                        detail: format!("系数[{r}][{c}] 非有限值，原始表示保留"),
                    });
                }
                row.push(v);
            }
            rows.push(row);
        }
        Ok(SpillMatrix {
            detectors: fluor.clone(),
            fluorochromes: fluor,
            rows,
            source,
        })
    }

    pub fn n(&self) -> usize {
        self.rows.len()
    }

    pub fn determinant(&self) -> f64 {
        determinant(&self.rows)
    }

    /// 1-范数条件数 κ1。精确奇异时返回 inf。
    pub fn condition_number(&self) -> f64 {
        let Some(inv) = invert(&self.rows) else {
            return f64::INFINITY;
        };
        norm1(&self.rows) * norm1(&inv)
    }

    pub fn status(&self) -> SpillStatus {
        let det = determinant(&self.rows);
        if det.abs() < 1e-10 {
            SpillStatus::Singular
        } else {
            let cond = self.condition_number();
            if !cond.is_finite() || cond > NEAR_SINGULAR_COND {
                SpillStatus::NearSingular { det, cond }
            } else {
                SpillStatus::WellConditioned { det, cond }
            }
        }
    }

    pub fn inverse_direct(&self) -> Option<Vec<Vec<f64>>> {
        invert(&self.rows)
    }

    /// 候选逆：
    /// - 良态：仅直接逆；
    /// - 近奇异：直接逆 + 两个 ridge 候选，全部保留待裁定；
    /// - 奇异：无直接逆，仅 ridge 候选，并明确标注非唯一。
    pub fn candidates(&self) -> Vec<InverseCandidate> {
        let n = self.n();
        let mut out = Vec::new();
        if let Some(inv) = invert(&self.rows) {
            let cond = norm1(&self.rows) * norm1(&inv);
            let recon = matmul(&inv, &self.rows);
            out.push(InverseCandidate {
                method: InverseMethod::Direct,
                ridge: 0.0,
                matrix: inv,
                residual: frob_diff(&recon, &identity(n)),
                stable: cond <= NEAR_SINGULAR_COND,
                cond,
            });
        }
        for ridge in [1e-3, 1e-2] {
            let ridged = ridge_add(&self.rows, ridge);
            if let Some(inv) = invert(&ridged) {
                let recon = matmul(&inv, &self.rows);
                out.push(InverseCandidate {
                    method: InverseMethod::Ridge,
                    ridge,
                    matrix: inv,
                    residual: frob_diff(&recon, &identity(n)),
                    stable: false,
                    cond: f64::NAN,
                });
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpillStatus {
    WellConditioned { det: f64, cond: f64 },
    NearSingular { det: f64, cond: f64 },
    Singular,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InverseMethod {
    Direct,
    Ridge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InverseCandidate {
    pub method: InverseMethod,
    pub ridge: f64,
    pub matrix: Vec<Vec<f64>>,
    pub residual: f64,
    pub stable: bool,
    pub cond: f64,
}

pub fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b[0].len();
    let mut out = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            out[i][j] = s;
        }
    }
    out
}

fn norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    (0..n)
        .map(|c| (0..n).map(|r| a[r][c].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

fn frob_diff(a: &[Vec<f64>], b: &[Vec<f64>]) -> f64 {
    a.iter()
        .zip(b)
        .flat_map(|(ra, rb)| ra.iter().zip(rb).map(|(x, y)| (x - y).powi(2)))
        .sum::<f64>()
        .sqrt()
}

fn ridge_add(a: &[Vec<f64>], ridge: f64) -> Vec<Vec<f64>> {
    let mut m = a.to_vec();
    let scale = norm1(a).max(1.0);
    for i in 0..m.len() {
        m[i][i] += ridge * scale;
    }
    m
}
