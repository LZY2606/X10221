//! 派生层的可逆/规范化：asinh 变换。原始层不做清洗。

pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}
