//! 源材料信封：原始通道值、spillover 元数据、补偿矩阵、变换参数、
//! 门多边形、父子关系六类分别版本化；来源字节与解释正文分开保存。

use crate::comp::{CompMatrix, SpilloverMeta};
use crate::fcs::FcsFile;
use crate::lineage::LineageDoc;
use crate::transform::ChannelTransform;
use serde::{Deserialize, Serialize};

pub const KIND_CHANNELS: &str = "channels";
pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_COMP: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_LINEAGE: &str = "lineage";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawChannelDoc {
    pub fcs: FcsFile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body")]
pub enum MaterialBody {
    Channels(RawChannelDoc),
    Spillover(SpilloverMeta),
    CompMatrix(CompMatrix),
    Transform(Vec<ChannelTransform>),
    Gate(GateWrapper),
    Lineage(LineageDoc),
}

/// 单门包装（每个门多边形独立版本化，节点通过 gate_material_version_id 引用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateWrapper {
    pub gate_id: String,
    pub def: crate::gate::GateDef,
}

impl MaterialBody {
    pub fn kind(&self) -> &'static str {
        match self {
            MaterialBody::Channels(_) => KIND_CHANNELS,
            MaterialBody::Spillover(_) => KIND_SPILLOVER,
            MaterialBody::CompMatrix(_) => KIND_COMP,
            MaterialBody::Transform(_) => KIND_TRANSFORM,
            MaterialBody::Gate(_) => KIND_GATE,
            MaterialBody::Lineage(_) => KIND_LINEAGE,
        }
    }
}

/// 材料版本信封：解释正文 + 它引用的原始字节（blob 引用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialEnvelope {
    pub id: String,
    pub kind: String,
    pub seq: u32,
    pub body: MaterialBody,
    /// 原始字节 blob 引用（如 FCS 文件）。解释与来源分开保存。
    pub source_blob_id: Option<String>,
    /// 原始字节在 blob 中的文件名校验名（可追溯）。
    pub source_name: Option<String>,
    pub note: String,
}
