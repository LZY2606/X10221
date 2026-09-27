//! 领域类型：所有领域量携带单位或坐标基准；原始表示在导入时保留，派生层才规范化。
use serde::{Deserialize, Serialize};
use std::fmt;

newtype_id!(pub ChannelId);
newtype_id!(pub EventId);
newtype_id!(pub MaterialId);
newtype_id!(pub BranchId);
newtype_id!(pub ArtifactId);
newtype_id!(pub EventRowId);
newtype_id!(pub ResultId);

pub const ROOT_PARENT: &str = "root";

/// 通道测量值的单位/基准。原始层保留原始基准，派生层才转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleUnit {
    /// FCS $PnE 线性刻度（原始 ADC 通道单位）
    ChannelRaw,
    /// 线性物理刻度（如散射线性单位）
    Linear,
    /// 对数刻度 decades
    LogDecades,
    /// asinh 变换后的双曲反正弦坐标（基准为 transform 参数版本）
    AsinhCo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub short: String,
    pub unit: ScaleUnit,
    /// 原始 FCS 增益参数（保留原始表示）
    pub fcs_gain: f64,
    /// 原始 FCS $PnE 表示，如 "4,1"
    pub fcs_pne: String,
}

/// 单事件某通道的原始测量值。NaN 保留在原位置，附带诊断。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelValue {
    /// 原始数值；无法转换/无效值用 NaN 保留在原位置
    pub raw: f64,
    pub status: ValueStatus,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ValueStatus {
    #[default]
    Ok,
    /// 检测器饱和（平台标记的极端信号）
    Saturated,
    /// 无效值（如 NaN），保留原位置不丢弃
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEvent {
    pub id: EventId,
    /// 以通道 short 名索引的原始通道值
    pub channels: std::collections::BTreeMap<String, ChannelValue>,
}

/// spillover 元数据：与矩阵分开版本化（平台元数据 vs 解析后的系数）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub fluorochromes: Vec<String>,
    /// 平台报告的探测器/滤光片，纯元数据，解释不唯一时新建分支
    pub detectors: Vec<String>,
    pub source_label: String,
    pub fcs_keyword: String,
}

/// 补偿矩阵 S：测量 = S · 真实。补偿即求解 S c = x。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub channel_names: Vec<String>,
    /// 行主序 n*n，对角线应为 1.0
    pub rows: Vec<Vec<f64>>,
    pub basis: String,
}

/// asinh 变换参数（每个荧光通道）： y = asinh(x / cofactor) * decades_base
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub cofactors: std::collections::BTreeMap<String, f64>,
    /// 散射通道为线性；荧光通道 asinh
    pub mode: std::collections::BTreeMap<String, String>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 门多边形。坐标基准指向某个 transform 参数版本（坐标基准随版本钉住）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub stable_id: String,
    /// 可改名：rename 不改变 stable_id、父子关系与结果身份
    pub display_name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// 顶点坐标基准
    pub coordinate_basis: String,
    pub vertices: Vec<Point>,
    pub parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTree {
    /// 按绘制/读取顺序保存，稳定顺序也是身份的一部分
    pub gates: Vec<GatePolygon>,
}

/// 材料类型（原始通道值、spillover 元数据、补偿矩阵、变换参数、门、父子关系分开版本化）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    EventsRaw,
    SpilloverMeta,
    CompensationMatrix,
    TransformParams,
    GateTree,
    Manifest,
}

impl fmt::Display for MaterialKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", serde_json::to_value(self).unwrap().as_str().unwrap())
    }
}
