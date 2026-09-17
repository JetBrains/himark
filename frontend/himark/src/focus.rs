// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::event::EventResult;
use imba::focus::FocusData;
use imba::{ClipboardClient, ImeClient};

use crate::app::AppCommand;
use crate::Application;

/// The semantic focus chain of a window — a STATE WALK over the view
/// tree. No arena, no layout, no realize: focus is state, and the
/// views carry it (imba `View::focus_data`).
pub(crate) fn window_focus_data<'a>(
    store: &'a imba::store::Store,
    ui: &'a imba::UiCtx,
    window: crate::WindowId,
) -> Option<FocusData<'a, AppCommand>> {
    let entity = crate::Windows::window_ref(store, window)?;
    Some(
        imba::View::focus_data(entity, store, ui)
            .map(move |command| AppCommand::Content(window, command)),
    )
}

impl Application {
    pub fn with_ime_client<R>(
        &mut self,
        window: crate::WindowId,
        f: impl FnOnce(&mut dyn ImeClient) -> R,
    ) -> Option<R> {
        // Two asks, one source of truth: the SEMANTIC walk names the
        // focused seat, and the layout fold answers by RECOGNIZING
        // that key — it never re-decides focus. This is the only
        // remaining build-to-ask, and it fires only while composing.
        let size = self.window_viewport(window)?;
        let store = self.frame_store(window);
        let target = {
            let mut data = window_focus_data(&store, self.ui.as_ref(), window)?;
            data.seat.take()?
        };
        let mut arena = std::mem::take(&mut self.ui_arena);
        arena.reset();
        let mut f = Some(f);
        let mut answer = None;
        let performed = {
            let widget = self.layout(
                window,
                &arena,
                &store,
                self.ui.as_ref(),
                imba::constraints::Constraints::tight(size),
            );
            let mut widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_size(size));
            let performed = imba::Widget::layout_data(&mut widget, target)
                .ime
                .take()
                .map(|mut seat| {
                    if std::env::var("HIMARK_TRACE_IME").is_ok() {
                        eprintln!("[ime] seat origin = {:?}", seat.origin);
                    }
                    (seat.ask)(seat.origin, seat.clip, &mut |client| {
                        if let Some(f) = f.take() {
                            answer = Some(f(client));
                        }
                    })
                });
            drop(widget);
            performed
        };
        self.ui_arena = arena;

        if let Some(result) = performed {
            self.perform_chain_result(result);
        }
        answer
    }

    pub fn with_clipboard_client<R>(
        &mut self,
        window: crate::WindowId,
        f: impl FnOnce(&mut dyn ClipboardClient) -> R,
    ) -> Option<R> {
        let store = self.window_store(window);
        let ui = self.ui.clone();
        let mut f = Some(f);
        let mut answer = None;
        let performed = {
            let mut data = window_focus_data(&store, ui.as_ref(), window)?;
            data.clipboard.as_mut().map(|seat| {
                seat(&mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        };
        if let Some(result) = performed {
            self.perform_chain_result(result);
        }
        answer
    }

    fn perform_chain_result(&mut self, result: EventResult<AppCommand>) {
        match result {
            EventResult::Command(command) => {
                self.perform_batch(vec![command]);
            }
            EventResult::Commands(commands) => {
                self.perform_batch(commands);
            }
            EventResult::Ignored | EventResult::Handled | EventResult::Reveal(_) => {}
        }
    }
}

/// The focused location out of an already-collected focus chain.
pub(crate) fn focused_location(
    data: &mut FocusData<'_, AppCommand>,
) -> Option<crate::ResourceLocation> {
    data.location
        .take()
        .and_then(|location| location.downcast_ref::<crate::ResourceLocation>().cloned())
}
