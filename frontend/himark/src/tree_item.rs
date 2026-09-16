// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::Effects,
    event::{Event, EventResult},
    store::Store,
    LayoutExt as _, Thunk, UiCtx, View, Widget,
};
use skia_safe::{Paint, Size};

/// What the label NAMES — resolved to a theme color at display time
/// (rows live long in ropes; a baked color would survive a theme
/// switch).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum TreeTint {
    #[default]
    Label,
    Directory,
    File,
}

#[derive(Clone)]
pub struct TreeLabel {
    label: String,
    pick: bool,
    dim: bool,

    tint: TreeTint,

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
            tint: TreeTint::Label,
            trail: Vec::new(),
        }
    }

    pub fn tinted(mut self, tint: TreeTint) -> Self {
        self.tint = tint;
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

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        let mut style = crate::ui::RowStyle::drawer(store, ui);
        // Tree rows breathe more than menu rows.
        style.air = crate::ui::space::M;
        let tree = crate::env::Themes::of(store).ui().tree.clone();
        let label_style = match (self.dim, self.tint) {
            (true, _) => style.trail.clone(),
            (false, TreeTint::Directory) => style.label.clone().colored(tree.directory.0),
            (false, TreeTint::File) => style.label.clone().colored(tree.file.0),
            (false, TreeTint::Label) => style.label.clone(),
        };
        let mut row = crate::ui::ListRow::new(arena, style.clone())
            .label_styled(&label_style, self.label.clone());
        for (text, color) in &self.trail {
            row = row.trail_styled(&style.trail.clone().colored(*color), text.clone());
        }
        let pick = self.pick;
        row.on_event(
            move |_arena: &Arena, event: &Event<'_>, _size| match event {
                Event::MouseDown { .. } if pick => EventResult::Command(TreeLabelCommand::Activate),
                Event::MouseDown { .. } => EventResult::Handled,
                _ => EventResult::Ignored,
            },
        )
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

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        TreeItemChrome {
            view: self,
            store,
            ui,
        }
    }
}

/// The tree item's indent-and-disclosure frame, REIFIED (docs/UI.md
/// stage 2, the `WindowFrame` shape): a layout STRUCT because the
/// indent offset, the toggle zone and the inner's width are all cut
/// from the incoming constraints. The compositor WIDGET underneath
/// stays bespoke — its toggle zone is PRIORITY-ordered over the inner
/// content (`toggle_on_body` claims presses the inner would otherwise
/// answer first) and it forwards drags/scrolls to the inner with no
/// containment test, neither of which the fallback-ordered
/// `.on_event` primitives can express.
struct TreeItemChrome<'a, V: Clone> {
    view: &'a TreeItemView<V>,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl<V: Clone> imba::LayoutValue for TreeItemChrome<'_, V> {}

impl<'a, V> imba::Layout<'a, TreeItemCommand<V::Command>> for TreeItemChrome<'a, V>
where
    V: View + Clone,
    V::Command: Send + 'static,
{
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, TreeItemCommand<V::Command>> {
        let TreeItemChrome { view, store, ui } = self;
        let tree = crate::env::Themes::of(store).ui().tree.clone();
        let colors = crate::env::Themes::of(store).ui().peeker.clone();
        let inset = f32::from(view.depth) * tree.indent;
        let width = constraints.max.width.max(1.0);
        let offset = inset + tree.text_x;
        let inner = imba::Layout::layout(
            view.inner.display(arena, store, ui),
            arena,
            Constraints {
                min: Size::default(),
                max: Size::new((width - offset).max(1.0), constraints.max.height),
            },
        );
        let height = inner.size().height;
        imba::ThunkBox::new(
            arena,
            TreeItemWidget {
                inner,
                offset,
                triangle_x: inset + tree.text_x * 0.28,
                triangle_half: (tree.font_size * 0.28).max(4.0),
                expanded: view.expanded,

                zone: match (view.expanded.is_some(), view.toggle_on_body) {
                    (true, true) => width,
                    (true, false) => offset,
                    (false, _) => 0.0,
                },
                color: colors.dim_text.0,
                size: Size::new(width, height),
                _command: std::marker::PhantomData,
            },
        )
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
                    // The STANDARD chevron — the same stroke the
                    // editor gutter draws for folds: down when
                    // expanded, right when collapsed.
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(self.color);
                    paint.set_stroke(true);
                    paint.set_stroke_width(2.0);
                    let cx = self.triangle_x + self.triangle_half;
                    let cy = self.size.height * 0.5;
                    let arm = self.triangle_half * 0.8;
                    let mut path = skia_safe::PathBuilder::new();
                    if expanded {
                        path.move_to((cx - arm, cy - arm * 0.6));
                        path.line_to((cx, cy + arm * 0.8));
                        path.line_to((cx + arm, cy - arm * 0.6));
                    } else {
                        path.move_to((cx - arm * 0.6, cy - arm));
                        path.line_to((cx + arm * 0.8, cy));
                        path.line_to((cx - arm * 0.6, cy + arm));
                    }
                    canvas.draw_path(&path.detach(), &paint);
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
