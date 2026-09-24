// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn fonts() -> skia_safe::textlayout::FontCollection {
    crate::test_document::test_fonts_collection().clone()
}

fn run_effect(
    effect: imba::effect::AnyEffect<SplitDiffCommand>,
    workshop: &std::sync::Arc<crate::env::Workshop>,
) -> SplitDiffCommand {
    use imba::effect::{block_on, EffectHandler};
    let (value, lift) = effect.into_payload().split();
    let outcome: Box<dyn std::any::Any + Send + Sync> = match value.downcast::<RepairDiffEffect>() {
        Ok(effect) => {
            let handler = RepairDiffHandler(std::sync::Arc::clone(workshop));
            Box::new(block_on(Box::pin(
                async move { handler.handle(*effect).await },
            )))
        }
        Err(value) => match value.downcast::<crate::repair::RepairEffect>() {
            Ok(effect) => {
                let handler = crate::repair::RepairHandler(std::sync::Arc::clone(workshop));
                Box::new(block_on(Box::pin(
                    async move { handler.handle(*effect).await },
                )))
            }
            Err(value) => match value.downcast::<crate::reparse::ReparseEffect>() {
                Ok(effect) => {
                    let handler = crate::reparse::ReparseHandler(std::sync::Arc::clone(workshop));
                    Box::new(block_on(Box::pin(
                        async move { handler.handle(*effect).await },
                    )))
                }
                Err(_) => panic!("run_effect: unknown pane effect type"),
            },
        },
    };
    lift(outcome).expect("a notification lands nothing — this harness drives only landing effects")
}

fn test_workshop() -> std::sync::Arc<crate::env::Workshop> {
    std::sync::Arc::new(crate::env::Workshop::new(
        std::sync::Arc::new(fonts),
        crate::theme::Theme::embedded(),
    ))
}

fn perform_collect(
    view: &mut SplitDiffView,
    store: &mut Store,
    ui: &UiCtx,
    command: SplitDiffCommand,
) -> Vec<imba::effect::AnyEffect<SplitDiffCommand>> {
    let mut batch = imba::effect::Batch::new();
    view.perform(store, ui, command, &mut batch.effects());
    launches(batch)
}

fn launches(
    mut batch: imba::effect::Batch<SplitDiffCommand>,
) -> Vec<imba::effect::AnyEffect<SplitDiffCommand>> {
    use imba::effect::Message;
    // Settle requests are the ENGINE's business (a synchronous pulse
    // before paint), never a handler's — strip them like it does.
    let _ = batch.take_settle();
    let messages = batch.drain();
    let cancelled: std::collections::HashSet<_> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Cancel(token) => Some(*token),
            Message::Relaunch(previous, _, _) => Some(*previous),
            Message::Launch(..) => None,
        })
        .collect();
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::Launch(token, effect) | Message::Relaunch(_, token, effect)
                if !cancelled.contains(&token) =>
            {
                Some(effect)
            }
            _ => None,
        })
        .collect()
}

fn drain(view: &mut SplitDiffView, effects: Vec<imba::effect::AnyEffect<SplitDiffCommand>>) {
    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    let workshop = test_workshop();
    let mut pending = effects;
    while let Some(effect) = pending.pop() {
        let command = run_effect(effect, &workshop);
        pending.extend(perform_collect(view, &mut store, &ui, command));
    }
}

fn track(left: &mut crate::Document, right: &mut crate::Document) -> DiffState {
    let operation = myersdiff::diff(left.text(), right.text());
    let id = right.add_diff(operation, left.revision());
    let left_marks = left.add_markup();
    let right_marks = right.add_markup();
    DiffState::attach(id, left, right, left_marks, right_marks, None)
        .expect("the entry was just installed")
}

fn normalize(view: &mut SplitDiffView) {
    let minimal = myersdiff::diff(view.left.document.text(), view.right.document.text());
    let id = view.state.id;
    let base_revision = view.left.document.revision();
    assert!(view
        .right
        .document
        .install_normalized_diff(id, minimal, base_revision));
    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    let effects = perform_collect(view, &mut store, &ui, SplitDiffCommand::Resync);
    drain(view, effects);
}

