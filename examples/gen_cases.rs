//! Generates the offline domain case pack under `cases/`.
//! Run: cargo run --example gen_cases
//!
//! Materials stay in domain formats: real FCS 3.1 files for events, a
//! spillover CSV mirroring the $SPILL keyword word, JSON only for transform /
//! gate / hierarchy layers. The manifest drives a deterministic replay that
//! walks the full narrative instead of presenting unrelated buttons.

use flowline::domain::{Coord, Point, Unit, UnitBasis};
use flowline::fcs::write_fcs31_float;
use std::fs;
use std::path::PathBuf;

fn channels() -> Vec<(String, String)> {
    vec![
        ("FITC".into(), "CD4-FITC".into()),
        ("PE".into(), "CD8-PE".into()),
    ]
}

fn spill_csv(det: &[&str], fluo: &[&str], matrix: &[[f64; 2]]) -> String {
    let mut s = String::new();
    s.push_str(&format!("{},{},{}\n", det.len(), det.join(","), fluo.join(",")));
    for row in matrix {
        s.push_str(&format!("{},{}\n", row[0], row[1]));
    }
    s
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let case_dir = root.join("cases/flow-boundary");
    fs::create_dir_all(case_dir.join("fcs")).unwrap();
    fs::create_dir_all(case_dir.join("layers")).unwrap();

    // --- Baseline events (float32 bit patterns are preserved by FCS). -------
    // Event layout: [FITC, PE]. e7 sits exactly on a gate edge after transform;
    // e8 carries an extreme finite value kept at the raw layer.
    let baseline: [f32; 18] = [
        10.0, 10.0,   // e0 background
        950.0, 40.0,  // e1 FITC+
        920.0, 55.0,  // e2 FITC+
        35.0, 880.0,  // e3 PE+
        60.0, 905.0,  // e4 PE+
        500.0, 480.0,// e5 double+
        30.0, 30.0,  // e6 background
        800.0, 60.0, // e7 FITC+ boundary candidate
        1.0e30, 20.0,// e8 extreme signal, retained
        22.0, 24.0,  // e9 background
    ];
    let mut spill_kw = spill_csv(
        &["FITC", "PE"],
        &["FITC", "PE"],
        &[[1.0, 0.05], [0.02, 1.0]],
    );
    spill_kw.pop(); // no trailing newline inside the keyword
    let fcs_bytes = write_fcs31_float(&channels(), &baseline, &[("$SPILL".into(), spill_kw)]);
    fs::write(case_dir.join("fcs/baseline.fcs"), &fcs_bytes).unwrap();

    // --- Near-singular single stain: FITC/PE detectors perfectly collinear. -
    let collinear: [f32; 12] = [
        100.0, 100.0,
        500.0, 500.0,
        900.0, 900.0,
        700.0, 700.0,
        200.0, 200.0,
        450.0, 450.0,
    ];
    fs::write(
        case_dir.join("fcs/single-stain-singular.fcs"),
        write_fcs31_float(&channels(), &collinear, &[]),
    )
    .unwrap();

    // --- Well-conditioned single stain control. ----------------------------
    let control: [f32; 12] = [
        900.0, 30.0,
        850.0, 45.0,
        40.0, 900.0,
        55.0, 870.0,
        20.0, 20.0,
        950.0, 60.0,
    ];
    fs::write(
        case_dir.join("fcs/single-stain.fcs"),
        write_fcs31_float(&channels(), &control, &[]),
    )
    .unwrap();

    // --- Layer versions -----------------------------------------------------
    let basis = UnitBasis { unit: Unit::RawFcsPulse, basis: "$PnE=0,0 linear pulse".into() };
    let pt = |x: f64, y: f64| Point {
        x: Coord { basis: "asinh:cofactor=150".into(), value: x },
        y: Coord { basis: "asinh:cofactor=150".into(), value: y },
    };
    // Polygon in asinh space tuned so e7 lands on the lower edge (boundary).
    let fitc_gate = serde_json::json!({
        "gate_key": "fitc_pos_v1",
        "label": "CD4 FITC+",
        "channels": ["FITC", "PE"],
        "transform_method": "asinh",
        "edge_policy_proposal": "exclude",
        "points": [pt(1.85, 0.02), pt(4.0, 0.02), pt(4.0, 3.2), pt(1.85, 2.6)]
    });
    fs::write(case_dir.join("layers/gate-fitc.v1.json"), fitc_gate.to_string()).unwrap();

    let fitc_gate_v2 = serde_json::json!({
        "gate_key": "fitc_pos_v2",
        "label": "CD4 FITC+ (tightened)",
        "channels": ["FITC", "PE"],
        "transform_method": "asinh",
        "edge_policy_proposal": "exclude",
        "points": [pt(2.05, 0.02), pt(4.0, 0.02), pt(4.0, 3.2), pt(2.05, 2.4)]
    });
    fs::write(case_dir.join("layers/gate-fitc.v2.json"), fitc_gate_v2.to_string()).unwrap();

    let pe_gate = serde_json::json!({
        "gate_key": "pe_pos_v1",
        "label": "CD8 PE+",
        "channels": ["PE", "FITC"],
        "transform_method": "asinh",
        "edge_policy_proposal": "exclude",
        "points": [pt(1.85, 0.02), pt(4.0, 0.02), pt(4.0, 3.2), pt(1.85, 2.6)]
    });
    fs::write(case_dir.join("layers/gate-pe.v1.json"), pe_gate.to_string()).unwrap();

    let hier_v1 = serde_json::json!({
        "nodes": [
            {"population_key": "cd4", "gate_key": "fitc_pos_v1", "parent_population_key": null},
            {"population_key": "cd8", "gate_key": "pe_pos_v1", "parent_population_key": "cd4"}
        ]
    });
    fs::write(case_dir.join("layers/hierarchy.v1.json"), hier_v1.to_string()).unwrap();

    // v2: same gate lineage but the population is renamed/re-parented in one
    // atomic hierarchy revision.
    let hier_v2 = serde_json::json!({
        "nodes": [
            {"population_key": "cd4_tcells", "gate_key": "fitc_pos_v1", "parent_population_key": null},
            {"population_key": "cd8_tcells", "gate_key": "pe_pos_v1", "parent_population_key": "cd4_tcells"}
        ]
    });
    fs::write(case_dir.join("layers/hierarchy.v2.json"), hier_v2.to_string()).unwrap();

    let xform_v1 = serde_json::json!({
        "method": "asinh",
        "channel_order": ["FITC", "PE"],
        "cofactor": 150.0,
        "unit_in": "compensated_channel_unit",
        "unit_out": "asinh_dimensionless"
    });
    fs::write(case_dir.join("layers/transform.v1.json"), xform_v1.to_string()).unwrap();
    let xform_v2 = serde_json::json!({
        "method": "asinh",
        "channel_order": ["FITC", "PE"],
        "cofactor": 120.0,
        "unit_in": "compensated_channel_unit",
        "unit_out": "asinh_dimensionless"
    });
    fs::write(case_dir.join("layers/transform.v2.json"), xform_v2.to_string()).unwrap();

    // Matrices as domain CSV plus JSON metadata.
    fs::write(
        case_dir.join("layers/spill.well.csv"),
        spill_csv(&["FITC", "PE"], &["FITC", "PE"], &[[1.0, 0.05], [0.02, 1.0]]),
    )
    .unwrap();
    fs::write(
        case_dir.join("layers/spill.singular.csv"),
        spill_csv(&["FITC", "PE"], &["FITC", "PE"], &[[1.0, 1.0], [1.0, 1.0]]),
    )
    .unwrap();

    let manifest = serde_json::json!({
        "case_pack": "flow-boundary",
        "schema": "flowline.case/1",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "channels": [
            {"index": 0, "short_name": "FITC", "long_name": "CD4-FITC", "basis": basis},
            {"index": 1, "short_name": "PE", "long_name": "CD8-PE", "basis": basis}
        ],
        "stages": [
            {"seq": 0, "label": "导入：原始通道值/spillover/补偿/变换/门/父子关系分开版本化",
             "kind": "import_baseline",
             "fcs": "fcs/baseline.fcs",
             "single_stains": [
                {"fluorochrome": "FITC", "detector_channel": "FITC", "fcs": "fcs/single-stain.fcs"}
             ],
             "spill_csv": "layers/spill.well.csv",
             "transform": "layers/transform.v1.json",
             "gates": ["layers/gate-fitc.v1.json", "layers/gate-pe.v1.json"],
             "hierarchy": "layers/hierarchy.v1.json"},
            {"seq": 1, "label": "参数修订：asinh cofactor 150 -> 120（变换层新版本）",
             "kind": "revise_layer", "stream": "transform_params:baseline",
             "file": "layers/transform.v2.json", "expect_invalidates": ["all gates", "all population_pct"]},
            {"seq": 2, "label": "局部源数据改动：只替换一个事件（边界点位移）",
             "kind": "patch_event", "fcs": "fcs/baseline.fcs",
             "event_index": 7, "values": [805.0, 60.0],
             "expect_scope": ["fitc gate", "cd8 percentage"]},
            {"seq": 3, "label": "近奇异/奇异补偿：单染列完全共线，禁止默默求逆",
             "kind": "swap_spill", "fcs": "fcs/single-stain-singular.fcs",
             "spill_csv": "layers/spill.singular.csv",
             "expect_status": "blocked", "expect_event_judgement": "matrix_not_inverted"},
            {"seq": 4, "label": "门嵌套改名：父子层级重命名并重挂父群",
             "kind": "revise_layer", "stream": "gate_hierarchy:baseline",
             "file": "layers/hierarchy.v2.json", "expect_rename": {"cd4": "cd4_tcells", "cd8": "cd8_tcells"}},
            {"seq": 5, "label": "冲突人工决定：同版本两个窗口并发裁定",
             "kind": "conflict_scenario",
             "decision_slot": "gate_edge:fitc_pos_v1",
             "candidate_a": "fitc_pos_v1_edge_include",
             "candidate_b": "fitc_pos_v1_edge_exclude",
             "expect_second_save": "409_conflict"}
        ],
        "judgements": [
            {"id": "matrix_not_inverted",
             "rule": "near_singular_or_singular_matrix_must_not_be_silently_inverted",
             "machine": {"type": "compensation_invert_judgement", "threshold": 1e-10}},
            {"id": "pct_denominator_pinned",
             "rule": "gate_percentage_denominator_pinned_to_parent_result_version",
             "machine": {"type": "denominator_pin"}},
            {"id": "upstream_stale_no_overwrite",
             "rule": "revised_upstream_gate_marks_downstream_stale_without_overwriting_history",
             "machine": {"type": "invalidation_propagation", "overwrite": false}}
        ]
    });
    fs::write(case_dir.join("case.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    println!("case pack written to {}", case_dir.display());
}
