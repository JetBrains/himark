// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The COLUMN HEADER: every column's top band — a title, its quiet
//! subtitle, a row of tabs, the trailing text buttons — all words on
//! ONE baseline over the ground, a hairline under. There are no
//! icons and no fills: rank is size and weight (docs/Design.md,
//! "the headers share ONE baseline"). A column names its header
//! by its items; the band's height, baseline, gaps and inks come
//! from the toolbar chrome.

use imba::{
    arena::Arena,
    constraints::Constraints,
    layout::{Layout, LayoutExt as _},
    store::Store,
    ui::UiCtx,
};
use skia_safe::Size;

use crate::ui::{text, Hairline, TextStyle};

/// One word on the band: inert, or a press minting a command. A
/// button keeps a RESERVE after its word — a tab's slot is as wide
/// as the word in the title rank, so standing or dim it takes the
/// same room and its neighbours never shift.
enum Item<'a, Command> {
    Text(imba::layout::Text),
    Button(imba::layout::Text, f32, Box<dyn Fn() -> Command + 'a>),
}

/// The header's RANKS: the title (the file, the session, the active
/// tab), its subtitle, the display action (Sessions), the quiet
/// buttons and the dimmed tabs.
#[derive(Clone)]
pub struct HeaderStyles {
    pub title: TextStyle,
    pub subtitle: TextStyle,
    pub display: TextStyle,
    pub button: TextStyle,
    pub dim: TextStyle,
}

impl HeaderStyles {
    /// A tab's SLOT: the word's width in whichever rank is wider —
    /// the title rank's, standing or not — so a tab's size never
    /// follows its selection.
    pub fn tab_width(&self, label: &str) -> f32 {
        let title = self.title.shaper.advance(&self.title.font, label);
        let dim = self.dim.shaper.advance(&self.dim.font, label);
        title.max(dim)
    }

    pub fn of(store: &Store, ui: &UiCtx) -> Self {
        let chrome = editor::env::Themes::of(store).part(crate::chrome::ToolbarChrome::KEY);
        Self {
            title: crate::ui::title(store, ui).colored(chrome.title_color.0),
            subtitle: crate::ui::subtitle(store, ui),
            display: crate::ui::display(store, ui),
            button: crate::ui::subtitle(store, ui).colored(chrome.button_text_color.0),
            dim: crate::ui::subtitle(store, ui).colored(chrome.dim_text_color.0),
        }
    }
}

pub struct ColumnHeader<'a, Command> {
    chrome: std::sync::Arc<crate::chrome::ToolbarChrome>,
    ground: skia_safe::Color,
    styles: HeaderStyles,
    /// The first word's x — the column's text edge.
    lead: f32,
    /// (gap before, item): the leading group, left to right.
    leading: Vec<(f32, Item<'a, Command>)>,
    /// The trailing group, left to right, ending `trailing_inset`
    /// short of the right edge.
    trailing: Vec<(f32, Item<'a, Command>)>,
}

impl<'a, Command: 'a> ColumnHeader<'a, Command> {
    pub fn new(arena: &'a Arena, store: &Store, ui: &UiCtx, lead: f32) -> Self {
        let theme = editor::env::Themes::of(store);
        let _ = arena;
        Self {
            chrome: theme.part(crate::chrome::ToolbarChrome::KEY),
            ground: theme.part(::editor::theme::InkChrome::KEY).ground.0,
            styles: HeaderStyles::of(store, ui),
            lead,
            leading: Vec::new(),
            trailing: Vec::new(),
        }
    }

    pub fn styles(&self) -> &HeaderStyles {
        &self.styles
    }

    fn push(items: &mut Vec<(f32, Item<'a, Command>)>, gap: f32, item: Item<'a, Command>) {
        let gap = match items.is_empty() {
            true => 0.0,
            false => gap,
        };
        items.push((gap, item));
    }

    /// The title — the column's name. After tabs it stands the tabs'
    /// title gap off them; first, it leads.
    pub fn title(mut self, label: impl Into<String>) -> Self {
        let item = Item::Text(text(&self.styles.title, label));
        Self::push(&mut self.leading, self.chrome.tabs_title_gap, item);
        self
    }

    /// The subtitle, `subtitle_gap` after the title.
    pub fn subtitle(mut self, label: impl Into<String>) -> Self {
        let item = Item::Text(text(&self.styles.subtitle, label));
        Self::push(&mut self.leading, self.chrome.subtitle_gap, item);
        self
    }

    /// A tab: the active one stands in the title rank, the rest in
    /// the dim one; a press mints its command.
    pub fn tab(
        mut self,
        label: impl Into<String>,
        active: bool,
        on_press: impl Fn() -> Command + 'a,
    ) -> Self {
        let label = label.into();
        let style = match active {
            true => &self.styles.title,
            false => &self.styles.dim,
        };
        let slot = self.styles.tab_width(&label);
        let reserve = (slot - style.shaper.advance(&style.font, &label)).max(0.0);
        let item = Item::Button(text(style, label), reserve, Box::new(on_press));
        Self::push(&mut self.leading, self.chrome.tab_gap, item);
        self
    }

    /// A trailing TAB: the title rank while it stands, the quiet
    /// rank otherwise, in a slot the title rank sizes.
    pub fn trailing_tab(
        mut self,
        label: impl Into<String>,
        active: bool,
        on_press: impl Fn() -> Command + 'a,
    ) -> Self {
        let label = label.into();
        let style = match active {
            true => &self.styles.title,
            false => &self.styles.button,
        };
        let slot = self.styles.tab_width(&label);
        let reserve = (slot - style.shaper.advance(&style.font, &label)).max(0.0);
        let item = Item::Button(text(style, label), reserve, Box::new(on_press));
        Self::push(&mut self.trailing, self.chrome.button_text_gap, item);
        self
    }

    /// A trailing text button in the quiet rank.
    pub fn button(mut self, label: impl Into<String>, on_press: impl Fn() -> Command + 'a) -> Self {
        let item = Item::Button(text(&self.styles.button, label), 0.0, Box::new(on_press));
        Self::push(&mut self.trailing, self.chrome.button_text_gap, item);
        self
    }

    /// A trailing text button in the dim rank (a closed side column's
    /// tabs, the quiet toggles).
    pub fn dim_button(
        mut self,
        label: impl Into<String>,
        on_press: impl Fn() -> Command + 'a,
    ) -> Self {
        let item = Item::Button(text(&self.styles.dim, label), 0.0, Box::new(on_press));
        Self::push(&mut self.trailing, self.chrome.tab_gap, item);
        self
    }

    /// A trailing text button in the DISPLAY rank — the column's one
    /// big action.
    pub fn display_button(
        mut self,
        label: impl Into<String>,
        on_press: impl Fn() -> Command + 'a,
    ) -> Self {
        let item = Item::Button(text(&self.styles.display, label), 0.0, Box::new(on_press));
        Self::push(&mut self.trailing, self.chrome.button_text_gap, item);
        self
    }

    /// A trailing inert text (a count, a status).
    pub fn trailing_text(mut self, label: impl Into<String>) -> Self {
        let item = Item::Text(text(&self.styles.subtitle, label));
        Self::push(&mut self.trailing, self.chrome.button_text_gap, item);
        self
    }

    /// The band's height — what a column reserves for it.
    pub fn band_height(&self) -> f32 {
        self.chrome.height
    }

    fn group(arena: &'a Arena, items: Vec<(f32, Item<'a, Command>)>) -> imba::layout::Row<'a, Command> {
        let mut row = imba::layout::Row::new(arena);
        for (gap, item) in items {
            let insets = |reserve: f32| imba::layout::Insets {
                left: gap,
                right: reserve,
                ..Default::default()
            };
            row = match item {
                Item::Text(text) => row.child_by_baseline(text.pad_insets(insets(0.0))),
                Item::Button(text, reserve, on_press) => row.child_by_baseline(
                    text.on_click(move || on_press()).pad_insets(insets(reserve)),
                ),
            };
        }
        row
    }
}

