use super::*;

#[test]
fn scan_windows_never_split_characters() {
    let mut source = "a".repeat(SCAN_WINDOW - 1);
    source.push('🔥');

    source.push_str(" needle ");
    source.push_str(&"b".repeat(5000));
    source.push('\n');
    for _ in 0..8 {
        source.push_str("filler line without matches\n");
    }
    source.push_str("second needle\n");
    let text = text::Text::from_string_exact(&source);
    let matcher = Matcher::new("needle");
    let (hits, matches) = scan(&matcher, &text);
    assert_eq!(hits.len(), 2, "both matches found across the boundary");
    assert_eq!(matches.len(), 2, "and the exact ranges rode along");
}

#[test]
fn two_panels_highlight_one_document_independently() {
    let mut store = imba::store::Store::new();
    let mut document =
        himark::test_document::plain_document("alpha needle beta\ngamma needle delta\n");
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let id = himark::OpenDocuments::register(&mut store, document, None, "test".to_owned(), 0);

    let panel1 = SearchView::new();
    let panel2 = SearchView::new();
    let first_needle = 6..12;
    let second_needle = 24..30;
    let mut entry1 = panel1.take_list(&mut store);
    entry1.list.content_mut().install(
        &mut store,
        &fonts,
        vec![himark::InstallGroup::open(
            id,
            GroupSpans {
                ranges: vec![0..18],
                marks: vec![first_needle.clone()],
            },
        )],
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    panel1.put_list(&mut store, entry1);
    let mut entry2 = panel2.take_list(&mut store);
    entry2.list.content_mut().install(
        &mut store,
        &fonts,
        vec![himark::InstallGroup::open(
            id,
            GroupSpans {
                ranges: vec![18..37],
                marks: vec![second_needle.clone()],
            },
        )],
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    panel2.put_list(&mut store, entry2);
    let markup1 = panel1
        .list_ref(&store)
        .expect("panel1 row")
        .content()
        .installed_markups()[0];
    let markup2 = panel2
        .list_ref(&store)
        .expect("panel2 row")
        .content()
        .installed_markups()[0];
    let document = himark::OpenDocuments::document_ref(&store, id).expect("document stands");
    assert_eq!(
        document.markup_styled_ranges(markup1),
        vec![first_needle.clone()]
    );
    assert_eq!(
        document.markup_styled_ranges(markup2),
        vec![second_needle.clone()]
    );

    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    let extras = document.extras_keyed(editor);
    himark::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..37,
        &mut inline,
        &mut hidden,
    );
    assert!(
        !inline
            .iter()
            .any(|interval| interval.id == himark::StyleId::Match),
        "panel tints must not leak into the document's own editors"
    );

    let mut entry1 = panel1.take_list(&mut store);
    entry1.list.content_mut().install(
        &mut store,
        &fonts,
        Vec::new(),
        None,
        &mut imba::effect::Batch::new().effects(),
    );
    panel1.put_list(&mut store, entry1);
    let document = himark::OpenDocuments::document_ref(&store, id).expect("document stands");
    assert!(document.markup_styled_ranges(markup1).is_empty());
    assert_eq!(document.markup_styled_ranges(markup2), vec![second_needle]);
}

#[test]
fn search_is_case_insensitive_and_highlights_matches() {
    let text = text::Text::from_string_exact(
        "alpha Needle beta
",
    );
    let matcher = Matcher::new("NEEDLE");
    let (hits, matches) = scan(&matcher, &text);
    assert_eq!(hits.len(), 1, "case-insensitive match");
    assert_eq!(matches, vec![6..12]);

    let mut document = Document::new(text, himark::Markup::new());
    let fonts = himark::embedded_fonts::source()();
    let theme = himark::Theme::embedded();
    let editor = document.add_editor(
        400.0,
        None,
        himark::EditorBuild::Complete,
        &[],
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let markup_id = document.add_markup();
    let mut tints = himark::Markup::new();
    for range in &matches {
        tints.push_styled(range.clone(), himark::StyleId::Match);
    }
    document.replace_markup(
        markup_id,
        tints,
        &matches,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    document.show_markup(editor, markup_id);
    let view_inline = |document: &Document| {
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        let extras = document.extras_keyed(editor);
        himark::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
            0..18,
            &mut inline,
            &mut hidden,
        );
        inline
    };
    let inline = view_inline(&document);
    assert!(
        inline
            .iter()
            .any(|interval| interval.id == himark::StyleId::Match),
        "the match range styles"
    );

    document.remove_markup(
        markup_id,
        &matches,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let inline = view_inline(&document);
    assert!(
        inline
            .iter()
            .all(|interval| interval.id != himark::StyleId::Match),
        "clearing removes the highlight"
    );
}
