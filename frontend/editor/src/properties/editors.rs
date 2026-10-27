// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Properties over the editor's document state: undo and redo, the
//! edit log, carets and marked text, and several editors — bounded
//! or complete — sharing one document. Every edit here goes through
//! the same doors the app uses (`perform`, `insert`, `edit`), on
//! random texts with multi-byte characters, and the oracle is either
//! an algebraic law (undo inverts, positions outside an edit shift
//! by its delta) or a fresh build of the same layout.

use std::ops::Range;

use imba::effect::{block_on, Batch, EffectHandler, Message};
use operation::{op::Op, operation::Operation};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use crate::caret::{Caret, MultiCaret};
use crate::document::{Document, EditorBuild};
use crate::edit_log::EditLog;
use crate::editor::EditorId;
use crate::editor_view::{EditorCommand, Motion};
use crate::repair::{RepairEffect, RepairHandler};
use crate::test_document::{list_document, plain_document, test_ui};

fn theme() -> crate::theme::Theme {
    crate::theme::embedded()
}

/// A store seeded with the theme the layouts are built with, so
/// edits through `perform` (which reads the theme from the store)
/// repair with the SAME theme a fresh build uses.
fn seeded_store() -> imba::store::Store {
    let mut store = imba::store::Store::new();
    crate::env::Themes::set(&mut store, theme());
    store
}

fn text_of(document: &Document) -> String {
    let count = document.text().byte_count();
    document.text().view().byte_string(0, count)
}

fn boundaries(text: &str) -> Vec<u32> {
    let mut out: Vec<u32> = text.char_indices().map(|(index, _)| index as u32).collect();
    out.push(text.len() as u32);
    out
}

/// A char boundary of `text`, chosen by a fraction so the SAME
/// generated value stays meaningful as the text changes length.
fn pick(text: &str, fraction: f64) -> u32 {
    let boundaries = boundaries(text);
    let index = ((fraction * boundaries.len() as f64) as usize).min(boundaries.len() - 1);
    boundaries[index]
}

/// A replacement over `text` on char boundaries: delete `start..end`
/// and insert `inserted` there, spelled delete-then-insert as the
/// editor's own `replace_marked` spells it. Answers the operation,
/// the replaced range and the inserted byte count.
fn replacement(text: &str, a: f64, b: f64, inserted: &str) -> (Operation, Range<u32>, u32) {
    let (start, end) = {
        let (a, b) = (pick(text, a), pick(text, b));
        (a.min(b), a.max(b))
    };
    let byte_count = text.len() as u32;
    let mut ops = Vec::with_capacity(4);
    if start > 0 {
        ops.push(Op::Retain(start));
    }
    if end > start {
        ops.push(Op::Delete(text[start as usize..end as usize].to_owned()));
    }
    if !inserted.is_empty() {
        ops.push(Op::Insert(inserted.to_owned()));
    }
    if byte_count > end {
        ops.push(Op::Retain(byte_count - end));
    }
    (Operation::from_ops(ops), start..end, inserted.len() as u32)
}

/// The naive model of a text under an operation.
fn apply(model: &str, operation: &Operation) -> String {
    let mut out = String::with_capacity(model.len());
    let mut offset = 0usize;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                out.push_str(&model[offset..offset + len as usize]);
                offset += len as usize;
            }
            Op::Insert(text) => out.push_str(&text),
            Op::Delete(text) => offset += text.len(),
        }
    }
    assert_eq!(offset, model.len(), "the operation covers the model");
    out
}

/// Where a position lands after replacing `replaced` with
/// `inserted` bytes, when the answer is unambiguous: strictly before
/// the replacement it stays, strictly after it shifts by the delta.
/// On or inside the replacement the bias decides, so `None`.
fn model_position(position: u32, replaced: &Range<u32>, inserted: u32) -> Option<u32> {
    if position < replaced.start {
        Some(position)
    } else if position > replaced.end {
        Some(position - (replaced.end - replaced.start) + inserted)
    } else {
        None
    }
}

