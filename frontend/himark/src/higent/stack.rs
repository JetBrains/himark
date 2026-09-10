use crate::fonts::ui_text_font;
use ahp_types::state::{ConfirmationOption, ConfirmationOptionKind, Message, PendingMessage};
use imba::{
    arena::Arena,
    container::{container, Container},
    event::{Event, EventResult},
    leaf::leaf,
    thunk_ext::ThunkExt,
    UiCtx,
};
use skia_safe::{Paint, Rect, Size};

use crate::higent::turn::TurnView;

type ChatChrome = crate::theme::ChatChrome;

pub enum StackCommand {
    Answer(usize),

    ToggleQueue,

    RemoveQueued(String),
}

#[derive(Clone)]
pub(crate) struct PermissionAsk {
    turn: String,
    tool: String,
    title: String,
    invocation: String,

    input: Option<String>,
    options: rpds::VectorSync<ConfirmationOption>,
}

impl PermissionAsk {
    pub(crate) fn new(
        turn: String,
        tool: String,
        title: String,
        invocation: String,
        input: Option<String>,
        options: impl IntoIterator<Item = ConfirmationOption>,
    ) -> Self {
        Self {
            turn,
            tool,
            title,
            invocation,
            input,
            options: options.into_iter().collect(),
        }
    }

