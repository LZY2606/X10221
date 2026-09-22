use crate::models::TransformSpec;

/// 派生层规范化：导入层保留原始表示，此处才应用变换。
/// 返回 (变换值, 诊断)；无法转换留在原位。
pub fn apply_transform(spec: &TransformSpec, raw: f64) -> (Option<f64>, Vec<String>) {
    match spec {
        TransformSpec::Linear { scale, .. } => {
            if !raw.is_finite() {
                return (None, vec!["非有限值不参与线性变换".into()]);
            }
            if raw < 0.0 {
                return (
                    None,
                    vec![format!("线性通道出现负值 {raw}，保留原位不规范化")],
                );
            }
            (Some(raw * scale), vec![])
        }
        TransformSpec::Log10Offset { offset, .. } => {
            if !raw.is_finite() {
                return (None, vec!["非有限值不参与对数变换".into()]);
            }
            let shifted = raw + offset;
            if shifted <= 0.0 {
                return (
                    None,
                    vec![format!(
                        "log10 要求 raw+offset>0：{raw}+{offset}={shifted}，保留原位"
                    )],
                );
            }
            (Some(shifted.log10()), vec![])
        }
    }
}

pub fn channel_id(spec: &TransformSpec) -> &str {
    match spec {
        TransformSpec::Linear { channel_id, .. }
        | TransformSpec::Log10Offset { channel_id, .. } => channel_id,
    }
}
