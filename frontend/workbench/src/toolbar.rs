// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The window's TOOLBAR: one static band over the body, in three
//! sections that stand over the body's columns — the chat's
//! (Sessions · Chat, the chat's name), the main area's (the focused
//! file, its directory, Contents) and the side column's (its tabs).
//! A hairline splits section from section at the SAME x the body's
//! splitter stands, from the one derivation (`columns::Columns`).
//! Panels come and go; the band stays. There are no icons: a
//! registered button is a WORD (docs/ui/toolbar.md, docs/Design.md).

use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, Thunk};
use skia_safe::{Canvas, Rect, Size};

use crate::columns::Columns;
use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolbarSide {
    /// The chat section's tabs (Sessions).
    #[default]
    Left,

    /// The side column's tabs.
    Right,

    /// Legacy name — the chat's own word (Chat), the chat section's
    /// toggle.
    Well,
}

#[derive(Clone)]
pub struct ToolbarButton {
    pub command: &'static str,

    /// The word the band shows.
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
    /// A registered word pressed: its command id, for the shell's
    /// run road.
    Word(&'static str),

    /// The STANDING Chat tab pressed: the chat column rolls away.
    RollAwayChat,
}

impl std::fmt::Display for ToolbarCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolbarCommand::Word(id) => write!(out, "toolbar word {id}"),
            ToolbarCommand::RollAwayChat => out.write_str("toolbar roll away chat"),
        }
    }
}

/// Where the chat section's first word starts: past the platform
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

/// The static band: three sections over the body's columns. It
/// keeps the request slot the shells drain.
#[derive(Clone, Default)]
pub struct Toolbar {
    request: hikit::modal::RequestSlot<ToolbarRequest>,
}

/// One section's natural width: its lead, its words with the tab
/// gap between, the trailing inset — what the section keeps when
/// the column under it is gone.
struct Natural {
    lead: f32,
    words: f32,
    count: usize,
}

impl Natural {
    fn new(lead: f32) -> Self {
        Self {
            lead,
            words: 0.0,
            count: 0,
        }
    }

    /// A tab's slot — the title rank's width, whatever rank it
    /// shows in (`HeaderStyles::tab_width`).
    fn tab(&mut self, styles: &hikit::header::HeaderStyles, label: &str) {
        self.words += styles.tab_width(label);
        self.count += 1;
    }