fn pair(left: &str, right: &str, width: f32) -> SplitDiffView {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let theme = crate::theme::Theme::embedded();
    let f = fonts();
    let mut left_document = crate::test_document::plain_document(left);
    let left_editor = left_document.add_editor(
        width,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut right_document = crate::test_document::plain_document(right);
    let right_editor = right_document.add_editor(
        width,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let state = track(&mut left_document, &mut right_document);
    let (left_marks, right_marks) = state.mark_markups();
    left_document.show_markup(left_editor, left_marks);
    right_document.show_markup(right_editor, right_marks);
    let mut view = SplitDiffView::new(
        EditorView {
            document: left_document,
            editor: left_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        EditorView {
            document: right_document,
            editor: right_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        state,
    );

    let mut batch = imba::effect::Batch::new();
    {
        let mut fx = batch.effects();
        view.settle_after(None, None);
        view.pair_lane(&mut fx);
    }
    drain(&mut view, launches(batch));

    normalize(&mut view);
    view
}

fn assert_aligned(view: &SplitDiffView) {
    let left_layout = &view.left.document.editors[&view.left.editor].layout;
    let right_layout = &view.right.document.editors[&view.right.editor].layout;
    let spacers = left_layout.element_spacers();
    let mut aligned = 0;
    for (index, range) in left_layout.element_byte_ranges().iter().enumerate() {
        let boundary = range.start;

        if range.is_empty() {
            continue;
        }
        let mapped = view
            .state
            .diff()
            .transform_offset(boundary, operation::Bias::Right);
        if view
            .state
            .diff()
            .transform_offset_back(mapped, operation::Bias::Right)
            != boundary
        {
            continue;
        }
        let right_ranges = right_layout.element_byte_ranges();
        let Some(right_index) = right_ranges.iter().position(|r| r.start == mapped) else {
            continue;
        };
        aligned += 1;
        let left_top = left_layout.height_before(boundary) as i64 + spacers[index] as i64;
        let right_top = right_layout.height_before(mapped) as i64
            + right_layout.element_spacers()[right_index] as i64;
        assert_eq!(left_top, right_top, "boundary {boundary}↔{mapped}");
    }
    assert!(aligned > 0, "some boundaries must pair");

    for (layout, other, forward) in [
        (left_layout, right_layout, true),
        (right_layout, left_layout, false),
    ] {
        for (range, spacer) in layout
            .element_byte_ranges()
            .into_iter()
            .zip(layout.element_spacers())
        {
            if spacer <= 0.5 {
                continue;
            }
            let boundary = range.start;
            let diff = view.state.diff();
            let (mapped, round_trip) = match forward {
                true => {
                    let mapped = diff.transform_offset(boundary, operation::Bias::Right);
                    (
                        mapped,
                        diff.transform_offset_back(mapped, operation::Bias::Right),
                    )
                }
                false => {
                    let mapped = diff.transform_offset_back(boundary, operation::Bias::Right);
                    (
                        mapped,
                        diff.transform_offset(mapped, operation::Bias::Right),
                    )
                }
            };
            assert_eq!(
                round_trip, boundary,
                "spacer {spacer} at unpaired boundary {boundary} (forward {forward})"
            );
            assert!(
                other
                    .element_byte_ranges()
                    .iter()
                    .any(|r| r.start == mapped),
                "spacer {spacer} at {boundary}: no counterpart element at {mapped}"
            );
        }
    }
}

#[test]
fn a_fresh_pair_settles_aligned_with_marks() {
    let mut view = pair(
        "shared one\nshared two\nleft-only paragraph long enough to wrap when the width narrows below its natural extent\nshared tail\n",
        "shared one\nshared two\nshared tail\nadded on the right\n",
        260.0,
    );
    let mut store = Store::new();

    let _ = perform_collect(
        &mut view,
        &mut store,
        &UiCtx::dont_use_too_slow(),
        SplitDiffCommand::Left(EditorCommand::Move {
            motion: crate::editor_view::Motion::Right,
            select: false,
        }),
    );
    assert_aligned(&view);
}

#[test]
fn settling_without_changes_leaves_the_markup_generation_alone() {
    let mut view = pair(
        "alpha\nbeta\ngamma\n",
        "alpha\nBETA changed\ngamma\n",
        240.0,
    );
    let mut store = Store::new();
    let _ = perform_collect(
        &mut view,
        &mut store,
        &UiCtx::dont_use_too_slow(),
        SplitDiffCommand::Left(EditorCommand::Move {
            motion: crate::editor_view::Motion::Right,
            select: false,
        }),
    );
    let before = (
        view.left.document.markup_generation(),
        view.right.document.markup_generation(),
    );
    for _ in 0..3 {
        let _ = perform_collect(
            &mut view,
            &mut store,
            &UiCtx::dont_use_too_slow(),
            SplitDiffCommand::Right(EditorCommand::Move {
                motion: crate::editor_view::Motion::Right,
                select: false,
            }),
        );
    }
    let after = (
        view.left.document.markup_generation(),
        view.right.document.markup_generation(),
    );
    assert_eq!(before, after, "no-change settles must not churn markup");
}

#[test]
fn typing_and_landings_keep_the_pair_aligned() {
    let mut view = pair(
        "alpha\nbeta\ngamma\n",
        "alpha\nBETA changed\ngamma\n",
        240.0,
    );
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workshop = test_workshop();

    let mut pending: Vec<imba::effect::AnyEffect<SplitDiffCommand>> = Vec::new();
    for step in 0..6 {
        let effects = perform_collect(
            &mut view,
            &mut store,
            &ui,
            SplitDiffCommand::Right(EditorCommand::InsertText {
                text: format!("x{step}—"),
            }),
        );
        pending.extend(effects);
        assert_aligned(&view);

        if step % 2 == 1 {
            if let Some(effect) = pending.pop() {
                let command = run_effect(effect, &workshop);
                let more = perform_collect(&mut view, &mut store, &ui, command);
                pending.extend(more);
                assert_aligned(&view);
            }
        }
    }

    while let Some(effect) = pending.pop() {
        let command = run_effect(effect, &workshop);
        pending.extend(perform_collect(&mut view, &mut store, &ui, command));
    }
    assert_aligned(&view);

    let left_text = {
        let mut v = view.left.document.text().view();
        let n = v.byte_count();
        v.byte_string(0, n)
    };
    let right_text = {
        let mut v = view.right.document.text().view();
        let n = v.byte_count();
        v.byte_string(0, n)
    };

    let mut out = String::new();
    let mut at = 0usize;
    for op in view.state.diff().iter() {
        match op {
            operation::Op::Retain(len) => {
                out.push_str(&left_text[at..at + len as usize]);
                at += len as usize;
            }
            operation::Op::Delete(text) => at += text.len(),
            operation::Op::Insert(text) => out.push_str(&text),
        }
    }
    assert_eq!(
        out, right_text,
        "the live diff stays a valid transformation"
    );
}

#[test]
fn typing_across_boundaries_never_leaves_stale_spacers() {
    let source = "one\ntwo\nthree\nfour\nfive\nsix\n";
    let mut view = pair(source, source, 240.0);
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();

    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::Click {
            kind: crate::editor_view::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 40.0),
        }),
    );
    for text in [
        "X",
        "\n",
        "long inserted line that wraps at this width\n",
        "Y—",
    ] {
        let _ = perform_collect(
            &mut view,
            &mut store,
            &ui,
            SplitDiffCommand::Right(EditorCommand::InsertText {
                text: text.to_owned(),
            }),
        );
        assert_aligned(&view);
    }
}

#[test]
fn a_stale_repair_landing_must_not_revert_spacers() {
    let mut left_source = String::new();
    for i in 0..1200 {
        left_source.push_str(&format!("line number {i}\n"));
    }
    let block_at = left_source.find("line number 30\n").unwrap() as u32;
    let (head, tail) = left_source.split_at(block_at as usize);
    let right_source = format!("{head}added A\nadded B\nadded C\n{tail}");
    let mut view = pair(&left_source, &right_source, 240.0);
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let workshop = test_workshop();

    let viewport = || EditorCommand::Viewport {
        width: 240.0,
        top: 0.0,
        bottom: 400.0,
        anchor: 0,
    };
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(viewport()),
    );
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(viewport()),
    );
    assert_aligned(&view);

    let left_end = view.left.document.editors[&view.left.editor]
        .layout
        .height_before(u32::MAX);
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::Click {
            kind: crate::editor_view::ClickKind::Set,
            point: skia_safe::Point::new(1.0, left_end - 5.0),
        }),
    );
    let filler = "filler line\n".repeat(150);
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::InsertText { text: filler }),
    );
    let held: Vec<_> = effects
        .into_iter()
        .filter(|effect| effect.is::<RepairDiffEffect>())
        .collect();
    assert!(
        !held.is_empty(),
        "the budget-cut insert defers a paired repair"
    );

    let block_y = view.right.document.editors[&view.right.editor]
        .layout
        .height_before(block_at);
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::Click {
            kind: crate::editor_view::ClickKind::Set,
            point: skia_safe::Point::new(1.0, block_y + 5.0),
        }),
    );
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::InsertText {
            text: "typed one\ntyped two\n".to_owned(),
        }),
    );
    assert_aligned(&view);

    for effect in held {
        let command = run_effect(effect, &workshop);
        let _ = perform_collect(&mut view, &mut store, &ui, command);
    }
    assert_aligned(&view);
}

