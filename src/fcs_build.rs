//! build.rs 专用：仅 std 的 FCS 编码与确定性案例生成（镜像 src/fcs 的领域规则）。
#![allow(clippy::all)]

mod types {
    #[derive(Debug, Clone)]
    pub struct SpilloverMatrix {
        pub detectors: Vec<String>,
        pub fluorochromes: Vec<String>,
        pub rows: Vec<Vec<f64>>,
        pub source: String,
    }
    #[derive(Debug, Clone)]
    pub struct ChannelSpec {
        pub name: String,
        pub unit: String,
        pub range: Option<f64>,
    }
}
use types::*;

pub fn channels() -> Vec<ChannelSpec> {
    vec![
        ChannelSpec { name: "FSC-A".into(), unit: "a.u.".into(), range: Some(262143.0) },
        ChannelSpec { name: "SSC-A".into(), unit: "a.u.".into(), range: Some(262143.0) },
        ChannelSpec { name: "FITC-A".into(), unit: "channels".into(), range: Some(262143.0) },
        ChannelSpec { name: "PE-A".into(), unit: "channels".into(), range: Some(262143.0) },
        ChannelSpec { name: "PerCP-A".into(), unit: "channels".into(), range: Some(262143.0) },
    ]
}

pub const FLUOR: [&str; 3] = ["FITC-A", "PE-A", "PerCP-A"];

