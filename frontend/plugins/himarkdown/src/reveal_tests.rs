use super::*;

#[test]
fn syntax_reveals_on_the_caret_line_and_rehides_off_it() {
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let mut document = document_from_markdown("# Title\n\nsome **bold** words\n", &fonts, &theme);
    let editor = document.add_editor(
        700.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let hidden_at = |document: &himark::Document, line: std::ops::Range<u32>| {
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let extras = document.extras_keyed(editor);
        himark::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
            line,
            &mut inline,
            &mut hidden,
        );
        hidden.len()
    };

    assert_eq!(hidden_at(&document, 0..8), 0, "caret line shows its #");
    assert!(
        hidden_at(&document, 9..29) >= 2,
        "the ** pair hides off-caret"
    );

    document.set_caret(editor, 15);
    document.refresh_unhide(editor, &fonts, &theme);
    assert!(hidden_at(&document, 0..8) >= 1, "the # re-hides off-caret");
    assert_eq!(hidden_at(&document, 9..29), 0, "caret line shows its **");
}
