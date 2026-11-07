// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SESSION C header (`himark-ahp.h`) — generated only when the
//! capi feature is on (the hitailnet precedent: the plugin owns its
//! whole face).

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    if std::env::var_os("CARGO_FEATURE_CAPI").is_none() {
        return;
    }
    let root = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    match cbindgen::Builder::new()
        .with_crate(&root)
        .with_config(cbindgen::Config::from_root_or_default(&root))
        .generate()
    {
        Ok(built) => {
            built.write_to_file(format!("{root}/include/himark-ahp.h"));
        }
        Err(error) => println!("cargo:warning=cbindgen failed for hiahp: {error}"),
    }
}
