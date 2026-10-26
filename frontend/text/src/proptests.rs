// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative laws of `Text` against a `String` model. A text is a
//! rope over UTF-8 bytes with newline and UTF-16 metrics; the model
//! is the plain `String` it stands for, with every count and every
//! conversion computed by a naive scan — the scan is too slow to
//! ship and too simple to be wrong, which makes it the oracle.

use proptest::prelude::*;

use operation::{op::Op, operation::Operation};

use crate::{line_number::LineNumber, text::Text};

const PIECES: &[&str] = &[
    "a", "bc", "hello", " ", "\n", "é", "中", "😀", "—", "x\n", "\n\n",
];

const WORDS: &[&str] = &["", "q", "βγ", "😀", "\n", "tail\n", "中a"];

/// A padding prefix long enough to cross the 1024-byte leaf capacity,
/// so a text with it spans several leaves and the leaf cache in
/// `TextView` gets exercised, not just its single-leaf fast path.
const PADDING: &str = "padding line é中😀 ....\n";

type Step = (u8, usize, usize);

fn step() -> impl Strategy<Value = Step> {
    (0..4u8, 1..=3usize, 0..WORDS.len())
}

fn plan() -> impl Strategy<Value = Vec<Step>> {
    prop::collection::vec(step(), 0..12)
}

fn text() -> impl Strategy<Value = String> {
    (
        prop_oneof![3 => Just(0usize), 1 => Just(60usize)],
        prop::collection::vec(prop::sample::select(PIECES), 0..40),
    )
        .prop_map(|(padding, pieces)| {
            let mut out = PADDING.repeat(padding);
            out.push_str(&pieces.concat());
            out
        })
}

/// The operation a plan spells over `source` — see the operation
/// crate's proptests; it covers the base exactly on char boundaries.
fn operation_over(source: &str, plan: &[Step]) -> Operation {
    let boundaries: Vec<usize> = source
        .char_indices()
        .map(|(index, _)| index)
        .chain([source.len()])
        .collect();
    let mut ops = Vec::new();
    let mut at = 0usize;
    let mut cursor = 0usize;
    while at + 1 < boundaries.len() {
        let (kind, step, word) = match plan.is_empty() {
            true => (0, 1, 0),
            false => plan[cursor % plan.len()],
        };
        cursor += 1;
        let next = (at + step).min(boundaries.len() - 1);
        let (from, to) = (boundaries[at], boundaries[next]);
        match kind {
            0 | 1 => ops.push(Op::Retain((to - from) as u32)),
            2 => ops.push(Op::Delete(source[from..to].to_owned())),
            _ => {
                ops.push(Op::Insert(WORDS[word].to_owned()));
                ops.push(Op::Retain((to - from) as u32));
            }
        }
        at = next;
    }
    if let Some(&(3, _, word)) = plan.last() {
        ops.push(Op::Insert(WORDS[word].to_owned()));
    }
    Operation::from_ops(ops)
}

/// A local edit (one deletion, one insertion) at plan-chosen char
/// boundaries — the shape typing takes, as distinct from the sweeping
/// plans above.
fn local_edit(source: &str, a: usize, b: usize, word: usize) -> Operation {
    let boundaries: Vec<usize> = source
        .char_indices()
        .map(|(index, _)| index)
        .chain([source.len()])
        .collect();
    let a = a % boundaries.len();
    let b = b % boundaries.len();
    let (from, to) = (boundaries[a.min(b)], boundaries[a.max(b)]);
    let mut ops = Vec::new();
    if from > 0 {
        ops.push(Op::Retain(from as u32));
    }
    if to > from {
        ops.push(Op::Delete(source[from..to].to_owned()));
    }
    ops.push(Op::Insert(WORDS[word % WORDS.len()].to_owned()));
    if source.len() > to {
        ops.push(Op::Retain((source.len() - to) as u32));
    }
    Operation::from_ops(ops)
}

