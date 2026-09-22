//! Tiny FCS 3.0 writer used by the case-pack generator and tests.

pub fn write(names: &[&str], ranges: &[f64], events: &[Vec<f32>]) -> Vec<u8> {
    let par = names.len();
    let tot = events.len();
    let mut text = String::new();
    let mut kv = |k: &str, v: &str| {
        text.push('\x0C');
        text.push_str(k);
        text.push('\x0C');
        text.push_str(v);
    };
    kv("$BEGINANALYSIS", "0");
    kv("$ENDANALYSIS", "0");
    kv("$BYTEORD", "1,2,3,4");
    kv("$DATATYPE", "F");
    kv("$MODE", "L");
    kv("$NEXTDATA", "0");
    kv("$PAR", &par.to_string());
    kv("$TOT", &tot.to_string());
    for (i, n) in names.iter().enumerate() {
        kv(&format!("$P{}B", i + 1), "32");
        kv(&format!("$P{}E", i + 1), "0,0");
        kv(&format!("$P{}N", i + 1), n);
        kv(&format!("$P{}R", i + 1), &format!("{}", ranges[i] as u64));
    }
    // Compute offsets: header is 58 bytes.
    let text_start = 58usize;
    // $BEGINDATA/$ENDDATA must be inside TEXT, but their values depend on TEXT
    // length; iterate to fixpoint.
    let mut data_start = 0usize;
    loop {
        let mut t = text.clone();
        let data_end_guess = data_start + tot * par * 4;
        t.push('\x0C');
        t.push_str("$BEGINDATA");
        t.push('\x0C');
        t.push_str(&data_start.to_string());
        t.push('\x0C');
        t.push_str("$ENDDATA");
        t.push('\x0C');
        t.push_str(&data_end_guess.to_string());
        t.push('\x0C');
        let new_data_start = text_start + t.len() + 1; // +1 leading delimiter
        if new_data_start == data_start {
            text = t;
            break;
        }
        data_start = new_data_start;
    }
    let data_end = data_start + tot * par * 4 - 1;
    let mut out = Vec::new();
    out.extend_from_slice(b"FCS3.0    ");
    out.extend_from_slice(format!("{:>8}{:>8}{:>8}{:>8}", text_start, text_start + text.len(), data_start, data_end).as_bytes());
    out.extend_from_slice(b"                "); // analysis offsets blank, pad header to 58
    out.push(b'\x0C');
    out.extend_from_slice(text.as_bytes());
    for ev in events {
        for v in ev {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}
