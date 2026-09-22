
/// Near-singular spillover: rows for FITC/PE/PerCP are highly collinear.
/// Determinant is non-zero but the 1-norm condition number exceeds the
/// review threshold, so inversion must be blocked pending adjudication.
fn near_singular_spillover() -> Vec<Vec<f64>> {
    vec![
        vec![1.000000, 0.320000, 0.020000],
        vec![0.300000, 1.000000, 0.319999],
        vec![0.010000, 0.319999, 1.000000],
    ]
}

fn rect(a: (f64, f64), b: (f64, f64)) -> Option<Vec<Point>> {
    let _ = (a, b);
    None
}

fn main() {
    let dir = Path::new("casepack");
    fs::create_dir_all(dir).unwrap();

    let meta = channel_meta();
    let events = build_events();
    let spill = near_singular_spillover();
    let verdict = linalg::verdict(&spill, flowline::casepack::NEAR_SINGULAR_THRESHOLD).unwrap();
    assert!(verdict.near_singular, "case pack must trigger near-singular rule");
    let ridge = linalg::ridge_unmixing(&spill, 0.01).unwrap();

    let spill_raw = format!(
        "{},{},{},{},1.0,0.32,0.02,0.30,1.0,0.319999,0.01,0.319999,1.0",
        CASE_SPILL_KEYWORD, 3,
        CHANNELS[2], CHANNELS[3], CHANNELS[4]
    );
    let fcs_bytes = encode_fcs_float32(
        &CHANNELS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        &events,
        &[(CASE_SPILL_KEYWORD.to_string(), spill_raw.clone())],
    );
    fs::write(dir.join("boundary-chamber.fcs"), &fcs_bytes).unwrap();

    let asinh = |cofactor: f64| json!({
        "channel": "", "method": "asinh", "cofactor": cofactor, "offset": 0.0,
        "input_unit": "ADC count", "output_basis": "asinh dimensionless"
    });
    let mut transforms = Vec::new();
    for name in ["FITC-A", "PE-A", "PerCP-A", "APC-A"] {
        let mut t = asinh(150.0);
        t["channel"] = json!(name);
        transforms.push(t);
    }

    // Gate geometries are rectangles in asinh space for the compensated
    // fluorescence gates and raw FSC/SSC for the lymphocyte gate.
    let gates = json!([
        {
            "gate_id": "gate:lymphocytes", "version": 1,
            "name": "Lymphocytes", "gate_kind": "rectangle",
            "x_channel": "FSC-A", "y_channel": "SSC-A",
            "transform_id": "transform:raw-scatter", "transform_version": 1,
            "rectangle": [{"x": 80000.0, "y": 60000.0}, {"x": 120000.0, "y": 110000.0}],
            "coordinates_basis": "raw ADC counts", "boundary_inclusive": true,
            "notes": "FSC/SSC scatter gate; boundary events included"
        },
        {
            "gate_id": "gate:cd3", "version": 1,
            "name": "CD3+ T cells", "gate_kind": "rectangle",
            "x_channel": "PerCP-A", "y_channel": "SSC-A",
            "transform_id": "transform:global", "transform_version": 1,
            "rectangle": [{"x": 5.5, "y": 5.0}, {"x": 7.6, "y": 7.2}],
            "coordinates_basis": "asinh(compensated ADC)", "boundary_inclusive": true,
            "notes": "CD3 gate in PerCP-Cy5.5 channel"
        },
        {
            "gate_id": "gate:cd4", "version": 1,
            "name": "CD4+ T cells", "gate_kind": "rectangle",
            "x_channel": "PE-A", "y_channel": "PerCP-A",
            "transform_id": "transform:global", "transform_version": 1,
            "rectangle": [{"x": 6.0, "y": 5.9}, {"x": 7.5, "y": 7.2}],
            "coordinates_basis": "asinh(compensated ADC)", "boundary_inclusive": true,
            "notes": "CD4+ subset of CD3+"
        },
        {
            "gate_id": "gate:cd8", "version": 1,
            "name": "CD8+ T cells", "gate_kind": "rectangle",
            "x_channel": "FITC-A", "y_channel": "PerCP-A",
            "transform_id": "transform:global", "transform_version": 1,
            "rectangle": [{"x": 6.0, "y": 5.9}, {"x": 7.5, "y": 7.2}],
            "coordinates_basis": "asinh(compensated ADC)", "boundary_inclusive": true,
            "notes": "CD8+ subset of CD3+"
        },
        {
            "gate_id": "gate:cd19", "version": 1,
            "name": "CD19+ B cells", "gate_kind": "rectangle",
            "x_channel": "APC-A", "y_channel": "SSC-A",
            "transform_id": "transform:global", "transform_version": 1,
            "rectangle": [{"x": 6.0, "y": 5.0}, {"x": 7.5, "y": 7.2}],
            "coordinates_basis": "asinh(compensated ADC)", "boundary_inclusive": true,
            "notes": "B-cell gate"
        }
    ]);

    let manifest = json!({
        "case_id": CASE_ID,
        "title": "Flow boundary chamber: compensation matrix, gate lineage, population uncertainty",
        "narrative": [
            "Import FCS events, single-stain controls and a draft gate tree.",
            "Review separately versioned raw channels, spillover metadata, compensation matrix, transforms, gates and lineage.",
            "Spillover is near-singular: silent inversion is blocked; reviewer must pick a candidate.",
            "After adjudication, revise a transform (local impact) and an upstream gate (transitive stale propagation).",
            "Rename a nested gate, then trigger a same-version concurrent save conflict in two windows."
        ],
        "fcs_file": "boundary-chamber.fcs",
        "single_stain_controls": [
            {"control_id": "ss:fitc", "channel": "FITC-A", "fluorophore": "FITC", "event_indexes": [20, 21], "unit": "ADC count"},
            {"control_id": "ss:pe", "channel": "PE-A", "fluorophore": "PE", "event_indexes": [22, 23], "unit": "ADC count"},
            {"control_id": "ss:percp", "channel": "PerCP-A", "fluorophore": "PerCP-Cy5.5", "event_indexes": [24, 25], "unit": "ADC count"}
        ],
        "channels": meta,
        "spillover": {
            "keyword": CASE_SPILL_KEYWORD,
            "detectors": [CHANNELS[2], CHANNELS[3], CHANNELS[4]],
            "fluorophores": ["FITC", "PE", "PerCP-Cy5.5"],
            "matrix": spill,
            "unit": {"scale": "fraction of primary detector signal", "basis": "instrument $SPILL row-normalised coefficients"},
            "raw_representation": spill_raw,
            "condition_number_1norm": verdict.condition_number_1norm,
            "near_singular": true
        },
        "ridge_lambda": 0.01,
        "ridge_unmixing": ridge,
        "transforms": transforms,
        "raw_scatter_transform": {
            "transform_id": "transform:raw-scatter", "version": 1,
            "channel": "FSC-A/SSC-A", "method": "linear", "cofactor": 1.0, "offset": 0.0,
            "input_unit": "ADC count", "output_basis": "ADC count"
        },
        "gates": gates,
        "lineage": [
            {"gate_id": "gate:lymphocytes", "parent_gate_id": null},
            {"gate_id": "gate:cd3", "parent_gate_id": "gate:lymphocytes"},
            {"gate_id": "gate:cd4", "parent_gate_id": "gate:cd3"},
            {"gate_id": "gate:cd8", "parent_gate_id": "gate:cd3"},
            {"gate_id": "gate:cd19", "parent_gate_id": "gate:lymphocytes"}
        ],
        "comp_required_gates": ["gate:cd3", "gate:cd4", "gate:cd8", "gate:cd19"],
        "expected_event_count": events.len()
    });
    fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    println!(
        "wrote casepack: {} FCS events; cond1={:.3e} rcond={:.3e}",
        events.len(),
        verdict.condition_number_1norm,
        verdict.reciprocal_condition
    );
    let _ = rect;
}
