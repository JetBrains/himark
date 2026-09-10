use imba::event::EventResult;
use imba::focus::FocusData;
use imba::{ClipboardClient, ImeClient};

use crate::app::AppCommand;
use crate::Application;

impl Application {
    fn with_focus_chain<R>(
        &mut self,
        window: crate::WindowId,
        f: impl FnOnce(&mut FocusData<'_, AppCommand>) -> R,
    ) -> Option<R> {
        let size = self.window_viewport(window)?;
        let store = self.window_store(window);
        let mut arena = std::mem::take(&mut self.ui_arena);
        arena.reset();
        let (result, pending) = {
            let widget = self.layout(
                window,
                &arena,
                &store,
                self.ui.as_ref(),
                imba::constraints::Constraints::tight(size),
            );
            let mut widget = imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_size(size));
            let mut data = imba::Widget::focus_data(&mut widget);
            let result = f(&mut data);

            drop(data);
            (result, ())
        };
        let _ = pending;
        self.ui_arena = arena;
        Some(result)
    }

    pub fn with_ime_client<R>(
        &mut self,
        window: crate::WindowId,
        f: impl FnOnce(&mut dyn ImeClient) -> R,
    ) -> Option<R> {
        let mut f = Some(f);
        let mut answer = None;
        let performed = self.with_focus_chain(window, |data| {
            data.ime.take().map(|mut seat| {
                if std::env::var("HIMARK_TRACE_IME").is_ok() {
                    eprintln!("[ime] seat origin = {:?}", seat.origin);
                }
                (seat.ask)(seat.origin, seat.clip, &mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        })?;

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
        let mut f = Some(f);
        let mut answer = None;
        let performed = self.with_focus_chain(window, |data| {
            data.clipboard.as_mut().map(|seat| {
                seat(&mut |client| {
                    if let Some(f) = f.take() {
                        answer = Some(f(client));
                    }
                })
            })
        })?;
        if let Some(result) = performed {
            self.perform_chain_result(result);
        }
        answer
    }

    pub fn focused_location(&mut self, window: crate::WindowId) -> Option<crate::ResourceLocation> {
        self.with_focus_chain(window, |data| {
            data.location
                .take()
                .and_then(|location| location.downcast_ref::<crate::ResourceLocation>().cloned())
        })
        .flatten()
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
