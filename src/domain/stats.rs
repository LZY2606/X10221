//! 群体计数与百分比。百分比的分母 *钉住父群版本*，不跟随最新父群。

use serde::{Deserialize, Serialize};

use super::geometry::PointStatus;

pub const PERCENT_RULE: &str = "percent.denominator_pinned_to_parent_version.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationCount {
    pub inside: usize,
    pub boundary: usize,
    pub outside: usize,
    /// 非有限、无法转换而未计入分子的事件数（事件本身仍保留）。
    pub excluded: usize,
}

impl PopulationCount {
    pub fn total(&self) -> usize {
        self.inside + self.boundary + self.outside + self.excluded
    }

    /// 入群数：inside + boundary（边界点显式入内）。
    pub fn included(&self) -> usize {
        self.inside + self.boundary
    }
}

pub fn count_statuses(statuses: &[PointStatus]) -> PopulationCount {
    let mut c = PopulationCount {
        inside: 0,
        boundary: 0,
        outside: 0,
        excluded: 0,
    };
    for s in statuses {
        match s {
            PointStatus::Inside => c.inside += 1,
            PointStatus::Boundary => c.boundary += 1,
            PointStatus::Outside => c.outside += 1,
        }
    }
    c
}

/// 百分比 = 子入群数 / 钉住的父群版本入群数。
/// 父版本与当前父版本不一致时，结果应被标记过期（见 engine），
/// 这里只做纯计算。
pub fn percent_of_pinned_parent(child_included: usize, parent_included: usize) -> Option<f64> {
    if parent_included == 0 {
        return None;
    }
    Some(child_included as f64 / parent_included as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_counts_as_included() {
        let s = vec![
            PointStatus::Inside,
            PointStatus::Boundary,
            PointStatus::Outside,
            PointStatus::Inside,
        ];
        let c = count_statuses(&s);
        assert_eq!(c.included(), 3);
        assert_eq!(c.total(), 4);
    }

    #[test]
    fn percentage_uses_pinned_parent() {
        // 旧父版本分母 100 => 30%；即使父后来修订为 80，钉住的仍给 30%。
        assert_eq!(percent_of_pinned_parent(30, 100), Some(0.30));
        assert_eq!(percent_of_pinned_parent(30, 0), None);
    }
}