pub fn spill_well() -> SpilloverMatrix {
    SpilloverMatrix {
        detectors: names(), fluorochromes: names(),
        rows: vec![
            vec![1.0, 0.08, 0.05],
            vec![0.12, 1.0, 0.06],
            vec![0.04, 0.10, 1.0],
        ],
        source: "single-stain controls (FCS $SPILLOVER)".into(),
    }
}
fn names() -> Vec<String> {
    FLUOR.iter().map(|s| s.to_string()).collect()
}
pub fn spill_near() -> SpilloverMatrix {
    SpilloverMatrix {
        detectors: names(), fluorochromes: names(),
        rows: vec![
            vec![1.0, 0.08, 0.05],
            vec![0.12, 1.0, 0.06],
            vec![0.1596, 1.0020, 0.0795],
        ],
        source: "single-stain controls (revision 2, near-singular)".into(),
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        let r = x.wrapping_mul(0x2545F4914F6CDD1D);
        (r >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        let u1 = self.next().max(1e-12);
        let u2 = self.next();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

pub fn asinh_fwd(x: f64) -> f64 { (x / 150.0).asinh() }
pub fn asinh_inv(y: f64) -> f64 { y.sinh() * 150.0 }

fn compensated_events(seed: u64) -> Vec<[f64; 5]> {
    let mut rng = Rng(seed);
    let mut out: Vec<[f64; 5]> = Vec::new();
    macro_rules! push {
        ($fsc:expr,$ssc:expr,$f:expr,$p:expr,$pc:expr) => {
            out.push([$fsc, $ssc, $f, $p, $pc]);
        };
    }
    for _ in 0..46 {
        push!(90000.0 + rng.gauss()*9000.0, 55000.0 + rng.gauss()*8000.0,
              12000.0 + rng.gauss()*2500.0, 40000.0 + rng.gauss()*6000.0, (600.0 + rng.gauss()*400.0).max(0.0));
    }
    for _ in 0..40 {
        push!(95000.0 + rng.gauss()*8000.0, 60000.0 + rng.gauss()*7000.0,
              13000.0 + rng.gauss()*2500.0, (500.0 + rng.gauss()*300.0).max(0.0), 35000.0 + rng.gauss()*5000.0);
    }
    for _ in 0..24 {
        push!(88000.0 + rng.gauss()*7000.0, 52000.0 + rng.gauss()*7000.0,
              (700.0+rng.gauss()*300.0).max(0.0), (600.0+rng.gauss()*300.0).max(0.0), (500.0+rng.gauss()*300.0).max(0.0));
    }
    for _ in 0..30 {
        push!(160000.0 + rng.gauss()*12000.0, 150000.0 + rng.gauss()*15000.0,
              20000.0 + rng.gauss()*5000.0, 18000.0 + rng.gauss()*5000.0, 12000.0 + rng.gauss()*4000.0);
    }
    for _ in 0..16 {
        push!(25000.0 + rng.gauss()*5000.0, 20000.0 + rng.gauss()*4000.0,
              (400.0+rng.gauss()*200.0).max(0.0), (400.0+rng.gauss()*200.0).max(0.0), (400.0+rng.gauss()*200.0).max(0.0));
    }
    // 极端信号
    push!(92000.0, 58000.0, 262143.0, 900.0, 700.0);
    push!(90000.0, 60000.0, 800.0, 262143.0, 600.0);
    push!(89000.0, 54000.0, 0.0, 0.0, 0.0);
    push!(91000.0, 57000.0, -1200.0, 500.0, -300.0);
    push!(262143.0, 262143.0, 262143.0, 262143.0, 262143.0);
    // 边界点（half_open：上升沿排除，水平/下降沿包含）
    push!(70000.0, 30000.0, asinh_inv(4.0), 500.0, 500.0);          // lymph 上升沿 -> 排除
    push!(95000.0, 55000.0, asinh_inv(3.5), 500.0, 500.0);          // cd3 上升沿 -> 排除
    push!(95000.0, 60000.0, asinh_inv(4.0), 500.0, 500.0);          // lymph 水平下沿 -> 包含；cd3 内部
    out
}

fn forward_spill(events: &mut [[f64; 5]], s: &SpilloverMatrix) {
    for ev in events.iter_mut() {
        let c = [ev[2], ev[3], ev[4]];
        for r in 0..3 {
            ev[2 + r] = s.rows[r][0]*c[0] + s.rows[r][1]*c[1] + s.rows[r][2]*c[2];
        }
    }
}

fn main_tube(seed: u64, s: &SpilloverMatrix) -> Vec<Vec<f64>> {
    let mut ev = compensated_events(seed);
    forward_spill(&mut ev, s);
    ev.into_iter().map(|a| a.to_vec()).collect()
}

fn single_stain(target: usize, s: &SpilloverMatrix) -> Vec<Vec<f64>> {
    let mut rng = Rng(0x9E3779B97F4A7C15 ^ ((target as u64) << 32));
    let mut ev: Vec<[f64; 5]> = Vec::new();
    for _ in 0..120 {
        let mut c = [70000.0 + rng.gauss()*14000.0, 45000.0 + rng.gauss()*9000.0,
                     300.0 + rng.gauss()*200.0, 300.0 + rng.gauss()*200.0, 300.0 + rng.gauss()*200.0];
        c[2 + target] = 55000.0 + rng.gauss()*9000.0;
        ev.push(c);
    }
    forward_spill(&mut ev, s);
    ev.into_iter().map(|a| a.to_vec()).collect()
}

/// 只影响局部依赖的源数据改动：新增一个 CD4+ 事件并调整一个 DN 事件。
fn revised_tube(s: &SpilloverMatrix) -> Vec<Vec<f64>> {
    let mut ev = compensated_events(0xABCDEF);
    ev.push([92000.0, 58000.0, 12500.0, 42000.0, 600.0]); // 额外 CD4+
    if let Some(e) = ev.iter_mut().find(|e| e[2] < 2000.0 && e[0] < 35000.0) {
        e[0] = 90000.0; // 一粒碎屑被划入淋巴（只影响 g_lymph 局部）
    }
    forward_spill(&mut ev, s);
    ev.into_iter().map(|a| a.to_vec()).collect()
}

fn spill_keyword(s: &SpilloverMatrix) -> String {
    let mut parts: Vec<String> = vec!["3".into()];
    parts.extend(s.fluorochromes.iter().cloned());
    for row in &s.rows {
        for v in row { parts.push(format!("{v:.6}")); }
    }
    parts.join(",")
}

fn encode_fcs(channels: &[ChannelSpec], events: &[Vec<f64>], extra: &std::collections::BTreeMap<String,String>) -> Vec<u8> {
    let mut kv: std::collections::BTreeMap<String,String> = std::collections::BTreeMap::new();
    kv.insert("FCS3.1".into(), " ".into());
    kv.insert("$BEGINANALYSIS".into(), "0".into());
    kv.insert("$ENDANALYSIS".into(), "0".into());
    kv.insert("$BEGINSTEXT".into(), "0".into());
    kv.insert("$ENDSTEXT".into(), "0".into());
    kv.insert("$BYTEORD".into(), "1,2,3,4".into());
    kv.insert("$DATATYPE".into(), "F".into());
    kv.insert("$MODE".into(), "L".into());
    kv.insert("$NEXTDATA".into(), "0".into());
    kv.insert("$PAR".into(), channels.len().to_string());
    kv.insert("$TOT".into(), events.len().to_string());
    for (i, ch) in channels.iter().enumerate() {
        let n = i + 1;
        kv.insert(format!("$P{n}N"), ch.name.clone());
        kv.insert(format!("$P{n}U"), ch.unit.clone());
        if let Some(r) = ch.range { kv.insert(format!("$P{n}R"), (r as u64).to_string()); }
        kv.insert(format!("$P{n}B"), "32".into());
        kv.insert(format!("$P{n}E"), "0,0".into());
    }
    for (k, v) in extra { if k != "FCS3.1" { kv.insert(k.clone(), v.clone()); } }
    let mut data = Vec::new();
    for ev in events { for v in ev { data.extend_from_slice(&(*v as f32).to_le_bytes()); } }
    let text_start = 58u64;
    kv.insert("$BEGINDATA".into(), "000000000000".into());
    kv.insert("$ENDDATA".into(), "000000000000".into());
    let render = |kv: &std::collections::BTreeMap<String,String>| {
        let mut t = String::from('/');
        for (k, v) in kv {
            t.push_str(k); t.push('/'); t.push_str(&v.replace('/', "//")); t.push('/');
        }
        t
    };
    let probe = render(&kv);
    let data_start = text_start + probe.len() as u64;
    let data_stop = data_start + data.len() as u64 - 1;
    kv.insert("$BEGINDATA".into(), format!("{data_start:012}"));
    kv.insert("$ENDDATA".into(), format!("{data_stop:012}"));
    let text = render(&kv);
    let text_stop = data_start - 1;
    let header = format!("FCS3.1     {text_start:>8}{text_stop:>8}{data_start:>8}{data_stop:>8}0       0       ");
    assert_eq!(header.len(), 58);
    let mut out = header.into_bytes();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(&data);
    out
}

fn keywords(role: &str, s: Option<&SpilloverMatrix>) -> std::collections::BTreeMap<String,String> {
    let mut kv = std::collections::BTreeMap::new();
    kv.insert("$FIL".to_string(), format!("{role}.fcs"));
    kv.insert("$SRC".to_string(), "flow-lineage offline case pack".to_string());
    kv.insert("GTUNITS".to_string(), "a.u.,a.u.,channels,channels,channels".to_string());
    if let Some(s) = s { kv.insert("$SPILLOVER".to_string(), spill_keyword(s)); }
    kv
}

pub fn generate() -> Vec<(&'static str, Vec<u8>)> {
    let ch = channels();
    let s = spill_well();
    let main = main_tube(0x1234_5678, &s);
    let mut out = vec![
        ("cases/flow/baseline/main_tube.fcs", encode_fcs(&ch, &main, &keywords("main_tube", Some(&s)))),
        ("cases/flow/baseline/sst_fitc.fcs", encode_fcs(&ch, &single_stain(0, &s), &keywords("sst_fitc", Some(&s)))),
        ("cases/flow/baseline/sst_pe.fcs", encode_fcs(&ch, &single_stain(1, &s), &keywords("sst_pe", Some(&s)))),
        ("cases/flow/baseline/sst_percp.fcs", encode_fcs(&ch, &single_stain(2, &s), &keywords("sst_percp", Some(&s)))),
    ];
    // 近奇异矩阵对应的试管（原始字节按新 S 反推，第二分支使用）。
    let sn = spill_near();
    let near_main = main_tube(0x1234_5678, &sn);
    out.push(("cases/flow/revisions/main_tube_near_singular.fcs", encode_fcs(&ch, &near_main, &keywords("main_tube_near_singular", Some(&sn)))));
    // 只影响局部依赖的源数据修订（仍使用良态矩阵）。
    let rev = revised_tube(&s);
    out.push(("cases/flow/revisions/main_tube_revised.fcs", encode_fcs(&ch, &rev, &keywords("main_tube_revised", Some(&s)))));
    out
}
