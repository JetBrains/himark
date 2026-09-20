// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn fonts() -> skia_safe::textlayout::FontCollection {
    himark::embedded_fonts::source()()
}

fn theme() -> himark::Theme {
    himark::Theme::embedded()
}

fn languages() -> std::sync::Arc<SyntaxLanguages> {
    std::sync::Arc::new(crate::markdown_languages(SyntaxLanguages::new()))
}

fn base() -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("local"),
        vec!["repo".to_owned(), "notes".to_owned(), "page.md".to_owned()],
    )
}

fn sidecar_path() -> Vec<String> {
    vec![
        "repo".to_owned(),
        "notes".to_owned(),
        "src".to_owned(),
        "main.rs".to_owned(),
    ]
}

#[test]
fn split_fragment_parses_line_windows() {
    assert_eq!(split_fragment("a/b.rs"), ("a/b.rs", None));
    assert_eq!(split_fragment("a/b.rs#L10"), ("a/b.rs", Some((10, 10))));
    assert_eq!(split_fragment("a/b.rs#L10-40"), ("a/b.rs", Some((10, 40))));

    assert_eq!(split_fragment("a/b.rs#Lnope"), ("a/b.rs", None));
}

#[test]
fn resolve_walks_relative_against_the_base_dir() {
    let base = base();
    let resolved = resolve(&base, "src/main.rs").expect("resolved");
    assert_eq!(resolved.path(), ["repo", "notes", "src", "main.rs"]);
    assert_eq!(resolved.authority().as_str(), "local");
    let up = resolve(&base, "../top.rs").expect("resolved");
    assert_eq!(up.path(), ["repo", "top.rs"]);
    let here = resolve(&base, "./x.rs").expect("resolved");
    assert_eq!(here.path(), ["repo", "notes", "x.rs"]);
}

fn fetch_caller(path: Vec<String>, content: &'static str) -> imba::effect::EffectCaller {
    imba::effect::EffectCaller::new(std::sync::Arc::new(move |type_id, payload| {
        if type_id != std::any::TypeId::of::<FetchDocumentEffect>() {
            return None;
        }
        let effect = payload
            .downcast::<FetchDocumentEffect>()
            .expect("the fetch effect");
        let answer: Option<String> =
            (effect.location.path() == path.as_slice()).then(|| content.to_owned());
        Some(Box::pin(std::future::ready(
            Box::new(answer) as Box<dyn std::any::Any + Send + Sync>
        )))
    }))
}

fn host(source: &str) -> (Store, Document) {
        let ui = &imba::UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    store.put(himark::env::Fonts(himark::embedded_fonts::source()));
    store.put(himark::env::Parsers(languages()));
    let document = crate::document_from_markdown(source,
                &store, ui, &fonts(), &theme());
    (store, document)
}

fn enrich_input(document: &Document, source: &str) -> EnrichInput {
    EnrichInput {
        text: document.text().clone(),
        syntax: document.syntax().expect("markdown parses").clone(),
        revision: 0,
        changed: vec![0..source.len() as u32],
        previous: Markup::new(),
        base: Some(base()),
        caret: None,
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
        Poll::Pending => panic!("the fake fetch answers ready — no real parking in tests"),
    }
}

fn run(store: &mut Store, over: &EnrichInput, caller: imba::effect::EffectCaller) -> Markup {
    let ui = &imba::UiCtx::dont_use_too_slow();
    let fonts = fonts();
    let theme = theme();
    let fresh = {
        let cx = EnrichCx {
            fonts: &fonts,
            theme: &theme,
            caller,
            languages: himark::env::Parsers::of(store),
            measure: himark::MeasureCtx::Handed { store, ui },
        };
        poll(FenceEmbedEnricher.derive(over, &cx))
    };
    let mut entry = over.previous.clone();
    if !fresh.changed.is_empty() {
        entry.splice(&fresh.changed, fresh.replacement,
                store, ui, &fonts, &theme);
        FenceEmbedEnricher.install(store, ui, &mut entry, &fresh.changed, &fonts, &theme);
    }
    entry
}