    fn deny_index(&self) -> Option<usize> {
        self.options
            .iter()
            .position(|option| matches!(option.kind, ConfirmationOptionKind::Deny))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AskKeys {
    pub count: usize,
    pub deny: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct WidgetStack {
    ask: Option<PermissionAsk>,

    queue: rpds::VectorSync<PendingMessage>,

    collapsed: bool,
}

impl WidgetStack {
    pub(crate) fn new() -> Self {
        Self {
            ask: None,
            queue: rpds::VectorSync::new_sync(),
            collapsed: false,
        }
    }

    pub(crate) fn set_ask(&mut self, ask: PermissionAsk) {
        self.ask = Some(ask);
    }

    pub(crate) fn clear_ask(&mut self) {
        self.ask = None;
    }

    pub(crate) fn clear_ask_for_tool(&mut self, tool_call_id: &str) {
        if self
            .ask
            .as_ref()
            .is_some_and(|ask| ask.tool == tool_call_id)
        {
            self.ask = None;
        }
    }

    pub(crate) fn ask_turn(&self) -> Option<String> {
        self.ask.as_ref().map(|ask| ask.turn.clone())
    }

    pub(crate) fn ask_keys(&self) -> Option<AskKeys> {
        self.ask.as_ref().map(|ask| AskKeys {
            count: ask.options.len(),
            deny: ask.deny_index(),
        })
    }

    pub(crate) fn answer_payload(
        &self,
        index: usize,
    ) -> Option<(String, String, ConfirmationOption)> {
        let ask = self.ask.as_ref()?;
        let option = ask.options.iter().nth(index).cloned()?;
        Some((ask.turn.clone(), ask.tool.clone(), option))
    }

    pub(crate) fn permission_oracle(
        &self,
    ) -> Option<(String, String, Option<String>, Vec<String>)> {
        self.ask.as_ref().map(|ask| {
            (
                ask.title.clone(),
                ask.invocation.clone(),
                ask.input.clone(),
                ask.options
                    .iter()
                    .map(|option| option.label.clone())
                    .collect(),
            )
        })
    }

    pub(crate) fn seed_queue(&mut self, queue: impl IntoIterator<Item = PendingMessage>) {
        self.queue = queue.into_iter().collect();
    }

    pub(crate) fn insert_queued(&mut self, id: String, message: Message) {
        if !self.queue.iter().any(|held| held.id == id) {
            self.queue.push_back_mut(PendingMessage { id, message });
        }
    }

    pub(crate) fn remove_queued(&mut self, id: &str) {
        self.queue = self
            .queue
            .iter()
            .filter(|held| held.id != id)
            .cloned()
            .collect();
    }

    pub(crate) fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub(crate) fn queue_oracle(&self) -> Vec<(String, String)> {
        self.queue
            .iter()
            .map(|held| (held.id.clone(), held.message.text.clone()))
            .collect()
    }

    fn line(chrome: &ChatChrome) -> f32 {
        chrome.title_size * 1.9
    }

    fn ask_height(&self, chrome: &ChatChrome) -> f32 {
        let line = Self::line(chrome);
        let box_pad = chrome.pad * 0.75;
        self.ask
            .as_ref()
            .map(|ask| {
                let mut height = box_pad * 2.0 + line + line * 0.9;
                if ask.input.is_some() {
                    height += line * 1.3;
                }
                height + ask.options.len() as f32 * line + chrome.pad * 0.5
            })
            .unwrap_or(0.0)
    }

    fn queue_height(&self, chrome: &ChatChrome) -> f32 {
        if self.queue.is_empty() {
            return 0.0;
        }
        let line = Self::line(chrome);
        let rows = if self.collapsed { 0 } else { self.queue.len() };
        line + rows as f32 * line + chrome.pad * 0.5
    }

    pub(crate) fn height(&self, chrome: &ChatChrome) -> f32 {
        self.ask_height(chrome) + self.queue_height(chrome)
    }

    pub(crate) fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        ui: &'a UiCtx,
        chrome: &ChatChrome,
        width: f32,
    ) -> Container<'a, StackCommand> {
        let pad = chrome.pad;
        let box_pad = pad * 0.75;
        let line = Self::line(chrome);
        let radius = chrome.radius;
        let box_w = TurnView::content_width(chrome, (width - pad * 2.0).max(1.0));
        let box_x = ((width - box_w) / 2.0).max(pad);
        let ask_h = self.ask_height(chrome);
        let mut stack = container(arena, Size::new(width, self.height(chrome)));

        if let Some(ask) = &self.ask {
            let card_h = ask_h - pad * 0.5;
            let surface = chrome.ask_surface.0;
            let card_border = chrome.ask_border.0;
            let title = ask.title.clone();
            let invocation = ask.invocation.clone();
            let input_preview = ask.input.clone();
            let title_font = ui_text_font(ui, chrome.title_size * 0.8);
            let body_font = ui_text_font(ui, chrome.title_size * 0.85);
            let code_font = ui_text_font(ui, chrome.title_size * 0.8);
            let accent_color = chrome.accent.0;
            let text_color = chrome.text_color.0;
            let input_fill = chrome.input_fill.0;
            let has_preview = input_preview.is_some();
            let card = leaf::<StackCommand>(box_w, card_h)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(surface);
                    canvas.draw_round_rect(rect, radius, radius, &paint);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.0);
                    paint.set_color(card_border);
                    canvas.draw_round_rect(rect, radius, radius, &paint);
                    paint.set_stroke(false);
                    let mut y = rect.top + box_pad + title_font.size();
                    paint.set_color(accent_color);
                    canvas.draw_str(
                        title.as_str(),
                        (rect.left + box_pad, y),
                        &title_font,
                        &paint,
                    );
                    y += line * 0.9;
                    paint.set_color(text_color);
                    canvas.draw_str(
                        invocation.as_str(),
                        (rect.left + box_pad, y),
                        &body_font,
                        &paint,
                    );
                    if let Some(preview) = &input_preview {
                        y += line * 0.5;
                        let well = Rect::from_xywh(
                            rect.left + box_pad,
                            y,
                            rect.width() - box_pad * 2.0,
                            line,
                        );
                        paint.set_color(input_fill);
                        canvas.draw_round_rect(well, 6.0, 6.0, &paint);
                        paint.set_color(text_color);
                        canvas.draw_str(
                            preview.as_str(),
                            (well.left + box_pad * 0.75, well.top + line * 0.68),
                            &code_font,
                            &paint,
                        );
                    }
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Handled,
                    _ => EventResult::Ignored,
                });
            stack.place(box_x, 0.0, card);

