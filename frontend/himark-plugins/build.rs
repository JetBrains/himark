// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The HOST header: what the apple assembly adds beyond the engine
//! (`himark.h`) and the window shell (`himark-shell.h`, workbench-
//! shell's own build) — the production plugin-set mint alone. Only
//! the `capi` feature has a C face to describe.

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    if std::env::var_os("CARGO_FEATURE_CAPI").is_none() {
        return;
    }
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(&crate_dir))
        .generate()
    {
        Ok(built) => {
            built.write_to_file(format!("{crate_dir}/include/himark-host.h"));
        }
        // A stale header is a MISLEADING artifact — the failure must
        // surface, not ride silently into the app build.
        Err(error) => println!("cargo:warning=cbindgen failed: {error}"),
    }
}