fn embedded(entry: &Markup, len: u32) -> Option<himark::DocumentId> {
    entry
        .all_inlays_in(0..len)
        .into_iter()
        .find_map(|interval| {
            let view = interval.inlay.view_as::<EmbedView>()?;
            assert!(
                view.height() > 10.0,
                "the embed measures a real height store-lessly: {}",
                view.height()
            );
            Some(view.document())
        })
}

#[test]
fn an_addressed_fence_embeds_the_registered_file() {
    let source = "# Doc\n\n``` rust src/main.rs\nplaceholder\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let entry = run(
        &mut store,
        &over,
        fetch_caller(sidecar_path(), "fn main() {\n    let real = 1;\n}\n"),
    );
    let id = embedded(&entry, source.len() as u32).expect("an EditorIdView embed");

    let location = himark::OpenDocuments::location(&store, id).expect("registered location");
    assert_eq!(location.path(), sidecar_path().as_slice());
    assert_eq!(
        himark::OpenDocuments::by_location(&store, &location),
        Some(id),
        "the embed IS the by_location document a pane would open"
    );
    let shown = himark::OpenDocuments::document_ref(&store, id)
        .expect("the registered document")
        .text()
        .byte_string(
            0,
            himark::OpenDocuments::document_ref(&store, id)
                .unwrap()
                .text()
                .byte_count(),
        );
    assert!(
        shown.contains("let real"),
        "the shared document holds the fetched content: {shown:?}"
    );
}

#[test]
fn the_prepared_layout_attaches_equal_to_a_fresh_build() {
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "``` rust src/main.rs\nx\n```\n\n``` rust src/main.rs#L2-3\ny\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let content = "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\n";
    let entry = run(&mut store, &over, fetch_caller(sidecar_path(), content));
    let embeds: Vec<&EmbedView> = entry
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .filter_map(|interval| interval.inlay.view_as::<EmbedView>())
        .collect();
    assert_eq!(embeds.len(), 2, "both fences embedded");
    let whole = embeds[0];
    let windowed = embeds[1];
    assert_eq!(
        whole.document(),
        windowed.document(),
        "one registered document serves both"
    );

    let reference = himark::EditorView::complete(
        himark::OpenDocuments::document_ref(&store, whole.document())
            .expect("target")
            .clone(),
        720.0,
                &store, ui,
        &fonts(),
        &theme(),
    )
    .content_height();
    assert!(
        (whole.height() - reference).abs() < 0.5,
        "the prepared layout converges with a fresh build: prepared={} fresh={reference}",
        whole.height()
    );

    let target = himark::OpenDocuments::document_ref(&store, windowed.document()).expect("target");
    let text = target.text().byte_string(0, target.text().byte_count());
    let start = text.find("fn two").expect("line 2") as u32;
    let end = (text.find("fn three").expect("line 3") + "fn three() {}".len()) as u32;
    assert_eq!(
        target.window(windowed.editor()),
        start..end,
        "the dedup road still bounds to the #L window"
    );
    assert!(
        windowed.height() < whole.height(),
        "two lines measure under four: {} vs {}",
        windowed.height(),
        whole.height()
    );
}

#[test]
fn an_open_target_dedups_to_the_same_document() {
    let source = "``` rust src/main.rs\nx\n```\n";
    let (mut store, document) = host(source);

    let target = ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("local"),
        sidecar_path(),
    );
    let built = crate::document_from_markdown(
        "fn main() {}\n",
        &store,
        &imba::UiCtx::dont_use_too_slow(),
        &fonts(),
        &theme(),
    );
    let opened = himark::OpenDocuments::register(
        &mut store,
        built,
        Some(target.clone()),
        "main.rs".to_owned(),
        0,
    );
    let over = enrich_input(&document, source);
    let entry = run(
        &mut store,
        &over,
        fetch_caller(sidecar_path(), "ignored — already open"),
    );
    let id = embedded(&entry, source.len() as u32).expect("an embed");
    assert_eq!(id, opened, "the embed deduped to the pane's document");
}

