use std::path::{Path, PathBuf};

fn collect(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read cases dir")
        .map(|e| e.expect("case dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let key = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
        if path.is_dir() {
            collect(&path, &format!("{key}/"), out);
        } else {
            // FNV-1a fingerprint so changed fixture bytes trigger a rebuild; no extra crate needed.
            let bytes = std::fs::read(&path).expect("read case file");
            let mut hash = 0xcbf29ce484222325u64;
            for b in &bytes {
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            out.push(format!("{key}:{hash:016x}"));
        }
    }
}

fn main() {
    let mut files = Vec::new();
    collect(Path::new("cases"), "", &mut files);
    let stamp = files.join("\n");
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cases_stamp.txt");
    std::fs::write(dest, stamp).unwrap();
    println!("cargo:rerun-if-changed=cases");
}
