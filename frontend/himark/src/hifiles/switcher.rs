use std::sync::Arc;

use crate::{ModalRequest, ModalView};
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use crate::hifiles::{PANEL_PAD, PANEL_WIDTH};

#[derive(Clone)]
enum SwitcherRow {
    Session(crate::SessionId),
    New,
}

pub enum SwitcherCommand {
    Select(isize),

    Pick(usize),

    Dismiss,
}

pub struct SessionSwitcherView {
    window: crate::WindowId,
    current: crate::SessionId,
    rows: Vec<SwitcherRow>,
    labels: Vec<String>,
    selected: usize,
    request: Option<ModalRequest>,
}

impl Clone for SessionSwitcherView {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            current: self.current.clone(),
            rows: self.rows.clone(),
            labels: self.labels.clone(),
            selected: self.selected,

            request: None,
        }
    }
}

impl SessionSwitcherView {
    pub fn open(store: &Store, window: crate::WindowId) -> Self {
        let current = crate::Windows::window_ref(store, window)
            .expect("the window entity")
            .current_session();

        let mut list: Vec<(crate::SessionId, String)> = Vec::new();
        let default = crate::SessionId::local_default(store);
        list.push((default.clone(), "Local".to_owned()));
        for id in crate::Windows::window_ref(store, window)
            .map(|entity| entity.visited_sessions())
            .unwrap_or_default()
        {
            if !id.names_session() && list.iter().all(|(held, _)| held != &id) {
                let position = list.len();
                list.push((id, format!("Scratch {position}")));
            }
        }
        for (host, row) in crate::higent::Hosts::list(store) {
            for summary in row.sessions.iter() {
                let id = crate::SessionId {
                    host,
                    session: summary.resource.clone(),
                };
                if list.iter().any(|(held, _)| held == &id) {
                    continue;
                }
                let label = Some(summary.title.clone())
                    .filter(|title| !title.is_empty())
                    .or_else(|| {
                        crate::higent::session_folders(store, &id)
                            .first()
                            .map(|folder| folder.name().to_owned())
                    })
                    .unwrap_or_else(|| summary.resource.clone());
                list.push((id, label));
            }
        }
        let mut rows: Vec<SwitcherRow> = list
            .iter()
            .map(|(id, _)| SwitcherRow::Session(id.clone()))
            .collect();
        let mut labels: Vec<String> = list.into_iter().map(|(_, label)| label).collect();
        rows.push(SwitcherRow::New);
        labels.push("+ New Scratch".to_owned());
        let selected = rows
            .iter()
            .position(|row| matches!(row, SwitcherRow::Session(id) if *id == current))
            .unwrap_or(0);
        Self {
            window,
            current,
            rows,
            labels,
            selected,
            request: None,
        }
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    fn pick(&mut self, index: usize) {
        let Some(row) = self.rows.get(index) else {
            return;
        };
        let target = match row {
            SwitcherRow::Session(id) if *id == self.current => {
                self.request = Some(ModalRequest::Close);
                return;
            }
            SwitcherRow::Session(id) => Some(id.clone()),
            SwitcherRow::New => None,
        };
        let window = self.window;
        self.request = Some(ModalRequest::Perform(crate::AppCommand::Dynamic(
            window,
            Arc::new(SwitchSession { target }),
        )));
    }
}

impl View for SessionSwitcherView {
    type Command = SwitcherCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            SwitcherCommand::Select(delta) => {
                let last = self.rows.len().saturating_sub(1);
                self.selected = self.selected.saturating_add_signed(delta).min(last);
            }
            SwitcherCommand::Pick(index) => self.pick(index),
            SwitcherCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = constraints.max;
        let mut overlay = container(arena, size);

        let theme = crate::env::Themes::of(store);
        let chrome = theme.ui().peeker.clone();
        let row_height = chrome.row_height;
        let selected = self.selected;
        let labels = &self.labels;
        let row_font = crate::fonts::ui_font(ui, chrome.row_size);
        let panel = {
            let mut panel = container(arena, Size::new(PANEL_WIDTH, size.height));
            let chrome_bg = chrome.clone();
            let backdrop = leaf::<SwitcherCommand>(PANEL_WIDTH, size.height)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(chrome_bg.background.0);
                    canvas.draw_rect(rect, &paint);
                    let mut edge = Paint::default();
                    edge.set_color(chrome_bg.rule.0);
                    canvas.draw_rect(
                        Rect::from_xywh(rect.right - 1.0, rect.top, 1.0, rect.height()),
                        &edge,
                    );
                })
                .event(|_arena, event, _size| match event {
                    Event::MouseDown { .. } => EventResult::Handled,
                    _ => EventResult::Ignored,
                });
            panel.place(0.0, 0.0, backdrop);