#[test]
fn scrolling_derives_marks_for_the_revealed_window_only_once() {
    let mut left_source = String::new();
    for i in 0..1600 {
        left_source.push_str(&format!("paragraph number {i} with some length to it\n"));
    }
    // The tail line CHANGES (not appends): line washes are THE diff
    // markup's now (whole-document from birth); what the pane's own
    // windowed derivation still owes the revealed window is the WORD
    // tints, and a modified word is what mints them.
    let right_source =
        left_source.replace("paragraph number 1599 with", "paragraph number 1599 WITH");
    let mut view = pair(&left_source, &right_source, 240.0);
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();

    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::Move {
            motion: crate::editor_view::Motion::Right,
            select: false,
        }),
    );
    let head_generation = view.right.document.markup_generation();

    let bottom = view.right.document.editors[&view.right.editor]
        .layout
        .height_before(u32::MAX);
    let scroll = |top: f32| EditorCommand::Viewport {
        width: 240.0,
        top,
        bottom: top + 400.0,
        anchor: 0,
    };
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(scroll(bottom - 400.0)),
    );
    drain(&mut view, effects);
    let revealed_generation = view.right.document.markup_generation();
    assert!(
        revealed_generation > head_generation,
        "the revealed tail wash must derive"
    );

    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(scroll(bottom - 400.0)),
    );
    drain(&mut view, effects);
    assert_eq!(
        view.right.document.markup_generation(),
        revealed_generation,
        "a repeated viewport must not re-derive"
    );
}

