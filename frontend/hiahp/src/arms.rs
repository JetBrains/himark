// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SESSION place arms — every view the AHP half can mount for
//! a place, registered as ONE bridge arm (`install_arms`). Moved
//! verbatim from the engine's `mount_place_inner`.

use std::sync::Arc;

use imba::dyn_view::DynCommand;
use imba::store::Store;
use imba::ui::UiCtx;

use himark::app::{AppCommand, Himark};
use himark::bridge::ViewBridgeId;

/// Register the session vocabulary with the engine.
pub fn install_arms(app: &mut Himark) {
    app.register_bridge_arm(Arc::new(|app, place| mint_session_place(app, place)));
}

/// Mint the view a SESSION place names, plus the ready places whose
/// collections only the session catalog can find.
pub fn mint_session_place(
    app: &mut Himark,
    place: &hikit::navigation::NavigationLocation,
) -> Option<ViewBridgeId> {
    if let Some(place) = place.place::<crate::places::ChatPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
            let panel = ahp_chat::chats::Chats::open_with(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat,
                None,
            );
            Some(himark::mounts::panel_mount(id, panel))
        });
    }
    if let Some(place) = place.place::<crate::places::ChatTranscriptPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
            ahp_chat::chats::Chats::ensure_chat(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat.clone(),
                None,
            );
            Some(himark::mounts::panel_mount(id, Box::new(ahp_chat::transcript::TranscriptPane::new(
                    state.chats(),
                    chat,
                ))))
        });
    }
    if let Some(place) = place.place::<crate::places::ChatComposerPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let chat = ahp_session::session::agents::Agents::channel(store, &key)?.default_chat?;
            ahp_chat::chats::Chats::ensure_chat(
                store,
                ui,
                state.chats(),
                key.host,
                key.session.clone(),
                chat.clone(),
                None,
            );
            Some(himark::mounts::panel_mount(id, Box::new(ahp_chat::composer::ComposerPane::new(state.chats(), chat))))
        });
    }
    if let Some(place) = place.place::<crate::places::ChangesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            // A row tap opens THAT ROW'S DIFF — no canvas on the
            // mounted road (the desktop's multi-diff canvas stays a
            // workbench face); the shell pushes one split-diff.
            let row_key = key.clone();
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, location| {
                    imba::command::Verb::once(OpenDiffFromChanges {
                        origin: id,
                        session: row_key.clone(),
                        changes,
                        source,
                        reveal: location,
                    })
                });
            let view = changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
                opener,
            );
            let view = changesview::hichanges::Changes::mint_view(store, changes, view);
            Some(himark::mounts::modal_mount(id, changesview::changes_view::ChangesPane::new(changes, view)))
        });
    }
    if let Some(place) = place.place::<crate::places::ChangesCanvasPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            // The DESKTOP flavor: a row opens the MULTI-DIFF CANVAS
            // (user law — iOS keeps one inline diff per row). The
            // windowed verb forks windowless at the missing entity
            // and hands the shell a CanvasPlace.
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, reveal| {
                    imba::command::Verb::once(OpenCanvasFromChanges {
                        origin: id,
                        changes,
                        source,
                        reveal,
                    })
                });
            let view = changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::WorkingCopies,
                opener,
            );
            let view = changesview::hichanges::Changes::mint_view(store, changes, view);
            Some(himark::mounts::modal_mount(id, changesview::changes_view::ChangesPane::new(changes, view)))
        });
    }
    if let Some(place) = place.place::<crate::places::HistoryPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let changes = state.changes();
            let opener: changesview::changes_view::CanvasOpener =
                Arc::new(move |source, reveal| {
                    imba::command::Verb::once(OpenCanvasFromChanges {
                        origin: id,
                        changes,
                        source,
                        reveal,
                    })
                });
            let view = changesview::changes_view::ChangesView::open(
                store,
                ui,
                changes,
                changesview::changes_view::ViewSets::History,
                opener,
            );
            let view = changesview::hichanges::Changes::mint_view(store, changes, view);
            Some(himark::mounts::modal_mount(id, changesview::changes_view::ChangesPane::new(changes, view)))
        });
    }
    if let Some(place) = place.place::<crate::places::FolderPickerPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let pick =
                crate::higent::folder_grant::folder_pick(store, key.host, key.session.clone());
            let view = fx.scope(
                move |command: crate::higent::folder_picker::FolderPickerCommand| {
                    AppCommand::ToView(id, imba::dyn_view::DynCommand::new(command))
                },
                |fx| {
                    // The island's glass is the sheet — the picker
                    // keeps only its hairlines.
                    crate::higent::folder_picker::FolderPicker::open(store, ui, pick, fx)
                        .unfloored()
                },
            );
            Some(himark::mounts::modal_mount(id, view))
        });
    }
    if let Some(place) = place.place::<crate::places::SearchPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, _fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let lists = state.lists();
            let folders = ahp_session::session::folders::session_folders(store, &key);
            ::locations::LocationLists::adopt_folders(store, lists, &folders);
            let view = ::locations::search::SearchView::open(store, ui, lists);
            Some(himark::mounts::modal_mount(id, view))
        });
    }
    if let Some(place) = place.place::<crate::places::CommentsPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            crate::sessions::remember_session(store, id, key.clone());
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let comments = state.comments();
            let wire = state.comments_wire();
            let folders = ahp_session::session::folders::session_folders(store, &key);
            fx.scope(himark::app::AppCommand::Verb, |fx| {
                for folder in folders {
                    ahp_comments::ensure(store, wire, &folder, fx);
                }
            });
            let view = ::comments::panel::CommentsView::open(store, ui, comments);
            Some(himark::mounts::modal_mount(id, view))
        });
    }
    if let Some(place) = place.place::<::canvas::diff_canvas::CanvasPlace>() {
        let place = place.clone();
        return app.mount(move |store, ui, id, fx| {
            // The canvas mints through ITS navigator — the same
            // road a workbench pane rides.
            let panel = fx.scope(himark::app::AppCommand::Verb, |fx| {
                hikit::navigation::Navigator::navigate(
                    &::canvas::faces::CanvasNavigator,
                    store,
                    ui,
                    &place,
                    fx,
                )
            })?;
            Some(himark::mounts::panel_mount(id, panel))
        });
    }
    if let Some(place) = place.place::<crate::places::FilesPlace>() {
        let key = place.0.clone();
        return app.mount(move |store, ui, id, fx| {
            crate::sessions::remember_session(store, id, key.clone());
            // A tree born while a follow STANDS replays it — the
            // active editor is revealed however late the tree shows.
            if let Some((location, generation)) = crate::sessions::remember_files_tree(store, key.clone(), id) {
                if std::env::var_os("HIMARK_TRACE_NAV").is_some() {
                    eprintln!(
                        "[follow] tree {} mounts with standing follow {location:?} gen {generation}",
                        id.raw()
                    );
                }
                fx.follow_up(AppCommand::ToView(
                    id,
                    DynCommand::new(filetree::TreeCommand::Follow {
                        location,
                        generation,
                    }),
                ));
            }
            let state = ahp_session::session::state::Hosts::ensure_state(store, &key);
            let tree = fx.scope(
                move |command: filetree::TreeCommand| {
                    AppCommand::ToView(id, DynCommand::new(command))
                },
                |fx| crate::hifiles::open_panel(store, ui, key.clone(), state.trees(), None, fx),
            );
            Some(himark::mounts::modal_mount(id, tree))
        });
    }
    if let Some(place) = place.place::<himark::places::OpenDocumentPlace>() {
        let document = place.document;
        let target = place.target.clone();
        return app.mount(move |store, ui, id, fx| {
            let target = target.clone();
            // Mounted places carry no window to ask for a session: a
            // registered document lives in exactly one collection —
            // find it across the (few) session states.
            let collection = ahp_session::session::state::Hosts::states(store)
                .into_iter()
                .map(|state| state.documents())
                .collect::<Vec<_>>()
                .into_iter()
                .find(|collection| {
                    ::documents::OpenDocuments::document(store, *collection, document).is_some()
                })?;
            let mut held = ::documents::OpenDocuments::document(store, collection, document)?;
            let width = himark::mounts::fallback_editor_width(store);
            let editor = fx.scope(
                move |command: ::editor::editor_view::EditorCommand| {
                    AppCommand::Verb(imba::command::Verb::at(
                        collection,
                        ::documents::DocumentsCommand::Editor(document, command),
                    ))
                },
                // The TARGET rides into the mount, exactly as the
                // windowed road's `show_document` passes it — a
                // goto-definition landed at the top of the file
                // while this said `None`.
                |fx| ::documents::lifecycle::mount_editor(store, ui, &mut held, width, target, fx),
            );
            ::documents::scroll_stripes::enable_scroll_stripes(
                store, collection, document, &mut held, editor,
            );
            ::documents::OpenDocuments::put_document(store, collection, document, held);
            ::documents::OpenDocuments::touch(store, collection, document);
            // Remembered to its session: the palette's command walk
            // and the dispatch fork's session binding reach this
            // pane — comments.add, save-all and friends work from
            // the document pane, not just the chat's.
            if let Some(key) =
                ahp_session::session::state::Hosts::owner_key_of_documents(store, collection)
            {
                crate::sessions::remember_session(store, id, key);
            }
            // BARE — no imba scroller: the editor screen rides the
            // stage road, and the native scroller owns the physics
            // (docs/ui/imba-ffi.md, touch section).
            let view = ::documents::entity_view::EditorIdView::new(collection, document, editor)
                .with_gutter();
            Some(himark::mounts::editor_mount(id, view))
        });
    }
    if let Some(place) = place.place::<himark::places::DiffViewPlace>() {
        let documents = place.documents;
        let view = place.view;
        return app.mount(move |store, ui, id, fx| {
            if let Some(key) =
                ahp_session::session::state::Hosts::owner_key_of_documents(store, documents)
            {
                crate::sessions::remember_session(store, id, key);
            }
            let mut panel = ::canvas::diff_pane::DiffPanelView::over(documents, view);
            // One file, one column: the phone shows diffs INLINE —
            // the pane's own layout road, persisted in the view
            // state (a no-op when already inline).
            fx.scope(
                move |command: ::editor::unified_diff::UnifiedDiffCommand| {
                    AppCommand::ToView(id, DynCommand::new(command))
                },
                |fx| {
                    imba::View::perform(
                        &mut panel,
                        store,
                        ui,
                        ::editor::unified_diff::UnifiedDiffCommand::SetLayout(
                            ::editor::unified_diff::DiffLayout::Inline,
                        ),
                        fx,
                    )
                },
            );
            Some(himark::mounts::panel_mount(id, Box::new(panel)))
        });
    }
    if let Some(place) = place.place::<crate::places::PalettePlace>() {
        // The command walk runs BEFORE the mount, with the app in
        // hand: the session's mounted views answer what they offer,
        // and the global registry rides on top bound to the palette
        // itself — a windowed command landing on a mount forks to
        // its windowless road.
        let make = app.palette_factory()?;
        let session = place.0.clone();
        let mut commands = crate::engine::session_commands(app, &session);
        return app.mount(move |store, ui, id, _fx| {
            // The mount's facts, bound at mint: the origin bridge
            // and the session this palette was opened over — the
            // registry's session commands work from the native
            // palette exactly as from a window's.
            let mut bound = imba::focus::Bindings::new().bind(id);
            if let Some(state) = ahp_session::session::state::Hosts::state(store, &session).cloned()
            {
                bound = bound.bind(state);
            }
            bound = bound.bind(session.clone());
            commands.extend(himark::commands::Commands::of(store).iter().map(|command| {
                imba::PresentableCommand::new(
                    command.id(),
                    command.name(),
                    himark::app::AppCommand::Bound(bound.clone(), command.clone()),
                )
            }));
            Some(himark::mounts::dyn_modal_mount(id, make(store, ui, commands)))
        });
    }
    None
}

