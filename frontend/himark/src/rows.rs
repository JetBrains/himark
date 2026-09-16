// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    effect::Effects,
    event::{Event, EventResult},
    list::{ListCommand, ListSlice, ListView, SelectionStyle},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    LayoutExt as _, UiCtx, View,
};

#[derive(Clone)]
pub struct LabelRow {
    label: String,

    dim: bool,

    trail: Option<String>,
}

#[derive(Clone, Copy)]
pub enum RowCommand {
    Picked,
}

impl View for LabelRow {
    type Command = RowCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        let style = crate::ui::RowStyle::standard(store, ui);
        let dim = self.dim;
        let mut row = crate::ui::ListRow::new(arena, style.clone()).label_styled(
            match dim {
                true => &style.trail,
                false => &style.label,
            },
            self.label.clone(),
        );
        if let Some(trail) = &self.trail {
            row = row.trail(trail.clone());
        }
        row.on_event(
            move |_arena: &Arena, event: &Event<'_>, _size| match event {
                Event::MouseDown { .. } if !dim => EventResult::Command(RowCommand::Picked),
                Event::MouseDown { .. } => EventResult::Handled,
                _ => EventResult::Ignored,
            },
        )
    }
}

pub type RowListCommand = ScrollCommand<ListCommand<RowCommand>>;

pub fn panel_inset(ui: &::editor::theme::UiTheme) -> f32 {
    ui.peeker.margin * 4.0 / 3.0
}

pub fn paint_panel_chrome(
    canvas: &skia_safe::Canvas,
    rect: skia_safe::Rect,
    ui: &::editor::theme::UiTheme,
    title_font: &skia_safe::Font,
    title: &str,
    context: &str,
) {
    let panel = &ui.panel;
    let inset = panel_inset(ui);
    let rect = rect.with_inset((inset, inset));
    let mut paint = skia_safe::Paint::default();

    let echo = inset * 0.5;
    let sheet = rect.with_offset((echo, echo));
    paint.set_color(ui.peeker.background.0);
    canvas.draw_rect(sheet, &paint);
    paint.set_color(ui.toolbar.rule.0);
    for edge in [
        skia_safe::Rect::from_xywh(sheet.left, sheet.bottom - 1.0, sheet.width(), 1.0),
        skia_safe::Rect::from_xywh(sheet.right - 1.0, sheet.top, 1.0, sheet.height()),
    ] {
        canvas.draw_rect(edge, &paint);
    }
    paint.set_color(ui.peeker.background.0);
    canvas.draw_rect(rect, &paint);
    paint.set_color(panel.header_rule.0);
    canvas.draw_rect(
        skia_safe::Rect::from_xywh(
            rect.left,
            rect.top + panel.header_height - 1.0,
            rect.width(),
            1.0,
        ),
        &paint,
    );
    paint.set_color(ui.peeker.rule.0);
    for edge in [
        skia_safe::Rect::from_xywh(rect.left, rect.top, rect.width(), 1.0),
        skia_safe::Rect::from_xywh(rect.left, rect.bottom - 1.0, rect.width(), 1.0),
        skia_safe::Rect::from_xywh(rect.left, rect.top, 1.0, rect.height()),
        skia_safe::Rect::from_xywh(rect.right - 1.0, rect.top, 1.0, rect.height()),
    ] {
        canvas.draw_rect(edge, &paint);
    }
    paint.set_anti_alias(true);
    paint.set_color(panel.title_color.0);

    let mut x = rect.left + panel.title_x;
    let baseline = rect.top + panel.title_baseline;
    for ch in title.to_uppercase().chars() {
        let glyph = ch.to_string();
        canvas.draw_str(&glyph, (x, baseline), title_font, &paint);
        x += title_font.measure_str(&glyph, None).0 + 1.5;
    }
    if !context.is_empty() {
        let advance = title_font.measure_str(context, None).0;
        canvas.draw_str(
            context,
            (rect.right - advance - panel.title_x, baseline),
            title_font,
            &paint,
        );
    }
}

pub fn selection_style(store: &Store) -> SelectionStyle {
    let ui = crate::env::Themes::of(store).ui().clone();
    SelectionStyle {
        fill: ui.tree.highlight.0,
        accent: ui.peeker.accent.0,
        accent_width: ui.peeker.accent_width,
        accent_inset: 0.0,
        ..SelectionStyle::default()
    }
}

