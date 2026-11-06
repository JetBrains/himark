// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One header over FOUR crates: the engine bridge (himark-ffi), the
//! workbench's window surface, the tailnet face and this crate's own
//! door — cbindgen runs once per crate and the passes concatenate.

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
    let ffi = generate("../himark-ffi", false);
    let windows = generate("../workbench-shell", false);
    let tailnet = generate("../hitailnet", false);
    // The trailer (the tailnet state defines) must stay LAST; the
    // own pass carries header + trailer, so splice the dep passes in
    // before the trailer's marker.
    let assembled = match own.rfind("#define HIMARK_TAILNET_OFF") {
        Some(at) => format!("{}{}\n{}\n{}\n{}", &own[..at], ffi, windows, tailnet, &own[at..]),
        None => format!("{own}\n{ffi}\n{windows}\n{tailnet}"),
    };
    std::fs::write(format!("{root}/include/himark.h"), assembled).expect("write header");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../himark-ffi/src");
    println!("cargo:rerun-if-changed=../workbench-shell/src");
    println!("cargo:rerun-if-changed=../hitailnet/src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
