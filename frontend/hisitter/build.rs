fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("emscripten") {
        println!("cargo:rerun-if-changed=src/side_fetch.c");
        cc::Build::new()
            .file("src/side_fetch.c")
            .compile("hisitter_side_fetch");
    }
}
