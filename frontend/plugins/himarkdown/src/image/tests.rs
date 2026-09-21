// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

use himark::{ResourceLocation, ResourceType};

fn fonts() -> skia_safe::textlayout::FontCollection {
    himark::test_document::test_fonts_collection().clone()
}

fn theme() -> himark::Theme {
    himark::Theme::embedded()
}

fn base() -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        himark::Authority::new("local"),
        vec!["repo".to_owned(), "page.md".to_owned()],
    )
}

fn png() -> Vec<u8> {
    let mut surface = skia_safe::surfaces::raster_n32_premul((4, 2)).expect("surface");
    surface.canvas().clear(skia_safe::Color::BLUE);
    surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, None)
        .expect("encoded")
        .as_bytes()
        .to_vec()
}

#[test]
fn the_scanner_finds_images_and_leaves_links_alone() {
    let found = scan_images("see ![alt](pic.png) and [link](page.md) here", 0);
    assert_eq!(found.len(), 1, "a link is not an image: {found:?}");
    assert_eq!(found[0].reference, "pic.png");
    assert_eq!(found[0].range, 4..19, "the span covers ! through )");
}

#[test]
fn the_scanner_reads_every_spelling_of_a_target() {
    let cases = [
        ("![a](x.png)", "x.png"),
        ("![a](x.png \"title\")", "x.png"),
        ("![a](<x y.png>)", "x y.png"),
        ("![](https://host/p.png)", "https://host/p.png"),
    ];
    for (source, reference) in cases {
        let found = scan_images(source, 0);
        assert_eq!(
            found.first().map(|image| image.reference.as_str()),
            Some(reference),
            "{source:?}"
        );
    }

    assert!(scan_images("![a]()", 0).is_empty());
    assert!(scan_images("![a](unclosed", 0).is_empty());
}

#[test]
fn the_scanner_reports_document_coordinates() {
    let found = scan_images("![a](x.png) ![b](y.png)", 100);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].range, 100..111);
    assert_eq!(found[1].range, 112..123);
}

