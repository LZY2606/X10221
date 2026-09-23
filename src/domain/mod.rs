//! 纯函数领域核心：补偿矩阵分析、arcsinh 变换、多边形门控与谱系视图推导。
//!
//! 本层不接触数据库与 HTTP：所有推导都可从显式传入的来源版本重新计算，
//! 因而每个结论都能反查「哪条材料、哪版规则、哪次决定」。

use serde_json::{json, Value};

pub mod gate;
pub mod matrix;
pub mod view;

/// 来源材料种类（原始通道值 / spillover 元数据 / 补偿矩阵 / 变换参数 / 门控草案）。
pub const KIND_FCS: &str = "fcs_events";
pub const KIND_SPILL: &str = "spillover";
pub const KIND_COMP: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATES: &str = "gate_set";
pub const SOURCE_KINDS: [&str; 5] =
    [KIND_FCS, KIND_SPILL, KIND_COMP, KIND_TRANSFORM, KIND_GATES];

/// 近奇异判定阈值：1-范数条件数倒数 rcond 小于该值即视为近奇异。
pub const RCOND_THRESHOLD: f64 = 1e-6;
/// 极端原始信号判定范围（原始线性检测值，单位 a.u.）。
pub const EXTREME_LOW: f64 = -1000.0;
pub const EXTREME_HIGH: f64 = 200_000.0;

/// arcsinh 变换，`x` 为原始检测值（a.u.），返回 arcsinh 尺度坐标。
pub fn arcsinh_transform(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

/// 读取一个原始通道值：数字保留为 f64，其余表示（如 "<overflow>"）原样保留。
pub fn raw_number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

/// 稳定百分比：count / denominator * 100，分母为 0 时返回 null。
pub fn percent(count: usize, denom: usize) -> Option<f64> {
    if denom == 0 {
        None
    } else {
        Some(100.0 * count as f64 / denom as f64)
    }
}

/// 从 gate_set 载荷取门定义数组（可变）。
pub fn gate_vec_mut(gate_set: &mut Value) -> &mut Vec<Value> {
    gate_set
        .as_object_mut()
        .unwrap()
        .get_mut("gates")
        .unwrap()
        .as_array_mut()
        .unwrap()
}

/// 按 id 取门定义。
pub fn find_gate<'a>(gate_set: &'a Value, gate_id: &str) -> Option<&'a Value> {
    gate_set["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == gate_id)
}

/// 判断 `gate_id` 是否在以 `ancestor` 为根的父子链下游（含自身）。
pub fn is_descendant_or_self(gate_set: &Value, ancestor: &str, gate_id: &str) -> bool {
    let mut current = gate_id;
    loop {
        if current == ancestor {
            return true;
        }
        match find_gate(gate_set, current).and_then(|g| g["parent"].as_str()) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
}

/// 门父子拓扑顺序（父门先于子门）。
pub fn topo_order(gate_set: &Value) -> Vec<String> {
    let gates = gate_set["gates"].as_array().unwrap();
    let mut order: Vec<String> = Vec::new();
    fn emit(order: &mut Vec<String>, gate_set: &Value, id: &str, depth: u32) {
        if depth > 64 || order.contains(&id.to_string()) {
            return;
        }
        if let Some(parent) = find_gate(gate_set, id).and_then(|g| g["parent"].as_str()) {
            emit(order, gate_set, parent, depth + 1);
        }
        if !order.contains(&id.to_string()) {
            order.push(id.to_string());
        }
    }
    for g in gates {
        emit(&mut order, gate_set, g["id"].as_str().unwrap(), 0);
    }
    order
}

/// 构造机器可读诊断条目。
pub fn diag(code: &str, message: impl Into<String>, object: Value) -> Value {
    json!({
        "code": code,
        "message": message.into(),
        "object": object,
    })
}
