use flowlineage::domain::math::*;

#[test]
fn well_conditioned_matrix_inverts_and_compensates() {
    let m = vec![
        vec![1.0, 0.02, 0.01, 0.005],
        vec![0.10, 1.0, 0.03, 0.01],
        vec![0.02, 0.05, 1.0, 0.02],
        vec![0.01, 0.01, 0.04, 1.0],
    ];
    let v = assess_matrix(&m);
    assert!(!v.singular && !v.near_singular);
    assert!(v.inverse.is_some());
    assert!(v.candidates.is_empty());
    assert!(v.condition_number.unwrap() < NEAR_SINGULAR_COND);

    let inv = v.inverse.unwrap();
    let out = apply_compensation(&[1000.0, 500.0, 300.0, 200.0], &inv);
    assert_eq!(out.len(), 4);
    // 补偿后主要荧光信号应低于含溢出的原始值（FITC 通道）
    assert!(out[0] < 1000.0, "补偿后 FITC 应被拉低，实际 {out:?}");
}

#[test]
fn exact_singular_is_not_inverted_silently() {
    let mut m = vec![
        vec![1.0, 0.02, 0.01, 0.005],
        vec![1.0, 0.02, 0.01, 0.005],
        vec![0.02, 0.05, 1.0, 0.02],
        vec![0.01, 0.01, 0.04, 1.0],
    ];
    let v = assess_matrix(&m);
    assert!(v.singular);
    assert!(v.inverse.is_none());
    assert!(v.candidates.is_empty(), "精确奇异不给候选");
    // 非方阵也判定奇异
    m.push(vec![1.0, 2.0]);
    assert!(assess_matrix(&m).singular);
}

#[test]
fn near_singular_keeps_all_candidates_without_auto_inverse() {
    let m = vec![
        vec![1.0, 1.0000005, 0.01, 0.005],
        vec![1.0000005, 1.0, 0.03, 0.01],
        vec![0.02, 0.05, 1.0, 0.02],
        vec![0.01, 0.01, 0.04, 1.0],
    ];
    let v = assess_matrix(&m);
    assert!(v.near_singular);
    assert!(v.inverse.is_none(), "近奇异不能默默给逆");
    let ids: Vec<&str> = v.candidates.iter().map(|c| c.candidate_id.as_str()).collect();
    assert_eq!(ids, vec!["c1-ridge-1e-3", "c2-ridge-1e-2", "c3-unstable-direct"]);
    // 不同正则强度确实给出不同逆，候选差异被保留
    assert_ne!(v.candidates[0].inverse, v.candidates[1].inverse);
}

#[test]
fn transforms_have_basis_and_log10_rejects_nonpositive() {
    let a = TransformSpec::Asinh { cofactor: 150.0 };
    assert!(a.apply(18000.0).unwrap() > 4.0);
    assert!(a.basis().contains("asinh"));
    let l = TransformSpec::Log10 { decades: 4.0 };
    assert!(l.apply(0.0).is_err());
    assert!(l.apply(-5.0).is_err());
    assert_eq!(l.apply(1000.0).unwrap(), 3.0);
    let bad = TransformSpec::Asinh { cofactor: 0.0 };
    assert!(bad.apply(1.0).is_err());
}

#[test]
fn polygon_includes_boundary_points_stably() {
    let poly = vec![
        Point { x: 0.0, y: 0.0 },
        Point { x: 10.0, y: 0.0 },
        Point { x: 10.0, y: 10.0 },
        Point { x: 0.0, y: 10.0 },
    ];
    assert!(point_in_polygon(Point { x: 5.0, y: 5.0 }, &poly));
    assert!(!point_in_polygon(Point { x: 11.0, y: 5.0 }, &poly));
    // 顶点
    assert!(point_in_polygon(Point { x: 0.0, y: 0.0 }, &poly));
    // 边的中点必须算在内，不允许抖动
    assert!(point_in_polygon(Point { x: 5.0, y: 0.0 }, &poly));
    assert!(point_in_polygon(Point { x: 10.0, y: 5.0 }, &poly));
    // 退化多边形
    assert!(!point_in_polygon(Point { x: 1.0, y: 1.0 }, &[Point { x: 0.0, y: 0.0 }, Point { x: 1.0, y: 1.0 }]));
}
