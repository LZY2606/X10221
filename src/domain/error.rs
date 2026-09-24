use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlowError {
    #[error("导入失败，已回滚：{0}")]
    ImportRollback(String),
    #[error("FCS 解析失败：{0}")]
    Fcs(String),
    #[error("近奇异补偿矩阵，拒绝默默求逆：rcond={rcond:.3e} 低于阈值 {threshold:.0e}；已保留全部合法候选")]
    NearSingular { rcond: f64, threshold: f64 },
    #[error("版本冲突：期望基础版本 {expected}，当前版本 {current}")]
    VersionConflict { expected: i64, current: i64 },
    #[error("找不到对象：{0}")]
    NotFound(String),
    #[error("门嵌套非法：{0}")]
    BadHierarchy(String),
    #[error("存储错误：{0}")]
    Db(String),
    #[error("数据校验失败：{0}")]
    Validation(String),
}

pub type Result<T> = std::result::Result<T, FlowError>;

impl From<rusqlite::Error> for FlowError {
    fn from(e: rusqlite::Error) -> Self {
        FlowError::Db(e.to_string())
    }
}
