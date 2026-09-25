//! 派生层：补偿后事件、门控成员与群体百分比。
//! 规则 CASE-POP-01：门的百分比分母必须钉住父群版本。
//! 派生结果不可变；上游修订后新建结果版本，旧结果保留但标记过期，不覆盖。

use crate::gate::Membership;
use serde::{Deserialize, Serialize};

/// 对某事件各通道的补偿结果；不可转换的事件保留诊断且不计入数值补偿。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompensatedEventRow {
    pub event_index: u32,
    /// 通道顺序与补偿矩阵一致；无法补偿（奇异矩阵/溢出）时为 None。
    pub values: Option<Vec<f64>>,
    pub basis: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopulationResult {
    pub node_id: String,
    /// 被门控命中的事件下标（稳定顺序）。
    pub member_events: Vec<u32>,
    pub numerator_count: u32,
    /// 分母计数与钉住的父群结果版本；根群分母为全部事件。
    pub denominator_count: u32,
    /// 分母来源：父群派生结果 id（钉住版本），根群为 "root:all-events"。
    pub denominator_source: String,
    /// 百分比 = numerator/denominator，分母 0 时为 None。
    pub percent_of_parent: Option<f64>,
    /// 逐事件成员判定（边界点等可追溯）。
    pub memberships: Vec<EventMembership>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMembership {
    pub event_index: u32,
    pub membership: MembershipTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipTag {
    Inside,
    Boundary,
    Outside,
    Invalid,
}

impl From<Membership> for MembershipTag {
    fn from(m: Membership) -> Self {
        match m {
            Membership::Inside => MembershipTag::Inside,
            Membership::Boundary => MembershipTag::Boundary,
            Membership::Outside => MembershipTag::Outside,
            Membership::Invalid => MembershipTag::Invalid,
        }
    }
}

impl PopulationResult {
    /// 组装群体结果。denominator_source 必须是父群当时的派生结果 id，
    /// 调用方负责钉版本，本函数只做记录与算术。
    pub fn build(
        node_id: &str,
        memberships: Vec<EventMembership>,
        denominator_count: u32,
        denominator_source: String,
    ) -> Self {
        let member_events: Vec<u32> = memberships
            .iter()
            .filter(|m| m.membership == MembershipTag::Inside)
            .map(|m| m.event_index)
            .collect();
        let numerator_count = member_events.len() as u32;
        let percent_of_parent = if denominator_count > 0 {
            Some(numerator_count as f64 / denominator_count as f64 * 100.0)
        } else {
            None
        };
        PopulationResult {
            node_id: node_id.into(),
            member_events,
            numerator_count,
            denominator_count,
            denominator_source,
            percent_of_parent,
            memberships,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(i: u32, tag: MembershipTag) -> EventMembership {
        EventMembership { event_index: i, membership: tag }
    }

    #[test]
    fn percent_is_pinned_to_given_parent_version() {
        let ms = vec![m(0, MembershipTag::Inside), m(1, MembershipTag::Boundary), m(2, MembershipTag::Inside)];
        let r = PopulationResult::build("cd3", ms, 8, "der_pop_lym_abcd1234".into());
        assert_eq!(r.numerator_count, 2); // 边界点不计入
        assert_eq!(r.denominator_count, 8);
        assert_eq!(r.denominator_source, "der_pop_lym_abcd1234");
        assert!((r.percent_of_parent.unwrap() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn zero_denominator_yields_none_not_silent_zero() {
        let r = PopulationResult::build("x", vec![], 0, "der_pop_root".into());
        assert!(r.percent_of_parent.is_none());
    }
}
