// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Shared generators: a marked-up document, a wrap width, and edits
//! spelled as FRACTIONS of the text so one spec still applies after
//! the text has changed shape under earlier edits.

use std::ops::Range;

use operation::{op::Op, operation::Operation};
use proptest::prelude::*;
use text::text::Text;

use crate::{
    document::Document,
    markup::Markup,
    theme::{StyleId, TextAlignment},
};

const WORDS: &[&str] = &[
    "alpha",
    "beta",
    "gamma",
    "delta",
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "a",
    "I",
    "héllo",
    "naïve",
    "日本語",
    "😀",
    "🎉x",
    "x😀y",
    "tab\tbed",
    "-",
    "1.",
    "#",
    "שלום",
    "مرحبا",
    "עברית",
    "a-שלום",
];

pub(super) fn word() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => proptest::sample::select(WORDS).prop_map(str::to_owned),
        1 => "[a-z]{20,60}",
        1 => Just(String::new()),
    ]
}

pub(super) fn line() -> impl Strategy<Value = String> {
    prop_oneof![
        1 => Just(String::new()),
        6 => proptest::collection::vec(word(), 1..12).prop_map(|words| words.join(" ")),
    ]
}

pub(super) fn source() -> impl Strategy<Value = String> {
    (proptest::collection::vec(line(), 1..=10), any::<bool>()).prop_map(|(lines, trailing)| {
        let mut source = lines.join("\n");
        if trailing {
            source.push('\n');
        }
        source
    })
}

#[derive(Clone, Debug)]
pub(super) enum Block {
    Plain,
    Header(u8),
    ListItem,
    CodeBlock,
    Quote,
    Indent(u8),
    Centered,
    Right,
}

fn block() -> impl Strategy<Value = Block> {
    prop_oneof![
        5 => Just(Block::Plain),
        1 => (1u8..=3).prop_map(Block::Header),
        1 => Just(Block::ListItem),
        1 => Just(Block::CodeBlock),
        1 => Just(Block::Quote),
        1 => (1u8..=2).prop_map(Block::Indent),
        1 => Just(Block::Centered),
        1 => Just(Block::Right),
    ]
}

#[derive(Clone, Debug)]
pub(super) enum Inline {
    Emphasis,
    Strong,
    InlineCode,
    Link,
    Strikethrough,
    Hidden,
}

fn inline() -> impl Strategy<Value = Inline> {
    prop_oneof![
        Just(Inline::Emphasis),
        Just(Inline::Strong),
        Just(Inline::InlineCode),
        Just(Inline::Link),
        Just(Inline::Strikethrough),
        Just(Inline::Hidden),
    ]
}

/// A document to build: the text, one block style per line, and
/// inline spans placed by fractions of their line.
#[derive(Clone, Debug)]
pub(super) struct Spec {
    pub source: String,
    pub blocks: Vec<Block>,
    pub spans: Vec<(usize, f32, f32, Inline)>,
}

pub(super) fn document_spec() -> impl Strategy<Value = Spec> {
    source()
        .prop_flat_map(|source| {
            let count = line_ranges(&source).len();
            (
                Just(source),
                proptest::collection::vec(block(), count),
                proptest::collection::vec((0..count, 0.0f32..1.0, 0.0f32..=1.0, inline()), 0..=4),
            )
        })
        .prop_map(|(source, blocks, spans)| Spec {
            source,
            blocks,
            spans,
        })
}

/// Byte ranges of the lines, newline included; an empty text is one
/// empty line.
pub(super) fn line_ranges(source: &str) -> Vec<Range<u32>> {
    let mut out = Vec::new();
    let mut at = 0u32;
    for line in source.split_inclusive('\n') {
        let end = at + line.len() as u32;
        out.push(at..end);
        at = end;
    }
    if out.is_empty() {
        out.push(0..0);
    }
    out
}

