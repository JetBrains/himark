use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    leaf::leaf,
    list::{ListCommand, ListSlice, ListView, SelectionStyle},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};
use skia_safe::Paint;

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

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let chrome = crate::env::Themes::of(store).ui().peeker.clone();
        let width = constraints.max.width.max(1.0);
        let row_height = chrome.row_height;
        let font = crate::fonts::ui_font(ui, chrome.row_size);
        let dim = self.dim;
        let label = self.label.clone();
        let trail = self.trail.clone();
        let trail_font = crate::fonts::ui_text_font(ui, chrome.row_size);
        leaf::<RowCommand>(width, row_height)
            .paint_instead(move |_arena, canvas, rect| {
                let mut text = Paint::default();
                text.set_anti_alias(true);
                let baseline = rect.top + rect.height() - chrome.row_baseline;
                text.set_color(if dim {
                    chrome.dim_text.0
                } else {
                    chrome.text.0
                });
                canvas.draw_str(
                    &label,
                    (rect.left + chrome.row_text_x, baseline),
                    &font,
                    &text,
                );
                if let Some(trail) = &trail {
                    let advance = trail_font.measure_str(trail, None).0;
                    text.set_color(chrome.dim_text.0);
                    canvas.draw_str(
                        trail,
                        (rect.right - advance - chrome.row_text_x, baseline),
                        &trail_font,
                        &text,
                    );
                }
            })
            .event(move |_arena, event, _size| match event {
                Event::MouseDown { .. } if !dim => EventResult::Command(RowCommand::Picked),
                Event::MouseDown { .. } => EventResult::Handled,
                _ => EventResult::Ignored,
            })
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
            scroll: ScrollView::new(ListView::from_measured([])),
            len: 0,
        }
    }

    pub fn set(&mut self, store: &Store, labels: &[String], note: Option<String>, selected: usize) {
        self.set_with_trails(store, labels, &[], note, selected)
    }

    pub fn set_with_trails(
        &mut self,
        store: &Store,
        labels: &[String],
        trails: &[Option<String>],
        note: Option<String>,
        selected: usize,
    ) {
        let chrome = crate::env::Themes::of(store).ui().peeker.clone();
        let row_height = chrome.row_height.max(1.0);
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
                row_height,
            );
        }
        if let Some(label) = note {
            slice.push(
                LabelRow {
                    label,
                    dim: true,
                    trail: None,
                },
                row_height,
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

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        self.scroll.layout(arena, store, ui, constraints)
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
        let ui = UiCtx::new();
        let mut list = RowList::new();
        let labels: Vec<String> = (0..300).map(|index| format!("row {index}")).collect();
        list.set(&store, &labels, None, 0);
        list.select(250);
        assert_eq!(list.scroll_y(), 0.0);

        let viewport = Size::new(600.0, 400.0);
        let mut ticks_with_commands = 0;
        for tick in 0..120 {
            let commands = {
                let arena = Arena::default();
                let widget =
                    imba::View::layout(&list, &arena, &store, &ui, Constraints::tight(viewport));
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
