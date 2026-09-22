//! 固定边界场景的事件构造：
//! 矩阵奇异、极端信号（随基线诊断）、边界点（随基线）、门嵌套改名、
//! 上游过期传播、同版本并发修改、局部源数据改动、参数修订、冲突人工决定。

use crate::case;
use crate::domain::*;

/// 场景 1：提交近奇异补偿矩阵 v2（荧光门重新钉到 v2）。不做隐式求逆。
pub fn singular_comp_bundle() -> RevisionBundle {
    let gates = case::gates_repinned(
        "COMP-v2",
        "v2",
        "补偿重拟合 v2：近奇异，荧光门在裁定前阻塞（保留 v1 旧结果）",
    );
    RevisionBundle {
        compensation: Some(case::comp_v2_near_singular()),
        gates,
        ..Default::default()
    }
}

/// 场景 1（分歧 A）：拒绝近奇异补偿——荧光门回到 v1 重算。
pub fn reject_singular_bundle() -> RevisionBundle {
    let gates = case::gates_repinned(
        "COMP-v1",
        "v1back",
        "拒绝近奇异 v2：荧光门重新钉回良态 COMP-v1",
    );
    RevisionBundle { gates, ..Default::default() }
}

/// 场景 2：变换参数修订 cofactor 150→100（参数修订；不直接影响当前线性空间门，
/// 但任何 asinh 空间门会按依赖失效）。
pub fn transform_revision_bundle() -> RevisionBundle {
    RevisionBundle { transform: Some(case::transform_v2()), ..Default::default() }
}

/// 场景 3：门嵌套改名（G-FITC 仅改名，gate_id 稳定）。
pub fn rename_bundle() -> RevisionBundle {
    case::gate_renamed_bundle()
}

/// 场景 4：上游门修订（细胞门扩大），下游 FITC/PE 标过期不覆盖。
pub fn upstream_gate_bundle() -> RevisionBundle {
    case::gate_revised_cells_bundle()
}

/// 场景 4b：显式重算下游到新的上游门版本（旧结果保留，另存新结果）。
pub fn recompute_downstream_after_cells() -> RevisionBundle {
    // FITC/PE 门形状不变，仅把 supersedes 指向前版并产出新版本线，父群钉到 GV-CELLS-v2。
    let mut gates = Vec::new();
    for mut g in case::gates_v1() {
        if g.gate_id == "G-FITC" {
            g.version_id = "GV-FITC-v3-reparent".into();
            g.supersedes = Some("GV-FITC-v1".into());
            g.name = "FITC 阳性（复核改名）".into();
            g.note = "显式重算：钉到扩大后的细胞门 GV-CELLS-v2".into();
            gates.push(g);
        } else if g.gate_id == "G-PE" {
            g.version_id = "GV-PE-v2-reparent".into();
            g.supersedes = Some("GV-PE-v1".into());
            g.note = "显式重算：跟随新 FITC 群体".into();
            gates.push(g);
        }
    }
    RevisionBundle { gates, ..Default::default() }
}

/// 场景 5：局部源数据改动（DS-v2，仅 E05 FITC 微调），按依赖计算失效范围。
pub fn local_source_patch_bundle() -> RevisionBundle {
    case::source_patch_bundle()
}

/// 场景 5b：把指定门显式重钉到 DS-v2（旧 DS-v1 派生保留为过期）。
pub fn repin_dataset_bundle() -> RevisionBundle {
    let mut gates = Vec::new();
    for mut g in case::gates_v1() {
        if matches!(g.kind, GateKind::Root) {
            continue;
        }
        g.version_id = format!("{}-ds2", g.version_id);
        g.supersedes = Some(
            case::gates_v1()
                .iter()
                .find(|x| x.gate_id == g.gate_id)
                .unwrap()
                .version_id
                .clone(),
        );
        g.space.dataset_version = "DS-v2".into();
        g.note = "显式重算到 DS-v2：旧 DS-v1 派生保留并标过期".into();
        gates.push(g);
    }
    RevisionBundle { gates, ..Default::default() }
}

/// 候选裁定（边界点 或 近奇异补偿）。
pub fn decision(
    set_id: &str,
    accepted: Option<&str>,
    rejected: Vec<String>,
    rationale: &str,
) -> EventPayload {
    EventPayload::Decision {
        candidate_set_id: set_id.to_string(),
        accepted_candidate_id: accepted.map(|s| s.to_string()),
        rejected,
        rationale: rationale.to_string(),
    }
}

pub fn retraction(target_event_id: &str, reason: &str) -> EventPayload {
    EventPayload::Retracted {
        target_event_id: target_event_id.to_string(),
        reason: reason.to_string(),
    }
}
