// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The window's registered BUTTONS, and where their words land. The
//! redesign has no toolbar band and no icons: a registered button is
//! a WORD a column header shows at its trailing edge — the chat
//! column carries the session actions (Sessions), the tree column
//! its file's (Contents) and, while the side column is closed, the
//! side column's tabs; the dock's header carries those tabs itself
//! (docs/ui/toolbar.md, docs/Design.md).

use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, Thunk};
use skia_safe::{Canvas, Rect, Size};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolbarSide {
    /// The chat column's trailing words.
    #[default]
    Left,

    /// The side column's tabs.
    Right,

    /// Legacy name — the chat's own word, shown by the tree column
    /// while the chat is hidden.
    Well,
}

#[derive(Clone)]
pub struct ToolbarButton {
    pub command: &'static str,

    /// The word the header shows.
    pub label: &'static str,

    pub order: f32,

    pub side: ToolbarSide,

    /// A glyph for shells that still draw icons (none of ours).
    pub glyph: std::sync::Arc<dyn Fn(&Canvas, Rect, skia_safe::Color) + Send + Sync>,
}

#[derive(Clone, Default)]
pub struct ToolbarButtons(pub(crate) Vec<ToolbarButton>);

impl ToolbarButtons {
    pub fn of(store: &Store) -> ToolbarButtons {
        crate::registry::Registry::of(store)
            .map(|registry| registry.toolbar_buttons.clone())
            .unwrap_or_default()
    }

    pub fn register(store: &mut Store, button: ToolbarButton) {
        crate::registry::Registry::update(store, |registry| {
            registry.toolbar_buttons.0.push(button);
            registry
                .toolbar_buttons
                .0
                .sort_by(|a, b| a.order.total_cmp(&b.order));
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolbarButton> {
        self.0.iter()
    }

    /// The buttons of one side, in order.
    pub fn on(&self, side: ToolbarSide) -> impl Iterator<Item = &ToolbarButton> {
        self.0.iter().filter(move |button| button.side == side)
    }

    /// The side column's TABS — their words, in order.
    pub fn tabs(&self) -> Vec<&ToolbarButton> {
        self.on(ToolbarSide::Right).collect()
    }
}

#[derive(Clone)]
pub enum ToolbarRequest {
    Command(&'static str),
}

#[derive(Clone)]
pub enum ToolbarCommand {
    Button(usize),
}

impl std::fmt::Display for ToolbarCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolbarCommand::Button(_) => out.write_str("toolbar button"),
        }
    }
}

/// Where the LEFTMOST column's title starts: past the platform
/// semaphore (the clearance the shell wrote) and never short of the
/// chrome's own lead inset.
pub(crate) fn leading_inset(store: &Store, ui: &UiCtx) -> f32 {
    let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
    let clearance = ui
        .get::<crate::toolbar::ChromeClearance>()
        .map(|clearance| clearance.0)
        .unwrap_or(0.0);
    chrome.lead_inset.max(clearance)
}

/// The window-level toolbar: no band of its own any more — the
/// headers show the words. It keeps the request slot the shells
/// drain, and lays an empty strip so the window frame's layers keep
/// their shape.
#[derive(Clone, Default)]
pub struct Toolbar {
    request: hikit::modal::RequestSlot<ToolbarRequest>,
}

impl Toolbar {
    pub(crate) fn take_request(&mut self) -> Option<ToolbarRequest> {
        self.request.take()
    }

    pub(crate) fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, ToolbarCommand> {
        imba::focus::FocusData::default()
    }

    pub(crate) fn perform(
        &mut self,
        store: &mut Store,
        _ui: &UiCtx,
        command: ToolbarCommand,
        _fx: &mut imba::effect::Effects<'_, ToolbarCommand>,
    ) {
        match command {
            ToolbarCommand::Button(index) => {
                if let Some(button) = ToolbarButtons::of(store).0.get(index) {
                    self.request.file(ToolbarRequest::Command(button.command));
                }
            }
        }
    }

    /// An EMPTY strip: nothing is drawn and nothing is hit — the
    /// words live in the column headers.
    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        window_width: f32,
    ) -> impl Thunk<'a, ToolbarCommand> + 'a {
        imba::layout::Layout::layout(
            imba::layout::Fill::<ToolbarCommand>::new(),
            arena,
            Constraints::tight(Size::new(window_width.max(1.0), 0.0)),
        )
    }
}

/// The chat's own word — `chat.composer` (⌘I): front the session's
/// chat. The tree column shows it while the chat is hidden.
pub fn composer_button() -> crate::toolbar::ToolbarButton {
    crate::toolbar::ToolbarButton {
        command: "chat.composer",
        label: "Chat",
        order: 0.0,
        side: crate::toolbar::ToolbarSide::Well,
        glyph: std::sync::Arc::new(|_canvas, _rect, _color| {}),
    }
}

/// The host chrome inset the toolbar clears (the macOS traffic
/// lights) — written by the shell at boot and on fullscreen moves.
#[derive(Clone, Copy, Default)]
pub struct ChromeClearance(pub f32);
