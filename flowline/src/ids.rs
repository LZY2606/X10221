//! 稳定身份。
//!
//! - [`StableId`]：跨版本/跨导出导入不变的实体身份（如门 `g-singlets`）。
//! - [`Oid`]：某次版本化落库后分配的不可变对象 ID（`art_...` / `evt_...` / `res_...`），
//!   每个显示值都带一个这样的稳定身份，并排视图与历史回放都引用它。
//! - [`Stid`]：来源字节身份（FNV-1a 64bit，非加密用途），导入后任何修订都会得到新 stid，
//!   用于精确判断“同一份材料”。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Oid(pub String);

impl Oid {
    pub fn new(prefix: &str, n: u64) -> Self {
        Oid(format!("{}_{:012x}", prefix, n))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Oid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StableId(pub String);

impl StableId {
    pub fn new(s: impl Into<String>) -> Self {
        StableId(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 来源字节指纹（FNV-1a 64 bit）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Stid(pub u64);

impl Stid {
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut h: u64 = 0xcbf29ce484222325;
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        Stid(h)
    }
    pub fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// 毫秒级 Unix 时间戳；测试中显式注入以保证可复现。
pub fn ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 进程内序号器（OID 分配）。
#[derive(Debug, Default)]
pub struct Seq {
    n: AtomicU64,
}

impl Seq {
    pub const fn new(start: u64) -> Self {
        Seq {
            n: AtomicU64::new(start),
        }
    }
    pub fn next(&self, prefix: &str) -> Oid {
        Oid::new(prefix, self.n.fetch_add(1, Ordering::SeqCst) + 1)
    }
}