/// The session road of `mount_place`: mint, then remember the
/// ORIGIN's session on the new bridge (ready places carry no
/// session key of their own — the place handle rode it).
pub fn mount_place(
    app: &mut Himark,
    place: &hikit::navigation::NavigationLocation,
    session: Option<ahp_wire::SessionId>,
) -> Option<ViewBridgeId> {
    let minted = himark::mounts::mount_place(app, place)?;
    if let Some(session) = session {
        app.with_store(|store| crate::sessions::remember_session(store, minted, session));
    }
    Some(minted)
}

/// The mounted changes tree's canvas ask: a ONCE verb, because the
/// modal-request channel refuses Shell verbs (that guard predates
/// the windowless fork) — this one performs the windowless open
/// directly: commit changeset ask, then the CanvasPlace to the
/// seated shell.
struct OpenCanvasFromChanges {
    origin: ViewBridgeId,
    changes: imba::store::Id<changesview::hichanges::Changes>,
    source: changesview::hichanges::CanvasSource,
    reveal: Option<editor::location::ResourceLocation>,
}

impl imba::command::DynamicOnceCommand for OpenCanvasFromChanges {
    fn perform(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut imba::command::Fx<'_>) {
        if let changesview::hichanges::CanvasSource::Commit { folder, id } = &self.source {
            if let Some(history) =
                changesview::hichanges::Changes::of(store, self.changes).map(|held| held.history())
            {
                changesview::hihistory::History::ask(
                    store,
                    history,
                    changesview::hihistory::HistoryAsk::CommitFiles(folder.clone(), id.clone()),
                );
            }
        }
        himark::shell::navigate_origin(
            store,
            ui,
            self.origin,
            ::canvas::diff_canvas::CanvasPlace {
                changes: self.changes,
                source: self.source,
                reveal: self.reveal,
            },
            fx,
        );
    }
}


