//! 生成仓库自带离线案例包“流式边界室：补偿矩阵、门控系谱与细胞群不确定性”。
//! 产物全部写入 cases/flow-boundary-chamber/，可重复生成（确定性）。
use flowlineage::fcs::write_fcs31;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "cases/flow-boundary-chamber".into());
    let root = Path::new(&out);
    fs::create_dir_all(root.join("samples")).unwrap();

    let channels: Vec<String> = vec![
        "FSC-A".into(),
        "SSC-A".into(),
        "FITC-A".into(),
        "PE-A".into(),
        "APC-A".into(),
        "PerCP-A".into(),
    ];

    // ---- 样本事件（原始 detector unit；含极端、边界、负、NaN）----
    let lymph = |fitc: f64, pe: f64, apc: f64, percp: f64| {
        vec![90000.0, 60000.0, fitc, pe, apc, percp]
    };
    let debris = |i: f64| vec![4000.0 + i, 3000.0, 200.0, 200.0, 150.0, 150.0];
    let mut events: Vec<Vec<f64>> = Vec::new();
    // 10 个淋巴非 T：FITC/APC 低
    for i in 0..10 {
        events.push(lymph(300.0 + (i as f64) * 40.0, 500.0, 400.0, 300.0));
    }
    // 7 个 T CD4：FITC 高、APC 低
    for i in 0..7 {
        events.push(lymph(18000.0 + (i as f64) * 120.0, 14000.0 + i as f64, 600.0, 500.0));
    }
    // 5 个 T CD8：FITC 高、APC 高
    for i in 0..5 {
        events.push(lymph(17000.0 + (i as f64) * 100.0, 1500.0, 22000.0 + i as f64 * 90.0, 700.0));
    }
    // 1 个边界点：恰好落在 CD3 门左竖边 x=4.38、y∈[0.80,5.30] 的中点上
    {
        let cof = 150.0f64;
        let edge_x = 4.3000f64; // 恰在 r1 左竖边上（验证含边界规则），在 r2 门外
        let mid_y: f64 = (0.00 + 5.30) / 2.0;
        let raw_x = edge_x.sinh() * cof;
        let raw_y = mid_y.sinh() * cof;
        events.push(lymph(raw_x, raw_y, 500.0, 400.0));
    }
    // 极端：饱和（荧光超量程），同时 FSC/SSC 远在淋巴门外
    events.push(vec![300000.0, 260000.0, 300000.0, 280000.0, 900.0, 800.0]);
    // 极端：负基线荧光信号，FSC/SSC 也落在淋巴门外
    events.push(vec![205000.0, 175000.0, -180.0, -90.0, -40.0, 300.0]);
    // 极端：NaN（保留原位，不能丢），FSC/SSC 同样在门外
    events.push(vec![160000.0, 140000.0, f64::NAN, 700.0, 500.0, 400.0]);
    // 碎屑 2 个
    events.push(debris(10.0));
    events.push(debris(90.0));

    let mut meta = BTreeMap::new();
    meta.insert("EXPERIMENT_ID".into(), "FLOW-BOUNDARY-CHAMBER".into());
    meta.insert("$FIL".into(), "sample_events.fcs".into());
    let sample = write_fcs31(&channels, &events, &meta);
    fs::write(root.join("samples/sample_events.fcs"), &sample).unwrap();

    // ---- 单染对照（用于 spillover 元数据溯源）----
    let ss_events: Vec<Vec<f64>> = (0..20)
        .map(|i| {
            vec![
                80000.0,
                50000.0,
                1000.0 + (i as f64) * 30.0,
                900.0,
                850.0,
                700.0,
            ]
        })
        .collect();
    let mut meta2 = BTreeMap::new();
    meta2.insert("EXPERIMENT_ID".into(), "FLOW-BOUNDARY-CHAMBER-SS".into());
    meta2.insert("$FIL".into(), "single_stain_fitc.fcs".into());
    let ss = write_fcs31(&channels, &ss_events, &meta2);
    fs::write(root.join("samples/single_stain_fitc.fcs"), &ss).unwrap();

    // 修订版单染（数值略调，仅影响 spillover → 补偿，门几何等不受影响）
    let ss2_events: Vec<Vec<f64>> = (0..20)
        .map(|i| {
            vec![
                80000.0,
                50000.0,
                1100.0 + (i as f64) * 28.0,
                940.0,
                830.0,
                710.0,
            ]
        })
        .collect();
    let mut meta3 = meta2.clone();
    meta3.insert("$FIL".into(), "single_stain_fitc_rev2.fcs".into());
    let ss2 = write_fcs31(&channels, &ss2_events, &meta3);
    fs::write(root.join("samples/single_stain_fitc_rev2.fcs"), &ss2).unwrap();

    println!("案例包已生成: {}", root.display());
}
