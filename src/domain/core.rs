//! 稳定身份、内容寻址与单位/坐标基准。
//!
//! 所有进入系统的材料先以原始表示保存，其身份由原始字节的 BLAKE3 摘要确定；
//! 派生层（补偿后信号、门归属、群体统计）的身份由输入身份 + 规则版本确定性导出。

use serde::{Deserialize, Serialize};

/// 内容寻址标识：`kind:hex16`。身份与数据库行无关，导出/删除/再导入保持不变。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cid(pub String);

impl Cid {
    pub fn from_bytes(kind: &str, data: &[u8]) -> Self {
        let h = blake3::hash(data);
        Cid(format!("{}:{}", kind, &h.to_hex()[..16]))
    }
    pub fn from_jsonable<T: Serialize>(kind: &str, value: &T) -> Self {
        let canon = canonical_json(value);
        Self::from_bytes(kind, canon.as_bytes())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Cid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 确定性 JSON 编码：键排序、无多余空白。保证“同样内容 -> 同样身份”。
pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serializable");
    let canon = canonicalize(v);
    serde_json::to_string(&canon).expect("canonical json")
}

fn canonicalize(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(mut m) => {
            let mut pairs: Vec<(String, serde_json::Value)> =
                m.drain().map(|(k, vv)| (k, canonicalize(vv))).collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs.into_iter().collect::<serde_json::Map<String, serde_json::Value>>()
                .into_iter()
                .map(|(k, vv)| (k, vv))
                .collect::<serde_json::Value>()
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(canonicalize).collect())
        }
        other => other,
    }
}

/// 领域量携带的单位或坐标基准，禁止无量纲混用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "ref")]
pub enum Basis {
    /// 原始通道刻度，value 为通道短名（如 FITC-A）。
    RawChannel(String),
    /// 补偿后线性刻度，value 为荧光通道短名。
    Compensated(String),
    /// 双曲反正缩，value 为通道短名；坐标含义由 TransformParams 钉住。
    Asinh(String),
    /// 常用对数尺度，value 为通道短名。
    Log(String),
    /// 散射/时间等线性尺度。
    Linear(String),
    /// 无量纲（仅用于门计数比例）。
    Dimensionless,
}

impl Basis {
    pub fn label(&self) -> String {
        match self {
            Basis::RawChannel(c)
            | Basis::Compensated(c)
            | Basis::Asinh(c)
            | Basis::Log(c)
            | Basis::Linear(c) => c.clone(),
            Basis::Dimensionless => "-".to_string(),
        }
    }
}

/// 带基准的测量值；解析失败的原始值以 `RawText` 原样保留并附诊断。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "form")]
pub enum Measured {
    Number { value: f64, basis: Basis, unit: String },
    RawText { text: String, basis: Basis, diagnostic: String },
}

impl Measured {
    pub fn number(value: f64, basis: Basis, unit: &str) -> Self {
        Measured::Number { value, basis, unit: unit.to_string() }
    }
    pub fn raw(text: String, basis: Basis, diagnostic: impl Into<String>) -> Self {
        Measured::RawText { text, basis, diagnostic: diagnostic.into() }
    }
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Measured::Number { value, .. } => Some(*value),
            Measured::RawText { .. } => None,
        }
    }
}

/// 事件（一个细胞/颗粒）的原始通道读数，按参数名保存，保持 FCS 列序。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// 1-based 行号，与 FCS 事件顺序一致（稳定坐标基准的一部分）。
    pub index: u32,
    pub values: Vec<(String, Measured)>,
}

/// 对若干事件的内容寻址快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTable {
    pub parameter_order: Vec<String>,
    pub events: Vec<EventRecord>,
}

/// 判定状态：派生结论的生命周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// 自动计算，等待审阅。
    Pending,
    /// 审阅接受。
    Accepted,
    /// 审阅否定（该候选被否，仍保留）。
    Rejected,
    /// 输入存在多候选且无法唯一决定，必须显式裁定。
    Ambiguous,
    /// 上游依赖已修订，本结论不再是当前版本但被保留。
    Stale,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_is_stable_and_order_independent() {
        let a = Cid::from_jsonable("x", &serde_json::json!({"b": 1, "a": 2}));
        let b = Cid::from_jsonable("x", &serde_json::json!({"a": 2, "b": 1}));
        assert_eq!(a, b);
        let c = Cid::from_jsonable("x", &serde_json::json!({"a": 2, "b": 2}));
        assert_ne!(a, c);
        let raw = Cid::from_bytes("blob", b"abc");
        assert_eq!(raw.as_str().len(), "blob:".len() + 16);
    }
}
