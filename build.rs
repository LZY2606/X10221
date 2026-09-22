//! 构建时从确定性生成器写出 FCS 领域材料。
//! 生成器在 src/fcs 内、仅依赖 std，build.rs 通过独立模块引用。

#[path = "src/fcs_build.rs"]
mod fcs_build;

use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/fcs_build.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let base = Path::new("cases/flow/baseline");
    let rev = Path::new("cases/flow/revisions");
    fs::create_dir_all(base).unwrap();
    fs::create_dir_all(rev).unwrap();

    let files = fcs_build::generate();
    for (relpath, bytes) in files {
        let p = Path::new(&relpath);
        write_atomic(p, &bytes);
        println!("cargo:rerun-if-changed={relpath}");
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    // 确定性：若现有内容一致则不重写（避免触发额外重建）。
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return;
        }
    }
    let tmp = path.with_extension("fcs.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}
