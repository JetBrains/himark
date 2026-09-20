// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The scroll-stripe batch-tail sweep (docs/editor/scroll-stripe.md §4): one
//! choke covering every path that can move the projection — typing,
//! flagged markup swaps, repair landings, theme switches. O(1) when
//! nothing is flagged; per flagged document, a fingerprint compare per
//! enabled editor.

use imba::store::Store;

use crate::{DocumentId, OpenDocuments};

/// The pane-mount door: opts the editor into the track AND seats THE
/// standing stripes diff on it, if the registry already tracks one
/// for this document (reopen after the base was located). The other
/// direction — the diff arriving while panes already show tracks —
/// is `track_diff`'s registration.
pub fn enable_scroll_stripes(
    store: &Store,
    id: DocumentId,
    document: &mut editor::Document,
    editor: editor::EditorId,
) {
    document.enable_scroll_stripes(editor);
    if let Some(markup) = OpenDocuments::stripe_diff(store, id)
        .and_then(|handle| document.diff(handle.id).map(|entry| entry.markup()))
    {
        document.mark_scroll_stripes(editor, markup);
    }
}

pub fn sync_scroll_stripe_lanes<R: 'static>(
    store: &mut Store,
    fx: &mut imba::effect::Effects<'_, R>,
    wrap: impl Fn(DocumentId, editor::EditorCommand) -> R + Send + Clone + 'static,
) {
    let wanting: Vec<DocumentId> = store
        .get::<OpenDocuments>()
        .map(|docs| {
            docs.entries
                .iter()
                .filter(|(_, entity)| entity.document.wants_scroll_stripes())
                .map(|(id, _)| *id)
                .collect()
        })
        .unwrap_or_default();
    if wanting.is_empty() {
        return;
    }
    let theme = editor::env::Themes::of(store);
    for id in wanting {
        let Some(mut document) = OpenDocuments::document(store, id) else {
            continue;
        };
        let launches = document.scroll_stripe_launches(&theme);
        if launches.is_empty() {
            continue;
        }
        for launch in launches {
            let editor = launch.editor;
            let mut slot = launch.supersedes;
            let wrap = wrap.clone();
            fx.relaunch_erased(
                &mut slot,
                imba::effect::AnyEffect::new(launch.effect).map(move |outcome| {
                    wrap(id, editor::EditorCommand::ApplyScrollStripes(outcome))
                }),
            );
            document.note_scroll_stripe_token(editor, slot);
        }
        OpenDocuments::put_document(store, id, document);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::test_document::plain_document;

    #[test]
    fn the_sweep_launches_once_and_the_landing_settles_the_track() {
        let mut store = Store::new();
        let source: String = (0..100).fold(String::new(), |mut text, index| {
            use std::fmt::Write;
            let _ = writeln!(text, "line {index:03}");
            text
        });
        let id = OpenDocuments::register(
            &mut store,
            plain_document(&source),
            None,
            "doc".to_owned(),
            0,
        );
        let fonts = editor::env::Fonts::of(&store)();
        let theme = editor::env::Themes::of(&store);
        let ui = &imba::UiCtx::dont_use_too_slow();
        let mut sink = imba::effect::Batch::new();
        let quiet = &mut sink.effects();

        let mut document = OpenDocuments::document(&store, id).expect("registered");
        let editor_id = document.add_editor(
            400.0,
            None,
            editor::EditorBuild::Complete,
            &[],
            &store,
            ui,
            &fonts,
            &theme,
            quiet,
        );
        document.enable_scroll_stripes(editor_id);
        let markup = document.add_markup();
        document.show_markup(editor_id, markup);
        document.mark_scroll_stripes(editor_id, markup);
        let mut tints = editor::Markup::new();
        tints.push_styled(90..95, editor::ThemeStyleId::Match);
        document.replace_markup(markup, tints, &[],
            &store, ui, &fonts, &theme, quiet);
        OpenDocuments::put_document(&mut store, id, document);

        let mut batch = imba::effect::Batch::new();
        sync_scroll_stripe_lanes(&mut store, &mut batch.effects(), |document, command| {
            (document, command)
        });
        let mut outcomes = Vec::new();
        for launch in batch.surviving_launches() {
            let Ok(effect) = launch
                .into_payload()
                .split()
                .0
                .downcast::<editor::scroll_stripe::ScrollStripeEffect>()
            else {
                continue;
            };
            outcomes.push(editor::scroll_stripe::land(*effect, &theme));
        }
        assert_eq!(outcomes.len(), 1, "one lane per enabled editor");

        let ui = imba::UiCtx::dont_use_too_slow();
        let outcome = outcomes.pop().expect("counted");
        let landing = editor::EditorCommand::ApplyScrollStripes(outcome);
        let mut document = OpenDocuments::document(&store, id).expect("registered");
        document.perform(&mut store, &ui, editor_id, landing, quiet);
        let landed = document
            .scroll_stripes(editor_id)
            .expect("the track landed");
        assert_eq!(landed.segments.len(), 1);
        assert_eq!(landed.segments[0].byte, 90);
        OpenDocuments::put_document(&mut store, id, document);

        let mut again = imba::effect::Batch::<()>::new();
        sync_scroll_stripe_lanes(&mut store, &mut again.effects(), |_, _| ());
        assert_eq!(
            again.drain().len(),
            0,
            "an unmoved fingerprint owes nothing"
        );
    }
}