    fn width(&self, chrome: &::editor::theme::ToolbarChrome) -> f32 {
        self.lead
            + self.words
            + chrome.tab_gap * self.count.saturating_sub(1) as f32
            + chrome.trailing_inset
    }
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
        _store: &mut Store,
        _ui: &UiCtx,
        command: ToolbarCommand,
        _fx: &mut imba::effect::Effects<'_, ToolbarCommand>,
    ) {
        match command {
            ToolbarCommand::Word(id) => self.request.file(ToolbarRequest::Command(id)),
            // The window answers this one before it reaches here.
            ToolbarCommand::RollAwayChat => {}
        }
    }

    /// The natural width of the chat section — the band keeps it
    /// while the chat column is gone.
    pub fn chat_section_width(store: &Store, ui: &UiCtx) -> f32 {
        let chrome = ::editor::env::Themes::of(store).ui().toolbar.clone();
        let styles = hikit::header::HeaderStyles::of(store, ui);
        let buttons = ToolbarButtons::of(store);
        let mut natural = Natural::new(leading_inset(store, ui));
        for button in Self::chat_tabs(store, &buttons) {
            natural.tab(&styles, button.label);
        }
        natural.width(&chrome)
    }

    /// The natural width of the side column's section — the band
    /// keeps it while the dock is closed.
    pub fn side_section_width(store: &Store, ui: &UiCtx) -> f32 {
        let theme = ::editor::env::Themes::of(store);
        let chrome = theme.ui().toolbar.clone();
        let styles = hikit::header::HeaderStyles::of(store, ui);
        let mut natural = Natural::new(theme.ui().window.side_text_edge);
        for button in ToolbarButtons::of(store).tabs() {
            natural.tab(&styles, button.label);
        }
        natural.width(&chrome)
    }

    /// The chat section's tabs: the Left words (the drawer's word
    /// is the main section's), then the Well word.
    fn chat_tabs<'b>(store: &Store, buttons: &'b ToolbarButtons) -> Vec<&'b ToolbarButton> {
        let drawer = crate::registry::Registry::of(store)
            .and_then(|registry| registry.drawer_button.as_ref())
            .map(|button| button.command);
        buttons
            .on(ToolbarSide::Left)
            .filter(|button| drawer != Some(button.command))
            .chain(buttons.on(ToolbarSide::Well))
            .collect()
    }

    /// The band over `columns`: the chat section as wide as the
    /// chat's revealed column (never narrower than its words), the
    /// side section as wide as the dock's (likewise), the main
    /// section between; a rule between neighbours.
    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        workbench: &'a Workbench,
        columns: &Columns,
    ) -> impl Thunk<'a, ToolbarCommand> + 'a {
        use imba::layout::Layout as _;
        type Header<'a> = hikit::header::ColumnHeader<'a, ToolbarCommand>;

        let theme = ::editor::env::Themes::of(store);
        let chrome = theme.ui().toolbar.clone();
        let window = theme.ui().window.clone();
        let styles = hikit::header::HeaderStyles::of(store, ui);
        let buttons = ToolbarButtons::of(store);
        let drawer = crate::registry::Registry::of(store)
            .and_then(|registry| registry.drawer_button.as_ref())
            .map(|button| (button.label, button.command));
        let band = Size::new(columns.size.width.max(1.0), columns.band.max(1.0));
        let divider = window.divider_width.max(1.0);
        let rule = window.divider.0;
        let tight = |width: f32| Constraints::tight(Size::new(width.max(1.0), band.height));
        let word = |command: &'static str| move || ToolbarCommand::Word(command);

        // The chat section: Sessions (the drawer over the chat
        // column) and Chat — a TOGGLE: standing, it rolls the column
        // away; otherwise it fronts the chat. The chat's name follows
        // unless the drawer stands over the column.
        let side_open = workbench.side_open();
        let chat_showing = columns.chat.is_some();
        let lead = leading_inset(store, ui);
        let mut natural = Natural::new(lead);
        let mut chat_section = Header::new(arena, store, ui, lead);
        for button in Self::chat_tabs(store, &buttons) {
            let command = button.command;
            let active = match button.side {
                ToolbarSide::Well => chat_showing && !side_open,
                _ => side_open,
            };
            natural.tab(&styles, button.label);
            chat_section = match button.side {
                ToolbarSide::Well => {
                    chat_section.tab(button.label, active, move || match active {
                        true => ToolbarCommand::RollAwayChat,
                        false => ToolbarCommand::Word(command),
                    })
                }
                _ => chat_section.tab(button.label, active, word(command)),
            };
        }
        if let Some(chat) = workbench.chat().filter(|_| chat_showing && !side_open) {
            chat_section = chat_section.title(chat.panel().title(store));
            if let Some(subtitle) = chat.panel().subtitle(store) {
                chat_section = chat_section.subtitle(format!("in {subtitle}"));
            }
        }
        let chat_right = columns.chat_revealed().max(natural.width(&chrome));

        // The side section: every registered tab, the open dock's in
        // the title rank.
        let owner = workbench
            .dock()
            .filter(|dock| dock.target_width() > 0.0)
            .map(|dock| dock.owner());
        let mut natural = Natural::new(window.side_text_edge);
        let mut side_section = Header::new(arena, store, ui, window.side_text_edge);
        for button in buttons.tabs() {
            let active = owner == Some(button.command);
            natural.tab(&styles, button.label);
            side_section = side_section.tab(button.label, active, word(button.command));
        }
        let side_width = columns
            .dock_revealed()
            .max(natural.width(&chrome))
            .min(band.width);
        let side_left = band.width - side_width;

        // The main section between: the focused file, its directory
        // and, when the focused panel offers one, Contents.
        let main_left = chat_right;
        let main_width = side_left - main_left;

        let mut surface = imba::container::container(arena, band);
        surface.place_boxed(0.0, 0.0, chat_section.layout(arena, tight(chat_right)));
        if main_width >= 1.0 {
            let mut main_section = Header::new(arena, store, ui, window.code_text_edge);
            if !workbench.root.is_vacant() {
                let (title, subtitle) = crate::workbench::tree_header_words(workbench, store);
                main_section = main_section.title(title);
                if let Some(subtitle) = subtitle {
                    main_section = main_section.subtitle(subtitle);
                }
                if let Some((label, command)) = drawer {
                    if workbench.root.focused_pane().offers_drawer(store) {
                        main_section = main_section.button(label, word(command));
                    }
                }
            }
            surface.place_boxed(main_left, 0.0, main_section.layout(arena, tight(main_width)));
        }
        surface.place_boxed(side_left, 0.0, side_section.layout(arena, tight(side_width)));
        // The rules between sections stand where the body's
        // splitters do: inside the chat column's last pixels, on the
        // dock's first.
        for x in [chat_right - divider, side_left] {
            surface.place_boxed(
                x,
                0.0,
                hikit::ui::rule_v::<ToolbarCommand>(rule, band.height)
                    .layout(arena, Constraints::tight(Size::new(divider, band.height))),
            );
        }
        surface
    }
}

/// The chat's own word — `chat.composer` (⌘I): front the session's
/// chat. The band's chat section shows it as the Chat tab.
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
