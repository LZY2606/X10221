use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    if !dir.exists() {
        return;
    }
    for entry in fs::read_dir(dir).expect("read asset dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            let rel = path.strip_prefix(env::current_dir().unwrap()).unwrap().to_path_buf();
            let key = rel.to_string_lossy().replace('\\', "/");
            out.push((key, path));
        }
    }
}

fn main() {
    let base = env::current_dir().unwrap();
    let mut files = Vec::new();
    collect(&base.join("case"), &mut files);
    collect(&base.join("web"), &mut files);
    let mut entries = String::new();
    for (key, _) in &files {
        entries.push_str(&format!("    ({:?}, include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/\", {:?}))),\n", key, key));
        println!("cargo:rerun-if-changed={}", key);
    }
    let generated = format!(
        "pub fn embedded_asset(path: &str) -> Option<&'static [u8]> {{\n    match path {{\n{entries}        _ => None,\n    }}\n}}\n"
    );
    let out_dir = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out_dir).join("assets.inc.rs"), generated).unwrap();
}
