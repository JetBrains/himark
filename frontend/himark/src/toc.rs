// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    event::{Event, EventResult, Key as InputKey},
    store::Store,
    LayoutExt as _, UiCtx, View,
};
use skia_safe::{Paint, Size};

use crate::forest::{ForestList, ForestNode, ForestSearcher};
use crate::list_keyboard::{ListKeyCommand, ListKeyboardController};
use crate::modal::{ModalRequest, ModalView};
use crate::tree_item::{tree_toggle, TreeListCommand};
use imba::list::{ActivateTrigger, ListOps};

pub(crate) const OUTLINE_CAP: usize = 2_000;

pub type SearchListCommand = ListKeyCommand<TreeListCommand>;

pub enum TocCommand {
    List(SearchListCommand),
    Dismiss,
}

pub struct TocView {
    window: crate::WindowId,

    context: String,
    pub(crate) search: ListKeyboardController<ForestList<u64>, ForestSearcher<u64>>,

    targets: rpds::HashTrieMapSync<u64, crate::ResourceLocation>,
    request: Option<ModalRequest>,
}

impl Clone for TocView {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            context: self.context.clone(),
            search: self.search.clone(),
            targets: self.targets.clone(),

            request: None,
        }
    }
}

#[derive(Default)]
struct DirTrie {
    dirs: std::collections::BTreeMap<String, DirTrie>,
    files: Vec<crate::ResourceLocation>,
}