            let list_height = size.height - PANEL_PAD * 2.0;
            let list = leaf::<SwitcherCommand>(PANEL_WIDTH - 1.0, list_height)
                .paint_instead(move |_arena, canvas, rect| {
                    let mut highlight = Paint::default();
                    highlight.set_anti_alias(true);
                    highlight.set_color(chrome.highlight.0);
                    let mut accent = Paint::default();
                    accent.set_color(chrome.accent.0);
                    let mut text = Paint::default();
                    text.set_anti_alias(true);
                    text.set_color(chrome.text.0);
                    let mut dim = Paint::default();
                    dim.set_anti_alias(true);
                    dim.set_color(chrome.dim_text.0);
                    for (line, label) in labels.iter().enumerate() {
                        let top = rect.top + line as f32 * row_height;
                        if line == selected {
                            canvas.draw_round_rect(
                                Rect::from_xywh(
                                    rect.left + 4.0,
                                    top,
                                    rect.width() - 8.0,
                                    row_height,
                                ),
                                chrome.highlight_radius,
                                chrome.highlight_radius,
                                &highlight,
                            );
                            canvas.draw_rect(
                                Rect::from_xywh(
                                    rect.left,
                                    top + chrome.accent_inset,
                                    chrome.accent_width,
                                    row_height - chrome.accent_inset * 2.0,
                                ),
                                &accent,
                            );
                        }
                        let baseline = top + row_height - chrome.row_baseline;
                        let paint = match line == labels.len() - 1 {
                            true => &dim,
                            false => &text,
                        };
                        canvas.draw_str(
                            label,
                            (rect.left + chrome.row_text_x, baseline),
                            &row_font,
                            paint,
                        );
                    }
                })
                .event(move |_arena, event, _size| match event {
                    Event::MouseDown { point, .. } => {
                        let row = (point.y / row_height).max(0.0) as usize;
                        EventResult::Command(SwitcherCommand::Pick(row))
                    }
                    _ => EventResult::Ignored,
                });
            panel.place(0.0, PANEL_PAD, list);
            panel
        };
        overlay.place(0.0, 0.0, panel);

        let row_count = self.rows.len();
        let selected = self.selected;
        let keymap =
            leaf::<SwitcherCommand>(size.width, size.height).event(move |_arena, event, _size| {
                match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } => EventResult::Command(SwitcherCommand::Dismiss),
                    Event::KeyDown {
                        key: InputKey::Up, ..
                    } => EventResult::Command(SwitcherCommand::Select(-1)),
                    Event::KeyDown {
                        key: InputKey::Down,
                        ..
                    } => EventResult::Command(SwitcherCommand::Select(1)),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } if selected < row_count => {
                        EventResult::Command(SwitcherCommand::Pick(selected))
                    }
                    _ => EventResult::Ignored,
                }
            });
        overlay.place(0.0, 0.0, keymap);

        overlay
    }
}

impl ModalView for SessionSwitcherView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct SwitchSession {
    pub target: Option<crate::SessionId>,
}

impl crate::DynamicCommand for SwitchSession {
    fn id(&self) -> &'static str {
        "session.switch-to"
    }
    fn name(&self) -> String {
        "Switch to Session".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let target = match self.target.clone() {
            Some(target) => target,
            None => crate::SessionId::mint_scratch(store),
        };
        crate::switch_session(store, window, target, fx)
    }
}

pub struct ToggleSessionSwitcher;

impl crate::DynamicCommand for ToggleSessionSwitcher {
    fn id(&self) -> &'static str {
        "session.switch"
    }
    fn name(&self) -> String {
        "Switch Session".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.has_side_panel() {
            entity.roll_away_side_panel();
            crate::Windows::put(store, window, entity);
            return;
        }
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = SessionSwitcherView::open(store, window);
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_side_panel(store, Box::new(panel), fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

#[cfg(test)]
mod tests;
