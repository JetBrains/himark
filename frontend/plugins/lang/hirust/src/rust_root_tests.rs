// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use editor::StyleId;

fn test_theme() -> editor::Theme {
    editor::Theme::embedded()
}

#[test]
fn a_rust_file_is_a_document_rooted_in_rust() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::embedded_fonts::source()();
    let source = "fn main() {\n    let greeting = 1;\n}\n";
    let languages = himarkdown::markdown_languages(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &test_theme(),
    );

    assert_eq!(
        document.syntax().map(|root| root.language.as_str()),
        Some("rs")
    );
    let mut inline = Vec::new();
    let mut hidden = Vec::new();

    let extras: Vec<_> = document.document_scoped_markups().collect();
    let marks = editor::OverlaidMarkup::new(document.markup(), &extras).marks_inline_hidden_in(
        0..11,
        &mut inline,
        &mut hidden,
    );
    assert!(
        marks.ids().contains(&StyleId::CodeBlock) || marks.ids().contains(&StyleId::SourceCode),
        "a code-language root renders monospace"
    );
    assert!(
        inline
            .iter()
            .any(|interval| interval.id == StyleId::Keyword),
        "the fn keyword is colored at the root level"
    );

    let _editor = document.add_editor(
        400.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    document.edit(
        &operation::Operation::insert_at(0, "pub "),
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new(languages);
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("a rust root reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(0..15, &mut inline, &mut hidden);
    assert!(
        inline
            .iter()
            .any(|interval| interval.id == StyleId::Keyword),
        "colors follow the edit"
    );
}

#[test]
fn typing_inside_a_function_keeps_distant_body_tokens() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::embedded_fonts::source()();
    let source = "fn main() {\n    let first = 1;\n    let second = 2;\n    let third = 3;\n}\n";
    let languages = himarkdown::markdown_languages(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &test_theme(),
    );
    let _editor = document.add_editor(
        400.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let keyword_at = |document: &editor::Document, needle: &str, label: &str| {
        let mut view = document.text().view();
        let count = view.byte_count();
        let text = view.byte_string(0, count);
        let at = text
            .find(needle)
            .unwrap_or_else(|| panic!("{label}: {needle} in {text:?}")) as u32;
        let line_end = at + needle.len() as u32;
        let mut inline = Vec::new();
        let mut hidden = Vec::new();
        document
            .markup()
            .marks_inline_hidden_in(at..line_end, &mut inline, &mut hidden);
        assert!(
            inline
                .iter()
                .any(|interval| interval.id == editor::StyleId::Keyword),
            "{label}: the `let` before {needle:?} keeps its color ({} marks in {at}..{line_end})",
            inline.len(),
        );
    };
    keyword_at(&document, "let third", "before typing");

    let at = source.find("first").unwrap() as u32 + "first".len() as u32;
    document.edit(
        &operation::Operation::insert_at(at, "x"),
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new(languages);
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("a rust root reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    keyword_at(&document, "let third", "after typing + landing");
}

#[test]
fn typing_into_a_fenced_identifier_recolors_the_whole_token() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::embedded_fonts::source()();
    let theme = test_theme();
    let source = "# T\n\n```rust\nfn main() {}\n```\n";
    let mut document = himarkdown::document_from_markdown(source, store, ui, &fonts, &theme);
    let _editor = document.add_editor(
        400.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let parsers = std::sync::Arc::new(himarkdown::markdown_languages(languages()));

    let outcome = editor::ReparseWork::capture(&document, parsers.clone())
        .expect("a cold document reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    document.edit(
        &operation::Operation::insert_at(20, "xx"),
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("an edited document reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(13..28, &mut inline, &mut hidden);
    let function = inline
        .iter()
        .find(|interval| interval.id == StyleId::Function)
        .expect("the function name stays colored");

    assert_eq!(
        function.range,
        3..9,
        "the landing recolors the grown identifier, not just the old span"
    );
}

#[test]
fn declarations_emit_outline_items_at_parse() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::embedded_fonts::source()();
    let source = "struct Point {\n    x: f32,\n}\n\nimpl Point {\n    pub fn len(&self) -> f32 {\n        0.0\n    }\n}\n";
    let languages = himarkdown::markdown_languages(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &test_theme(),
    );
    let parsers = std::sync::Arc::new(languages);
    let outcome = editor::ReparseWork::capture(&document, parsers)
        .expect("a rust root reparses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );

    let items = document.outline_items();
    let titles: Vec<&str> = items
        .iter()
        .map(|(_, _, _, item)| item.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["struct Point", "impl Point", "pub fn len"],
        "grammar-field titles, document order"
    );

    let impl_range = items[1].2.clone();
    let len_range = items[2].2.clone();
    assert!(
        impl_range.start <= len_range.start && len_range.end <= impl_range.end,
        "the fn sits inside the impl"
    );
    assert!(document.has_outline());

    let (syntax, key, range, _) = items[2].clone();
    assert_eq!(
        document.resolve_outline(syntax, key),
        Some(range.clone()),
        "the (syntax, key) address resolves"
    );
    document.edit(
        &operation::Operation::insert_at(0, "// head\n"),
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    assert_eq!(
        document.resolve_outline(syntax, key),
        Some(range.start + 8..range.end + 8),
        "the interval shifted with the edit — no bytes stored anywhere"
    );
}

#[test]
fn declaration_names_carry_the_header_style() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::embedded_fonts::source()();
    let source = "struct Widget;\n\nimpl Widget {\n    fn frobnicate(&self) {\n        self.helper();\n    }\n}\n\nfn helper(widget: Widget) {}\n";
    let languages = himarkdown::markdown_languages(languages());
    let document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &test_theme(),
    );
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(0..source.len() as u32, &mut inline, &mut hidden);
    let names: Vec<std::ops::Range<u32>> = inline
        .iter()
        .filter(|interval| interval.id == StyleId::DeclarationName)
        .map(|interval| interval.range.clone())
        .collect();
    let covers = |needle: &str| {
        let at = source.find(needle).unwrap() as u32;
        let end = at + needle.len() as u32;
        names
            .iter()
            .any(|range| range.start == at && range.end == end)
    };
    assert!(covers("Widget"), "the struct name: {names:?}");
    assert!(covers("frobnicate"), "the method name: {names:?}");
    let declared = source.find("fn helper").unwrap() as u32 + 3;
    assert!(
        names
            .iter()
            .any(|range| range.start == declared && range.end == declared + 6),
        "the fn name: {names:?}"
    );

    let call = source.find("self.helper").unwrap() as u32 + "self.".len() as u32;
    assert!(
        !names.iter().any(|range| range.start == call),
        "a call site is not a declaration: {names:?}"
    );

    let theme = test_theme();
    let attributes = theme.attributes(StyleId::DeclarationName);
    assert!(
        attributes.font_size.unwrap_or(0.0)
            > theme
                .attributes(StyleId::SourceCode)
                .font_size
                .unwrap_or(f32::MAX),
        "declaration names outsize the code base"
    );
}

#[test]
#[ignore]
fn render_declaration_names_snapshot() {
    let store = &imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let Ok(path) = std::env::var("HIRUST_SNAPSHOT") else {
        return;
    };
    let fonts = editor::embedded_fonts::source()();
    let source = "/// A live document channel.\npub struct DocumentChannels {\n    slots: HashMap<Location, Slot>,\n    suppressed: HashSet<(Location, u64)>,\n}\n\nimpl DocumentChannels {\n    /// Starts the channel once — idempotent.\n    pub fn ensure(&self, location: Location) {\n        if self.slots.contains_key(&location) {\n            return;\n        }\n        self.spawn(location);\n    }\n\n    fn spawn(&self, location: Location) {\n        run(async move { life(location).await });\n    }\n}\n\nfn life(location: Location) -> Life {\n    Life::open(location)\n}\n";
    let languages = himarkdown::markdown_languages(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(source),
        "rs",
        &languages,
        store,
        ui,
        &fonts,
        &test_theme(),
    );
    let editor = document.add_editor(
        860.0,
        None,
        editor::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &fonts,
        &test_theme(),
        &mut imba::effect::Batch::new().effects(),
    );
    let height = document.content_height(editor).ceil() as i32 + 40;
    let mut surface = skia_safe::surfaces::raster_n32_premul((900, height)).expect("surface");
    surface
        .canvas()
        .clear(skia_safe::Color::from_rgb(0x10, 0x14, 0x1e));
    surface.canvas().translate((20.0, 20.0));
    document.paint(
        editor,
        surface.canvas(),
        skia_safe::Rect::from_wh(860.0, height as f32),
        true,
        store,
        ui,
        &fonts,
        &test_theme(),
    );
    let image = surface.image_snapshot();
    let data = image
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("png");
    std::fs::write(path, data.as_bytes()).expect("snapshot written");
}