#[derive(Clone)]
pub struct RowList {
    scroll: ScrollView<ListView<LabelRow, usize>>,
    len: usize,
}

impl RowList {
    pub fn new() -> Self {
        Self {
            scroll: ScrollView::new(ListView::empty()),
            len: 0,
        }
    }

    pub fn set(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        labels: &[String],
        note: Option<String>,
        selected: usize,
    ) {
        self.set_with_trails(store, ui, labels, &[], note, selected)
    }

    pub fn set_with_trails(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        labels: &[String],
        trails: &[Option<String>],
        note: Option<String>,
        selected: usize,
    ) {
        self.len = labels.len();
        let selected = selected.min(self.len.saturating_sub(1));
        let mut slice: ListSlice<LabelRow, usize> = ListSlice::new();
        for (index, label) in labels.iter().enumerate() {
            slice.push_keyed(
                index,
                LabelRow {
                    label: label.clone(),
                    dim: false,
                    trail: trails.get(index).cloned().flatten(),
                },
                store,
                ui,
            );
        }
        if let Some(label) = note {
            slice.push(
                LabelRow {
                    label,
                    dim: true,
                    trail: None,
                },
                store,
                ui,
            );
        }
        let scroll_y = self.scroll.scroll_y();
        let mut list = ListView::from_slice(slice).with_selection(selection_style(store));
        if self.len > 0 {
            list.select_only(selected);
        }
        self.scroll = ScrollView::new(list);
        self.scroll.set_scroll_y(scroll_y);
    }

    pub fn select(&mut self, index: usize) {
        if self.len == 0 {
            return;
        }
        self.scroll
            .content_mut()
            .select_only(index.min(self.len - 1));
    }

    pub fn selected(&self) -> usize {
        self.scroll.content().cursor().copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn scroll_y(&self) -> f32 {
        self.scroll.scroll_y()
    }

    pub fn picked(&self, command: &RowListCommand) -> Option<usize> {
        let ScrollCommand::Content(command) = command else {
            return None;
        };
        let index = match command {
            ListCommand::Child(index, RowCommand::Picked) => *index,
            ListCommand::Focus(index, Some(then)) => match then.as_ref() {
                ListCommand::Child(_, RowCommand::Picked) => *index,
                _ => return None,
            },
            _ => return None,
        };
        (index < self.len).then_some(index)
    }
}

impl Default for RowList {
    fn default() -> Self {
        Self::new()
    }
}

impl View for RowList {
    type Command = RowListCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        if matches!(command, ScrollCommand::SetScrollY(_)) {
            self.scroll.content_mut().cancel_reveal();
        }
        self.scroll.perform(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        self.scroll.display(arena, store, ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imba::anim::AnimationClock;
    use skia_safe::Size;

    #[test]
    fn selection_reveal_glides_the_scroll_until_visible() {
        let mut store = Store::new();
        let ui = UiCtx::cold();
        let mut list = RowList::new();
        let labels: Vec<String> = (0..300).map(|index| format!("row {index}")).collect();
        list.set(&store, &imba::UiCtx::cold(), &labels, None, 0);
        list.select(250);
        assert_eq!(list.scroll_y(), 0.0);

        let viewport = Size::new(600.0, 400.0);
        let mut ticks_with_commands = 0;
        for tick in 0..120 {
            let commands = {
                let arena = Arena::default();
                let widget = imba::Layout::layout(
                    imba::View::display(&list, &arena, &store, &ui),
                    &arena,
                    imba::constraints::Constraints::tight(viewport),
                );
                let event = Event::AnimationClock {
                    now: AnimationClock::from_millis(tick as f64 * 8.0),
                };
                let widget =
                    imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_size(viewport));
                match imba::Widget::handle_event(
                    &widget,
                    &arena,
                    &event,
                    skia_safe::Rect::from_size(viewport),
                ) {
                    EventResult::Command(command) => vec![command],
                    EventResult::Commands(commands) => commands,
                    _ => Vec::new(),
                }
            };
            if commands.is_empty() {
                break;
            }
            ticks_with_commands += 1;
            for command in commands {
                imba::View::perform(
                    &mut list,
                    &mut store,
                    &ui,
                    command,
                    &mut imba::effect::Batch::new().effects(),
                );
            }
        }
        assert!(ticks_with_commands > 0, "the reveal drove the scroll");
        assert!(
            list.scroll_y() > 0.0,
            "the scroll moved toward the selection: {}",
            list.scroll_y()
        );
    }
}
