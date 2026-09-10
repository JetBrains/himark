use himark::PanelView;
use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, MouseButton},
    list::{ListCommand, ListSlice, ListView},
    scroll::{ScrollCommand, ScrollView},
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Paint, PathBuilder, Rect, Size};

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 18.0;
const BRANCHING: u64 = 10;
const MAX_DEPTH: u16 = 7;
const ROOTS: u64 = 100_000;

fn root_id(index: u64) -> u64 {
    (index + 1) << 32
}

fn child_id(parent: u64, pick: u64) -> u64 {
    (parent & !0xFFFF_FFFF) | ((parent & 0xFFFF_FFFF) * 16 + pick)
}

fn depth_of(id: u64) -> u16 {
    let mut path = id & 0xFFFF_FFFF;
    let mut depth = 0;
    while path > 0 {
        path /= 16;
        depth += 1;
    }
    depth
}

fn label_of(id: u64) -> String {
    let root = id >> 32;
    let mut picks = Vec::new();
    let mut path = id & 0xFFFF_FFFF;
    while path > 0 {
        picks.push(path % 16);
        path /= 16;
    }
    picks.reverse();
    let mut label = format!("node {root}");
    for pick in picks {
        label.push_str(&format!(".{pick}"));
    }
    label
}

#[derive(Clone)]
struct TreeDemoRow {
    id: u64,
    expanded: bool,
}

pub enum RowCommand {
    Toggle,
}

impl TreeDemoRow {
    fn expandable(&self) -> bool {
        depth_of(self.id) < MAX_DEPTH
    }

    fn inset(&self) -> f32 {
        f32::from(depth_of(self.id)) * INDENT
    }

    fn keyed(id: u64, expanded: bool, slice: &mut ListSlice<TreeDemoRow, u64>) {
        slice.push_keyed(id, TreeDemoRow { id, expanded }, ROW_HEIGHT);
    }
}

impl View for TreeDemoRow {
    type Command = RowCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let theme = himark::env::Themes::of(store);
        imba::eager(TreeDemoRowWidget {
            row: self,
            color: theme
                .base()
                .color
                .unwrap_or(skia_safe::Color::from_argb(0xFF, 0x80, 0x80, 0x80)),
            font: himark::fonts::ui_text_font(ui, 13.0),
            size: Size::new(constraints.max.width, ROW_HEIGHT),
        })
    }
}

struct TreeDemoRowWidget<'a> {
    row: &'a TreeDemoRow,
    color: skia_safe::Color,
    font: skia_safe::Font,
    size: Size,
}

impl<'a> Widget<'a, RowCommand> for TreeDemoRowWidget<'a> {
    fn size(&self) -> Size {
        self.size
    }

    fn handle_event(
        &self,
        _arena: &Arena,
        event: &Event<'_>,
        _viewport: Rect,
    ) -> EventResult<RowCommand> {
        match event {
            Event::Paint { canvas, .. } => {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(self.color);
                let inset = self.row.inset();
                if self.row.expandable() {
                    let mut triangle = PathBuilder::new();
                    let center_y = ROW_HEIGHT * 0.5;
                    if self.row.expanded {
                        triangle.move_to((inset + 4.0, center_y - 1.0));
                        triangle.line_to((inset + 12.0, center_y - 1.0));
                        triangle.line_to((inset + 8.0, center_y + 4.0));
                    } else {
                        triangle.move_to((inset + 5.0, center_y - 4.5));
                        triangle.line_to((inset + 11.0, center_y));
                        triangle.line_to((inset + 5.0, center_y + 4.5));
                    }
                    triangle.close();
                    canvas.draw_path(&triangle.detach(), &paint);
                }
                canvas.draw_str(
                    label_of(self.row.id),
                    (inset + 18.0, ROW_HEIGHT * 0.5 + 4.5),
                    &self.font,
                    &paint,
                );
                EventResult::Handled
            }
            Event::MouseDown {
                mods: _,
                button: MouseButton::Left,
                ..
            } if self.row.expandable() => EventResult::Command(RowCommand::Toggle),
            _ => EventResult::Ignored,
        }
    }
}

type Rows = ScrollView<ListView<TreeDemoRow, u64>>;

pub enum Command {
    Rows(ScrollCommand<ListCommand<RowCommand>>),
}

#[derive(Clone)]
pub struct TreeDemoView {
    rows: Rows,
}

impl TreeDemoView {
    pub fn new() -> Self {
        let mut slice: ListSlice<TreeDemoRow, u64> = ListSlice::new();
        for index in 0..ROOTS {
            TreeDemoRow::keyed(root_id(index), false, &mut slice);
        }
        Self {
            rows: ScrollView::new(ListView::from_slice(slice)),
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.content().len()
    }

    pub fn content_height(&self) -> f32 {
        self.rows.content().total_height()
    }

    fn toggle(&mut self, index: usize) {
        let tree = self.rows.content_mut();
        let Some(id) = tree.key_at(index).copied() else {
            return;
        };
        let Some(range) = tree.row_range(&id) else {
            return;
        };
        if range.len() > 1 {
            let mut slice: ListSlice<TreeDemoRow, u64> = ListSlice::new();
            TreeDemoRow::keyed(id, false, &mut slice);
            tree.splice_slice_animated(range, slice);
        } else if depth_of(id) < MAX_DEPTH {
            let mut slice: ListSlice<TreeDemoRow, u64> = ListSlice::new();
            TreeDemoRow::keyed(id, true, &mut slice);
            for pick in 1..=BRANCHING {
                TreeDemoRow::keyed(child_id(id, pick), false, &mut slice);
            }
            slice.cover(id, 0..(BRANCHING as usize + 1));
            tree.splice_slice_animated(range, slice);
        }
    }
}

impl Default for TreeDemoView {
    fn default() -> Self {
        Self::new()
    }
}

impl View for TreeDemoView {
    type Command = Command;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            Command::Rows(ScrollCommand::Content(ListCommand::Child(
                index,
                RowCommand::Toggle,
            ))) => {
                self.toggle(index);
            }

            Command::Rows(ScrollCommand::Content(ListCommand::Focus(index, then))) => {
                fx.scope(Command::Rows, |fx| {
                    self.rows.perform(
                        store,
                        ui,
                        ScrollCommand::Content(ListCommand::Focus(index, None)),
                        fx,
                    )
                });
                if let Some(inner) = then {
                    if let ListCommand::Child(index, RowCommand::Toggle) = *inner {
                        self.toggle(index);
                    }
                }
            }
            Command::Rows(command) => fx.scope(Command::Rows, |fx| {
                self.rows.perform(store, ui, command, fx)
            }),
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        self.rows
            .layout(arena, store, ui, constraints)
            .map(Command::Rows)
    }
}

impl PanelView for TreeDemoView {
    type Place = himark::NoPlace;

    fn title(&self, _store: &Store) -> String {
        "Tree Demo".to_owned()
    }

    fn dismantle(&mut self, _store: &mut Store) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct OpenTreeDemo;

impl himark::DynamicCommand for OpenTreeDemo {
    fn id(&self) -> &'static str {
        "demo.tree"
    }
    fn name(&self) -> String {
        "Open Tree Demo".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        store: &mut Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        let mut entity = himark::Windows::window(store, window).expect("the window entity");
        let _ = entity.open_panel(store, Box::new(TreeDemoView::new()), fx);
        himark::Windows::put(store, window, entity);
    }
}
