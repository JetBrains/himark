// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(&crate_dir))
        .generate()
    {
        Ok(builder) => {
            builder.write_to_file(format!("{crate_dir}/include/himark.h"));
        }
        // A stale header is a MISLEADING artifact — the failure must
        // surface, not ride silently into the app build.
        Err(error) => println!("cargo:warning=cbindgen failed: {error}"),
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
