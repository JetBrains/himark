// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use imba::effect::Batch;

fn test_ui() -> UiCtx {
    let ui = UiCtx::dont_use_too_slow();
    ui.set(::editor::env::UiFonts(
        ::editor::test_document::test_fonts_collection().clone(),
    ));
    ui
}

fn test_store() -> Store {
    let mut store = Store::new();
    crate::env::Themes::set(&mut store, crate::Theme::embedded());
    store
}

fn face(line: &str, markdown: &str) -> ToolFace {
    ToolFace {
        line: line.to_owned(),
        markdown: crate::Text::from_string_exact(markdown),
        failed: false,
        live: false,
    }
}

fn call(id: &str, name: &str) -> ToolCallSpec {
    ToolCallSpec {
        id: id.to_owned(),
        display_name: name.to_owned(),
        face: face(
            &format!("{name} — Ran {id}"),
            &format!("**{name}** — Ran {id}\n\n```\noutput of {id}\n```"),
        ),
    }
}

fn live_call(id: &str, name: &str) -> ToolCallSpec {
    let mut spec = call(id, name);
    spec.face.live = true;
    spec.face.line = format!("{name} — running… {id}");
    spec
}

fn group(store: &Store, ui: &UiCtx, specs: Vec<ToolCallSpec>) -> ToolGroup {
    ToolGroup::new(store, ui, specs, 600.0).0
}

fn drive(group: &mut ToolGroup, store: &mut Store, ui: &UiCtx, key: ToolRowKey) {
    let mut batch: Batch<CellCommand> = Batch::new();
    group.perform_keyed(
        store,
        ui,
        key,
        TreeItemCommand::Toggle,
        &mut batch.effects(),
    );
}

fn click(group: &mut ToolGroup, store: &mut Store, ui: &UiCtx, row: usize) {
    let mut batch: Batch<CellCommand> = Batch::new();
    group.perform(
        store,
        ui,
        ListCommand::Focus(
            row,
            Some(Box::new(ListCommand::Child(row, TreeItemCommand::Toggle))),
        ),
        &mut batch.effects(),
    );
}

fn tick(group: &mut ToolGroup, store: &mut Store, ui: &UiCtx, millis: f64) {
    let mut batch: Batch<CellCommand> = Batch::new();
    group.perform(
        store,
        ui,
        ListCommand::Animate(imba::anim::AnimationClock::from_millis(millis)),
        &mut batch.effects(),
    );
}

fn update(group: &mut ToolGroup, store: &mut Store, ui: &UiCtx, update: ToolUpdate) {
    let mut batch: Batch<CellCommand> = Batch::new();
    group.update(store, ui, update, &mut batch.effects());
}

#[test]
fn a_run_collapses_to_counts_by_display_name() {
    let store = test_store();
    let ui = test_ui();
    let group = group(
        &store,
        &ui,
        vec![
            call("one", "Bash"),
            call("two", "Bash"),
            call("three", "Grep"),
            call("four", "Bash"),
        ],
    );
    assert_eq!(group.oracle(), vec!["> 3 × Bash, 1 × Grep".to_owned()]);
}

#[test]
fn a_lone_call_shows_its_own_line() {
    let store = test_store();
    let ui = test_ui();
    let group = group(&store, &ui, vec![call("one", "Bash")]);
    assert_eq!(group.oracle(), vec!["> Bash — Ran one".to_owned()]);
}

#[test]
fn opening_the_run_lists_its_calls() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(&store, &ui, vec![call("one", "Bash"), call("two", "Grep")]);
    drive(&mut group, &mut store, &ui, ToolRowKey::Group);
    assert_eq!(
        group.oracle(),
        vec![
            "v 1 × Bash, 1 × Grep".to_owned(),
            "  > Bash — Ran one".to_owned(),
            "  > Grep — Ran two".to_owned(),
        ],
        "the counts stay on the group row while its calls list"
    );
}