#[test]
fn a_plain_fence_embeds_nothing() {
    let source = "``` rust\nlet plain = true;\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let entry = run(&mut store, &over, fetch_caller(Vec::new(), ""));
    assert!(embedded(&entry, source.len() as u32).is_none());
}

#[test]
fn a_gone_target_embeds_nothing() {
    let source = "``` rust missing/file.rs\nx\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let entry = run(
        &mut store,
        &over,
        fetch_caller(vec!["other".to_owned()], "unused"),
    );
    assert!(embedded(&entry, source.len() as u32).is_none());
}

#[test]
fn no_fetch_capability_embeds_nothing() {
    let source = "``` rust src/main.rs\nx\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let entry = run(
        &mut store,
        &over,
        imba::effect::EffectCaller::disconnected(),
    );
    assert!(embedded(&entry, source.len() as u32).is_none());
}

#[test]
fn no_base_embeds_nothing() {
    let source = "``` rust src/main.rs\nx\n```\n";
    let (mut store, document) = host(source);
    let mut over = enrich_input(&document, source);
    over.base = None;
    let entry = run(
        &mut store,
        &over,
        fetch_caller(sidecar_path(), "fn main() {}\n"),
    );
    assert!(embedded(&entry, source.len() as u32).is_none());
}

#[test]
fn destroying_the_embed_releases_the_editor_and_target() {
    let source = "``` rust src/main.rs\nx\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let mut entry = run(
        &mut store,
        &over,
        fetch_caller(sidecar_path(), "fn main() {}\n"),
    );
    let id = embedded(&entry, source.len() as u32).expect("an embed");
    assert!(himark::OpenDocuments::document_ref(&store, id).is_some());

    entry.destroy_inlays_in(&[0..source.len() as u32], &mut store);
    assert!(
        himark::OpenDocuments::document_ref(&store, id).is_none(),
        "the editorless target was released"
    );
}

#[test]
fn line_window_slices_1_based_inclusive() {
    let text = Text::from_string_exact("one\ntwo\nthree\nfour\n");
    assert_eq!(line_window(&text, 2, 3), Some(4..13), "two\\nthree");
    assert_eq!(line_window(&text, 1, 1), Some(0..3), "one");

    assert_eq!(line_window(&text, 3, 99), Some(8..19));
    assert_eq!(line_window(&text, 99, 100), None);
}

#[test]
fn a_line_fragment_windows_the_embed() {
        let ui = &imba::UiCtx::dont_use_too_slow();
    let source = "``` rust src/main.rs#L2-3\nx\n```\n";
    let (mut store, document) = host(source);
    let over = enrich_input(&document, source);
    let entry = run(
        &mut store,
        &over,
        fetch_caller(
            sidecar_path(),
            "line one\nline two\nline three\nline four\nline five\nline six\n",
        ),
    );
    let embed = entry
        .all_inlays_in(0..source.len() as u32)
        .into_iter()
        .find_map(|interval| interval.inlay.view_as::<EmbedView>().copied())
        .expect("the embed");
    let target = himark::OpenDocuments::document_ref(&store, embed.document()).expect("target");
    let window = target.window(embed.editor());
    let text = target.text();
    let shown = text.byte_string(window.start as usize, (window.end - window.start) as usize);
    assert_eq!(shown, "line two\nline three", "the window's lines only");

    let whole = himark::EditorView::complete(
        crate::document_from_markdown("x",
                &store, ui, &fonts(), &theme()),
        720.0,
                &store, ui,
        &fonts(),
        &theme(),
    );
    let one_line = whole.content_height().max(1.0);
    assert!(
        embed.height() < one_line * 4.0,
        "two lines of window, not six of file: height={} one_line≈{}",
        embed.height(),
        one_line
    );
}