impl<'a, Command> imba::layout::LayoutValue for ColumnHeader<'a, Command> {}

impl<'a, Command: 'a> Layout<'a, Command> for ColumnHeader<'a, Command> {
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let ColumnHeader {
            chrome,
            ground,
            styles: _,
            lead,
            leading,
            trailing,
        } = self;
        let width = constraints.max.width.max(1.0);
        let height = chrome.height;
        let room = (width - lead - chrome.trailing_inset).max(1.0);
        // One Row, one baseline group: the leading words, a flexible
        // gap, the trailing words. The gap is MAIN-AXIS only so the
        // group's extent stays the words' (the ListRow rule).
        let words = imba::layout::Row::new(arena)
            .child_by_baseline(Self::group(arena, leading))
            .weighted(1.0, imba::layout::Fill::new().height(0.0))
            .child_by_baseline(Self::group(arena, trailing))
            .layout(
                arena,
                Constraints {
                    min: Size::new(room, 0.0),
                    max: Size::new(room, height),
                },
            );
        // The group's line lands on the chrome's baseline — the same
        // y in every header, whatever each one's largest word.
        let baseline = imba::Thunk::first_baseline(&words).unwrap_or(0.0);
        let mut band = imba::container::container(arena, Size::new(width, height));
        band.place_boxed(lead, chrome.title_baseline - baseline, words);
        // A press on the band's bare ground is NOBODY's: it falls
        // through to the window (the titlebar drag on macOS).
        // The ground paints FIRST, the rule over it, the words last.
        imba::ThunkBox::new(
            arena,
            imba::thunk_ext::ThunkExt::paint_below(
                Hairline::new(chrome.rule.0).bottom().wrap(band),
                move |_arena, canvas, rect| {
                    let mut paint = skia_safe::Paint::default();
                    paint.set_color(ground);
                    canvas.draw_rect(rect, &paint);
                },
            ),
        )
    }
}

/// The width a group of dim tab labels takes in a header — what a
/// neighboring header reserves for them before the dock opens.
pub fn tabs_width(store: &Store, ui: &UiCtx, labels: &[&str]) -> f32 {
    if labels.is_empty() {
        return 0.0;
    }
    let chrome = editor::env::Themes::of(store).part(crate::chrome::ToolbarChrome::KEY);
    let style = HeaderStyles::of(store, ui).dim;
    let words: f32 = labels
        .iter()
        .map(|label| style.shaper.advance(&style.font, label))
        .sum();
    words + chrome.tab_gap * labels.len().saturating_sub(1) as f32 + chrome.trailing_inset
}
