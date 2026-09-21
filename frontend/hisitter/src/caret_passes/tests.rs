// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use editor::{CaretContext, EnrichCx, EnrichInput, Enricher, Markup, StyleId, Syntax, Text};

use super::{BraceMatchPass, OccurrencePass};
use crate::TsTree;

fn fonts() -> skia_safe::textlayout::FontCollection {
    editor::embedded_fonts::source()()
}

fn theme() -> editor::Theme {
    editor::Theme::embedded()
}

fn rust_tree(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("rust grammar loads");
    parser.parse(source, None).expect("rust parses")
}

fn input_at(source: &str, caret: u32, previous: Markup) -> EnrichInput {
    EnrichInput {
        text: Text::from_string_exact(source),
        syntax: Syntax::new(
            "rust",
            Some(Box::new(TsTree(rust_tree(source)))),
            Markup::new(),
        ),
        revision: 0,
        changed: Vec::new(),
        previous,
        base: None,
        caret: Some(CaretContext {
            selection: caret..caret,
            offset: caret,
        }),
    }
}

fn poll<T>(mut future: std::pin::Pin<Box<dyn std::future::Future<Output = T> + '_>>) -> T {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn raw() -> RawWaker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| raw(), |_| {}, |_| {}, |_| {});
        RawWaker::new(std::ptr::null(), &VTABLE)
    }

    let waker = unsafe { Waker::from_raw(raw()) };
    let mut cx = Context::from_waker(&waker);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the caret passes never park"),
    }
}

fn landed(pass: &dyn Enricher, input: &EnrichInput) -> Vec<Range<u32>> {
    let fonts = fonts();
    let theme = theme();
    let store = &imba::store::Store::new();
    let ui = &imba::UiCtx::dont_use_too_slow();
    let cx = EnrichCx {
        fonts: &fonts,
        theme: &theme,
        caller: imba::effect::EffectCaller::disconnected(),
        languages: None,
        measure: editor::MeasureCtx::Handed { store, ui },
    };
    let enrichment = poll(pass.derive(input, &cx));
    let mut markup = input.previous.clone();
    if !enrichment.changed.is_empty() {
        markup.splice(
            &enrichment.changed,
            enrichment.replacement,
            store,
            ui,
            &fonts,
            &theme,
        );
    }
    markup.styled_ranges_in(0..u32::MAX)
}

fn at(source: &str, pattern: &str, nth: usize) -> u32 {
    source
        .match_indices(pattern)
        .nth(nth)
        .unwrap_or_else(|| panic!("{pattern:?} #{nth} in {source:?}"))
        .0 as u32
}

#[test]
fn a_caret_on_an_opener_lights_the_pair() {
    let source = r#"fn main() { let s = ("x)", (1 + 2)); }"#;
    let open = at(source, "(\"x)\"", 0);
    let close = at(source, "));", 0) + 1;
    let input = input_at(source, open, Markup::new());
    let lit = landed(&BraceMatchPass, &input);
    assert_eq!(
        lit,
        vec![open..open + 1, close..close + 1],
        "the tuple's parens light — the string's ')' never pairs"
    );
}

#[test]
fn a_caret_after_a_closer_lights_the_pair() {
    let source = "fn main() { let a = 1; }";
    let open = at(source, "{", 0);
    let close = at(source, "}", 0);
    let input = input_at(source, close + 1, Markup::new());
    assert_eq!(
        landed(&BraceMatchPass, &input),
        vec![open..open + 1, close..close + 1]
    );
}

#[test]
fn a_bracket_less_caret_clears_the_previous_highlight() {
    let source = "fn main() { let a = 1; }";
    let mut previous = Markup::new();
    let open = at(source, "{", 0);
    previous.push_styled(open..open + 1, StyleId::BraceMatch);

    let input = input_at(source, at(source, "main", 0) + 2, previous);
    assert_eq!(
        landed(&BraceMatchPass, &input),
        Vec::<Range<u32>>::new(),
        "the stale pair washed away"
    );
}

#[test]
fn occurrences_light_matching_identifiers_only() {
    let source = r#"fn main() { let value = 1; let value2 = value + value; let s = "value"; }"#;
    let caret = at(source, "value", 0) + 1;
    let input = input_at(source, caret, Markup::new());
    let lit = landed(&OccurrencePass, &input);
    let expected: Vec<Range<u32>> = [0usize, 2, 3]
        .into_iter()
        .map(|nth| {
            let start = at(source, "value", nth);
            start..start + 5
        })
        .collect();
    assert_eq!(
        lit, expected,
        "the three `value` identifiers light; `value2` and the string stay dark"
    );
}

#[test]
fn a_caret_move_swaps_the_highlighted_word() {
    let source = "fn main() { let alpha = beta; let gamma = beta + alpha; }";
    let first = landed(
        &OccurrencePass,
        &input_at(source, at(source, "alpha", 0), Markup::new()),
    );
    assert_eq!(first.len(), 2, "both `alpha`s lit");

    let mut previous = Markup::new();
    for range in &first {
        previous.push_styled(range.clone(), StyleId::Occurrence);
    }
    let second = landed(
        &OccurrencePass,
        &input_at(source, at(source, "beta", 0), previous),
    );
    let expected: Vec<Range<u32>> = (0..2)
        .map(|nth| {
            let start = at(source, "beta", nth);
            start..start + 4
        })
        .collect();
    assert_eq!(second, expected, "only the `beta`s remain lit");
}

#[test]
fn a_nested_syntax_matches_through_the_innermost_tree() {
    let host = "# doc\n\nfn f() {}\n\ntail\n";
    let fence: Range<u32> = {
        let start = at(host, "fn f", 0);
        start..start + "fn f() {}".len() as u32
    };
    let rust_source = &host[fence.start as usize..fence.end as usize];
    let mut root_markup = Markup::new();
    root_markup.add_syntax(
        fence.clone(),
        Syntax::new(
            "rust",
            Some(Box::new(TsTree(rust_tree(rust_source)))),
            Markup::new(),
        ),
    );
    let caret = at(host, "(", 0);
    let input = EnrichInput {
        text: Text::from_string_exact(host),
        syntax: Syntax::new("markdown", None, root_markup),
        revision: 0,
        changed: Vec::new(),
        previous: Markup::new(),
        base: None,
        caret: Some(CaretContext {
            selection: caret..caret,
            offset: caret,
        }),
    };
    let open = at(host, "(", 0);
    let close = at(host, ")", 0);
    assert_eq!(
        landed(&BraceMatchPass, &input),
        vec![open..open + 1, close..close + 1],
        "the fence's rust tree answered, at absolute offsets"
    );
}