fn model_apply(base: &str, operation: &Operation) -> String {
    let mut cursor = 0usize;
    let mut out = String::new();
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                let end = cursor + len as usize;
                out.push_str(&base[cursor..end]);
                cursor = end;
            }
            Op::Insert(text) => out.push_str(&text),
            Op::Delete(text) => {
                let end = cursor + text.len();
                assert_eq!(&base[cursor..end], text.as_str());
                cursor = end;
            }
        }
    }
    assert_eq!(cursor, base.len());
    out
}

fn model_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (index, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn model_utf16_to_byte(source: &str, utf16: u32) -> u32 {
    let mut units = 0u32;
    for (byte, ch) in source.char_indices() {
        if units >= utf16 {
            return byte as u32;
        }
        units += ch.len_utf16() as u32;
    }
    source.len() as u32
}

fn model_byte_to_utf16(source: &str, byte: usize) -> u32 {
    source[..byte].chars().map(|ch| ch.len_utf16() as u32).sum()
}

/// Every count and conversion a text and its view expose, checked
/// against the string at every offset, every line and every UTF-16
/// unit. Shared by the construction and edit properties.
fn assert_text_matches_model(text: &Text, model: &str) -> Result<(), TestCaseError> {
    prop_assert_eq!(text.to_string(), model);
    prop_assert_eq!(text.byte_count(), model.len());
    let starts = model_line_starts(model);
    prop_assert_eq!(text.line_count(), LineNumber(starts.len()));

    let mut view = text.view();
    prop_assert_eq!(view.byte_count(), model.len());
    prop_assert_eq!(view.line_count(), LineNumber(starts.len()));
    prop_assert_eq!(view.text().to_string(), model);

    for byte in 0..=model.len() {
        prop_assert_eq!(
            view.is_char_boundary(byte),
            model.is_char_boundary(byte),
            "is_char_boundary({})",
            byte
        );
        let expected_line = match model.is_empty() {
            true => 0,
            false => model.as_bytes()[..byte]
                .iter()
                .filter(|&&b| b == b'\n')
                .count(),
        };
        prop_assert_eq!(
            view.line_at(byte),
            LineNumber(expected_line),
            "line_at({})",
            byte
        );
    }
    prop_assert!(!view.is_char_boundary(model.len() + 1));

    for (line, &start) in starts.iter().enumerate() {
        prop_assert_eq!(
            view.line_start_offset(LineNumber(line)),
            start,
            "line_start_offset({})",
            line
        );
        let end = starts.get(line + 1).copied().unwrap_or(model.len());
        prop_assert_eq!(
            view.line_end_offset(LineNumber(line)),
            end,
            "line_end_offset({})",
            line
        );
    }

    let total_units = model_byte_to_utf16(model, model.len());
    for utf16 in 0..=total_units + 2 {
        prop_assert_eq!(
            view.utf16_to_byte(utf16),
            model_utf16_to_byte(model, utf16),
            "utf16_to_byte({})",
            utf16
        );
    }
    for (byte, _) in model.char_indices().chain([(model.len(), ' ')]) {
        prop_assert_eq!(
            view.byte_to_utf16(byte as u32),
            model_byte_to_utf16(model, byte),
            "byte_to_utf16({})",
            byte
        );
    }
    prop_assert_eq!(view.byte_to_utf16(model.len() as u32 + 7), total_units);
    Ok(())
}

/// The slices a text hands out — through the view, the byte reader
/// and the page — agree with the string over the given char-boundary
/// ranges, and clamp the way each promises to.
fn assert_slices_match_model(
    text: &Text,
    model: &str,
    ranges: &[(usize, usize)],
) -> Result<(), TestCaseError> {
    let boundaries: Vec<usize> = model
        .char_indices()
        .map(|(index, _)| index)
        .chain([model.len()])
        .collect();
    let mut view = text.view();
    for &(a, b) in ranges {
        let a = boundaries[a % boundaries.len()];
        let b = boundaries[b % boundaries.len()];
        let (from, to) = (a.min(b), a.max(b));
        let expected = &model[from..to];
        prop_assert_eq!(
            view.byte_string(from, to),
            expected,
            "byte_string({}, {})",
            from,
            to
        );
        prop_assert_eq!(
            view.substring(from as u32..to as u32),
            expected,
            "substring({}..{})",
            from,
            to
        );
        let mut bytes = Vec::new();
        view.byte_range_into(from, to, &mut bytes);
        prop_assert_eq!(
            bytes.as_slice(),
            expected.as_bytes(),
            "byte_range_into({}, {})",
            from,
            to
        );
        prop_assert_eq!(
            text.byte_string(from, to - from),
            expected,
            "Text::byte_string({}, {})",
            from,
            to - from
        );
        let page = text.page_at(from, to, usize::MAX);
        prop_assert_eq!(
            page.as_slice(),
            expected.as_bytes(),
            "page_at({}, {})",
            from,
            to
        );
        let capped = text.page_at(from, to, 5);
        prop_assert_eq!(
            capped.as_slice(),
            &expected.as_bytes()[..(to - from).min(5)]
        );
    }
    // Clamped reads: past the end the substring and the reader stop
    // at the text's end; a page starting at or past its end is empty.
    let len = model.len();
    prop_assert_eq!(view.substring(0..len as u32 + 9), model);
    prop_assert_eq!(text.byte_string(0, len + 9), model);
    prop_assert!(text.page_at(len, len + 4, 10).is_empty());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// A text built from a string reports the string's counts and
    /// answers every conversion the way a naive scan does.
    #[test]
    fn a_text_built_from_a_string_measures_like_the_string(source in text()) {
        let text = Text::from_string_exact(&source);
        assert_text_matches_model(&text, &source)?;
    }

    /// Every slice a text can hand out is the matching slice of the
    /// string, whichever door it comes through.
    #[test]
    fn every_slice_of_a_text_is_the_slice_of_the_string(
        source in text(),
        ranges in prop::collection::vec((0..200usize, 0..200usize), 0..12),
    ) {
        let text = Text::from_string_exact(&source);
        assert_slices_match_model(&text, &source, &ranges)?;
    }

    /// Editing is splicing: a sequence of sweeping operations lands
    /// the text on the string the model splices to, and the edited
    /// text measures like that string.
    #[test]
    fn a_sequence_of_sweeping_edits_lands_on_the_model_splice(
        source in text(),
        plans in prop::collection::vec(plan(), 1..5),
    ) {
        let mut text = Text::from_string_exact(&source);
        let mut model = source;
        for plan in &plans {
            let operation = operation_over(&model, plan);
            text = text.edit(&operation);
            model = model_apply(&model, &operation);
            prop_assert_eq!(text.to_string(), model.clone());
            prop_assert_eq!(text.byte_count(), model.len());
        }
        assert_text_matches_model(&text, &model)?;
    }

    /// The typing shape — many small local replacements — also lands
    /// on the model and leaves a text that measures like it.
    #[test]
    fn a_sequence_of_local_edits_lands_on_the_model_splice(
        source in text(),
        edits in prop::collection::vec((0..300usize, 0..300usize, 0..WORDS.len()), 1..12),
        ranges in prop::collection::vec((0..200usize, 0..200usize), 0..6),
    ) {
        let mut text = Text::from_string_exact(&source);
        let mut model = source;
        for &(a, b, word) in &edits {
            let operation = local_edit(&model, a, b, word);
            text = text.edit(&operation);
            model = model_apply(&model, &operation);
            prop_assert_eq!(text.to_string(), model.clone());
        }
        assert_text_matches_model(&text, &model)?;
        assert_slices_match_model(&text, &model, &ranges)?;
    }

    /// An edit leaves its source untouched: `edit` is a function from
    /// one text to another, and the source still measures like the
    /// original string afterwards.
    #[test]
    fn editing_does_not_disturb_the_source_text(
        source in text(),
        plan in plan(),
    ) {
        let text = Text::from_string_exact(&source);
        let operation = operation_over(&source, &plan);
        let edited = text.edit(&operation);
        prop_assert_eq!(text.to_string(), source.clone());
        prop_assert_eq!(edited.to_string(), model_apply(&source, &operation));
        prop_assert_eq!(text.byte_count(), source.len());
        prop_assert_eq!(text, Text::from_string_exact(&source));
    }
}
