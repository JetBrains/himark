// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The contents drawer: the TOC (a dir/file forest over a location
//! set) and the document outline (the syntax items, derived
//! off-thread per revision stamp). Windowless — a pick files a
//! ModalRequest; the outline's jump rides the injected place
//! builder; the shell's toggle and toolbar button live in himark.

use std::sync::Arc;

use imba::{arena::Arena, constraints::Constraints, event::{Event, EventResult, Key as InputKey}, store::Store, layout::LayoutExt as _, ui::UiCtx, View};
use skia_safe::Size;

use hikit::{
    tree_toggle, EditorPlace, ForestList, ForestNode, ForestSearcher, ListKeyCommand,
    ListKeyboardController, ModalRequest, ModalView, TreeListCommand,
};
use imba::list::{ActivateTrigger, ListOps};

pub(crate) const OUTLINE_CAP: usize = 2_000;

pub type SearchListCommand = ListKeyCommand<TreeListCommand>;

#[derive(Clone)]
pub enum TocCommand {
    List(SearchListCommand),
    Dismiss,
}

impl std::fmt::Display for TocCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TocCommand::List(command) => command.fmt(out),
            TocCommand::Dismiss => out.write_str("toc dismiss"),
        }
    }
}

pub struct TocView {
    context: String,
    search: ListKeyboardController<ForestList<u64>, ForestSearcher<u64>>,

    targets: rpds::HashTrieMapSync<u64, editor::location::ResourceLocation>,
    request: Option<ModalRequest>,
}

impl Clone for TocView {
    fn clone(&self) -> Self {
        Self {
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
    files: Vec<editor::location::ResourceLocation>,
}

impl DirTrie {
    fn emit(
        mut self,
        mut label: String,
        next: &mut u64,
        targets: &mut rpds::HashTrieMapSync<u64, editor::location::ResourceLocation>,
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
                tint: hikit::TreeTint::Label,
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
            tint: hikit::TreeTint::Label,
            action: None,
            children,
        }
    }
}

impl TocView {
    pub fn for_locations(
        store: &Store,
        ui: &UiCtx,
        locations: &[editor::location::ResourceLocation],
    ) -> Option<Self> {
        if locations.is_empty() {
            return None;
        }
        let mut sorted: Vec<&editor::location::ResourceLocation> = locations.iter().collect();
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
                tint: hikit::TreeTint::Label,
                action: None,
                children: Vec::new(),
            });
            targets.insert_mut(key, file);
        }

        let mut forest = ForestList::new(store);
        forest.set(&nodes, store, ui);
        Some(Self {
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
                editor::env::Fonts::of(store),
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

    /// The search controller — the shell's tests drive the key
    /// table through it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn search(&self) -> &ListKeyboardController<ForestList<u64>, ForestSearcher<u64>> {
        &self.search
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        {
            let theme_ui = editor::env::Themes::of(store).ui().clone();
            let title_font = hikit::fonts::ui_font(ui, theme_ui.panel.title_size);
            let searching = self.search.searching();
            DrawerPanel {
                content: imba::layout::LayoutBox::new(
                    arena,
                    self.search
                        .display(arena, store, ui)
                        .map_layout(TocCommand::List),
                ),
                theme: theme_ui,
                title_font,
                shaper: imba::layout::TextShaper::of(ui),
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

pub type OutlineKey = (::editor::markup::SyntaxId, ::editor::markup::IntervalId);

#[derive(Clone, Debug)]
pub struct OutlineRow {
    pub syntax: ::editor::markup::SyntaxId,
    pub key: ::editor::markup::IntervalId,
    pub depth: u8,
    pub title: String,

    pub line: u32,
}

pub struct OutlineEffect {
    document: ::editor::document::Document,
    stamp: (u64, u64),
}

impl std::fmt::Display for OutlineEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "outline revision {}", self.stamp.0)
    }
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
            let line = documents::line_col_at(&mut view, range.start as usize).line + 1;
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

#[derive(Clone)]
pub enum OutlineCommand {
    List(SearchListCommand),
    Dismiss,

    Refresh,

    Landed(OutlineRows),
}

impl std::fmt::Display for OutlineCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutlineCommand::List(command) => command.fmt(out),
            OutlineCommand::Dismiss => out.write_str("outline dismiss"),
            OutlineCommand::Refresh => out.write_str("outline refresh"),
            OutlineCommand::Landed(_) => out.write_str("outline landed"),
        }
    }
}