/// A short text with multi-byte characters, line breaks and the
/// markdown markers the editor styles, so layouts wrap, break and
/// style differently across edits.
fn source() -> impl Strategy<Value = String> {
    let token = prop_oneof![
        4 => "[a-z]{1,8}",
        3 => Just(" ".to_owned()),
        2 => Just("\n".to_owned()),
        1 => Just("é".to_owned()),
        1 => Just("😀".to_owned()),
        1 => Just("# ".to_owned()),
        1 => Just("- ".to_owned()),
    ];
    prop::collection::vec(token, 0..40).prop_map(|tokens| tokens.concat())
}

fn snippet() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-z ]{1,6}",
        1 => Just("\n".to_owned()),
        1 => Just("é😀".to_owned()),
        1 => Just(String::new()),
    ]
}

fn widths() -> impl Strategy<Value = f32> {
    prop::sample::select(vec![220.0f32, 360.0, 600.0])
}

fn fx() -> Batch<EditorCommand> {
    Batch::new()
}

fn add_editor(
    document: &mut Document,
    width: f32,
    bounds: Option<crate::document::FragmentKey>,
    build: EditorBuild,
    store: &imba::store::Store,
) -> EditorId {
    document.add_editor(
        width,
        bounds,
        build,
        &[],
        store,
        test_ui(),
        &theme(),
        &mut fx().effects(),
    )
}

/// Drive the repair lane to completion the way the engine would: the
/// batch's launched repair effects run on the workshop and land back
/// through `perform`, until nothing more launches. Layout repair is
/// budgeted, so a comparison against a fresh build must settle first.
fn drain_repairs(
    document: &mut Document,
    editor: EditorId,
    store: &mut imba::store::Store,
    mut batch: Batch<EditorCommand>,
) {
    let ui = test_ui();
    let mut workshop = None;
    let mut rounds = 0;
    loop {
        let launched: Vec<_> = batch
            .drain()
            .into_iter()
            .filter_map(|message| match message {
                Message::Launch(_, effect) | Message::Relaunch(_, _, effect)
                    if effect.is::<RepairEffect>() =>
                {
                    Some(effect)
                }
                _ => None,
            })
            .collect();
        if launched.is_empty() {
            break;
        }
        assert!(rounds < 16, "the repair lane must converge");
        rounds += 1;
        batch = Batch::new();
        let workshop = workshop.get_or_insert_with(|| crate::test_document::test_workshop(theme()));
        for effect in launched {
            let (value, lift) = effect.into_payload().split();
            let repair = value.downcast::<RepairEffect>().expect("a repair effect");
            let handler = RepairHandler(std::sync::Arc::clone(workshop));
            let outcome: Box<dyn std::any::Any + Send + Sync> = Box::new(block_on(Box::pin(
                async move { handler.handle(*repair).await },
            )));
            let command = lift(outcome).expect("a repair lands a command");
            document.perform(store, ui, editor, command, &mut batch.effects());
        }
    }
}

fn assert_carets_sane(document: &Document, editor: EditorId) -> Result<(), TestCaseError> {
    let text = text_of(document);
    let window = document.window(editor);
    for caret in document.carets(editor).carets() {
        let selection = caret.selection();
        prop_assert!(
            window.start <= selection.start && selection.end <= window.end,
            "{}",
            format!("caret {selection:?} inside the window {window:?}")
        );
        prop_assert!(
            selection.start <= caret.offset() && caret.offset() <= selection.end,
            "{}",
            format!(
                "the offset {} sits inside its selection {selection:?}",
                caret.offset()
            )
        );
        for byte in [selection.start, selection.end, caret.offset()] {
            prop_assert!(
                text.is_char_boundary(byte as usize),
                "{}",
                format!("byte {byte} of {text:?} is a char boundary")
            );
        }
    }
    if let Some(marked) = document.marked_range(editor) {
        prop_assert!(marked.start <= marked.end && marked.end as usize <= text.len());
        prop_assert!(text.is_char_boundary(marked.start as usize));
        prop_assert!(text.is_char_boundary(marked.end as usize));
    }
    Ok(())
}

