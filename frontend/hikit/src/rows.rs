// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    effect::Effects,
    event::{Event, EventResult},
    layout::LayoutExt as _,
    list::{ListSlice, SelectionStyle},
    store::Store,
    ui::UiCtx,
    View,
};

#[derive(Clone)]
pub struct LabelRow {
    label: String,

    dim: bool,

    trail: Option<String>,
}

impl LabelRow {
    pub fn new(label: String, dim: bool, trail: Option<String>) -> Self {
        Self { label, dim, trail }
    }
}

impl View for LabelRow {
    type Command = std::convert::Infallible;

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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
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
        // The row body consumes nothing: an unclaimed click is the
        // LIST's to answer — `Select` then `Activate(Click)`
        // (docs/ui/list-keyboard.md §2).
        let _ = dim;
        row.on_event(
            move |_arena: &Arena, event: &Event<'_>, _size| match event {
                Event::MouseDown { .. } => EventResult::handled(),
                _ => EventResult::Ignored,
            },
        )
    }
}

/// The label-row slice builder the palette/peeker/completion popups
/// share: keyed rows for the pickable labels, one dim UNKEYED note
/// row at the tail (never addressable, never selectable).
pub fn label_slice(
    store: &Store,
    ui: &UiCtx,
    labels: &[String],
    trails: &[Option<String>],
    note: Option<String>,
) -> ListSlice<LabelRow, usize> {
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
    slice
}

pub fn selection_style(store: &Store) -> SelectionStyle {
    let ui = editor::env::Themes::of(store).ui().clone();
    // The typographic design: the current row is a flat WASH across
    // the whole column — no well, no hairline, no accent bar
    // (docs/Design.md, "no glass, no pills, no boxes").
    SelectionStyle {
        fill: ui.ink.selected.0,
        accent: skia_safe::Color::TRANSPARENT,
        accent_width: 0.0,
        accent_inset: 0.0,
        radius: 0.0,
        border: skia_safe::Color::TRANSPARENT,
        inset_x: 0.0,
        ..SelectionStyle::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imba::anim::AnimationClock;
    use imba::list::ListView;
    use imba::scroll::ScrollView;
    use skia_safe::Size;

    #[test]
    fn selection_reveal_glides_the_scroll_until_visible() {
        let mut store = Store::new();
        let ui = UiCtx::dont_use_too_slow();
        let labels: Vec<String> = (0..300).map(|index| format!("row {index}")).collect();
        let slice = label_slice(
            &store,
            ::editor::test_document::test_ui(),
            &labels,
            &[],
            None,
        );
        let mut list =
            ScrollView::new(ListView::from_slice(slice).with_selection(selection_style(&store)));
        list.content_mut().select_only(250);
        assert_eq!(list.scroll_y(), 0.0);

        let viewport = Size::new(600.0, 400.0);
        let mut ticks_with_commands = 0;
        for tick in 0..120 {
            let commands = {
                let arena = Arena::default();
                let widget = imba::layout::Layout::layout(
                    imba::View::display(&list, &arena, &store, &ui),
                    &arena,
                    imba::constraints::Constraints::tight(viewport),
                );
                let event = Event::AnimationClock {
                    now: AnimationClock::from_millis(tick as f64 * 8.0),
                };
                let widget =
                    imba::Thunk::realize(widget, &arena, skia_safe::Rect::from_size(viewport));
                imba::Widget::handle_event(
                    &widget,
                    &arena,
                    &event,
                    skia_safe::Rect::from_size(viewport),
                )
                .into_commands()
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