pub struct OutlineView {
    /// The jump-to-place request builder, injected at mount — the
    /// shell's window rides in the closure; the drawer never holds
    /// one.
    jump: Arc<dyn Fn(EditorPlace) -> ModalRequest + Send + Sync>,
    documents: imba::store::Id<documents::OpenDocuments>,
    document: documents::DocumentId,
    location: editor::location::ResourceLocation,
    rows: Vec<OutlineRow>,

    derived: Option<(u64, u64)>,

    launched: Option<(u64, u64)>,
    lane: Option<imba::effect::CancellationToken>,
    search: ListKeyboardController<ForestList<OutlineKey>, ForestSearcher<OutlineKey>>,
    request: Option<ModalRequest>,
}

impl Clone for OutlineView {
    fn clone(&self) -> Self {
        Self {
            jump: self.jump.clone(),
            documents: self.documents,
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
        ui: &imba::ui::UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        document: documents::DocumentId,
        location: editor::location::ResourceLocation,
        jump: Arc<dyn Fn(EditorPlace) -> ModalRequest + Send + Sync>,
    ) -> Self {
        Self {
            jump,
            documents,
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
                editor::env::Fonts::of(store),
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

    /// The search controller — the shell's tests drive the key
    /// table through it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn search(
        &self,
    ) -> &ListKeyboardController<ForestList<OutlineKey>, ForestSearcher<OutlineKey>> {
        &self.search
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

    fn stamp_of(document: &::editor::document::Document) -> (u64, u64) {
        (document.revision(), document.markup_generation())
    }

    fn relaunch(&mut self, store: &Store, fx: &mut imba::effect::Effects<'_, OutlineCommand>) {
        let Some(document) =
            documents::OpenDocuments::document_ref(store, self.documents, self.document)
        else {
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
        let Some(range) =
            documents::OpenDocuments::document_ref(store, self.documents, self.document)
                .and_then(|document| document.resolve_outline(key.0, key.1))
        else {
            return;
        };
        self.request = Some((self.jump)(EditorPlace {
            location: self.location.clone(),
            caret: range.start,
            scroll_y: 0.0,
        }));
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
                type Search =
                    ListKeyboardController<ForestList<OutlineKey>, ForestSearcher<OutlineKey>>;
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
                let line_color = editor::env::Themes::of(store).ui().peeker.dim_text.0;

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
                            tint: hikit::TreeTint::Label,
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        {
            let theme_ui = editor::env::Themes::of(store).ui().clone();
            let title_font = hikit::fonts::ui_font(ui, theme_ui.panel.title_size);
            let stale =
                documents::OpenDocuments::document_ref(store, self.documents, self.document)
                    .is_some_and(|document| {
                        let stamp = Self::stamp_of(document);
                        self.derived != Some(stamp) && self.launched != Some(stamp)
                    });
            let searching = self.search.searching();
            DrawerPanel {
                content: imba::layout::LayoutBox::new(
                    arena,
                    self.search
                        .display(arena, store, ui)
                        .map_layout(OutlineCommand::List),
                ),
                theme: theme_ui,
                title_font,
                shaper: imba::layout::TextShaper::of(ui),
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
    content: imba::layout::LayoutBox<'a, Command>,
    theme: ::editor::theme::UiTheme,
    title_font: skia_safe::Font,
    shaper: std::rc::Rc<imba::layout::TextShaper>,
    context: String,
    /// The toc panel scales its content pad with the height; the
    /// outline panel uses a hairline.
    scaled_pad: bool,
    keys: Keys,
}

impl<Command, Keys> imba::layout::LayoutValue for DrawerPanel<'_, Command, Keys> {}

impl<'a, Command: 'a, Keys> imba::layout::Layout<'a, Command> for DrawerPanel<'a, Command, Keys>
where
    Keys: imba::layout::EventHandler<Command> + 'a,
{
    fn layout(self, arena: &'a Arena, constraints: Constraints) -> imba::ThunkBox<'a, Command> {
        let size = constraints.max;
        let inset = hikit::rows::panel_inset(&self.theme);
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
            .pad_insets(imba::layout::Insets {
                left: inset + 1.0,
                top,
                right: inset + 1.0,
                bottom,
            })
            .backdrop(
                move |_arena: &Arena, canvas: &skia_safe::Canvas, rect: skia_safe::Rect| {
                    hikit::rows::paint_panel_chrome(
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
            .sized(hikit::rows::DRAWER_WIDTH, size.height);
        imba::layout::ZBox::new(arena)
            .child(imba::layout::Fill::new().on_event(keys))
            .child(panel)
            .layout(arena, constraints)
    }
}