fn assert_layout_fresh(
    document: &Document,
    editor: EditorId,
    store: &imba::store::Store,
) -> Result<(), TestCaseError> {
    let (pending, ..) = document.probe_state(editor);
    prop_assert_eq!(pending, None, "repairs have settled");
    let live = document.element_heights(editor);
    let fresh = document.fresh_layout_heights(editor, store, test_ui(), &theme());
    prop_assert_eq!(
        &live,
        &fresh,
        "live heights equal a fresh build at width {}",
        document.layout_width(editor)
    );
    // Elements tile the WHOLE text, window or not: a bounded layout
    // keeps zero-height filler outside its window.
    let ranges = document.element_byte_ranges(editor);
    let byte_count = document.text().byte_count() as u32;
    let window = document.window(editor);
    let mut next = 0u32;
    for (range, (start, height)) in ranges.iter().zip(&live) {
        prop_assert_eq!(
            range.start,
            next,
            "{}",
            format!("elements tile without gaps: {ranges:?}")
        );
        prop_assert_eq!(range.start, *start);
        next = range.end;
        let outside = range.end <= window.start || range.start >= window.end;
        if outside && !range.is_empty() {
            prop_assert_eq!(
                *height,
                0.0,
                "{}",
                format!("element {range:?} outside the window {window:?} takes no height")
            );
        }
    }
    prop_assert_eq!(
        next,
        byte_count,
        "{}",
        format!("elements cover the text: {ranges:?}")
    );
    Ok(())
}

#[derive(Debug, Clone)]
enum Action {
    Type(String),
    Enter { soft: bool },
    Backspace,
    DeleteForward,
    DeleteWordBack,
    DeleteWordForward,
    Paste(String),
    Caret(f64),
    Select(f64, f64),
    DeleteSelections,
    Move(Motion, bool),
}

fn action() -> impl Strategy<Value = Action> {
    let motion = prop::sample::select(vec![
        Motion::Left,
        Motion::Right,
        Motion::WordLeft,
        Motion::WordRight,
        Motion::Up,
        Motion::Down,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocumentStart,
        Motion::DocumentEnd,
    ]);
    prop_oneof![
        4 => snippet().prop_map(Action::Type),
        1 => any::<bool>().prop_map(|soft| Action::Enter { soft }),
        2 => Just(Action::Backspace),
        1 => Just(Action::DeleteForward),
        1 => Just(Action::DeleteWordBack),
        1 => Just(Action::DeleteWordForward),
        1 => snippet().prop_map(Action::Paste),
        3 => (0.0..=1.0f64).prop_map(Action::Caret),
        2 => (0.0..=1.0f64, 0.0..=1.0f64).prop_map(|(a, b)| Action::Select(a, b)),
        1 => Just(Action::DeleteSelections),
        2 => (motion, any::<bool>()).prop_map(|(motion, select)| Action::Move(motion, select)),
    ]
}

fn perform_action(
    document: &mut Document,
    editor: EditorId,
    store: &mut imba::store::Store,
    action: &Action,
) -> Batch<EditorCommand> {
    let mut batch = fx();
    let text = text_of(document);
    let command = match action {
        Action::Type(text) => EditorCommand::InsertText { text: text.clone() },
        Action::Enter { soft } => EditorCommand::Enter { soft: *soft },
        Action::Backspace => EditorCommand::Backspace,
        Action::DeleteForward => EditorCommand::DeleteForward,
        Action::DeleteWordBack => EditorCommand::DeleteWordBack,
        Action::DeleteWordForward => EditorCommand::DeleteWordForward,
        Action::Paste(text) => EditorCommand::Paste { text: text.clone() },
        Action::DeleteSelections => EditorCommand::DeleteSelections,
        Action::Move(motion, select) => EditorCommand::Move {
            motion: *motion,
            select: *select,
        },
        Action::Caret(fraction) => {
            document.set_caret(editor, pick(&text, *fraction));
            return batch;
        }
        Action::Select(a, b) => {
            document.set_carets(
                editor,
                MultiCaret::one(Caret::selecting(pick(&text, *a), pick(&text, *b))),
            );
            return batch;
        }
    };
    document.perform(store, test_ui(), editor, command, &mut batch.effects());
    batch
}