#[test]
fn a_line_typed_before_a_shared_paragraph_aligns_that_paragraph() {
    let source = "alpha alpha\nbeta beta\ngamma gamma\n";
    let mut view = pair(source, source, 240.0);
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();

    let beta = source.find("beta").unwrap() as u32;
    let beta_y = view.right.document.editors[&view.right.editor]
        .layout
        .height_before(beta);
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::Click {
            kind: crate::editor_view::ClickKind::Set,
            point: skia_safe::Point::new(1.0, beta_y + 5.0),
        }),
    );
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::InsertText {
            text: "inserted right line\n".to_owned(),
        }),
    );

    assert_aligned(&view);

    let left_layout = &view.left.document.editors[&view.left.editor].layout;
    let spacers: Vec<_> = left_layout
        .element_byte_ranges()
        .into_iter()
        .zip(left_layout.element_spacers())
        .filter(|(_, spacer)| *spacer > 0.5)
        .collect();
    let ops: Vec<_> = view.state.diff().iter().collect();
    assert_eq!(spacers.len(), 1, "one filler: {spacers:?} diff {ops:?}");
    assert_eq!(
        spacers[0].0.start, beta,
        "the filler sits at the shared boundary: {spacers:?} diff {ops:?}"
    );
}

#[test]
fn clicking_a_half_takes_focus_from_the_other() {
    let mut view = pair("alpha\nbeta\n", "alpha\nBETA\n", 240.0);
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let click = |x: f32, y: f32| EditorCommand::Click {
        kind: crate::editor_view::ClickKind::Set,
        point: skia_safe::Point::new(x, y),
    };
    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(click(2.0, 5.0)),
    );
    assert_eq!(view.right.focus(), crate::EditorFocus::Text);
    assert_eq!(view.left.focus(), crate::EditorFocus::None);

    let _ = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(click(2.0, 5.0)),
    );
    assert_eq!(
        view.left.focus(),
        crate::EditorFocus::Text,
        "left takes focus"
    );
    assert_eq!(
        view.right.focus(),
        crate::EditorFocus::None,
        "and the right yields it"
    );
}

#[test]
fn height_only_commands_resync_without_rediffing() {
    let mut view = pair(
        "head\nmiddle one\nmiddle two\ntail\n",
        "head\nmiddle one changed\nmiddle two\ntail\n",
        240.0,
    );
    let mut store = Store::new();
    let ui = UiCtx::dont_use_too_slow();
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::Viewport {
            width: 240.0,
            top: 0.0,
            bottom: 300.0,
            anchor: 0,
        }),
    );
    assert!(
        effects.iter().all(|effect| effect.is::<RepairDiffEffect>()),
        "a scroll viewport report launches nothing but the paired repair"
    );
    drain(&mut view, effects);
    assert_aligned(&view);
}