#[derive(Clone, Default)]
struct Fetches(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl Fetches {
    fn caller(&self, answer: Option<Vec<u8>>) -> imba::effect::EffectCaller {
        let asked = self.0.clone();
        imba::effect::EffectCaller::new(std::sync::Arc::new(move |type_id, payload| {
            if type_id != std::any::TypeId::of::<FetchResourceBytesEffect>() {
                return None;
            }
            let effect = payload
                .downcast::<FetchResourceBytesEffect>()
                .expect("the bytes effect");
            asked.lock().expect("asks").push(effect.reference.clone());
            Some(Box::pin(std::future::ready(
                Box::new(answer.clone()) as Box<dyn std::any::Any + Send + Sync>
            )))
        }))
    }

    fn asked(&self) -> Vec<String> {
        self.0.lock().expect("asks").clone()
    }
}

fn md(source: &str) -> himark::Document {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    crate::document_from_markdown(source, store, ui, &fonts(), &theme())
}

fn input(document: &himark::Document, source: &str, previous: Markup) -> EnrichInput {
    EnrichInput {
        text: document.text().clone(),
        syntax: document.syntax().expect("markdown parses").clone(),
        revision: 0,
        changed: vec![0..source.len() as u32],
        previous,
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

fn run(over: &EnrichInput, caller: imba::effect::EffectCaller) -> Markup {
    let store = &imba::store::Store::new();
    let ui = himark::test_document::test_ui();
    let fonts = fonts();
    let theme = theme();
    let fresh = {
        let cx = EnrichCx {
            fonts: &fonts,
            theme: &theme,
            caller,
            languages: None,
            measure: himark::MeasureCtx::Handed { store, ui },
        };
        poll(ImageEnricher.derive(over, &cx))
    };
    let mut entry = over.previous.clone();
    if !fresh.changed.is_empty() {
        entry.splice(&fresh.changed, fresh.replacement, store, ui, &fonts, &theme);
    }
    entry
}

fn pictures(entry: &Markup, len: u32) -> Vec<(Range<u32>, InlayMode, String)> {
    entry
        .all_inlays_in(0..len)
        .into_iter()
        .filter_map(|interval| {
            let inlay = interval.inlay.view_as::<ImageInlay>()?;
            Some((
                interval.range.clone(),
                interval.inlay.mode(),
                inlay.reference().to_owned(),
            ))
        })
        .collect()
}

#[test]
fn an_image_lands_under_its_line() {
    let source = "# Notes\n\n![a diagram](pic.png)\n";
    let document = md(source);
    let fetches = Fetches::default();
    let entry = run(
        &input(&document, source, Markup::new()),
        fetches.caller(Some(png())),
    );
    let found = pictures(&entry, source.len() as u32);
    assert_eq!(found.len(), 1, "one picture: {found:?}");
    let (range, mode, reference) = &found[0];
    assert_eq!(mode, &InlayMode::Under, "the picture sits BELOW the line");
    assert_eq!(reference, "pic.png");
    let start = source.find("![").expect("the image") as u32;
    assert_eq!(range, &(start..source.len() as u32 - 1));
    assert_eq!(fetches.asked(), ["pic.png"], "asked for it once");
}

#[test]
fn an_absolute_url_reaches_the_host_verbatim() {
    let source = "![remote](https://example.com/a/b.png)\n";
    let document = md(source);
    let fetches = Fetches::default();
    let entry = run(
        &input(&document, source, Markup::new()),
        fetches.caller(Some(png())),
    );
    assert_eq!(fetches.asked(), ["https://example.com/a/b.png"]);
    assert_eq!(pictures(&entry, source.len() as u32).len(), 1);
}

#[test]
fn an_unfetchable_image_lands_no_widget() {
    let source = "![gone](missing.png)\n";
    let document = md(source);
    let entry = run(
        &input(&document, source, Markup::new()),
        Fetches::default().caller(None),
    );
    assert!(pictures(&entry, source.len() as u32).is_empty());

    let entry = run(
        &input(&document, source, Markup::new()),
        Fetches::default().caller(Some(b"not an image".to_vec())),
    );
    assert!(pictures(&entry, source.len() as u32).is_empty());
}

#[test]
fn a_second_run_carries_the_loaded_picture_over() {
    let source = "![a](pic.png) hello\n";
    let document = md(source);
    let first = Fetches::default();
    let entry = run(
        &input(&document, source, Markup::new()),
        first.caller(Some(png())),
    );
    assert_eq!(first.asked().len(), 1);

    let second = Fetches::default();
    let again = run(&input(&document, source, entry), second.caller(None));
    assert!(second.asked().is_empty(), "no second fetch");
    let found = pictures(&again, source.len() as u32);
    assert_eq!(found.len(), 1, "the loaded picture survived: {found:?}");
    assert_eq!(found[0].2, "pic.png");
}

#[test]
fn a_changed_reference_fetches_again() {
    let source = "![a](one.png)\n";
    let document = md(source);
    let entry = run(
        &input(&document, source, Markup::new()),
        Fetches::default().caller(Some(png())),
    );

    let edited = "![a](two.png)\n";
    let document = md(edited);
    let fetches = Fetches::default();
    let again = run(
        &input(&document, edited, entry),
        fetches.caller(Some(png())),
    );
    assert_eq!(fetches.asked(), ["two.png"]);
    let found = pictures(&again, edited.len() as u32);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].2, "two.png");
}

#[test]
fn a_document_without_a_location_shows_nothing() {
    let source = "![a](pic.png)\n";
    let document = md(source);
    let mut over = input(&document, source, Markup::new());
    over.base = None;
    let fetches = Fetches::default();
    let entry = run(&over, fetches.caller(Some(png())));
    assert!(fetches.asked().is_empty(), "nothing was even asked");
    assert!(pictures(&entry, source.len() as u32).is_empty());
}
