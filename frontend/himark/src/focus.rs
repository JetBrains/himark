// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::event::EventResult;
use imba::focus::FocusData;
use imba::{clipboard::ClipboardClient, ime::ImeClient};

use crate::app::AppCommand;
use crate::app::Himark;

/// The semantic focus chain of a window — a STATE WALK over the view
/// tree. No arena, no layout, no realize: focus is state, and the
/// views carry it (imba `View::focus_data`).
pub(crate) fn window_focus_data<'a>(
    store: &'a imba::store::Store,
    ui: &'a imba::ui::UiCtx,
    window: ::workbench::window::WindowId,
) -> Option<FocusData<'a, AppCommand>> {
    let entity = ::workbench::window::Windows::window_ref(store, window)?;
    let data = imba::View::focus_data(entity, store, ui)
        .map(move |command| crate::shell::content(window, command));
    // The window's own facts ride the walk's ROOT — the generic
    // bindings commands read instead of a window parameter. Bound
    // last: an inner view that binds its own session shadows these.
    let mut data = data.bind(window);
    data = data.bind(crate::workspace::entity_session(entity));
    if let Some(state) = crate::workspace::session_state(store, window) {
        data = data.bind(state);
    }
    Some(data)
}

/// The mounted twin: the bridge's own walk, with the origin and its
/// session bound at the root — what `window_focus_data` does for a
/// window.
pub(crate) fn bind_mounted<'a>(
    store: &imba::store::Store,
    id: crate::bridge::ViewBridgeId,
    data: FocusData<'a, AppCommand>,
) -> FocusData<'a, AppCommand> {
    let mut data = data.bind(id);
    if let Some(session) = crate::mounts::session_of(store, id) {
        if let Some(state) = ahp_session::session::state::Hosts::state(store, &session).cloned() {
            data = data.bind(state);
        }
        data = data.bind(session);
    }
    data
}

impl Himark {
    pub fn with_ime_client<R>(
        &mut self,
        window: ::workbench::window::WindowId,
        f: impl FnOnce(&mut dyn ImeClient) -> R,
    ) -> Option<R> {
        // Two asks, one source of truth: the SEMANTIC walk names the
        // focused client, and the layout fold answers by RECOGNIZING
        // that key — it never re-decides focus. The fold runs over
        // the bridge's standing frame; it fires only while composing.
        let ui = self.ui.clone();
        let mut f = Some(f);
        let mut answer = None;
        let performed = if self.windows().entity(window).is_some() {
            // The window road keeps its exactness: a fresh frame
            // at the live viewport, the window's own walk.
            let size = self.window_viewport(window)?;
            let frame = self.ensure_frame(window, size);
            let target = {
                let mut data = window_focus_data(frame.store(), ui.as_ref(), window)?;
                data.seat.take()?
            };
            frame.layout_data(target).ime.take().map(|mut client| {
                if std::env::var("HIMARK_TRACE_IME").is_ok() {
                    eprintln!("[ime] client origin = {:?}", client.origin);
                }
                (client.ask)(client.origin, client.clip, &mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        } else {
            // A mounted view folds over its frame at the LAST
            // DRAWN size — rebuilt first if a batch quiesced it,
            // or an ask landing between draws answers "no focus"
            // and the host keyboard drops — and walks ITSELF for
            // the seat.
            let id = crate::bridge::ViewBridgeId::from_raw(window.raw());
            self.mounted_frame(id, None)?;
            let slot = self.bridges.get_mut(&id)?;
            let crate::himark::Mounted { bridge, frame, .. } = slot;
            let frame = frame.as_mut()?;
            let target = {
                let mut data = bridge.view().focus(frame.store(), ui.as_ref())?;
                data.seat.take()?
            };
            frame.layout_data(target).ime.take().map(|mut client| {
                if std::env::var("HIMARK_TRACE_IME").is_ok() {
                    eprintln!("[ime] client origin = {:?}", client.origin);
                }
                (client.ask)(client.origin, client.clip, &mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        };

        if let Some(result) = performed {
            self.perform_chain_result(window, result);
        }
        answer
    }

    pub fn with_clipboard_client<R>(
        &mut self,
        window: ::workbench::window::WindowId,
        f: impl FnOnce(&mut dyn ClipboardClient) -> R,
    ) -> Option<R> {
        let store = self.window_store(window);
        let ui = self.ui.clone();
        let id = crate::bridge::ViewBridgeId::from_raw(window.raw());
        let mut f = Some(f);
        let mut answer = None;
        let performed = {
            let mut data = self
                .bridges
                .get(&id)
                .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))
                .or_else(|| window_focus_data(&store, ui.as_ref(), window))?;
            data.clipboard.as_mut().map(|client| {
                client(&mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        };
        if let Some(result) = performed {
            self.perform_chain_result(window, result);
        }
        answer
    }

    fn perform_chain_result(
        &mut self,
        window: ::workbench::window::WindowId,
        result: EventResult<AppCommand>,
    ) {
        let EventResult::Handled(handled) = result else {
            return;
        };
        if let Some(reveal) = handled.reveal {
            // Typing on a mounted road asks a caret-follow the
            // engine can't scroll — park it for the host.
            let id = crate::bridge::ViewBridgeId::from_raw(window.raw());
            if id.is_mounted() {
                self.park_reveal(id, reveal);
            }
        }
        if !handled.commands.is_empty() {
            self.perform_batch(handled.commands);
        }
    }
}

/// The focused location out of an already-collected focus chain.
pub(crate) fn focused_location(
    data: &mut FocusData<'_, AppCommand>,
) -> Option<editor::location::ResourceLocation> {
    data.location.take().and_then(|location| {
        location
            .downcast_ref::<editor::location::ResourceLocation>()
            .cloned()
    })
}
