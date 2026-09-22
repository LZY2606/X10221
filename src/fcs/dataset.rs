//! 离线领域案例包的确定性数据生成器（仅依赖 std）。
//!! 所有案例文件在 build.rs 中由本模块重新生成，保证 FCS 字节与门草案一致。

use super::types::SpilloverMatrix;
use super::ChannelSpec;
use std::collections::BTreeMap;

pub fn channels() -> Vec<ChannelSpec> {
    let mk = |i: usize, name: &str, unit: &str, range: Option<f64>| ChannelSpec {
        index: i,
        name: name.into(),
        unit: unit.into(),
        range,
        gain: None,
        voltage: None,
    };
    vec![
        mk(1, "FSC-A", "a.u.", Some(262143.0)),
        mk(2, "SSC-A", "a.u.", Some(262143.0)),
        mk(3, "FITC-A", "channels", Some(262143.0)),
        mk(4, "PE-A", "channels", Some(262143.0)),
        mk(5, "PerCP-A", "channels", Some(262143.0)),
    ]
}

/// 三个荧光通道的列名（与 spillover 顺序一致）。
pub const FLUOR: [&str; 3] = ["FITC-A", "PE-A", "PerCP-A"];

/// 良态 spillover：detector 行 × fluor 列。
pub fn spill_well() -> SpilloverMatrix {
    SpilloverMatrix {
        detectors: FLUOR.iter().map(|s| s.to_string()).collect(),
        fluorochromes: FLUOR.iter().map(|s| s.to_string()).collect(),
        rows: vec![
            vec![1.0, 0.08, 0.05],
            vec![0.12, 1.0, 0.06],
            vec![0.04, 0.10, 1.0],
        ],
        source: "single-stain controls (FCS $SPILLOVER)".into(),
    }
}

/// 近奇异：第 3 行几乎是前两行的线性组合，条件数很大但仍可逆。
pub fn spill_near_singular() -> SpilloverMatrix {
    SpilloverMatrix {
        detectors: FLUOR.iter().map(|s| s.to_string()).collect(),
        fluorochromes: FLUOR.iter().map(|s| s.to_string()).collect(),
        rows: vec![
            vec![1.0, 0.08, 0.05],
            vec![0.12, 1.0, 0.06],
            // 0.04*row1 + 1.0*row2 ≈ (0.16, 1.0032, 0.08)；此处刻意贴近。
            vec![0.159_6, 1.002_0, 0.079_5],
        ],
        source: "single-stain controls (revision 2, near-singular)".into(),
    }
}