/// The changes tree asked to open a row: find THAT ROW'S pair in
/// the set's listing (the same normalization the canvas rows use)
/// and launch the pair-open with the MOUNTED origin — the landing
/// installs the view and hands the shell a ready `DiffViewPlace`.
struct OpenDiffFromChanges {
    origin: ViewBridgeId,
    session: ahp_wire::SessionId,
    changes: imba::store::Id<changesview::hichanges::ChangeSets>,
    source: changesview::hichanges::CanvasSource,
    reveal: Option<editor::location::ResourceLocation>,
}

impl imba::command::DynamicOnceCommand for OpenDiffFromChanges {
    fn perform(self: Box<Self>, store: &mut Store, ui: &UiCtx, fx: &mut imba::command::Fx<'_>) {
        let Some(reveal) = self.reveal else {
            eprintln!("[mount] changes open: no reveal (a folder row?)");
            return;
        };
        let (_, listing) = ::canvas::diff_canvas::canvas_files(store, self.changes, &self.source);
        let ::canvas::diff_canvas::CanvasListing::Ready(files) = listing else {
            eprintln!("[mount] changes open: listing not ready");
            return;
        };
        let Some(file) = files.into_iter().find(|file| file.new == reveal) else {
            eprintln!("[mount] a changes row vanished before its diff opened — dropped");
            return;
        };
        // The seated shell owns the diff's opening: hand it the
        // place (MountFrontend resolves the sides against the
        // origin's session and launches the same effect this arm
        // used to push by hand).
        himark::shell::navigate_origin(
            store,
            ui,
            self.origin,
            himark::places::DiffPlace {
                old: file.old,
                new: file.new,
            },
            fx,
        );
    }
}