#[test]
fn fuzzed_editing_keeps_the_pair_aligned() {
    let mut rng = 0x243F6A8885A308D3u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let words = [
        "alpha",
        "be",
        "gamma\u{e9}\u{e9}",
        "\u{1F680}",
        "delta",
        "wrap wrap wrap wrap",
        "x",
        "longish-token-that-wraps",
    ];
    let ui = UiCtx::dont_use_too_slow();
    let workshop = test_workshop();

    for seed in 0..40u64 {
        for _ in 0..seed {
            let _ = next();
        }

        let lines = 8 + (next() % 13) as usize;
        let mut left_source = String::new();
        for _ in 0..lines {
            let count = 1 + (next() % 5) as usize;
            for _ in 0..count {
                left_source.push_str(words[(next() % words.len() as u64) as usize]);
                left_source.push(' ');
            }
            left_source.push('\n');
        }

        let mut right_lines: Vec<String> =
            left_source.lines().map(|line| line.to_owned()).collect();
        for _ in 0..1 + next() % 3 {
            let at = (next() % right_lines.len() as u64) as usize;
            match next() % 3 {
                0 => right_lines.insert(at, "inserted on the right".to_owned()),
                1 => {
                    right_lines.remove(at);
                    if right_lines.is_empty() {
                        right_lines.push("kept".to_owned());
                    }
                }
                _ => right_lines[at] = format!("{} CHANGED", right_lines[at]),
            }
        }
        let right_source = format!("{}\n", right_lines.join("\n"));

        let mut view = pair(&left_source, &right_source, 160.0 + (next() % 120) as f32);
        let mut store = Store::new();
        let mut pending: Vec<imba::effect::AnyEffect<SplitDiffCommand>> = Vec::new();

        for step in 0..14 {
            let side_right = next() % 2 == 0;
            let height = match side_right {
                true => &view.right,
                false => &view.left,
            }
            .document
            .editors
            .values()
            .next()
            .map(|editor| editor.layout.height_before(u32::MAX))
            .unwrap_or(0.0);
            let command = match next() % 6 {
                0 | 1 => {
                    let text = match next() % 4 {
                        0 => "x".to_owned(),
                        1 => "\n".to_owned(),
                        2 => format!("{} ", words[(next() % words.len() as u64) as usize]),
                        _ => "two\nlines".to_owned(),
                    };
                    EditorCommand::InsertText { text }
                }
                2 => EditorCommand::Backspace,
                3 => EditorCommand::Click {
                    kind: crate::editor_view::ClickKind::Set,
                    point: skia_safe::Point::new(
                        (next() % 150) as f32,
                        (next() as f32 / u64::MAX as f32) * height.max(1.0),
                    ),
                },
                4 => EditorCommand::Viewport {
                    width: view.left.document.layout_width(view.left.editor),
                    top: (next() % 300) as f32,
                    bottom: 300.0 + (next() % 300) as f32,
                    anchor: 0,
                },
                _ => EditorCommand::Move {
                    motion: crate::editor_view::Motion::Right,
                    select: false,
                },
            };
            let command = match side_right {
                true => SplitDiffCommand::Right(command),
                false => SplitDiffCommand::Left(command),
            };
            pending.extend(perform_collect(&mut view, &mut store, &ui, command));

            if next() % 3 == 0 {
                if let Some(effect) = pending.pop() {
                    let command = run_effect(effect, &workshop);
                    pending.extend(perform_collect(&mut view, &mut store, &ui, command));
                }
            }

            while let Some(effect) = pending.pop() {
                let command = run_effect(effect, &workshop);
                pending.extend(perform_collect(&mut view, &mut store, &ui, command));
            }
            assert_aligned(&view);

            let before = (
                view.left.document.editors[&view.left.editor]
                    .layout
                    .element_spacers(),
                view.right.document.editors[&view.right.editor]
                    .layout
                    .element_spacers(),
            );
            {
                let left_editor = view.left.editor;
                let right_editor = view.right.editor;
                let end = view.left.document.text().byte_count() as u32;
                let left_state = view.left.document.editors.get_mut(&left_editor).unwrap();
                let left_layout = &mut left_state.layout as *mut crate::DocumentLayout;
                let right_state = view.right.document.editors.get_mut(&right_editor).unwrap();
                let right_layout = &mut right_state.layout;

                let left_layout = unsafe { &mut *left_layout };
                if (left_layout.layout_width() - right_layout.layout_width()).abs() <= 1.0 {
                    super::align::sync_spacers(
                        left_layout,
                        right_layout,
                        view.state.diff(),
                        0..end,
                        usize::MAX,
                        &mut 0,
                    );
                }
            }
            let after = (
                view.left.document.editors[&view.left.editor]
                    .layout
                    .element_spacers(),
                view.right.document.editors[&view.right.editor]
                    .layout
                    .element_spacers(),
            );
            assert_eq!(
                before, after,
                "seed {seed} step {step}: the incremental sync must be the full sync's fixed point"
            );
        }

        while let Some(effect) = pending.pop() {
            let command = run_effect(effect, &workshop);
            pending.extend(perform_collect(&mut view, &mut store, &ui, command));
        }
        assert_aligned(&view);
    }
}

