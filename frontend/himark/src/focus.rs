// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::event::EventResult;
use imba::focus::FocusData;
use imba::{clipboard::ClipboardClient, ime::ImeClient};

use crate::app::AppCommand;
use crate::app::Himark;

/// The mounted twin: the bridge's own walk, with the origin and its
/// session bound at the root — what `window_focus_data` does for a
/// window.
pub(crate) fn bind_mounted<'a>(
    store: &imba::store::Store,
    id: crate::bridge::ViewBridgeId,
    data: FocusData<'a, AppCommand>,
) -> FocusData<'a, AppCommand> {
    // The origin rides as the NON-SIGNAL fact on every road; the
    // bare `ViewBridgeId` stays the MOUNTED signal (commands fork
    // their windowless half on it).
    let mut data = data.bind(crate::commands::BoundOrigin(id));
    if id.is_mounted() {
        data = data.bind(id);
    }
    data.bindings =
        std::mem::take(&mut data.bindings).merge_under(crate::perform::mount_bindings(store, id));
    data
}

impl Himark {
    pub fn with_ime_client<R>(
        &mut self,
        id: crate::bridge::ViewBridgeId,
        f: impl FnOnce(&mut dyn ImeClient) -> R,
    ) -> Option<R> {
        // Two asks, one source of truth: the SEMANTIC walk names the
        // focused client, and the layout fold answers by RECOGNIZING
        // that key — it never re-decides focus. The fold runs over
        // the bridge's standing frame (a window rides its bridge
        // slot like any mount); the view walks ITSELF for the seat
        // (`WorkbenchView::focus` is the window's walk). A window's
        // live viewport keys the rebuild; a mount folds at the LAST
        // DRAWN size — rebuilt first if a batch quiesced it, or an
        // ask landing between draws answers "no focus" and the host
        // keyboard drops.
        let ui = self.ui.clone();
        let mut f = Some(f);
        let mut answer = None;
        let performed = {
            let live = self
                .bridges
                .get(&id)
                .and_then(|slot| slot.bridge.view().live_viewport(&self.store));
            self.mounted_frame(id, live, false)?;
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
            self.perform_chain_result(id, result);
        }
        answer
    }

    pub fn with_clipboard_client<R>(
        &mut self,
        id: crate::bridge::ViewBridgeId,
        f: impl FnOnce(&mut dyn ClipboardClient) -> R,
    ) -> Option<R> {
        let store = self.store.clone();
        let ui = self.ui.clone();
        let mut f = Some(f);
        let mut answer = None;
        let performed = {
            let mut data = self
                .bridges
                .get(&id)
                .and_then(|slot| slot.bridge.view().focus(&store, ui.as_ref()))?;
            data.clipboard.as_mut().map(|client| {
                client(&mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        };
        if let Some(result) = performed {
            self.perform_chain_result(id, result);
        }
        answer
    }

    fn perform_chain_result(
        &mut self,
        id: crate::bridge::ViewBridgeId,
        result: EventResult<AppCommand>,
    ) {
        let EventResult::Handled(handled) = result else {
            return;
        };
        if let Some(reveal) = handled.reveal {
            // Typing on a mounted road asks a caret-follow the
            // engine can't scroll — park it for the host.
            if id.is_mounted() {
                self.push_reveal(id, reveal);
            }
        }
        if !handled.commands.is_empty() {
            self.perform_batch(handled.commands);
        }
    }
}

/// The focused location out of an already-collected focus chain.
pub fn focused_location(
    data: &mut FocusData<'_, AppCommand>,
) -> Option<editor::location::ResourceLocation> {
    data.location.take().and_then(|location| {
        location
            .downcast_ref::<editor::location::ResourceLocation>()
            .cloned()
    })
}
