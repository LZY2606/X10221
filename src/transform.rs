use crate::domain::{Basis, ChannelTransform, Raw, TransformKind};

pub fn apply_channel(t: &ChannelTransform, raw: &Raw) -> Option<f64> {
    let x = raw.as_f64()?;
    Some(match t.kind {
        TransformKind::Linear => x,
        TransformKind::Asinh => {
            let cf = if t.cofactor > 0.0 { t.cofactor } else { 1.0 };
            (x / cf).asinh() * cf / 5.0_f64.ln().max(1e-12)
        }
    })
}

/// Output basis of a transform step.
pub fn out_basis(t: &ChannelTransform) -> Basis {
    match t.kind {
        TransformKind::Linear => Basis::Compensated,
        TransformKind::Asinh => Basis::Transformed,
    }
}

/// Build per-channel lookup of transforms applicable at a given basis.
pub fn transform_for<'a>(
    doc: &'a crate::domain::TransformDoc,
    channel: &str,
) -> Option<&'a ChannelTransform> {
    doc.channels.iter().find(|c| c.channel == channel)
}
