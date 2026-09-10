use super::*;

#[test]
fn functions_emit_foldables() {
    let source = "fn one() {\n    1;\n}\n\nfn tiny() { 2 }\n";
    let fonts = editor::embedded_fonts::source()();
    let theme = editor::Theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &registry,
        &fonts,
        &theme,
    );
    let outcome = editor::ReparseWork::capture(&document, registry)
        .expect("parse")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let start = source.find('{').expect("the brace") as u32 + 1;
    let end = source.find('}').expect("the brace") as u32;
    assert_eq!(
        document.foldables_in(0..source.len() as u32),
        vec![start..end],
        "the multi-line function's interior folds; the one-liner does not"
    );
}