/// Undo or redo until the revision stops moving; answers how many
/// steps changed the text.
fn unwind(
    document: &mut Document,
    editor: EditorId,
    store: &mut imba::store::Store,
    redo: bool,
) -> usize {
    let mut steps = 0;
    loop {
        let revision = document.revision();
        let command = match redo {
            true => EditorCommand::Redo,
            false => EditorCommand::Undo,
        };
        let mut batch = fx();
        document.perform(store, test_ui(), editor, command, &mut batch.effects());
        drain_repairs(document, editor, store, batch);
        if document.revision() == revision {
            return steps;
        }
        steps += 1;
        assert!(steps < 10_000, "history must be finite");
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// Undo is an inverse of the command path: after any sequence of
    /// typing, deleting, pasting and caret motion, undoing until the
    /// history runs dry restores the original text byte for byte, and
    /// redoing until it runs dry restores the final text. Carets stay
    /// in bounds throughout.
    #[test]
    fn undoing_every_edit_restores_the_original_text_and_redoing_restores_the_final(
        source in source(),
        width in widths(),
        actions in prop::collection::vec(action(), 1..12),
    ) {
        let mut store = seeded_store();
        let mut document = plain_document(&source);
        let editor = add_editor(&mut document, width, None, EditorBuild::Complete, &store);

        for action in &actions {
            let batch = perform_action(&mut document, editor, &mut store, action);
            drain_repairs(&mut document, editor, &mut store, batch);
            assert_carets_sane(&document, editor)?;
        }
        let final_text = text_of(&document);
        let final_revision = document.revision();

        let undone = unwind(&mut document, editor, &mut store, false);
        prop_assert_eq!(text_of(&document), source.clone(), "undo restores the original");
        assert_carets_sane(&document, editor)?;

        let redone = unwind(&mut document, editor, &mut store, true);
        prop_assert_eq!(text_of(&document), final_text, "redo restores the final text");
        prop_assert_eq!(undone, redone, "every undo has its redo");
        assert_carets_sane(&document, editor)?;
        prop_assert!(
            (final_revision == 0) == (undone == 0),
            "an edited document has history to undo"
        );
        assert_layout_fresh(&document, editor, &store)?;
    }

    /// Positions outside an edit move by exactly its byte delta, for
    /// every editor on the document — the editing one and a sibling
    /// that only watches — and marked text follows the same rule.
    /// Positions on or inside the edit land inside the replacement.
    /// Everything stays on char boundaries.
    #[test]
    fn carets_and_marked_text_of_every_editor_shift_by_the_edit_delta(
        source in source(),
        width in widths(),
        edits in prop::collection::vec(
            (0.0..=1.0f64, 0.0..=1.0f64, snippet(), 0.0..=1.0f64, 0.0..=1.0f64, any::<bool>()),
            1..10,
        ),
    ) {
        let mut store = seeded_store();
        let ui = test_ui();
        let mut document = plain_document(&source);
        let acting = add_editor(&mut document, width, None, EditorBuild::Complete, &store);
        let watching = add_editor(&mut document, width, None, EditorBuild::Complete, &store);

        for (a, b, inserted, watcher_at, mark_at, through_editor) in &edits {
            // Composition on the watcher: a two-byte marked range at
            // a random boundary, inserted before the edit under test
            // so the edit's coordinates are read AFTER it. The
            // watcher's caret is then parked on a boundary of THAT
            // text.
            let text = text_of(&document);
            document.set_caret(watching, pick(&text, *mark_at));
            document.set_marked_text(watching, "xy", 0..2, None, &store, ui, &theme(), &mut fx().effects());
            let marked_before = document.marked_range(watching).expect("composing");
            let text = text_of(&document);
            document.set_caret(watching, pick(&text, *watcher_at));
            let watcher_before = document.caret_byte(watching);

            let (operation, replaced, inserted_len) = replacement(&text, *a, *b, inserted);
            if *through_editor {
                document.set_carets(acting, MultiCaret::one(Caret::selecting(replaced.start, replaced.end)));
                if replaced.is_empty() && inserted.is_empty() {
                    continue;
                }
                match inserted.is_empty() {
                    true => document.perform(&mut store, ui, acting, EditorCommand::DeleteSelections, &mut fx().effects()),
                    false => document.insert(acting, inserted, &store, ui, &theme(), &mut fx().effects()),
                }
            } else {
                document.edit(&operation, &store, ui, &theme(), &mut fx().effects());
            }
            prop_assert_eq!(text_of(&document), apply(&text, &operation), "the text follows the model");

            for editor in [acting, watching] {
                assert_carets_sane(&document, editor)?;
            }
            let watcher_after = document.caret_byte(watching);
            match model_position(watcher_before, &replaced, inserted_len) {
                Some(expected) => prop_assert_eq!(
                    watcher_after, expected,
                    "{}", format!("the watcher's caret {watcher_before} outside {replaced:?} (+{inserted_len}) shifts by the delta")
                ),
                None => prop_assert!(
                    replaced.start <= watcher_after && watcher_after <= replaced.start + inserted_len,
                    "{}", format!("the watcher's caret {watcher_before} on the edit {replaced:?} lands inside the replacement, got {watcher_after}")
                ),
            }
            let marked_after = document.marked_range(watching).expect("still composing");
            if marked_before.end < replaced.start {
                prop_assert_eq!(&marked_after, &marked_before, "marked text before the edit stays");
            } else if marked_before.start > replaced.end {
                let delta = inserted_len as i64 - (replaced.end - replaced.start) as i64;
                let shifted = (marked_before.start as i64 + delta) as u32..(marked_before.end as i64 + delta) as u32;
                prop_assert_eq!(&marked_after, &shifted, "marked text after the edit shifts by the delta");
            }
            document.unmark_text(watching);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// The edit log is a faithful history: revisions count the edits,
    /// `as_of` cuts back exactly, composing everything since a
    /// revision replays onto that revision's text, the edited ranges
    /// since a revision are sorted, disjoint, in bounds and cover
    /// every inserted byte, and `transform_range` from an older
    /// revision agrees with the naive delta model wherever that model
    /// is unambiguous.
    #[test]
    fn the_edit_log_replays_and_transforms_like_the_naive_model(
        source in source(),
        edits in prop::collection::vec((0.0..=1.0f64, 0.0..=1.0f64, snippet()), 1..8),
    ) {
        let mut log = EditLog::new();
        let mut history = vec![source.clone()];
        let mut shapes: Vec<(Range<u32>, u32)> = Vec::new();
        for (a, b, inserted) in &edits {
            let model = history.last().expect("a base").clone();
            let (operation, replaced, inserted_len) = replacement(&model, *a, *b, inserted);
            log.record(&operation, model.len() as u32);
            history.push(apply(&model, &operation));
            shapes.push((replaced, inserted_len));
            prop_assert_eq!(log.revision() as usize, history.len() - 1, "revisions count edits");
        }
        let final_text = history.last().expect("a final text").clone();

        for revision in 0..=log.revision() {
            prop_assert_eq!(log.as_of(revision).revision(), revision);
            let base = &history[revision as usize];
            match log.compose_since(revision) {
                Some(composed) => prop_assert_eq!(apply(base, &composed), final_text.clone(), "composition since {} replays", revision),
                None => prop_assert_eq!(revision, log.revision(), "only the head has nothing since it"),
            }

            let ranges = log.ranges_since(revision);
            let mut last_end = None;
            for range in &ranges {
                prop_assert!(range.start <= range.end && range.end as usize <= final_text.len(), "{}", format!("{range:?} in bounds of {}", final_text.len()));
                if let Some(end) = last_end {
                    prop_assert!(range.start > end, "{}", format!("ranges are sorted and disjoint: {ranges:?}"));
                }
                last_end = Some(range.end);
            }
            // Every byte inserted since `revision` is covered: walk
            // each insert forward through the later operations.
            for (index, operation) in log.since(revision).enumerate() {
                let at = revision as usize + index;
                let (replaced, inserted_len) = &shapes[at];
                if *inserted_len == 0 {
                    continue;
                }
                let mut inserted = replaced.start..replaced.start + inserted_len;
                for later in log.since(at as u64 + 1) {
                    inserted = EditLog::transform_range(inserted, later);
                }
                let _ = operation;
                if inserted.is_empty() {
                    continue;
                }
                prop_assert!(
                    ranges.iter().any(|range| range.start <= inserted.start && inserted.end <= range.end),
                    "{}", format!("inserted {inserted:?} (edit {at}) covered by ranges since {revision}: {ranges:?}")
                );
            }

            for point in boundaries(base) {
                let mut transformed = point..point;
                let mut model = Some(point);
                for (index, operation) in log.since(revision).enumerate() {
                    transformed = EditLog::transform_range(transformed, operation);
                    let (replaced, inserted_len) = &shapes[revision as usize + index];
                    model = model.and_then(|position| model_position(position, replaced, *inserted_len));
                }
                if let Some(expected) = model {
                    prop_assert_eq!(transformed, expected..expected, "point {} from revision {} follows the delta model", point, revision);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
enum FragmentEdit {
    /// The bounded editor types at a position inside its window.
    Inside(f64, String),
    /// The whole-document editor types anywhere.
    Whole(f64, String),
    /// A replacement anywhere, possibly straddling the fragment.
    Replace(f64, f64, String),
}

fn fragment_edit() -> impl Strategy<Value = FragmentEdit> {
    prop_oneof![
        3 => (0.0..=1.0f64, snippet()).prop_map(|(at, text)| FragmentEdit::Inside(at, text)),
        3 => (0.0..=1.0f64, snippet()).prop_map(|(at, text)| FragmentEdit::Whole(at, text)),
        2 => (0.0..=1.0f64, 0.0..=1.0f64, snippet()).prop_map(|(a, b, text)| FragmentEdit::Replace(a, b, text)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// A bounded editor lays out exactly its fragment: through edits
    /// inside it, outside it and straddling it, its window is the
    /// fragment's range, its elements tile that window, and its
    /// heights equal a fresh complete build of the same window. An
    /// insert strictly before the fragment shifts it whole; one
    /// inside it, its first byte and its end included, grows it —
    /// the window is an editing window, what its editor types stays
    /// in it.
    #[test]
    fn a_bounded_editor_tracks_its_fragment_and_matches_a_fresh_build(
        source in source(),
        width in widths(),
        (a, b) in (0.0..=1.0f64, 0.0..=1.0f64),
        edits in prop::collection::vec(fragment_edit(), 1..8),
    ) {
        let (start, end) = {
            let (a, b) = (pick(&source, a), pick(&source, b));
            (a.min(b), a.max(b))
        };
        prop_assume!(start < end);

        let mut store = seeded_store();
        let ui = test_ui();
        let mut document = plain_document(&source);
        let set = document.add_fragment_set();
        let key = document.add_editing_window(set, start..end);
        let bounded = add_editor(&mut document, width, Some(key), EditorBuild::Bounded, &store);
        let whole = add_editor(&mut document, width, None, EditorBuild::Complete, &store);

        prop_assert_eq!(document.window(bounded), start..end);
        assert_layout_fresh(&document, bounded, &store)?;

        for edit in &edits {
            let text = text_of(&document);
            let fragment = document.fragment_range(key).expect("the fragment lives");
            let mut batch = fx();
            let mut expected = None;
            let mut swallowed = false;
            match edit {
                FragmentEdit::Inside(at, inserted) => {
                    let window = document.window(bounded);
                    let inside = boundaries(&text)
                        .into_iter()
                        .filter(|byte| window.contains(byte) || *byte == window.end)
                        .collect::<Vec<_>>();
                    let position = inside[((at * inside.len() as f64) as usize).min(inside.len() - 1)];
                    document.set_caret(bounded, position);
                    prop_assert_eq!(document.caret_byte(bounded), position, "a caret inside the window is not clamped");
                    document.insert(bounded, inserted, &store, ui, &theme(), &mut batch.effects());
                    expected = Some(fragment.start..fragment.end + inserted.len() as u32);
                }
                FragmentEdit::Whole(at, inserted) => {
                    let position = pick(&text, *at);
                    document.set_caret(whole, position);
                    document.insert(whole, inserted, &store, ui, &theme(), &mut batch.effects());
                    let shift = inserted.len() as u32;
                    expected = Some(if position < fragment.start {
                        fragment.start + shift..fragment.end + shift
                    } else if position <= fragment.end {
                        fragment.start..fragment.end + shift
                    } else {
                        fragment.clone()
                    });
                }
                FragmentEdit::Replace(a, b, inserted) => {
                    let (operation, replaced, _) = replacement(&text, *a, *b, inserted);
                    if operation.is_empty() {
                        continue;
                    }
                    document.edit(&operation, &store, ui, &theme(), &mut batch.effects());
                    swallowed = replaced.start <= fragment.start && fragment.end <= replaced.end;
                }
            }
            drain_repairs(&mut document, whole, &mut store, batch);

            let Some(fragment) = document.fragment_range(key) else {
                // A replacement that covers the whole fragment removes
                // it; the bounded editor is then left with an empty
                // window. Anything else must keep the fragment.
                prop_assert!(swallowed, "only a replacement covering the fragment removes it");
                prop_assert_eq!(document.window(bounded), 0..0);
                break;
            };
            let byte_count = document.text().byte_count() as u32;
            prop_assert!(fragment.start <= fragment.end && fragment.end <= byte_count, "{}", format!("fragment {fragment:?} within {byte_count}"));
            prop_assert_eq!(document.window(bounded), fragment.clone(), "the window is the fragment");
            if let Some(expected) = expected {
                prop_assert_eq!(&fragment, &expected, "the fragment follows the insert");
            }
            let final_text = text_of(&document);
            prop_assert!(final_text.is_char_boundary(fragment.start as usize));
            prop_assert!(final_text.is_char_boundary(fragment.end as usize));
            assert_carets_sane(&document, bounded)?;
            assert_carets_sane(&document, whole)?;
            assert_layout_fresh(&document, bounded, &store)?;
            assert_layout_fresh(&document, whole, &store)?;
        }
    }

    /// Several complete editors on one document, at different
    /// widths, agree after edits through any of them: each one's
    /// elements tile the whole text and its heights equal a fresh
    /// build at its width, once repairs have settled.
    #[test]
    fn every_editor_on_a_document_matches_a_fresh_build_after_edits_through_any_of_them(
        source in source(),
        styled in any::<bool>(),
        widths in prop::collection::vec(widths(), 1..=3),
        actions in prop::collection::vec((0..3usize, action()), 1..10),
    ) {
        let mut store = seeded_store();
        let mut document = match styled {
            true => list_document(&source),
            false => plain_document(&source),
        };
        let editors: Vec<EditorId> = widths
            .iter()
            .map(|width| add_editor(&mut document, *width, None, EditorBuild::Complete, &store))
            .collect();
        for editor in &editors {
            assert_layout_fresh(&document, *editor, &store)?;
        }

        for (which, action) in &actions {
            let editor = editors[which % editors.len()];
            let batch = perform_action(&mut document, editor, &mut store, action);
            drain_repairs(&mut document, editor, &mut store, batch);
            for editor in &editors {
                assert_carets_sane(&document, *editor)?;
                assert_layout_fresh(&document, *editor, &store)?;
            }
        }
    }
}

/// Shrunk from the property above (2026-10-26, on 63ff887a): a
/// soft-wrapped row start is not a safe resume point when the edit
/// changes what the row ABOVE can absorb. After the first insert the
/// rows are " nl " | "eaaaaaaaaaaaa ..." (the long word does not fit
/// after "nl"). Typing "é😀" after the "e" makes "eé😀" a short,
/// breakable word (an emoji is a break opportunity) that a fresh
/// layout pulls back onto the first row; a repair resuming at the
/// soft row start kept the stale break at byte 4 and showed an extra
/// row. Only a unit's first row may be a safepoint.
#[test]
fn typing_a_breakable_word_lets_the_previous_row_absorb_it() {
    let store = seeded_store();
    let ui = test_ui();
    let mut document = plain_document("aaaaaaaaaaaa aaaaaauaa");
    let editor = add_editor(&mut document, 220.0, None, EditorBuild::Complete, &store);
    // The first insert is what makes the row start at byte 4 a
    // REPAIRED one; a fresh build of the same text passes.
    document.set_caret(editor, 0);
    document.insert(editor, " nl e", &store, ui, &theme(), &mut fx().effects());
    assert_eq!(
        document.element_heights(editor),
        document.fresh_layout_heights(editor, &store, ui, &theme()),
        "the live layout equals a fresh one after the first insert"
    );
    document.set_caret(editor, 5);
    document.insert(editor, "é😀", &store, ui, &theme(), &mut fx().effects());
    assert_eq!(document.probe_state(editor).0, None, "nothing pending");
    assert_eq!(
        document.element_heights(editor),
        document.fresh_layout_heights(editor, &store, ui, &theme()),
        "the live layout equals a fresh one"
    );
}

/// Shrunk 2026-10-26: a deletion before an editing window pulls it
/// to the text's start, where a window over the whole text — like
/// an unbounded layout — ends in the trailing empty line after a
/// final newline. The repair must lay that line, far from the edit.
#[test]
fn a_window_pulled_to_the_text_start_gains_the_trailing_empty_line() {
    let store = seeded_store();
    let ui = test_ui();
    let mut document = plain_document(" \n");
    let set = document.add_fragment_set();
    let key = document.add_editing_window(set, 1..2);
    let bounded = add_editor(
        &mut document,
        220.0,
        Some(key),
        EditorBuild::Bounded,
        &store,
    );
    let whole = add_editor(&mut document, 220.0, None, EditorBuild::Complete, &store);
    document.set_caret(whole, 2);
    document.insert(whole, "\n", &store, ui, &theme(), &mut fx().effects());
    assert_eq!(
        document.window(bounded),
        1..3,
        "an insert at the window's end grows it"
    );
    let text = text_of(&document);
    let (operation, _, _) = replacement(&text, 0.29167939411642646, 0.0, "a");
    document.edit(&operation, &store, ui, &theme(), &mut fx().effects());
    assert_eq!(text_of(&document), "a\n\n");
    assert_eq!(
        document.window(bounded),
        0..3,
        "the window now spans the whole text"
    );
    assert_eq!(document.probe_state(bounded).0, None, "nothing pending");
    assert_eq!(
        document.element_heights(bounded),
        document.fresh_layout_heights(bounded, &store, ui, &theme()),
        "the live layout equals a fresh one"
    );
}
