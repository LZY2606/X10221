//! 稳定身份生成：创建时分配、持久化、随导出/导入保留。

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成一个进程内唯一且跨导出/导入稳定的身份串（128 位十六进制）。
pub fn new_id() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h1 = std::collections::hash_map::RandomState::new().build_hasher();
    h1.write_u64(n);
    h1.write_u64(std::process::id() as u64);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
    h2.write_u64(a ^ 0x9e37_79b9_7f4a_7c15);
    h2.write_u64(n.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    let b = h2.finish();
    format!("{a:016x}{b:016x}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_are_unique() {
        let a = super::new_id();
        let b = super::new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
