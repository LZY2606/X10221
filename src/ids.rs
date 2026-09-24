//! 稳定身份：内容寻址的版本号与事件号。
//! 导出/导入后身份不变，因为 id 只取决于 (kind, line, branch, seq, payload)。

pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn version_id(kind: &str, line_id: &str, branch: &str, seq: i64, payload: &str) -> String {
    let input = format!("{kind}|{line_id}|{branch}|{seq}|{payload}");
    let short = fnv1a64(input.as_bytes()) & 0xffff_ffff;
    format!("{kind}:{line_id}@{branch}#{seq}:{short:08x}")
}

pub fn event_id(seq: i64, kind: &str, payload: &str) -> String {
    let input = format!("event|{seq}|{kind}|{payload}");
    let short = fnv1a64(input.as_bytes()) & 0xffff_ffff;
    format!("ev-{seq:06}-{short:08x}")
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