#[test]
fn two_diffs_share_a_document_without_clobbering_washes() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let theme = crate::theme::Theme::embedded();
    let f = fonts();
    let mut a = crate::test_document::plain_document("alpha\nbeta\ngamma\n");
    let editor_a = a.add_editor(
        360.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut b = crate::test_document::plain_document("alpha\nBETA\ngamma\n");
    let editor_b1 = b.add_editor(
        360.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let editor_b2 = b.add_editor(
        360.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut c = crate::test_document::plain_document("alpha\nBETA\nGAMMA\n");
    let editor_c = c.add_editor(
        360.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );

    let state1 = track(&mut a, &mut b);
    let state2 = track(&mut b, &mut c);
    let (pane1_left, pane1_right) = state1.mark_markups();
    let (pane2_left, pane2_right) = state2.mark_markups();
    a.show_markup(editor_a, pane1_left);
    b.show_markup(editor_b1, pane1_right);
    b.show_markup(editor_b2, pane2_left);
    c.show_markup(editor_c, pane2_right);

    let mut pane1 = SplitDiffView::new(
        EditorView {
            document: a,
            editor: editor_a,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        EditorView {
            document: b,
            editor: editor_b1,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        state1,
    );
    let mut batch = imba::effect::Batch::new();
    {
        let mut fx = batch.effects();
        pane1.settle_after(None, None);
        pane1.pair_lane(&mut fx);
    }
    drain(&mut pane1, launches(batch));
    let b = pane1.right.document.clone();

    let mut pane2 = SplitDiffView::new(
        EditorView {
            document: b,
            editor: editor_b2,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        EditorView {
            document: c,
            editor: editor_c,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        state2,
    );
    let mut batch = imba::effect::Batch::new();
    {
        let mut fx = batch.effects();
        pane2.settle_after(None, None);
        pane2.pair_lane(&mut fx);
    }
    drain(&mut pane2, launches(batch));
    let b = &pane2.left.document;

    let pane1_washes = b.markup_styled_ranges(pane1_right);
    let pane2_washes = b.markup_styled_ranges(pane2_left);
    assert!(!pane1_washes.is_empty(), "pane 1 washes B");
    assert!(!pane2_washes.is_empty(), "pane 2 washes B");
    assert_ne!(pane1_washes, pane2_washes, "each pane derived its own diff");

    let mut b = pane2.left.document.clone();
    b.remove_markup(
        pane1_right,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    b.remove_editor(editor_b1);
    assert!(b.markup_styled_ranges(pane1_right).is_empty());
    assert_eq!(b.markup_styled_ranges(pane2_left), pane2_washes);
    let washes_visible = b.extras_vec(editor_b2).iter().any(|markup| {
        use intervals::{IntervalQuery, Order};
        markup
            .query(0..u32::MAX, Order::Ascending)
            .any(|entry| matches!(entry.value, crate::markup::Decoration::Styled(_)))
    });
    assert!(
        washes_visible,
        "pane 2's half editor still merges its washes"
    );
}

#[test]
fn the_plain_repair_lane_skips_pair_managed_halves() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let theme = crate::theme::Theme::embedded();
    let f = fonts();
    let mut wall = String::new();
    for line in 0..4000u32 {
        wall.push_str(&format!("line {line}: lorem ipsum dolor sit amet\n"));
    }
    let mut document = crate::test_document::plain_document(&wall);

    let mut discarded = imba::effect::Batch::new();
    let pane = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut discarded.effects(),
    );
    let half = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut discarded.effects(),
    );
    document.manage_repairs_in_pair(half);

    let mut batch = imba::effect::Batch::new();
    let third = document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Bounded,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut batch.effects(),
    );
    let mut covered = Vec::new();
    for message in batch.drain() {
        let (imba::effect::Message::Launch(_, effect)
        | imba::effect::Message::Relaunch(_, _, effect)) = message
        else {
            continue;
        };
        let (value, _) = effect.into_payload().split();
        let repair = value
            .downcast::<crate::repair::RepairEffect>()
            .expect("the pending lane is one repair effect");
        let workshop = test_workshop();
        let handler = crate::repair::RepairHandler(workshop);
        covered.extend(handler.repairs(*repair).into_iter().map(|r| r.editor));
    }
    assert!(covered.contains(&pane), "the pane still repairs plainly");
    assert!(covered.contains(&third), "the opener still repairs plainly");
    assert!(
        !covered.contains(&half),
        "the pair-managed half must never ride the plain lane"
    );
}

#[test]
fn a_typing_storm_holds_one_pair_repair_in_flight() {
    use imba::effect::Message;
    let mut view = pair("alpha\nbeta\n", "alpha\ngamma\n", 400.0);
    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();

    let tokens = |batch: imba::effect::Batch<SplitDiffCommand>| {
        let mut cancels = Vec::new();
        let mut pair_repairs = Vec::new();
        for message in batch.drain() {
            match message {
                Message::Launch(token, effect) if effect.is::<RepairDiffEffect>() => {
                    pair_repairs.push(token)
                }
                Message::Relaunch(previous, token, effect) if effect.is::<RepairDiffEffect>() => {
                    cancels.push(previous);
                    pair_repairs.push(token);
                }
                Message::Cancel(token) => cancels.push(token),
                _ => {}
            }
        }
        (cancels, pair_repairs)
    };
    let insert = |text: &str| {
        SplitDiffCommand::Left(EditorCommand::InsertText {
            text: text.to_owned(),
        })
    };

    let mut first = imba::effect::Batch::new();
    view.perform(&mut store, &ui, insert("x"), &mut first.effects());
    let (_, pairs) = tokens(first);
    assert_eq!(pairs.len(), 1, "one pair repair holds its lane");

    let mut second = imba::effect::Batch::new();
    view.perform(&mut store, &ui, insert("y"), &mut second.effects());
    let (cancels, next_pairs) = tokens(second);
    assert!(
        cancels.contains(&pairs[0]),
        "the keystroke cancels the in-flight pair repair"
    );
    assert_eq!(next_pairs.len(), 1, "the lane refills with one");
}

#[test]
fn folds_derive_on_the_marks_worker_and_adjust_in_lockstep() {
    let middle: Vec<String> = (0..14).map(|n| format!("same {n}")).collect();
    let left_source = format!("LEFT HEAD\n{}\nLEFT TAIL\n", middle.join("\n"));
    let right_source = format!("RIGHT HEAD\n{}\nRIGHT TAIL\n", middle.join("\n"));
    let mut view = pair(&left_source, &right_source, 240.0);

    let strips = |document: &crate::Document,
                  marks: crate::MarkupId|
     -> Vec<(crate::markup::IntervalId, Range<u32>)> {
        document
            .feature_markup(marks)
            .map(|markup| {
                markup
                    .all_inlays_in(0..u32::MAX)
                    .into_iter()
                    .map(|interval| (interval.key.key, interval.range))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (lm, rm) = view.state.mark_markups();
    let left_strips = strips(&view.left.document, lm);
    let right_strips = strips(&view.right.document, rm);
    assert_eq!(left_strips.len(), 1, "one strip left: {left_strips:?}");
    assert_eq!(right_strips.len(), 1, "one strip right: {right_strips:?}");
    let (left_key, left_range) = left_strips[0].clone();
    let (right_key, right_range) = right_strips[0].clone();
    assert_eq!(left_key, right_key, "one SHARED key names the pair");
    assert_eq!(
        left_range.end - left_range.start,
        right_range.end - right_range.start,
        "identical bytes folded on both halves"
    );
    assert_aligned(&view);

    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    let key = crate::markup::InlayKey {
        layer: crate::markup::MarkupLayer::Markup(lm),
        key: left_key,
    };
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::Inlay {
            key,
            command: Box::new(fold::FoldCommand::RevealTop),
        }),
    );
    let left_after = strips(&view.left.document, lm);
    let right_after = strips(&view.right.document, rm);
    assert_eq!(left_after.len(), 1);
    assert_eq!(right_after.len(), 1);
    assert!(
        left_after[0].1.start > left_range.start,
        "the top edge revealed lines: {left_after:?}"
    );
    assert_eq!(
        left_after[0].1.start - left_range.start,
        right_after[0].1.start - right_range.start,
        "both halves moved identically"
    );
    drain(&mut view, effects);
    assert_aligned(&view);

    let key = crate::markup::InlayKey {
        layer: crate::markup::MarkupLayer::Markup(rm),
        key: right_key,
    };
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Right(EditorCommand::Inlay {
            key,
            command: Box::new(fold::FoldCommand::Remove),
        }),
    );
    drain(&mut view, effects);
    assert!(strips(&view.left.document, lm).is_empty(), "unfolded left");
    assert!(
        strips(&view.right.document, rm).is_empty(),
        "unfolded right"
    );

    let layout = &view.left.document.editors[&view.left.editor].layout;
    let mut scan = fold::LineScan::new(view.left.document.text());
    let mut at = left_range.start;
    while at < left_range.end {
        let Some(next) = scan.next_line_start(at, left_range.end) else {
            break;
        };
        assert!(
            layout.height_before(next) > layout.height_before(at) + 0.5,
            "the released line at {at} regained height"
        );
        at = next;
    }

    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::Click {
            kind: crate::editor_view::ClickKind::Set,
            point: skia_safe::Point::new(1.0, 1.0),
        }),
    );
    drain(&mut view, effects);
    let effects = perform_collect(
        &mut view,
        &mut store,
        &ui,
        SplitDiffCommand::Left(EditorCommand::InsertText {
            text: "typed\n".to_owned(),
        }),
    );
    drain(&mut view, effects);
    assert!(
        strips(&view.left.document, lm).is_empty(),
        "removal survives relandings"
    );
    assert_aligned(&view);
}

#[test]
fn prepare_marks_dresses_the_whole_document() {
    use intervals::IntervalQuery;

    let mut left = String::new();
    for line in 0..3000u32 {
        left.push_str(&format!("line {line}: the quiet unchanged middle\n"));
    }
    let mut right = left.clone();
    left.push_str("old tail\n");
    right.push_str("new tail\n");
    let left_text = Text::from_string_exact(left.clone());

    let diff = myersdiff::diff(&left_text, &Text::from_string_exact(right));
    let prepared = prepare_marks(&diff, &left_text);

    let deep = left.len() as u32 - 10;
    assert!(
        prepared.window.end >= left.len() as u32,
        "the seed window spans the document"
    );
    assert!(
        prepared
            .left
            .query(0..u32::MAX, intervals::Order::Ascending)
            .any(|entry| entry.range.end > deep),
        "the deep change washed on the left"
    );
    assert!(
        prepared
            .right
            .query(0..u32::MAX, intervals::Order::Ascending)
            .next()
            .is_some(),
        "the right side washed"
    );
    let strips = prepared.left.all_inlays_in(0..u32::MAX);
    assert!(!strips.is_empty(), "the unchanged middle folded");
    assert_eq!(
        strips[0].key.key,
        crate::markup::IntervalId(u32::MAX),
        "strip keys mint from the top of the id space — the pipeline's scheme"
    );
    assert_eq!(
        strips.len(),
        prepared.right.all_inlays_in(0..u32::MAX).len(),
        "strips pair by key across the halves"
    );
}

#[test]
fn a_seeded_attach_starts_settled_and_owes_no_marks_job() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let theme = crate::theme::Theme::embedded();
    let f = fonts();
    let mut left_source = String::new();
    for line in 0..300u32 {
        left_source.push_str(&format!("line {line}: the quiet unchanged middle\n"));
    }
    let mut right_source = left_source.clone();
    left_source.push_str("old tail\n");
    right_source.push_str("new tail\n");
    let mut left_document = crate::test_document::plain_document(&left_source);
    let mut right_document = crate::test_document::plain_document(&right_source);

    let operation = myersdiff::diff(left_document.text(), right_document.text());
    let id = right_document.add_diff(operation.clone(), left_document.revision());
    assert!(right_document.install_normalized_diff(
        id,
        operation.clone(),
        left_document.revision()
    ));
    let left_marks = left_document.add_markup();
    let right_marks = right_document.add_markup();
    let hunks = right_document.diff(id).expect("just installed").markup();
    let prepared = prepare_marks(&operation, left_document.text());
    let mut throwaway = imba::effect::Batch::new();
    left_document.replace_markup(
        left_marks,
        prepared.left.clone(),
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut throwaway.effects(),
    );
    right_document.replace_markup(
        right_marks,
        prepared.right.clone(),
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut throwaway.effects(),
    );

    let left_editor = left_document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[left_marks],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let right_editor = right_document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[hunks, right_marks],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let height = left_document.editors[&left_editor].layout.height();
    assert!(
        height < 3_000.0,
        "the first build collapsed the unchanged middle behind strips \
         (got {height}px for 300+ lines — unfolded would be ~10000px)"
    );

    assert!(
        left_document.editors[&left_editor]
            .markups
            .contains(&left_marks),
        "a shown markup registers on the editor"
    );

    let state = DiffState::attach(
        id,
        &left_document,
        &right_document,
        left_marks,
        right_marks,
        Some(prepared.window.clone()),
    )
    .expect("the entry stands");
    assert_eq!(
        state.fold_phase,
        fold::FoldPhase::Done,
        "folds already minted"
    );
    assert!(!state.marks_dirty, "washes already derived");
    assert_eq!(state.seen_generation, 1, "normalized at birth");

    let mut view = SplitDiffView::new(
        EditorView {
            document: left_document,
            editor: left_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        EditorView {
            document: right_document,
            editor: right_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        state,
    );

    let mut batch = imba::effect::Batch::new();
    {
        let mut fx = batch.effects();
        view.settle_after(None, None);
        view.pair_lane(&mut fx);
    }
    let effects = launches(batch);
    assert!(!effects.is_empty(), "the alignment effect launched");
    for effect in effects {
        let (value, _) = effect.into_payload().split();
        let repair = value
            .downcast::<RepairDiffEffect>()
            .expect("the pair lane launches paired repairs");
        assert!(
            repair.marks.is_none(),
            "a seeded pane owes no marks derivation at open"
        );
    }
}

#[test]
fn a_width_mismatched_pane_idles_instead_of_livelocking() {
    let store = &imba::store::Store::new();
    let ui = crate::test_document::test_ui();
    let theme = crate::theme::Theme::embedded();
    let f = fonts();
    let mut left_document = crate::test_document::plain_document("alpha\nbeta\n");
    let left_editor = left_document.add_editor(
        400.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let mut right_document = crate::test_document::plain_document("alpha\ngamma\n");
    let right_editor = right_document.add_editor(
        520.0,
        None,
        crate::document::EditorBuild::Complete,
        &[],
        store,
        ui,
        &f,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    let state = track(&mut left_document, &mut right_document);
    let mut view = SplitDiffView::new(
        EditorView {
            document: left_document,
            editor: left_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        EditorView {
            document: right_document,
            editor: right_editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        },
        state,
    );

    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    let workshop = test_workshop();
    let mut batch = imba::effect::Batch::new();
    {
        let mut fx = batch.effects();
        view.settle_after(None, None);
        view.pair_lane(&mut fx);
    }

    let mut effects = launches(batch);
    let mut rounds = 0;
    while let Some(effect) = effects.pop() {
        rounds += 1;
        assert!(
            rounds < 16,
            "the pair lane relaunched off its own landing — the livelock"
        );
        let command = run_effect(effect, &workshop);
        effects.extend(perform_collect(&mut view, &mut store, &ui, command));
    }
    assert!(
        view.state.marks_dirty,
        "the marks stay OWED while the widths disagree — parked, not spun"
    );
}

#[test]
fn folds_at_the_end_of_the_diff_survive_every_edge_command() {
    let tail: Vec<String> = (0..20).map(|n| format!("same {n}")).collect();
    let left_source = format!("LEFT HEAD\n{}\n", tail.join("\n"));
    let right_source = format!("RIGHT HEAD\n{}\n", tail.join("\n"));
    let mut view = pair(&left_source, &right_source, 240.0);
    let (lm, _) = view.state.mark_markups();
    let strip = view
        .left
        .document
        .feature_markup(lm)
        .and_then(|markup| {
            markup
                .all_inlays_in(0..u32::MAX)
                .into_iter()
                .map(|interval| (interval.key.key, interval.range))
                .next()
        })
        .expect("a fold strip over the unchanged tail");
    assert_eq!(
        strip.1.end,
        left_source.len() as u32,
        "the strip runs to EOF — the shape that crashed"
    );

    let ui = UiCtx::dont_use_too_slow();
    let mut store = Store::new();
    for command in [
        fold::FoldCommand::RevealBottom,
        fold::FoldCommand::RevealBottom,
        fold::FoldCommand::RevealTop,
        fold::FoldCommand::HideBottom,
        fold::FoldCommand::HideTop,
        fold::FoldCommand::Remove,
    ] {
        let key = crate::markup::InlayKey {
            layer: crate::markup::MarkupLayer::Markup(lm),
            key: strip.0,
        };
        let effects = perform_collect(
            &mut view,
            &mut store,
            &ui,
            SplitDiffCommand::Left(EditorCommand::Inlay {
                key,
                command: Box::new(command),
            }),
        );
        drain(&mut view, effects);
        assert_aligned(&view);
    }
}