/// 精确奇异：第 3 行等于第 1 行的常数倍，行列式为 0。
pub fn spill_singular() -> SpilloverMatrix {
    SpilloverMatrix {
        detectors: FLUOR.iter().map(|s| s.to_string()).collect(),
        fluorochromes: FLUOR.iter().map(|s| s.to_string()).collect(),
        rows: vec![
            vec![1.0, 0.08, 0.05],
            vec![0.12, 1.0, 0.06],
            vec![2.0, 0.16, 0.10],
        ],
        source: "single-stain controls (conflicting manual edit, singular)".into(),
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (r >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        // Box-Muller
        let u1 = self.next().max(1e-12);
        let u2 = self.next();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// 生成“补偿后真实信号”样本（FSC,SSC,FITC,PE,PerCP），随后用 S 反推原始信号。
/// 坐标基准：散色通道 a.u.，荧光 channels（18-bit ADC 上限 262143）。
pub fn main_events_compensated(seed: u64) -> Vec<[f64; 5]> {
    let mut rng = Rng(seed);
    let mut out: Vec<[f64; 5]> = Vec::new();
    let push = |out: &mut Vec<[f64; 5]>, fsc, ssc, fitc, pe, percp| {
        out.push([fsc, ssc, fitc, pe, percp]);
    };
    // 群体 A：淋巴细胞，CD3+ CD4+（FITC 中、PE 高）
    for _ in 0..46 {
        push(
            &mut out,
            90_000.0 + rng.gauss() * 9_000.0,
            55_000.0 + rng.gauss() * 8_000.0,
            12_000.0 + rng.gauss() * 2_500.0,
            40_000.0 + rng.gauss() * 6_000.0,
            600.0 + rng.gauss() * 400.0,
        );
    }
    // 群体 B：淋巴细胞，CD3+ CD8+（FITC 中、PerCP 高）
    for _ in 0..40 {
        push(
            &mut out,
            95_000.0 + rng.gauss() * 8_000.0,
            60_000.0 + rng.gauss() * 7_000.0,
            13_000.0 + rng.gauss() * 2_500.0,
            500.0 + rng.gauss() * 300.0,
            35_000.0 + rng.gauss() * 5_000.0,
        );
    }
    // 群体 C：淋巴细胞，双阴性
    for _ in 0..24 {
        push(
            &mut out,
            88_000.0 + rng.gauss() * 7_000.0,
            52_000.0 + rng.gauss() * 7_000.0,
            700.0 + rng.gauss() * 300.0,
            600.0 + rng.gauss() * 300.0,
            500.0 + rng.gauss() * 300.0,
        );
    }
    // 群体 D：髓系，FSC/SSC 较大，在淋巴门外
    for _ in 0..30 {
        push(
            &mut out,
            160_000.0 + rng.gauss() * 12_000.0,
            150_000.0 + rng.gauss() * 15_000.0,
            20_000.0 + rng.gauss() * 5_000.0,
            18_000.0 + rng.gauss() * 5_000.0,
            12_000.0 + rng.gauss() * 4_000.0,
        );
    }
    // 群体 E：碎屑，FSC 很低
    for _ in 0..16 {
        push(
            &mut out,
            25_000.0 + rng.gauss() * 5_000.0,
            20_000.0 + rng.gauss() * 4_000.0,
            400.0 + rng.gauss() * 200.0,
            400.0 + rng.gauss() * 200.0,
            400.0 + rng.gauss() * 200.0,
        );
    }
    // 极端信号：ADC 饱和、零、负（电子补偿/基线漂移），不丢弃。
    push(&mut out, 92_000.0, 58_000.0, 262143.0, 900.0, 700.0);
    push(&mut out, 90_000.0, 60_000.0, 800.0, 262143.0, 600.0);
    push(&mut out, 89_000.0, 54_000.0, 0.0, 0.0, 0.0);
    push(&mut out, 91_000.0, 57_000.0, -1_200.0, 500.0, -300.0);
    push(&mut out, 262143.0, 262143.0, 262143.0, 262143.0, 262143.0);

    // 边界点：补偿后恰好落在 CD3 门多边形的边/顶点上（见 gates 草案）。
    // CD3 门 x=FITC(asinh), y=SSC(线性归一)，此处给出线性空间对应值。
    let edge_fitc = asinh_inv(2.0);
    let vert_fitc = asinh_inv(4.0);
    push(&mut out, 90_000.0, 120_000.0, edge_fitc, 500.0, 500.0); // 边中点
    push(&mut out, 90_000.0, 40_000.0, vert_fitc, 500.0, 500.0); // 多边形顶点
    out
}

/// 标准 asinh：x' = asinh(x/cofactor)，cofactor=150（与变换参数草案一致）。
pub fn asinh_fwd(x: f64) -> f64 {
    (x / 150.0).asinh()
}
pub fn asinh_inv(y: f64) -> f64 {
    y.sinh() * 150.0
}

/// 用 S 把补偿后荧光反推为原始信号：raw_fl = S * comp_fl。
pub fn apply_spill_forward(events: &mut [[f64; 5]], s: &SpilloverMatrix) {
    for ev in events.iter_mut() {
        let comp = [ev[2], ev[3], ev[4]];
        let mut raw = [0.0f64; 3];
        for r in 0..3 {
            raw[r] = s.rows[r][0] * comp[0]
                + s.rows[r][1] * comp[1]
                + s.rows[r][2] * comp[2];
        }
        ev[2] = raw[0];
        ev[3] = raw[1];
        ev[4] = raw[2];
    }
}

pub fn main_tube_raw(seed: u64, s: &SpilloverMatrix) -> Vec<Vec<f64>> {
    let mut ev = main_events_compensated(seed);
    apply_spill_forward(&mut ev, s);
    ev.into_iter().map(|a| a.to_vec()).collect()
}

/// 单染对照：只在目标荧光通道为正，其余为背景；用同一 S 制造溢出。
pub fn single_stain(target: usize, s: &SpilloverMatrix) -> Vec<Vec<f64>> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ ((target as u64) << 32));
    let mut ev: Vec<[f64; 5]> = Vec::new();
    for _ in 0..120 {
        let mut comp = [
            70_000.0 + rng.gauss() * 14_000.0,
            45_000.0 + rng.gauss() * 9_000.0,
            300.0 + rng.gauss() * 200.0,
            300.0 + rng.gauss() * 200.0,
            300.0 + rng.gauss() * 200.0,
        ];
        // 目标荧光通道亮。
        comp[2 + target] = 55_000.0 + rng.gauss() * 9_000.0;
        ev.push(comp);
    }
    apply_spill_forward(&mut ev, s);
    ev.into_iter().map(|a| a.to_vec()).collect()
}

pub fn spill_keyword(s: &SpilloverMatrix) -> String {
    // FCS $SPILLOVER: n, labels(n), n*n 系数，逗号分隔。
    let mut parts: Vec<String> = vec!["3".into()];
    parts.extend(s.fluorochromes.iter().cloned());
    for row in &s.rows {
        for v in row {
            parts.push(format!("{v:.6}"));
        }
    }
    parts.join(",")
}

pub fn fcs_keywords(role: &str, s: Option<&SpilloverMatrix>) -> BTreeMap<String, String> {
    let mut kv = BTreeMap::new();
    kv.insert("$FIL".into(), format!("{role}.fcs"));
    kv.insert("$SRC".into(), "flow-lineage offline case pack".into());
    kv.insert("GTUNITS".into(), "a.u.,a.u.,channels,channels,channels".into());
    if let Some(s) = s {
        kv.insert("$SPILLOVER".into(), spill_keyword(s));
    }
    kv
}