impl DirTrie {
    fn emit(
        mut self,
        mut label: String,
        next: &mut u64,
        targets: &mut rpds::HashTrieMapSync<u64, crate::ResourceLocation>,
    ) -> ForestNode<u64> {
        while self.files.is_empty() && self.dirs.len() == 1 {
            let (segment, child) = self.dirs.pop_first().expect("one child");
            label.push('/');
            label.push_str(&segment);
            self = child;
        }
        let key = *next;
        *next += 1;
        let mut children = Vec::new();
        for (segment, child) in std::mem::take(&mut self.dirs) {
            children.push(child.emit(segment, next, targets));
        }
        for file in self.files {
            let key = *next;
            *next += 1;
            children.push(ForestNode {
                key,
                label: file.name().to_owned(),
                pick: true,
                dim: false,
                trail: Vec::new(),
                tint: crate::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
            targets.insert_mut(key, file);
        }
        ForestNode {
            key,
            label,
            pick: false,
            dim: true,
            trail: Vec::new(),
            tint: crate::TreeTint::Label,
            action: None,
            children,
        }
    }
}

impl TocView {
    pub fn for_locations(
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
        locations: &[crate::ResourceLocation],
    ) -> Option<Self> {
        if locations.is_empty() {
            return None;
        }
        let mut sorted: Vec<&crate::ResourceLocation> = locations.iter().collect();
        sorted.sort_by_key(|location| location.path().to_vec());
        sorted.dedup_by(|a, b| a == b);

        let total = sorted.len();
        sorted.truncate(OUTLINE_CAP);
        let capped = total > sorted.len();

        let mut root = DirTrie::default();
        for location in sorted {
            let path = location.path();
            let mut level = &mut root;
            for segment in &path[..path.len().saturating_sub(1)] {
                level = level.dirs.entry(segment.clone()).or_default();
            }
            level.files.push(location.clone());
        }

        let mut next = 0u64;
        let mut targets = rpds::HashTrieMapSync::new_sync();
        let mut nodes = Vec::new();
        for (segment, child) in root.dirs {
            nodes.push(child.emit(segment, &mut next, &mut targets));
        }
        for file in root.files {
            let key = next;
            next += 1;
            nodes.push(ForestNode {
                key,
                label: file.name().to_owned(),
                pick: true,
                dim: false,
                trail: Vec::new(),
                tint: crate::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
            targets.insert_mut(key, file);
        }

        let mut forest = ForestList::new(store);
        forest.set(&nodes, store, ui);
        Some(Self {
            window,
            context: match (capped, total) {
                (true, n) => format!("first {} of {n} files", OUTLINE_CAP),
                (false, 1) => "1 file".to_owned(),
                (false, n) => format!("{n} files"),
            },
            search: ListKeyboardController::searchable(
                forest,
                ForestSearcher::default(),
                store,
                ui,
                crate::env::Fonts::of(store),
            )
            .with_folds(),
            targets,
            request: None,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.search.inner().forest.rows()
    }

    pub fn visible_rows(&self) -> usize {
        self.search.inner().list().len()
    }

    fn pick(&mut self, key: u64, store: &Store, ui: &UiCtx) {
        match self.targets.get(&key) {
            Some(location) => {
                self.request = Some(ModalRequest::OpenLocations(vec![location.clone()]));
            }

            None => self.search.inner_mut().toggle(&key, store, ui),
        }
    }
}

impl View for TocView {
    type Command = TocCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, TocCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.search.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(TocCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.search.focus_data(store, ui).map(TocCommand::List))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: TocCommand,
        fx: &mut imba::effect::Effects<'_, TocCommand>,
    ) {
        match command {
            TocCommand::List(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.search.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.search.inner().list().key_at(index).copied()
                            else {
                                return;
                            };
                            self.search.inner_mut().list_mut().select_only(key);
                            return self.search.inner_mut().toggle(&key, store, ui);
                        }
                    }
                    _ => {}
                }
                type Search = ListKeyboardController<ForestList<u64>, ForestSearcher<u64>>;
                if let Some((index, trigger)) = Search::activated(&command) {
                    if let Some(key) = self.search.inner().list().key_at(index).copied() {
                        let searching = self.search.searching();
                        self.pick(key, store, ui);
                        match trigger {
                            // The deliberate pick ends the search in
                            // the same stroke.
                            ActivateTrigger::Enter if searching => {
                                return self.perform(
                                    store,
                                    ui,
                                    TocCommand::List(ListKeyCommand::Clear),
                                    fx,
                                );
                            }
                            ActivateTrigger::Enter | ActivateTrigger::Click => {}
                        }
                    }
                }
                fx.scope(TocCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
            }
            TocCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        {
            let theme_ui = crate::env::Themes::of(store).ui().clone();
            let title_font = crate::fonts::ui_font(ui, theme_ui.panel.title_size);
            let searching = self.search.searching();
            DrawerPanel {
                content: imba::LayoutBox::new(
                    arena,
                    self.search
                        .display(arena, store, ui)
                        .map_layout(TocCommand::List),
                ),
                theme: theme_ui,
                title_font,
                shaper: imba::TextShaper::of(ui),
                context: self.context.clone(),
                scaled_pad: true,
                keys: move |_arena: &Arena, event: &Event<'_>, _size: Size| match event {
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(TocCommand::Dismiss),
                    _ => EventResult::Ignored,
                },
            }
        }
    }
}

impl ModalView for TocView {
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

struct NavigateToPlace {
    place: crate::EditorPlace,
}

impl crate::DynamicCommand for NavigateToPlace {
    fn id(&self) -> &'static str {
        "toc.jump"
    }
    fn name(&self) -> String {
        "Jump to Symbol".to_owned()
    }
    fn perform(
        &self,
        app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        let _ = entity.navigate(
            store,
            ui,
            window,
            &crate::NavigationLocation::new(self.place.clone()),
            fx,
        );
        crate::Windows::put(store, window, entity);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "toc.toggle",
        order: 2.0,
        side: crate::ToolbarSide::Left,
        glyph: std::sync::Arc::new(|canvas, rect, color| {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let rows = [t + h * 0.14, t + h * 0.38, t + h * 0.62, t + h * 0.86];
            let mut path = skia_safe::PathBuilder::new();
            path.move_to((l, rows[0]));
            path.line_to((l + w, rows[0]));
            path.move_to((l + w * 0.28, rows[1]));
            path.line_to((l + w, rows[1]));
            path.move_to((l + w * 0.28, rows[2]));
            path.line_to((l + w, rows[2]));
            path.move_to((l, rows[3]));
            path.line_to((l + w, rows[3]));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

pub struct ToggleToc;

impl crate::DynamicCommand for ToggleToc {
    fn id(&self) -> &'static str {
        "toc.toggle"
    }
    fn name(&self) -> String {
        "Table of Contents".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let Some(mut entity) = crate::Windows::window(store, window) else {
            return;
        };
        if entity.side_panel().is_some_and(|panel| {
            panel.as_any().is::<TocView>() || panel.as_any().is::<OutlineView>()
        }) {
            entity.roll_away_side_panel();
            crate::Windows::put(store, window, entity);
            return;
        }
        let Some(panel) =
            entity
                .workbench()
                .root
                .focused_pane()
                .drawer_view(store, &_app.ui_ctx(), window)
        else {
            crate::Windows::put(store, window, entity);
            return;
        };
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_side_panel(store, panel, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub(crate) type OutlineKey = (::editor::SyntaxId, ::editor::IntervalId);

#[derive(Clone, Debug)]
pub struct OutlineRow {
    pub syntax: ::editor::SyntaxId,
    pub key: ::editor::IntervalId,
    pub depth: u8,
    pub title: String,

    pub line: u32,
}

pub struct OutlineEffect {
    document: ::editor::Document,
    stamp: (u64, u64),
}

impl imba::effect::Effect for OutlineEffect {
    type Result = OutlineRows;
}

#[derive(Clone)]
pub struct OutlineRows {
    pub rows: Vec<OutlineRow>,
    pub stamp: (u64, u64),
}

pub struct OutlineHandler;

impl imba::effect::EffectHandler<OutlineEffect> for OutlineHandler {
    async fn handle(&self, effect: OutlineEffect) -> OutlineRows {
        let items = effect.document.outline_items();
        let mut view = effect.document.text().view();
        let mut rows = Vec::with_capacity(items.len().min(OUTLINE_CAP));

        let mut enclosing: Vec<u32> = Vec::new();
        for (syntax, key, range, item) in items {
            if rows.len() >= OUTLINE_CAP {
                break;
            }
            while enclosing.last().is_some_and(|end| *end <= range.start) {
                enclosing.pop();
            }
            let depth = enclosing.len().min(u8::MAX as usize) as u8;
            enclosing.push(range.end);
            let line = crate::line_col_at(&mut view, range.start as usize).line + 1;
            rows.push(OutlineRow {
                syntax,
                key,
                depth,
                title: item.title,
                line,
            });
        }
        OutlineRows {
            rows,
            stamp: effect.stamp,
        }
    }
}

pub enum OutlineCommand {
    List(SearchListCommand),
    Dismiss,

    Refresh,

    Landed(OutlineRows),
}

pub struct OutlineView {
    window: crate::WindowId,
    document: crate::DocumentId,
    location: crate::ResourceLocation,
    rows: Vec<OutlineRow>,

    derived: Option<(u64, u64)>,

    launched: Option<(u64, u64)>,
    lane: Option<imba::effect::CancellationToken>,
    pub(crate) search:
        ListKeyboardController<ForestList<OutlineKey>, ForestSearcher<OutlineKey>>,
    request: Option<ModalRequest>,
}

impl Clone for OutlineView {
    fn clone(&self) -> Self {
        Self {
            window: self.window,
            document: self.document,
            location: self.location.clone(),
            rows: self.rows.clone(),
            derived: self.derived,
            launched: self.launched,
            lane: self.lane,
            search: self.search.clone(),
            request: None,
        }
    }
}

impl OutlineView {
    pub fn new(
        store: &Store,
        ui: &imba::UiCtx,
        window: crate::WindowId,
        document: crate::DocumentId,
        location: crate::ResourceLocation,
    ) -> Self {
        Self {
            window,
            document,
            location,
            rows: Vec::new(),
            derived: None,
            launched: None,
            lane: None,
            search: ListKeyboardController::searchable(
                ForestList::new(store),
                ForestSearcher::default(),
                store,
                ui,
                crate::env::Fonts::of(store),
            )
            .with_folds(),
            request: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String)> {
        self.rows
            .iter()
            .map(|row| (row.depth, row.title.clone()))
            .collect()
    }

    pub fn visible_rows(&self) -> usize {
        self.search.inner().list().len()
    }

    pub fn match_count(&self) -> usize {
        ListOps::match_count(self.search.inner().list())
    }

    pub fn cursor_title(&self) -> Option<String> {
        let key = self.search.inner().list().cursor()?;
        self.rows
            .iter()
            .find(|row| (row.syntax, row.key) == *key)
            .map(|row| row.title.clone())
    }

    fn stamp_of(document: &::editor::Document) -> (u64, u64) {
        (document.revision(), document.markup_generation())
    }

    fn relaunch(&mut self, store: &Store, fx: &mut imba::effect::Effects<'_, OutlineCommand>) {
        let Some(document) = crate::OpenDocuments::document_ref(store, self.document) else {
            return;
        };
        let stamp = Self::stamp_of(document);
        if self.launched == Some(stamp) || self.derived == Some(stamp) {
            return;
        }
        self.launched = Some(stamp);
        let effect = OutlineEffect {
            document: document.substance(),
            stamp,
        };
        fx.relaunch_erased(
            &mut self.lane,
            imba::effect::AnyEffect::new(effect).map(OutlineCommand::Landed),
        );
    }

    fn pick(&mut self, store: &Store, key: OutlineKey) {
        let Some(range) = crate::OpenDocuments::document_ref(store, self.document)
            .and_then(|document| document.resolve_outline(key.0, key.1))
        else {
            return;
        };
        self.request = Some(ModalRequest::Perform(crate::AppCommand::Dynamic(
            self.window,
            Arc::new(NavigateToPlace {
                place: crate::EditorPlace {
                    location: self.location.clone(),
                    caret: range.start,
                    scroll_y: 0.0,
                },
            }),
        )));
    }
}

impl View for OutlineView {
    type Command = OutlineCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, OutlineCommand> {
        use imba::focus::FocusData;
        // The key table is the controller's; the surface keeps only
        // its own dismissal.
        let searching = self.search.searching();
        let own = FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Escape if !searching => EventResult::Command(OutlineCommand::Dismiss),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        };
        own.merge_under(self.search.focus_data(store, ui).map(OutlineCommand::List))
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: OutlineCommand,
        fx: &mut imba::effect::Effects<'_, OutlineCommand>,
    ) {
        match command {
            OutlineCommand::List(command) => {
                match &command {
                    ListKeyCommand::Fold { expand, .. } => {
                        return self.search.inner_mut().fold_cursor(*expand, store, ui);
                    }
                    ListKeyCommand::Inner(inner) => {
                        if let Some(index) = tree_toggle(inner) {
                            let Some(key) = self.search.inner().list().key_at(index).copied()
                            else {
                                return;
                            };
                            self.search.inner_mut().list_mut().select_only(key);
                            return self.search.inner_mut().toggle(&key, store, ui);
                        }
                    }
                    _ => {}
                }
                type Search = ListKeyboardController<ForestList<OutlineKey>, ForestSearcher<OutlineKey>>;
                if let Some((index, trigger)) = Search::activated(&command) {
                    if let Some(key) = self.search.inner().list().key_at(index).copied() {
                        let searching = self.search.searching();
                        self.pick(store, key);
                        match trigger {
                            // The deliberate pick ends the search in
                            // the same stroke.
                            ActivateTrigger::Enter if searching => {
                                return self.perform(
                                    store,
                                    ui,
                                    OutlineCommand::List(ListKeyCommand::Clear),
                                    fx,
                                );
                            }
                            ActivateTrigger::Enter | ActivateTrigger::Click => {}
                        }
                    }
                }
                fx.scope(OutlineCommand::List, |fx| {
                    self.search.perform(store, ui, command, fx)
                });
            }
            OutlineCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
            OutlineCommand::Refresh => self.relaunch(store, fx),
            OutlineCommand::Landed(landed) => {
                if self.derived.is_some_and(|derived| derived == landed.stamp) {
                    return;
                }
                self.derived = Some(landed.stamp);
                self.lane = None;
                self.rows = landed.rows;
                let line_color = crate::env::Themes::of(store).ui().peeker.dim_text.0;

                let mut nodes: Vec<ForestNode<OutlineKey>> = Vec::new();
                let mut stack: Vec<(u8, ForestNode<OutlineKey>)> = Vec::new();
                let settle = |stack: &mut Vec<(u8, ForestNode<OutlineKey>)>,
                              nodes: &mut Vec<ForestNode<OutlineKey>>,
                              depth: u8| {
                    while stack.last().is_some_and(|(d, _)| *d >= depth) {
                        let (_, done) = stack.pop().expect("standing");
                        match stack.last_mut() {
                            Some((_, parent)) => parent.children.push(done),
                            None => nodes.push(done),
                        }
                    }
                };
                for row in &self.rows {
                    settle(&mut stack, &mut nodes, row.depth);
                    stack.push((
                        row.depth,
                        ForestNode {
                            key: (row.syntax, row.key),
                            label: row.title.clone(),
                            pick: true,
                            dim: false,
                            trail: vec![(row.line.to_string(), line_color)],
                            tint: crate::TreeTint::Label,
                            action: None,
                            children: Vec::new(),
                        },
                    ));
                }
                settle(&mut stack, &mut nodes, 0);

                self.search.inner_mut().set(&nodes, store, ui);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        {
            let theme_ui = crate::env::Themes::of(store).ui().clone();
            let title_font = crate::fonts::ui_font(ui, theme_ui.panel.title_size);
            let stale =
                crate::OpenDocuments::document_ref(store, self.document).is_some_and(|document| {
                    let stamp = Self::stamp_of(document);
                    self.derived != Some(stamp) && self.launched != Some(stamp)
                });
            let searching = self.search.searching();
            DrawerPanel {
                content: imba::LayoutBox::new(
                    arena,
                    self.search
                        .display(arena, store, ui)
                        .map_layout(OutlineCommand::List),
                ),
                theme: theme_ui,
                title_font,
                shaper: imba::TextShaper::of(ui),
                context: self.location.name().to_owned(),
                scaled_pad: false,
                keys: move |_arena: &Arena, event: &Event<'_>, _size: Size| match event {
                    Event::Paint { .. } if stale => EventResult::Command(OutlineCommand::Refresh),
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(OutlineCommand::Dismiss),
                    _ => EventResult::Ignored,
                },
            }
        }
    }
}

impl ModalView for OutlineView {
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

/// The drawer panel, REIFIED (docs/ui/UI.md stage 2): panel chrome
/// painted behind a shielded `DRAWER_WIDTH` surface, the content
/// padded inside it, a full-size key surface underneath. The content
/// pad depends on the incoming height, which is exactly why this is
/// a layout struct and not a `display`-time composition.
struct DrawerPanel<'a, Command, Keys> {
    content: imba::LayoutBox<'a, Command>,
    theme: ::editor::theme::UiTheme,
    title_font: skia_safe::Font,
    shaper: std::rc::Rc<imba::TextShaper>,
    context: String,
    /// The toc panel scales its content pad with the height; the
    /// outline panel uses a hairline.
    scaled_pad: bool,
    keys: Keys,
}

impl<Command, Keys> imba::LayoutValue for DrawerPanel<'_, Command, Keys> {}

impl<'a, Command: 'a, Keys> imba::Layout<'a, Command> for DrawerPanel<'a, Command, Keys>
where
    Keys: imba::EventHandler<Command> + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let size = constraints.max;
        let inset = crate::rows::panel_inset(&self.theme);
        let header = self.theme.panel.header_height;
        let (top, bottom) = match self.scaled_pad {
            true => {
                let pad = 8.0f32.min(size.height * 0.05);
                (inset + header + pad, inset + pad)
            }
            false => (inset + header, inset + 1.0),
        };
        let DrawerPanel {
            content,
            theme,
            title_font,
            shaper,
            context,
            keys,
            ..
        } = self;
        let panel = content
            .pad_insets(imba::Insets {
                left: inset + 1.0,
                top,
                right: inset + 1.0,
                bottom,
            })
            .backdrop(
                move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                    crate::rows::paint_panel_chrome(
                        &shaper,
                        canvas,
                        rect,
                        &theme,
                        &title_font,
                        "Contents",
                        &context,
                    );
                },
            )
            .shield()
            .sized(crate::DRAWER_WIDTH, size.height);
        imba::ZBox::new(arena)
            .child(imba::Fill::new().on_event(keys))
            .child(panel)
            .layout(arena, constraints)
    }
}
