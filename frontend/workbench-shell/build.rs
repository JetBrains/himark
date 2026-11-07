// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The WINDOW header: the workbench shell's own C surface — what a
//! native shell that composes windows includes beside the engine's
//! `himark.h`. The iOS shell never includes it.

fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(&crate_dir))
        .generate()
    {
        Ok(built) => {
            built.write_to_file(format!("{crate_dir}/include/himark-shell.h"));
        }
        // A stale header is a MISLEADING artifact — the failure must
        // surface, not ride silently into the app build.
        Err(error) => println!("cargo:warning=cbindgen failed: {error}"),
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