#[test]
fn a_body_is_born_on_opening_and_dies_on_closing() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(&store, &ui, vec![call("one", "Bash")]);
    assert_eq!(group.oracle().len(), 1, "closed: the line only");

    drive(
        &mut group,
        &mut store,
        &ui,
        ToolRowKey::Face("one".to_owned()),
    );
    let open = group.oracle();
    assert_eq!(open.len(), 2, "open: the line and its body — {open:?}");
    assert!(open[0].starts_with("v "), "the call reads open: {open:?}");
    assert!(
        open[1].contains("output of one"),
        "the body carries the full form: {open:?}"
    );

    drive(
        &mut group,
        &mut store,
        &ui,
        ToolRowKey::Face("one".to_owned()),
    );
    assert_eq!(group.oracle().len(), 1, "closed again: the body left");
}

#[test]
fn the_work_steers_the_disclosure_until_it_settles() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(
        &store,
        &ui,
        vec![call("one", "Bash"), live_call("two", "Bash")],
    );
    assert_eq!(
        group.oracle().len(),
        3,
        "a running call holds the run open: {:?}",
        group.oracle()
    );

    update(
        &mut group,
        &mut store,
        &ui,
        ToolUpdate::Face {
            id: "two".to_owned(),
            face: face("Bash — Ran two", "**Bash** — Ran two"),
        },
    );
    assert_eq!(
        group.oracle(),
        vec!["> 2 × Bash".to_owned()],
        "the settled run collapses back to its counts"
    );
}

#[test]
fn the_readers_choice_stands() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(
        &store,
        &ui,
        vec![call("one", "Bash"), live_call("two", "Bash")],
    );
    drive(&mut group, &mut store, &ui, ToolRowKey::Group);
    drive(&mut group, &mut store, &ui, ToolRowKey::Group);
    assert_eq!(group.oracle().len(), 3, "the reader re-opened the run");

    update(
        &mut group,
        &mut store,
        &ui,
        ToolUpdate::Face {
            id: "two".to_owned(),
            face: face("Bash — Ran two", "**Bash** — Ran two"),
        },
    );
    let rows = group.oracle();
    assert_eq!(rows.len(), 3, "the run stays open: {rows:?}");
    assert!(
        rows[2].contains("Ran two"),
        "the settled face moved on in place: {rows:?}"
    );
}

#[test]
fn a_second_call_mints_the_group_row() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(&store, &ui, vec![call("one", "Bash")]);
    assert_eq!(group.oracle(), vec!["> Bash — Ran one".to_owned()]);

    update(
        &mut group,
        &mut store,
        &ui,
        ToolUpdate::Add(live_call("two", "Grep")),
    );
    assert_eq!(
        group.oracle(),
        vec![
            "v 1 × Bash, 1 × Grep".to_owned(),
            "  > Bash — Ran one".to_owned(),
            "  > Grep — running… two".to_owned(),
        ],
        "the run stands, open while the newcomer works"
    );
}

#[test]
fn a_click_walks_the_run_open() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(&store, &ui, vec![call("one", "Bash"), call("two", "Grep")]);

    click(&mut group, &mut store, &ui, 0);
    assert_eq!(
        group.oracle().len(),
        3,
        "clicking the group row lists its calls: {:?}",
        group.oracle()
    );

    click(&mut group, &mut store, &ui, 1);
    let rows = group.oracle();
    assert_eq!(rows.len(), 4, "clicking a call opens its body: {rows:?}");
    assert!(
        rows[2].contains("output of one"),
        "the body is the full form: {rows:?}"
    );

    click(&mut group, &mut store, &ui, 0);
    assert_eq!(group.oracle().len(), 1, "clicking again closes the run");
}

#[test]
fn opening_unrolls_from_the_group_row() {
    let mut store = test_store();
    let ui = test_ui();
    let mut group = group(&store, &ui, vec![call("one", "Bash"), call("two", "Grep")]);
    let closed = group.rows.total_height();
    assert!(closed > 0.0, "the closed run stands one row tall");

    click(&mut group, &mut store, &ui, 0);
    let opening = group.rows.total_height();
    assert_eq!(group.oracle().len(), 3, "all three rows are in the list");
    assert!(
        (opening - closed).abs() < 1.0,
        "the entering block takes the old row's space and unrolls from          there — no flash, no jump: {closed} → {opening}"
    );

    tick(&mut group, &mut store, &ui, 0.0);
    tick(&mut group, &mut store, &ui, 5_000.0);
    let open = group.rows.total_height();
    assert!(
        open > closed * 2.0,
        "the sweep lands all three rows at full height: {opening} → {open}"
    );
}
