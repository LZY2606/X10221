//! 离线案例包「流式边界室：补偿矩阵、门控系谱与细胞群不确定性」。
//! 加载 fixtures/casepack 形成基线；重放六个边界场景，留下输入片段、
//! 判定状态与受影响对象，全部落成机器可读事件。

use std::path::Path;

use serde_json::json;

use crate::fcs;
use crate::lineage;
use crate::store::{Store, StoreError};
use crate::transform;

pub const LINE_FCS: &str = "fcs:events";
pub const LINE_MATRIX: &str = "matrix:comp";
pub const LINE_TRANSFORM: &str = "transform:main";
pub const LINE_SPILLOVER: &str = "spillover:main";
pub const LINE_GATEREL: &str "gaterel:tree";
