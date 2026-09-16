// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use crate::clipboard::ClipboardClient;
use crate::event::{EventResult, Key, Modifiers};
use crate::ime::ImeClient;
use crate::PresentableCommand;

pub type KeyHandler<'w, Command> = Box<dyn FnMut(Key, Modifiers) -> EventResult<Command> + 'w>;
pub type TextHandler<'w, Command> = Box<dyn FnMut(&str) -> EventResult<Command> + 'w>;

pub type ClipboardSeat<'w, Command> =
    Box<dyn FnMut(&mut dyn FnMut(&mut dyn ClipboardClient)) -> EventResult<Command> + 'w>;

pub struct ImeSeat<'w, Command> {
    pub origin: skia_safe::Point,
    pub clip: Option<skia_safe::Rect>,
    #[allow(clippy::type_complexity)]
    pub ask: Box<
        dyn FnMut(
                skia_safe::Point,
                Option<skia_safe::Rect>,
                &mut dyn FnMut(&mut dyn ImeClient),
            ) -> EventResult<Command>
            + 'w,
    >,
}

pub struct FocusData<'w, Command> {
    pub commands: Vec<PresentableCommand<Command>>,

    pub on_key: Option<KeyHandler<'w, Command>>,

    pub on_text: Option<TextHandler<'w, Command>>,

    pub ime: Option<ImeSeat<'w, Command>>,

    pub clipboard: Option<ClipboardSeat<'w, Command>>,

    pub location: Option<Box<dyn std::any::Any>>,
}

impl<Command> Default for FocusData<'_, Command> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            on_key: None,
            on_text: None,
            ime: None,
            clipboard: None,
            location: None,
        }
    }
}

impl<'w, Command> FocusData<'w, Command> {
    pub fn of_commands(commands: Vec<PresentableCommand<Command>>) -> Self {
        Self {
            commands,
            ..Self::default()
        }
    }

    pub fn key(&mut self, key: Key, mods: Modifiers) -> EventResult<Command> {
        match &mut self.on_key {
            Some(handler) => handler(key, mods),
            None => EventResult::Ignored,
        }
    }

    pub fn text(&mut self, text: &str) -> EventResult<Command> {
        match &mut self.on_text {
            Some(handler) => handler(text),
            None => EventResult::Ignored,
        }
    }

    pub fn translated(mut self, dx: f32, dy: f32) -> Self {
        if let Some(seat) = &mut self.ime {
            seat.origin.x += dx;
            seat.origin.y += dy;
            if let Some(clip) = &mut seat.clip {
                clip.offset((dx, dy));
            }
        }
        self
    }

    pub fn clipped(mut self, rect: skia_safe::Rect) -> Self {
        if let Some(seat) = &mut self.ime {
            seat.clip = Some(match seat.clip {
                Some(mut inner) => match inner.intersect(rect) {
                    true => inner,
                    false => skia_safe::Rect::new_empty(),
                },
                None => rect,
            });
        }
        self
    }

    pub fn map<Parent>(self, map: impl Fn(Command) -> Parent + Clone + 'w) -> FocusData<'w, Parent>
    where
        Command: 'w,
    {
        FocusData {
            commands: self
                .commands
                .into_iter()
                .map(|presentable| presentable.map(&map))
                .collect(),
            on_key: self.on_key.map(|mut handler| -> KeyHandler<'w, Parent> {
                let map = map.clone();
                Box::new(move |key, mods| handler(key, mods).map(&map))
            }),
            on_text: self.on_text.map(|mut handler| -> TextHandler<'w, Parent> {
                let map = map.clone();
                Box::new(move |text| handler(text).map(&map))
            }),
            ime: self.ime.map(|seat| {
                let map = map.clone();
                let ImeSeat {
                    origin,
                    clip,
                    mut ask,
                } = seat;
                ImeSeat {
                    origin,
                    clip,
                    ask: Box::new(move |origin, clip, visit| ask(origin, clip, visit).map(&map)),
                }
            }),
            clipboard: self.clipboard.map(|mut seat| -> ClipboardSeat<'w, Parent> {
                let map = map.clone();
                Box::new(move |visit| seat(visit).map(&map))
            }),
            location: self.location,
        }
    }

    pub fn merge_under(mut self, outer: FocusData<'w, Command>) -> FocusData<'w, Command>
    where
        Command: 'w,
    {
        self.commands.extend(outer.commands);
        FocusData {
            commands: self.commands,
            on_key: fallback(self.on_key, outer.on_key, |mut inner, mut outer| {
                Box::new(move |key, mods| match inner(key, mods) {
                    EventResult::Ignored => outer(key, mods),
                    result => result,
                })
            }),
            on_text: fallback(self.on_text, outer.on_text, |mut inner, mut outer| {
                Box::new(move |text| match inner(text) {
                    EventResult::Ignored => outer(text),
                    result => result,
                })
            }),
            ime: self.ime.or(outer.ime),
            clipboard: self.clipboard.or(outer.clipboard),
            location: self.location.or(outer.location),
        }
    }

    pub fn merge_over(self, below: FocusData<'w, Command>) -> FocusData<'w, Command>
    where
        Command: 'w,
    {
        self.merge_under(below)
    }
}

fn fallback<H>(inner: Option<H>, outer: Option<H>, compose: impl FnOnce(H, H) -> H) -> Option<H> {
    match (inner, outer) {
        (Some(inner), Some(outer)) => Some(compose(inner, outer)),
        (one, other) => one.or(other),
    }
}

pub fn frame_commands<V: crate::View>(
    view: &V,
    store: &crate::store::Store,
    ui: &crate::ui::UiCtx,
    size: skia_safe::Size,
) -> Vec<PresentableCommand<V::Command>> {
    let arena = crate::arena::Arena::default();
    let mut widget = crate::Thunk::realize(
        crate::Layout::layout(
            crate::View::display(view, &arena, store, ui),
            &arena,
            crate::constraints::Constraints::tight(size),
        ),
        &arena,
        skia_safe::Rect::from_size(size),
    );
    let commands = std::mem::take(&mut crate::Widget::focus_data(&mut widget).commands);
    drop(widget);
    commands
}

pub fn frame_key<V: crate::View>(
    view: &V,
    store: &crate::store::Store,
    ui: &crate::ui::UiCtx,
    size: skia_safe::Size,
    key: Key,
    mods: Modifiers,
) -> EventResult<V::Command> {
    let arena = crate::arena::Arena::default();
    let mut widget = crate::Thunk::realize(
        crate::Layout::layout(
            crate::View::display(view, &arena, store, ui),
            &arena,
            crate::constraints::Constraints::tight(size),
        ),
        &arena,
        skia_safe::Rect::from_size(size),
    );
    let result = crate::Widget::focus_data(&mut widget).key(key, mods);
    drop(widget);
    result
}
