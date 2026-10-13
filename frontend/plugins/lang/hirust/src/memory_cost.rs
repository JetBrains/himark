// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! GROUND TRUTH for the chat transcript's memory: what one parsed
//! document actually costs. A chat retains two of these per file edit
//! (`ahp_chat::file_edit::BuiltFileEdit` — a before and an after
//! side), and a long coding session holds hundreds of edits, so the
//! per-document number is the one that decides whether the transcript
//! fits in memory.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static LIVE: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size, Ordering::Relaxed);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run alone — the counter is process-wide:
/// `cargo test -p hirust memory_cost -- --test-threads=1 --nocapture`
#[test]
fn a_parsed_document_costs_what_it_costs() {
    let languages = crate::languages();
    let store = imba::store::Store::new();
    let ui = editor::test_document::test_ui();
    let fonts = editor::test_document::test_fonts_collection();
    let theme = editor::theme::Theme::embedded();

    // A real source file, parsed exactly the way a chat diff cell
    // parses each side of an edit.
    // Real files at real sizes — a 1 KB file hides the per-span cost
    // behind the fixed cost of a parse.
    for (name, source) in [
        ("hirust/lib.rs", include_str!("lib.rs") as &str),
        ("editor/document.rs", include_str!("../../../../editor/src/document.rs")),
        ("imba/list.rs", include_str!("../../../../imba/src/list.rs")),
    ] {
        let text = text::text::Text::from_string_exact(source);
        let before = LIVE.load(Ordering::Relaxed);
        let document = editor::document::Document::from_language(
            text, "rust", &languages, &store, ui, fonts, &theme,
        );
        let held = LIVE.load(Ordering::Relaxed) - before;
        let spans = document.markup().query_count();
        let (lane, styled, runs) = document.markup().probe_span_stats();
        eprintln!(
            "[doc-cost] {name}: {} KB source -> {} KB live ({:.0}x), {} spans, {} B/span",
            source.len() / 1024,
            held / 1024,
            held as f64 / source.len() as f64,
            spans,
            held.checked_div(spans).unwrap_or(0),
        );
        eprintln!(
            "[doc-cost]   bulk lane {lane} spans, {styled} styled, {runs} runs if merged \
             ({:.0}% fewer)",
            100.0 - (runs as f64 / styled.max(1) as f64) * 100.0,
        );
        let (leaf_max, child_max) = document.markup().probe_styles_extremes();
        eprintln!(
            "[doc-cost]   widest stored coordinate: leaf {leaf_max}, child {child_max} \
             (i16 holds {}, i32 holds {})",
            i16::MAX,
            i32::MAX,
        );
        assert!(spans > 0, "{name} parsed");
        drop(document);
    }
}
