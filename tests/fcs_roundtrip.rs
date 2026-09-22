use flowlineage::fcs::{write_fcs31, FcsFile};
use std::collections::BTreeMap;

#[test]
fn fcs_roundtrip_preserves_extreme_values_and_diagnostics() {
    let channels = vec!["FSC-A".to_string(), "FITC-A".to_string()];
    let events = vec![
        vec![90000.0, 18000.0],
        vec![95000.0, f64::NAN],
        vec![88000.0, -180.0],
        vec![91000.0, 300000.0], // 超量程
    ];
    let bytes = write_fcs31(&channels, &events, &BTreeMap::new());
    assert_eq!(&bytes[0..6], b"FCS3.1");
    let f = FcsFile::parse(&bytes).expect("解析");
    assert_eq!(f.events.len(), 4);
    assert_eq!(f.raw_events[1][1], "NaN");
    assert!(f.events[1][1].is_nan());
    let codes: Vec<&str> = f.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"NAN_VALUE"));
    assert!(codes.contains(&"NEGATIVE_RAW"));
    assert!(codes.contains(&"SATURATED"));
    // 单位/坐标基准保留
    assert_eq!(f.channels[0].unit, "raw detector unit");
    assert_eq!(f.channels[0].range, 262144.0);
}

#[test]
fn fcs_parse_failures_are_errors_not_partial() {
    assert!(FcsFile::parse(b"").is_err());
    assert!(FcsFile::parse(b"not an fcs file at all, make it longer than 58 bytes please!!").is_err());
}
