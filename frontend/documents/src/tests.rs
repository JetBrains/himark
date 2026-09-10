use super::*;
use editor::test_document::plain_document;

#[test]
fn the_retraction_rule_spares_dirty_documents() {
    let mut store = Store::new();
    let mut batch = imba::effect::Batch::<()>::new();

    let clean = plain_document("saved\n");
    let saved_revision = clean.revision();
    let clean_id = OpenDocuments::register(
        &mut store,
        clean,
        None,
        "clean.md".to_owned(),
        saved_revision,
    );

    let dirty = plain_document("typed\n");
    let stale_stamp = dirty.revision().wrapping_sub(1);
    let dirty_id =
        OpenDocuments::register(&mut store, dirty, None, "dirty.md".to_owned(), stale_stamp);

    OpenDocuments::remove_if_editorless(&mut store, clean_id, &mut batch.effects());
    OpenDocuments::remove_if_editorless(&mut store, dirty_id, &mut batch.effects());
    assert!(
        !OpenDocuments::contains(&store, clean_id),
        "editorless and clean: released"
    );
    assert!(
        OpenDocuments::contains(&store, dirty_id),
        "editorless but dirty: unsaved edits are never released"
    );
}

#[test]
fn the_stripes_join_resolves_the_tracked_base_diff() {
    let mut store = Store::new();
    let mut batch = imba::effect::Batch::<()>::new();
    let base_id = OpenDocuments::register(
        &mut store,
        plain_document("one\ntwo\n"),
        None,
        "base".to_owned(),
        0,
    );
    let target_id = OpenDocuments::register(
        &mut store,
        plain_document("one\nTWO\n"),
        None,
        "target".to_owned(),
        0,
    );

    let pane = OpenDocuments::track_diff(&mut store, base_id, target_id, false, None)
        .expect("both registered");
    assert!(OpenDocuments::stripe_diff(&store, target_id).is_none());

    let stripes =
        OpenDocuments::track_diff(&mut store, base_id, target_id, true, None).expect("dedups");
    assert_eq!(pane, stripes, "one diff per pair");
    let handle = OpenDocuments::stripe_diff(&store, target_id).expect("joined now");
    assert_eq!(handle.base, base_id);
    let entry_len = OpenDocuments::document_ref(&store, target_id)
        .and_then(|document| {
            document
                .diff(handle.id)
                .map(|entry| entry.operation().new_len())
        })
        .expect("the entry rides the target");
    assert_eq!(
        entry_len as usize,
        OpenDocuments::document_ref(&store, target_id)
            .expect("registered")
            .text()
            .view()
            .byte_count(),
        "the entry covers the target text"
    );

    OpenDocuments::remove_if_editorless(&mut store, base_id, &mut batch.effects());
    assert!(
        OpenDocuments::contains(&store, base_id),
        "a tracked base is kept — a stripes base is editorless by nature"
    );

    OpenDocuments::untrack_diff(&mut store, stripes, &mut batch.effects());
    assert!(
        OpenDocuments::diff_handle(&store, stripes).is_some(),
        "the pane still holds the diff"
    );
    OpenDocuments::untrack_diff(&mut store, stripes, &mut batch.effects());
    assert!(OpenDocuments::diff_handle(&store, stripes).is_none());
    assert!(
        !OpenDocuments::contains(&store, base_id),
        "released with the last face"
    );
    assert!(
        OpenDocuments::document_ref(&store, target_id)
            .is_none_or(|document| document.diff(stripes).is_none()),
        "the entry left the target"
    );
}

#[test]
fn a_moved_base_retires_the_stale_stripes_track() {
    let mut store = Store::new();
    let mut batch = imba::effect::Batch::<()>::new();
    let location = |authority: &str, name: &str| {
        editor::ResourceLocation::new(
            editor::ResourceType::document(),
            editor::Authority::new(authority),
            vec!["proj".to_owned(), name.to_owned()],
        )
    };
    let working = location("test", "file.md");
    let base_a = location("ref-a", "file.md");
    let base_b = location("ref-b", "file.md");

    let target = plain_document("one\nTWO\n");
    let stale_stamp = target.revision().wrapping_sub(1);
    let target_id = OpenDocuments::register(
        &mut store,
        target,
        Some(working.clone()),
        "file.md".to_owned(),
        stale_stamp,
    );
    OpenDocuments::set_base_requested(&mut store, target_id);

    diffs::land_base_built(
        &mut store,
        target_id,
        base_a.clone(),
        plain_document("one\ntwo\n"),
        &mut batch.effects(),
    );
    let first = OpenDocuments::stripe_diff(&store, target_id).expect("stripes tracked");
    assert_eq!(
        OpenDocuments::location(&store, first.base).as_ref(),
        Some(&base_a)
    );

    assert!(diffs::adopt_base_location(
        &mut store,
        target_id,
        Some(base_a.clone()),
        &mut batch.effects()
    )
    .is_none());
    assert_eq!(
        OpenDocuments::stripe_diff(&store, target_id).map(|handle| handle.id),
        Some(first.id),
        "an unchanged base keeps the tracked diff"
    );

    diffs::rearm_base_asks(&mut store, &|at| at == &working);
    assert!(
        !OpenDocuments::entity(&store, target_id)
            .expect("open")
            .base_requested(),
        "a tracking document re-arms too"
    );
    assert_eq!(
        diffs::adopt_base_location(
            &mut store,
            target_id,
            Some(base_b.clone()),
            &mut batch.effects()
        ),
        Some(base_b.clone()),
        "the moved base wants fetching"
    );
    assert!(
        OpenDocuments::stripe_diff(&store, target_id).is_none(),
        "the stale track retired with the move"
    );
    diffs::land_base_built(
        &mut store,
        target_id,
        base_b.clone(),
        plain_document("one\ntwo\nthree\n"),
        &mut batch.effects(),
    );
    let second = OpenDocuments::stripe_diff(&store, target_id).expect("re-tracked");
    assert_eq!(
        OpenDocuments::location(&store, second.base).as_ref(),
        Some(&base_b)
    );

    diffs::land_base_built(
        &mut store,
        target_id,
        base_a.clone(),
        plain_document("one\ntwo\n"),
        &mut batch.effects(),
    );
    let crossed = OpenDocuments::stripe_diff(&store, target_id).expect("still tracked");
    assert_eq!(
        OpenDocuments::location(&store, crossed.base).as_ref(),
        Some(&base_a),
        "a differing landing replaces the track"
    );

    assert!(
        diffs::adopt_base_location(&mut store, target_id, None, &mut batch.effects()).is_none()
    );
    assert!(
        OpenDocuments::stripe_diff(&store, target_id).is_none(),
        "a vanished base clears the phantom stripes"
    );
    assert!(
        OpenDocuments::contains(&store, target_id),
        "the dirty target itself survives the untrack"
    );
}
