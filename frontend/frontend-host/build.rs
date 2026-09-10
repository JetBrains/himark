fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");

    if let Ok(builder) = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cbindgen::Config::from_root_or_default(&crate_dir))
        .generate()
    {
        builder.write_to_file(format!("{crate_dir}/include/himark.h"));
    }
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");
}