            let mut option_y =
                box_pad + line + line * 0.9 + if has_preview { line * 1.3 } else { 0.0 };
            for (index, option) in ask.options.iter().enumerate() {
                let label = option.label.clone();
                let number = format!("{}", index + 1);
                let option_font = ui_text_font(ui, chrome.title_size * 0.85);
                let chip_font = ui_text_font(ui, chrome.title_size * 0.7);
                let text_color = chrome.text_color.0;
                let chip_color = chrome.accent.0;
                let row = leaf::<StackCommand>(box_w - box_pad * 2.0, line)
                    .paint_instead(move |_arena, canvas, rect| {
                        let mut paint = Paint::default();
                        paint.set_anti_alias(true);
                        let chip = Rect::from_xywh(
                            rect.left,
                            rect.top + line * 0.18,
                            line * 0.62,
                            line * 0.62,
                        );
                        paint.set_color(chip_color);
                        paint.set_stroke(true);
                        paint.set_stroke_width(1.0);
                        canvas.draw_round_rect(chip, 4.0, 4.0, &paint);
                        paint.set_stroke(false);
                        let number_w = chip_font.measure_str(number.as_str(), None).0;
                        canvas.draw_str(
                            number.as_str(),
                            (
                                chip.left + (chip.width() - number_w) / 2.0,
                                chip.top + chip.height() * 0.72,
                            ),
                            &chip_font,
                            &paint,
                        );
                        paint.set_color(text_color);
                        canvas.draw_str(
                            label.as_str(),
                            (rect.left + line * 0.9, rect.top + line * 0.66),
                            &option_font,
                            &paint,
                        );
                    })
                    .event(move |_arena, event, _size| match event {
                        Event::MouseDown { .. } => {
                            EventResult::Command(StackCommand::Answer(index))
                        }
                        _ => EventResult::Ignored,
                    });
                stack.place(box_x + box_pad, option_y, row);
                option_y += line;
            }
        }

        if !self.queue.is_empty() {
            let queue_y = ask_h;
            let header_font = ui_text_font(ui, chrome.title_size * 0.65);
            let row_font = ui_text_font(ui, chrome.title_size * 0.8);
            let dim = chrome.notice_color.0;
            let text_color = chrome.text_color.0;
            let count = self.queue.len();
            let collapsed = self.collapsed;
            let header = leaf::<StackCommand>(box_w, line)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(dim);

                    let mut path = skia_safe::PathBuilder::new();
                    let cx = rect.left + box_pad + line * 0.14;
                    let cy = rect.top + line * 0.5;
                    let r = line * 0.13;
                    if collapsed {
                        path.move_to((cx - r * 0.5, cy - r));
                        path.line_to((cx + r * 0.7, cy));
                        path.line_to((cx - r * 0.5, cy + r));
                    } else {
                        path.move_to((cx - r, cy - r * 0.5));
                        path.line_to((cx + r, cy - r * 0.5));
                        path.line_to((cx, cy + r * 0.7));
                    }
                    path.close();
                    canvas.draw_path(&path.detach(), &paint);
                    canvas.draw_str(
                        format!("PROMPT QUEUE {count}").as_str(),
                        (rect.left + box_pad + line * 0.5, rect.top + line * 0.66),
                        &header_font,
                        &paint,
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Command(StackCommand::ToggleQueue),
                    _ => EventResult::Ignored,
                });
            stack.place(box_x, queue_y, header);
            if !collapsed {
                for (index, held) in self.queue.iter().enumerate() {
                    let id = held.id.clone();
                    let text: String = held.message.text.lines().next().unwrap_or("").to_owned();
                    let row_font = row_font.clone();
                    let row_y = queue_y + line + index as f32 * line;
                    let row = leaf::<StackCommand>(box_w - box_pad * 2.0, line)
                        .paint_instead(move |_arena, canvas, rect| {
                            let mut paint = Paint::default();
                            paint.set_anti_alias(true);
                            paint.set_color(dim);
                            canvas.draw_circle(
                                (rect.left + line * 0.2, rect.top + line * 0.5),
                                2.5,
                                &paint,
                            );
                            paint.set_color(text_color);
                            canvas.draw_str(
                                text.as_str(),
                                (rect.left + line * 0.55, rect.top + line * 0.66),
                                &row_font,
                                &paint,
                            );
                            paint.set_color(dim);

                            let cx = rect.right - line * 0.5;
                            let cy = rect.top + line * 0.5;
                            let r = line * 0.16;
                            paint.set_stroke(true);
                            paint.set_stroke_width(1.6);
                            canvas.draw_line((cx - r, cy - r), (cx + r, cy + r), &paint);
                            canvas.draw_line((cx - r, cy + r), (cx + r, cy - r), &paint);
                            paint.set_stroke(false);
                        })
                        .event(move |_arena, event, size| match event {
                            Event::MouseDown { point, .. } => {
                                if point.x > size.width - line {
                                    EventResult::Command(StackCommand::RemoveQueued(id.clone()))
                                } else {
                                    EventResult::Handled
                                }
                            }
                            _ => EventResult::Ignored,
                        });
                    stack.place(box_x + box_pad, row_y, row);
                }
            }
        }

        stack
    }
}