pub(super) fn floor_char(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) fn build(spec: &Spec) -> Document {
    let ranges = line_ranges(&spec.source);
    let mut markup = Markup::builder();
    for (range, block) in ranges.iter().zip(&spec.blocks) {
        let range = range.clone();
        match block {
            Block::Plain => {}
            Block::Header(level) => markup.push_block_styles(range, [StyleId::Header(*level)]),
            Block::ListItem => markup.push_block_styles(range, [StyleId::ListItem]),
            Block::CodeBlock => markup.push_block_styles(range, [StyleId::CodeBlock]),
            Block::Quote => markup.push_block_styles(range, [StyleId::Quote]),
            Block::Indent(depth) => markup.push_block_styles(range, [StyleId::Indent(*depth)]),
            Block::Centered => markup.push_alignment(range, TextAlignment::Center),
            Block::Right => markup.push_alignment(range, TextAlignment::Right),
        }
    }
    for (line, start, len, inline) in &spec.spans {
        let range = &ranges[*line % ranges.len()];
        let text = &spec.source[range.start as usize..range.end as usize];
        let text = text.strip_suffix('\n').unwrap_or(text);
        if text.is_empty() {
            continue;
        }
        let from = floor_char(text, (text.len() as f32 * start) as usize);
        let to = floor_char(text, from + ((text.len() - from) as f32 * len) as usize).max(from);
        if from == to {
            continue;
        }
        let span = range.start + from as u32..range.start + to as u32;
        match inline {
            Inline::Emphasis => markup.push_inline(span, StyleId::Emphasis),
            Inline::Strong => markup.push_inline(span, StyleId::Strong),
            Inline::InlineCode => markup.push_inline(span, StyleId::InlineCode),
            Inline::Link => markup.push_inline(span, StyleId::Link),
            Inline::Strikethrough => markup.push_inline(span, StyleId::Strikethrough),
            Inline::Hidden => markup.push_hidden(span),
        }
    }
    Document::new(Text::from_string_exact(&spec.source), markup.finish())
}

/// An edit by fractions: where, how much of the rest to delete, and
/// what to insert.
#[derive(Clone, Debug)]
pub(super) struct EditSpec {
    pub at: f32,
    pub delete: f32,
    pub insert: String,
}

pub(super) fn edit_spec() -> impl Strategy<Value = EditSpec> {
    (
        0.0f32..=1.0,
        prop_oneof![
            3 => Just(0.0f32),
            2 => 0.0f32..0.3,
            1 => 0.0f32..=1.0,
        ],
        prop_oneof![
            2 => Just(String::new()),
            4 => word(),
            2 => line().prop_map(|line| format!("{line}\n")),
            1 => Just("\n".to_owned()),
            1 => Just("\n\n".to_owned()),
        ],
    )
        .prop_map(|(at, delete, insert)| EditSpec { at, delete, insert })
}

/// The spec against the text AS IT STANDS: a replacement on char
/// boundaries whose old length is exactly the text's byte count, or
/// nothing when it would change nothing.
pub(super) fn resolve(text: &Text, spec: &EditSpec) -> Option<Operation> {
    let mut view = text.view();
    let count = view.byte_count();
    let whole = view.byte_string(0, count);
    let at = floor_char(&whole, (count as f32 * spec.at) as usize);
    let end = floor_char(&whole, at + ((count - at) as f32 * spec.delete) as usize).max(at);
    let deleted = whole[at..end].to_owned();
    if deleted.is_empty() && spec.insert.is_empty() {
        return None;
    }
    let mut ops = Vec::new();
    if at > 0 {
        ops.push(Op::Retain(at as u32));
    }
    if !deleted.is_empty() {
        ops.push(Op::Delete(deleted));
    }
    if !spec.insert.is_empty() {
        ops.push(Op::Insert(spec.insert.clone()));
    }
    if end < count {
        ops.push(Op::Retain((count - end) as u32));
    }
    Some(Operation::from_ops(ops))
}

pub(super) fn wrap_width() -> impl Strategy<Value = f32> {
    (120u32..=900).prop_map(|width| width as f32)
}

pub(super) fn width() -> impl Strategy<Value = f32> {
    prop_oneof![
        4 => wrap_width(),
        1 => Just(crate::document::NOWRAP_LAYOUT_WIDTH),
    ]
}
