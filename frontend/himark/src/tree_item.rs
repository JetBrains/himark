use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Paint, Size};

#[derive(Clone)]
pub struct TreeLabel {
    label: String,
    pick: bool,
    dim: bool,

    strong: bool,

    trail: Vec<(String, skia_safe::Color)>,
}

#[derive(Clone, Copy)]
pub enum TreeLabelCommand {
    Activate,
}

impl TreeLabel {
    pub fn new(label: String, pick: bool, dim: bool) -> Self {
        Self {
            label,
            pick,
            dim,
            strong: false,
            trail: Vec::new(),
        }
    }

    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    pub fn with_trail(mut self, trail: Vec<(String, skia_safe::Color)>) -> Self {
        self.trail = trail;
        self
    }

    pub fn text(&self) -> &str {
        &self.label
    }
}

impl View for TreeLabel {
    type Command = TreeLabelCommand;

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
        let theme = crate::env::Themes::of(store);
        let tree = theme.ui().tree.clone();
        let colors = theme.ui().peeker.clone();
        let width = constraints.max.width.max(1.0);
        let font = match self.strong {
            true => crate::fonts::ui_font(ui, tree.font_size),
            false => crate::fonts::ui_text_font(ui, tree.font_size),
        };
        let trail_font = crate::fonts::ui_text_font(ui, tree.font_size);
        let pick = self.pick;
        let dim = self.dim;
        let label = self.label.clone();
        let trail = self.trail.clone();
        leaf::<TreeLabelCommand>(width, tree.row_height)
            .paint_instead(move |_arena, canvas, rect| {
                let baseline = rect.top + rect.height() * 0.5 + tree.font_size * 0.36;
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(if dim {
                    colors.dim_text.0
                } else {
                    colors.text.0
                });
                canvas.draw_str(&label, (rect.left, baseline), &font, &paint);
                let mut trail_x = rect.right - tree.text_x * 0.5;
                for (text, color) in trail.iter().rev() {
                    let advance = trail_font.measure_str(text, None).0;
                    trail_x -= advance;
                    paint.set_color(*color);
                    canvas.draw_str(text, (trail_x, baseline), &trail_font, &paint);
                    trail_x -= tree.font_size * 0.4;
                }
            })
            .event(move |_arena, event, _size| match event {
                Event::MouseDown { .. } if pick => EventResult::Command(TreeLabelCommand::Activate),
                Event::MouseDown { .. } => EventResult::Handled,
                _ => EventResult::Ignored,
            })
    }
}

#[derive(Clone)]
pub struct TreeItemView<V: Clone> {
    inner: V,
    depth: u16,
    expanded: Option<bool>,
    toggle_on_body: bool,
}

pub enum TreeItemCommand<C> {
    Toggle,
    Inner(C),
}

impl<V: Clone> TreeItemView<V> {
    pub fn leaf(inner: V, depth: u16) -> Self {
        Self {
            inner,
            depth,
            expanded: None,
            toggle_on_body: false,
        }
    }

    pub fn branch(inner: V, depth: u16, expanded: bool) -> Self {
        Self {
            inner,
            depth,
            expanded: Some(expanded),
            toggle_on_body: false,
        }
    }

    pub fn toggling_on_body(mut self) -> Self {
        self.toggle_on_body = true;
        self
    }

    pub fn inner(&self) -> &V {
        &self.inner
    }

    pub fn depth(&self) -> u16 {
        self.depth
    }
}

