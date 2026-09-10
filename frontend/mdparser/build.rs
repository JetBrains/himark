fn main() {
    println!("cargo:rerun-if-changed=grammar");
    let grammar =
        std::fs::read_to_string("grammar/grammar.json").expect("the vendored grammar.json reads");
    let (name, parser) =
        tree_sitter_generate::generate_parser_for_grammar(&grammar, Some((0, 5, 3)))
            .expect("the markdown block grammar generates");
    assert_eq!(name, "markdown", "the grammar names the language");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let parser_c = out.join("parser.c");
    std::fs::write(&parser_c, parser).expect("parser.c writes");

    cc::Build::new()
        .include("grammar")
        .file(parser_c)
        .file("grammar/scanner.c")
        .compile("tree-sitter-markdown-patched");
}
