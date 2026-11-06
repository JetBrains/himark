// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SHELL header: what the workbench's C surface and this
//! assembly crate add beyond the engine header (himark-ffi's
//! `himark.h`). The REVIEW FILE — the target is for it to shrink
//! to nothing as the window road retires.

fn generate(crate_dir: &str, with_header: bool) -> String {
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let mut config = cbindgen::Config::from_root_or_default(&root);
    if !with_header {
        config.header = None;
        config.trailer = None;
        config.include_guard = None;
        config.pragma_once = false;
        config.no_includes = true;
        config.autogen_warning = None;
    }
    let dir = std::path::Path::new(&root).join(crate_dir);
    match cbindgen::Builder::new()
        .with_crate(dir)
        .with_config(config)
        .generate()
    {
        Ok(built) => {
            let mut out = Vec::new();
            built.write(&mut out);
            String::from_utf8(out).expect("utf8 header")
        }
        // A stale header is a MISLEADING artifact — the failure must
        // surface, not ride silently into the app build.
        Err(error) => {
            println!("cargo:warning=cbindgen failed for {crate_dir}: {error}");
            String::new()
        }
    }
}

fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let own = generate(".", true);
    let windows = generate("../workbench-shell", false);
    let assembled = format!("{own}\n{windows}");
    std::fs::write(format!("{root}/include/himark-shell.h"), assembled).expect("write header");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../workbench-shell/src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
