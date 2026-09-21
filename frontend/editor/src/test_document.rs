// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use text::Text;

use crate::{
    document::Document,
    markup::{Markup, StyleId},
};

pub fn test_fonts() -> crate::FontSource {
    crate::embedded_fonts::source()
}

/// TEST SUPPORT: a KEPT `UiCtx`, one per test thread — its env slots
/// (the `ui_font` typeface cache above all) warm once per thread
/// instead of once per call, which is what a fresh
/// `UiCtx::dont_use_too_slow()` at every measure site costs. The
/// leak is bounded (one ctx per libtest worker); `UiCtx` is not
/// `Sync`, so the `'static` borrow cannot cross threads. Tests only —
/// production code is handed the real ctx and must thread it.
pub fn test_ui() -> &'static imba::UiCtx {
    thread_local! {
        static UI: &'static imba::UiCtx =
            Box::leak(Box::new(imba::UiCtx::dont_use_too_slow()));
    }
    UI.with(|ui| *ui)
}

pub fn test_workshop(theme: crate::theme::Theme) -> std::sync::Arc<crate::env::Workshop> {
    std::sync::Arc::new(crate::env::Workshop::new(
        crate::embedded_fonts::source(),
        theme,
    ))
}

pub fn plain_document(source: &str) -> Document {
    marked_document(source, &[])
}

pub fn marked_document(source: &str, blocks: &[(Range<u32>, StyleId)]) -> Document {
    let mut markup = Markup::builder();
    for (range, id) in blocks {
        markup.push_block_styles(range.clone(), [*id]);
    }
    Document::new(Text::from_string_exact(source), markup.finish())
}

pub fn fenced_code_document(source: &str) -> Document {
    let fence = source
        .rfind("```")
        .expect("source must have a closing fence");
    let code_end = source[fence..]
        .find('\n')
        .map(|newline| fence + newline + 1)
        .unwrap_or(source.len());
    let open_end = source
        .find('\n')
        .map(|newline| newline + 1)
        .unwrap_or(source.len());
    let close_start = fence.saturating_sub(1).max(open_end);

    let mut markup = Markup::builder();
    markup.push_block_styles(0..code_end as u32, [code_marks()]);
    markup.push_hidden(0..open_end as u32);
    markup.push_hidden(close_start as u32..code_end as u32);
    Document::new(Text::from_string_exact(source), markup.finish())
}

pub fn hidden_document(source: &str, hidden: Range<u32>) -> Document {
    let mut markup = Markup::builder();
    markup.push_hidden(hidden);
    Document::new(Text::from_string_exact(source), markup.finish())
}

pub fn list_document(source: &str) -> Document {
    let mut blocks = Vec::new();
    let mut offset = 0u32;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len() as u32;
        blocks.push((offset..end, list_item_marks()));
        offset = end;
    }
    marked_document(source, &blocks)
}

pub fn header_marks(level: u8) -> StyleId {
    StyleId::Header(level)
}

pub fn code_marks() -> StyleId {
    StyleId::CodeBlock
}

pub fn list_item_marks() -> StyleId {
    StyleId::ListItem
}