impl<V> View for TreeItemView<V>
where
    V: View + Clone,
    V::Command: Send + 'static,
{
    type Command = TreeItemCommand<V::Command>;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            TreeItemCommand::Toggle => {}
            TreeItemCommand::Inner(command) => fx.scope(TreeItemCommand::Inner, |fx| {
                self.inner.perform(store, ui, command, fx)
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
        let tree = crate::env::Themes::of(store).ui().tree.clone();
        let colors = crate::env::Themes::of(store).ui().peeker.clone();
        let inset = f32::from(self.depth) * tree.indent;
        let width = constraints.max.width.max(1.0);
        let offset = inset + tree.text_x;
        let inner = self.inner.layout(
            arena,
            store,
            ui,
            Constraints {
                min: Size::default(),
                max: Size::new((width - offset).max(1.0), constraints.max.height),
            },
        );
        let height = inner.size().height.max(tree.row_height);
        TreeItemWidget {
            inner,
            offset,
            triangle_x: inset + tree.text_x * 0.28,
            triangle_half: (tree.font_size * 0.28).max(4.0),
            expanded: self.expanded,

            zone: match (self.expanded.is_some(), self.toggle_on_body) {
                (true, true) => width,
                (true, false) => offset,
                (false, _) => 0.0,
            },
            color: colors.dim_text.0,
            size: Size::new(width, height),
            _command: std::marker::PhantomData,
        }
    }
}

struct TreeItemWidget<Inner, C> {
    inner: Inner,
    offset: f32,
    triangle_x: f32,
    triangle_half: f32,
    expanded: Option<bool>,
    zone: f32,
    color: skia_safe::Color,
    size: Size,
    _command: std::marker::PhantomData<C>,
}

impl<'a, Inner, C: 'a> Thunk<'a, TreeItemCommand<C>> for TreeItemWidget<Inner, C>
where
    Inner: Thunk<'a, C> + 'a,
{
    fn size(&self) -> Size {
        self.size
    }

    fn realize(
        self,
        arena: &'a imba::arena::Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, TreeItemCommand<C>> {
        let TreeItemWidget {
            inner,
            offset,
            triangle_x,
            triangle_half,
            expanded,
            zone,
            color,
            size,
            ..
        } = self;
        imba::WidgetBox::new(
            arena,
            TreeItemWidget {
                inner: inner.realize(arena, viewport),
                offset,
                triangle_x,
                triangle_half,
                expanded,
                zone,
                color,
                size,
                _command: std::marker::PhantomData,
            },
        )
    }
}

impl<'a, Inner, C: 'a> Widget<'a, TreeItemCommand<C>> for TreeItemWidget<Inner, C>
where
    Inner: Widget<'a, C>,
{
    fn size(&self) -> Size {
        self.size
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, TreeItemCommand<C>>> {
        imba::overlay::map_overlays(self.inner.overlays(), &TreeItemCommand::Inner)
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: skia_safe::Rect,
    ) -> EventResult<TreeItemCommand<C>> {
        let rect = skia_safe::Rect::from_xywh(
            self.offset,
            0.0,
            (self.size.width - self.offset).max(0.0),
            self.size.height,
        );
        let child_viewport =
            imba::container::viewport_for_child(viewport, rect).unwrap_or_default();
        match event {
            Event::Paint { canvas, .. } => {
                if let Some(expanded) = self.expanded {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(self.color);
                    let center_y = self.size.height * 0.5;
                    let half = self.triangle_half;
                    let left = self.triangle_x;
                    let frame =
                        skia_safe::Rect::from_xywh(left, center_y - half, half * 2.0, half * 2.0);
                    paint.set_stroke(true);
                    paint.set_stroke_width(1.5);
                    canvas.draw_rect(frame.with_inset((0.75, 0.75)), &paint);
                    if expanded {
                        paint.set_stroke(false);
                        let inset = half * 0.55;
                        canvas.draw_rect(frame.with_inset((inset, inset)), &paint);
                    }
                }
                canvas.save();
                canvas.translate((self.offset, 0.0));
                let result = self
                    .inner
                    .handle_event(arena, event, child_viewport)
                    .map(TreeItemCommand::Inner);
                canvas.restore();
                result
            }
            Event::MouseDown { point, .. } if self.expanded.is_some() && point.x < self.zone => {
                EventResult::Command(TreeItemCommand::Toggle)
            }
            Event::MouseDown { .. }
            | Event::MouseDrag { .. }
            | Event::MouseUp { .. }
            | Event::Scroll { .. } => {
                let local = event.translated(-self.offset, 0.0);
                self.inner
                    .handle_event(arena, &local, child_viewport)
                    .map(TreeItemCommand::Inner)
                    .reveal_translated(self.offset, 0.0)
            }
            _ => self
                .inner
                .handle_event(arena, event, child_viewport)
                .map(TreeItemCommand::Inner)
                .reveal_translated(self.offset, 0.0),
        }
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, TreeItemCommand<C>>
    where
        'a: 'w,
    {
        let offset = self.offset;
        self.inner
            .focus_data()
            .translated(offset, 0.0)
            .map(TreeItemCommand::Inner)
    }
}

pub type TreeListCommand =
    imba::scroll::ScrollCommand<imba::list::ListCommand<TreeItemCommand<TreeLabelCommand>>>;

pub fn tree_interaction(command: &TreeListCommand) -> Option<(usize, bool)> {
    use imba::list::ListCommand;
    use imba::scroll::ScrollCommand;
    let ScrollCommand::Content(command) = command else {
        return None;
    };
    let (index, command) = match command {
        ListCommand::Child(index, command) => (*index, command),
        ListCommand::Focus(index, Some(then)) => match then.as_ref() {
            ListCommand::Child(_, command) => (*index, command),
            _ => return None,
        },
        _ => return None,
    };
    match command {
        TreeItemCommand::Toggle => Some((index, true)),
        TreeItemCommand::Inner(TreeLabelCommand::Activate) => Some((index, false)),
    }
}
