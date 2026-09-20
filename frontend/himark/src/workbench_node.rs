// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::EditorCommand;

use crate::EditorIdView;
use imba::{
    arena::Arena,
    constraints::Constraints,
    scroll::{ScrollCommand, ScrollView},
    split::{Arrangement, Pane, SplitCommand, SplitView},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};

pub trait PanelView: imba::CloneDynView + Clone + Sized + 'static {
    type Place: crate::navigation::Place;

    fn title(&self, store: &Store) -> String;

    fn dismantle(&mut self, store: &mut Store);

    fn take_request(&mut self) -> Option<PanelRequest> {
        None
    }

    fn collapsed_height(&self, _store: &Store, _nominal_height: f32) -> Option<f32> {
        None
    }

    fn family_row(&self) -> Option<crate::FamilyRow> {
        None
    }

    fn as_any(&self) -> &dyn std::any::Any;

    fn full_bleed(&self) -> bool {
        false
    }

    fn navigation_location(&self, store: &Store) -> Option<Self::Place> {
        let _ = store;
        None
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &Self::Place,
        fx: &mut crate::app::AppFx<'_>,
    ) -> bool {
        let _ = (store, place, fx);
        false
    }

    fn drawer_view(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
    ) -> Option<Box<dyn crate::ModalView>> {
        let _ = (store, ui, window);
        None
    }
}

pub trait DynPanelView: imba::CloneDynView {
    fn clone_panel(&self) -> Box<dyn DynPanelView>;
    fn title(&self, store: &Store) -> String;
    fn dismantle(&mut self, store: &mut Store);
    fn take_request(&mut self) -> Option<PanelRequest>;
    fn family_row(&self) -> Option<crate::FamilyRow>;
    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn full_bleed(&self) -> bool;
    fn navigation_location_dyn(&self, store: &Store) -> Option<crate::NavigationLocation>;
    fn navigate_to_dyn(
        &mut self,
        store: &mut Store,
        location: &crate::NavigationLocation,
        fx: &mut crate::app::AppFx<'_>,
    ) -> bool;
    fn drawer_view_dyn(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
    ) -> Option<Box<dyn crate::ModalView>>;
}

impl<P: PanelView> DynPanelView for P {
    fn clone_panel(&self) -> Box<dyn DynPanelView> {
        Box::new(self.clone())
    }
    fn title(&self, store: &Store) -> String {
        PanelView::title(self, store)
    }
    fn dismantle(&mut self, store: &mut Store) {
        PanelView::dismantle(self, store)
    }
    fn take_request(&mut self) -> Option<PanelRequest> {
        PanelView::take_request(self)
    }
    fn family_row(&self) -> Option<crate::FamilyRow> {
        PanelView::family_row(self)
    }
    fn collapsed_height(&self, store: &Store, nominal_height: f32) -> Option<f32> {
        PanelView::collapsed_height(self, store, nominal_height)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        PanelView::as_any(self)
    }
    fn full_bleed(&self) -> bool {
        PanelView::full_bleed(self)
    }
    fn navigation_location_dyn(&self, store: &Store) -> Option<crate::NavigationLocation> {
        self.navigation_location(store)
            .map(crate::NavigationLocation::new)
    }
    fn navigate_to_dyn(
        &mut self,
        store: &mut Store,
        location: &crate::NavigationLocation,
        fx: &mut crate::app::AppFx<'_>,
    ) -> bool {
        match location.place::<P::Place>() {
            Some(place) => self.navigate_to(store, place, fx),
            None => false,
        }
    }
    fn drawer_view_dyn(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
    ) -> Option<Box<dyn crate::ModalView>> {
        self.drawer_view(store, ui, window)
    }
}

impl Clone for Box<dyn DynPanelView> {
    fn clone(&self) -> Self {
        self.clone_panel()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WidgetOrigin {
    Pane(usize),
    Family,
}

#[derive(Clone)]
pub enum PanelRequest {
    OpenLocations(Vec<crate::ResourceLocation>),

    Perform(std::sync::Arc<dyn crate::DynamicCommand>),
}

#[derive(Clone)]
pub(crate) struct ClosedPanel;

impl imba::View for ClosedPanel {
    type Command = imba::DynCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::Fill::new()
    }
}

impl PanelView for ClosedPanel {
    type Place = crate::navigation::NoPlace;

    fn title(&self, _store: &Store) -> String {
        String::new()
    }

    fn dismantle(&mut self, _store: &mut Store) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type EditorPane = ScrollView<EditorIdView>;
pub type PaneCommand = ScrollCommand<EditorCommand>;

pub enum Panel {
    Editor(EditorPane),

    Plugin(Box<dyn DynPanelView>),
}

impl Clone for Panel {
    fn clone(&self) -> Self {
        match self {
            Self::Editor(pane) => Self::Editor(pane.clone()),
            Self::Plugin(view) => Self::Plugin(view.clone_panel()),
        }
    }
}

pub enum PanelCommand {
    Editor(PaneCommand),
    Plugin(imba::DynCommand),

    Find(crate::find::FindCommand),

    Completion(crate::completion::CompletionFound),

    Hover(crate::hover::HoverFound),

    HoverTick(imba::anim::AnimationClock),
}

impl Panel {
    pub(crate) fn full_bleed(&self) -> bool {
        match self {
            Self::Editor(_) => false,
            Self::Plugin(view) => view.full_bleed(),
        }
    }

    pub fn editor(&self) -> Option<&EditorPane> {
        match self {
            Self::Editor(pane) => Some(pane),
            _ => None,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut EditorPane> {
        match self {
            Self::Editor(pane) => Some(pane),
            _ => None,
        }
    }

    pub fn dismantle(&mut self, store: &mut Store) {
        if let Self::Plugin(view) = self {
            view.dismantle(store);
        }
    }

    pub fn scroll_y(&self) -> f32 {
        match self {
            Self::Editor(pane) => pane.scroll_y(),
            Self::Plugin(_) => 0.0,
        }
    }

    pub fn set_scroll_y(&mut self, scroll_y: f32) {
        match self {
            Self::Editor(pane) => pane.set_scroll_y(scroll_y),
            Self::Plugin(_) => {}
        }
    }

    pub(crate) fn title(&self, store: &Store) -> String {
        match self {
            Self::Editor(pane) => {
                let document = pane.content().document();
                let name = crate::OpenDocuments::name(store, document)
                    .unwrap_or_else(|| "untitled".to_owned());
                // The unsaved mark rides the omnibox title.
                match crate::OpenDocuments::entity(store, document)
                    .is_some_and(|entity| entity.modified())
                {
                    true => format!("{name}*"),
                    false => name,
                }
            }
            Self::Plugin(view) => view.title(store),
        }
    }

    pub(crate) fn blank() -> Panel {
        Panel::Plugin(Box::new(ClosedPanel))
    }

    pub(crate) fn is_blank(&self) -> bool {
        match self {
            Self::Plugin(view) => view.as_any().is::<ClosedPanel>(),
            Self::Editor(_) => false,
        }
    }

    pub(crate) fn drawer_view(
        &self,
        store: &Store,
        ui: &UiCtx,
        window: crate::WindowId,
    ) -> Option<Box<dyn crate::ModalView>> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location = crate::OpenDocuments::location(store, view.document())?;

                let document = crate::OpenDocuments::document_ref(store, view.document())?;
                if !document.has_outline() {
                    return None;
                }
                Some(Box::new(crate::toc::OutlineView::new(
                    store,
                    ui,
                    window,
                    view.document(),
                    location,
                )) as Box<dyn crate::ModalView>)
            }
            Self::Plugin(view) => view.drawer_view_dyn(store, ui, window),
        }
    }

    pub(crate) fn navigation_location(&self, store: &Store) -> Option<crate::NavigationLocation> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location = crate::OpenDocuments::location(store, view.document())?;
                let document = crate::OpenDocuments::document_ref(store, view.document())?;

                if crate::is_scratch(&location) && document.revision() == 0 {
                    return None;
                }
                let caret = document.caret_byte(view.editor());
                Some(crate::NavigationLocation::new(crate::EditorPlace {
                    location,
                    caret,
                    scroll_y: pane.scroll_y(),
                }))
            }
            Self::Plugin(view) => view.navigation_location_dyn(store),
        }
    }

    pub(crate) fn navigate_to(
        &mut self,
        store: &mut Store,
        ui: &imba::UiCtx,
        target: &crate::NavigationLocation,
        fx: &mut crate::AppFx<'_>,
    ) -> bool {
        match self {
            Self::Editor(pane) => {
                let Some(place) = target.place::<crate::EditorPlace>() else {
                    return false;
                };
                let view = *pane.content();
                if crate::OpenDocuments::location(store, view.document()).as_ref()
                    != Some(&place.location)
                {
                    return false;
                }
                let Some(mut document) = crate::OpenDocuments::document(store, view.document())
                else {
                    return false;
                };
                let fonts = ::editor::env::Fonts::of(store)();
                let theme = ::editor::env::Themes::of(store);
                crate::app::entity_scope(view.document(), fx, |fx| {
                    document.reveal_at(view.editor(), place.caret, store, ui, &fonts, &theme, fx)
                });
                crate::OpenDocuments::put_document(store, view.document(), document);
                pane.set_scroll_y(place.scroll_y);
                true
            }
            Self::Plugin(view) => view.navigate_to_dyn(store, target, fx),
        }
    }

    fn placeholder(&self) -> Panel {
        match self {
            Self::Editor(pane) => Self::Editor(ScrollView::new(pane.content().clone())),
            Self::Plugin(view) => Self::Plugin(view.clone_panel()),
        }
    }
}

impl View for Panel {
    type Command = PanelCommand;

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match (self, command) {
            (Self::Editor(pane), PanelCommand::Editor(command)) => {
                let anchor = match &command {
                    ScrollCommand::Content(EditorCommand::Viewport { width, anchor, .. }) => {
                        let view = pane.content();
                        crate::OpenDocuments::document_ref(store, view.document())
                            .map(|document| {
                                (document.layout_width(view.editor()) - *width).abs() > 1.0
                            })
                            .unwrap_or(false)
                    }
                    .then_some(*anchor),
                    _ => None,
                };

                let cancels_reveal = matches!(&command, ScrollCommand::SetScrollY(_));
                fx.scope(PanelCommand::Editor, |fx| {
                    pane.perform(store, ui, command, fx)
                });
                if cancels_reveal {
                    {
                        let view = *pane.content();
                        if let Some(mut document) =
                            crate::OpenDocuments::document(store, view.document())
                        {
                            document.cancel_reveal(view.editor());
                            crate::OpenDocuments::put_document(store, view.document(), document);
                        }
                    }
                }
                if let Some(anchor) = anchor {
                    {
                        let view = *pane.content();
                        if let Some(document) =
                            crate::OpenDocuments::document_ref(store, view.document())
                        {
                            let target = document.height_before(view.editor(), anchor);
                            pane.set_scroll_y(target);
                        }
                    }
                }
            }
            (Self::Plugin(view), PanelCommand::Plugin(command)) => {
                fx.scope(PanelCommand::Plugin, |fx| {
                    view.as_mut().perform_dyn(store, ui, command, fx)
                });
            }

            _ => {}
        }
    }

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PanelCommand> {
        match self {
            Self::Editor(pane) => pane.focus_data(store, ui).map(PanelCommand::Editor),
            Self::Plugin(view) => view
                .as_ref()
                .focus_data_dyn(store, ui)
                .map(PanelCommand::Plugin),
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let content: imba::ThunkBox<'a, PanelCommand> = match self {
                Self::Editor(pane) => imba::ThunkBox::new(
                    arena,
                    imba::Layout::layout(pane.display(arena, store, ui), arena, constraints)
                        .map(PanelCommand::Editor)
                        .overlay_host(editor::sticky::HOST)
                        .overlay_host(editor::scroll_stripe::HOST),
                ),
                Self::Plugin(view) => imba::ThunkBox::new(
                    arena,
                    view.layout_dyn(arena, store, ui, constraints)
                        .map(PanelCommand::Plugin),
                ),
            };
            let mut panel = imba::container::container(arena, constraints.max);
            panel.place_boxed(0.0, 0.0, content);
            panel
        })
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum WorkbenchNode {
    Leaf(PaneSlot),
    Split(Box<SplitView<WorkbenchNode, WorkbenchNode>>),
}

impl WorkbenchNode {
    pub(crate) fn full_bleed(&self) -> bool {
        match self {
            Self::Leaf(slot) => slot.panel.full_bleed(),
            Self::Split(_) => false,
        }
    }
}

#[derive(Clone)]
pub struct PaneSlot {
    pub(crate) panel: Panel,
    pub(crate) back: rpds::VectorSync<crate::NavigationLocation>,
    pub(crate) forward: rpds::VectorSync<crate::NavigationLocation>,

    pub(crate) pending: Option<PendingWalk>,

    pub(crate) find: Option<crate::find::FindBar>,

    pub(crate) completion: crate::completion::Completion,

    pub(crate) hover: crate::hover::Hover,
}

#[derive(Clone)]
pub(crate) struct PendingWalk {
    pub(crate) target: crate::NavigationLocation,
    pub(crate) step: WalkStep,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WalkStep {
    Back,

    Forward,

    Replace,
}

impl PaneSlot {
    pub(crate) fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PanelCommand> {
        let panel = self.panel.focus_data(store, ui);
        match &self.find {
            // A focused find bar filters the keyboard before the
            // panel content; its input editor supplies the text seat.
            Some(find) if find.focused => find
                .focus_data(store, ui)
                .map(PanelCommand::Find)
                .merge_over(panel),
            _ => panel,
        }
    }

    pub(crate) fn of(panel: Panel) -> Self {
        Self {
            panel,
            back: rpds::VectorSync::new_sync(),
            forward: rpds::VectorSync::new_sync(),
            pending: None,
            find: None,
            completion: crate::completion::Completion::new(),
            hover: crate::hover::Hover::new(),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn history_depths(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    pub(crate) fn find_target(&self) -> Option<(crate::DocumentId, ::editor::EditorId)> {
        self.panel
            .editor()
            .map(|pane| (pane.content().document(), pane.content().editor()))
    }

    pub(crate) fn sync_find(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let Some(find) = &mut self.find else {
            return;
        };
        let target = self
            .panel
            .editor()
            .map(|pane| (pane.content().document(), pane.content().editor()));

        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        fx.scope(
            |command| PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command)),
            |fx| find.sync(store, target, ui, &fonts, &theme, fx),
        );

        let Some(find) = &mut self.find else {
            return;
        };
        fx.scope(PanelCommand::Find, |fx| {
            find.launch(store, target, fx, crate::find::FindCommand::Scanned)
        });
    }

    fn completion_editor(command: ::editor::EditorCommand) -> PanelCommand {
        PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
    }

    pub(crate) fn intercept_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: PanelCommand,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) -> Option<PanelCommand> {
        use imba::scroll::ScrollCommand;
        let PanelCommand::Editor(ScrollCommand::Content(::editor::EditorCommand::Inlay {
            key,
            command: inlay,
        })) = command
        else {
            return Some(command);
        };
        let rewrap = |inlay| {
            PanelCommand::Editor(ScrollCommand::Content(::editor::EditorCommand::Inlay {
                key,
                command: inlay,
            }))
        };
        if Some(key) != self.completion.inlay_key() {
            return Some(rewrap(inlay));
        }
        let popup = match inlay.downcast::<crate::completion::CompletionCommand>() {
            Ok(popup) => *popup,
            Err(other) => return Some(rewrap(other)),
        };
        use crate::completion::CompletionCommand;
        let Some((id, editor)) = self.completion.installed() else {
            return None;
        };
        let Some(mut document) = crate::OpenDocuments::document(store, id) else {
            self.completion.clear();
            return None;
        };
        match popup {
            CompletionCommand::Select(delta) => {
                self.completion.select(store, &mut document, editor, delta);
            }
            CompletionCommand::PickCursor => {
                let row = self.completion.selected();
                let _ = self.completion.apply_pick(
                    store,
                    ui,
                    &mut document,
                    editor,
                    row,
                    fx,
                    Self::completion_editor,
                );
            }
            CompletionCommand::Rows(rows) => {
                let picked = self
                    .completion
                    .rows_command(store, ui, &mut document, editor, rows);
                if let Some(row) = picked {
                    let _ = self.completion.apply_pick(
                        store,
                        ui,
                        &mut document,
                        editor,
                        row,
                        fx,
                        Self::completion_editor,
                    );
                }
            }
            CompletionCommand::Close => {
                self.completion
                    .drop_state(&mut document, store, ui, fx, Self::completion_editor);
            }
        }
        crate::OpenDocuments::put_document(store, id, document);
        None
    }

    pub(crate) fn sync_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        inserted: Option<&str>,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let target = self.find_target();

        if let Some(installed) = self.completion.installed() {
            if target != Some(installed) {
                match crate::OpenDocuments::document(store, installed.0) {
                    Some(mut old) => {
                        self.completion.drop_state(
                            &mut old,
                            store,
                            ui,
                            fx,
                            Self::completion_editor,
                        );
                        crate::OpenDocuments::put_document(store, installed.0, old);
                    }
                    None => self.completion.clear(),
                }
            }
        }
        let Some((id, editor)) = target else { return };
        if !self.completion.open() && inserted.is_none() {
            return;
        }
        let Some(mut document) = crate::OpenDocuments::document(store, id) else {
            return;
        };

        let markdown = document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
        if markdown {
            let Some(session) = crate::Gathered::scope(store).cloned() else {
                crate::OpenDocuments::put_document(store, id, document);
                return;
            };
            let typed_at =
                (inserted == Some("@")).then(|| document.caret_byte(editor).saturating_sub(1));
            self.completion.sync_path(
                store,
                ui,
                &mut document,
                editor,
                typed_at,
                &session,
                Some((id, editor)),
                fx,
                PanelCommand::Completion,
                Self::completion_editor,
            );
        } else {
            match crate::OpenDocuments::location(store, id) {
                Some(location) if !location.is_synthetic() => {
                    self.completion.sync_lsp(
                        store,
                        ui,
                        &mut document,
                        editor,
                        inserted,
                        false,
                        &location,
                        Some((id, editor)),
                        fx,
                        PanelCommand::Completion,
                        Self::completion_editor,
                    );
                }
                _ if self.completion.open() => {
                    self.completion.drop_state(
                        &mut document,
                        store,
                        ui,
                        fx,
                        Self::completion_editor,
                    );
                }
                _ => {}
            }
        }
        crate::OpenDocuments::put_document(store, id, document);
    }

    pub(crate) fn land_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        found: crate::completion::CompletionFound,
        _fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let _ = ui;
        let Some((id, editor)) = self.completion.installed() else {
            return;
        };
        let Some(mut document) = crate::OpenDocuments::document(store, id) else {
            self.completion.clear();
            return;
        };
        self.completion
            .land(store, ui, &mut document, editor, found);
        crate::OpenDocuments::put_document(store, id, document);
    }

    pub(crate) fn sync_hover(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        point: Option<skia_safe::Point>,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let target = self.find_target();

        if let Some(installed) = self.hover.installed() {
            if target != Some(installed) {
                match crate::OpenDocuments::document(store, installed.0) {
                    Some(mut old) => {
                        self.hover
                            .retract(store, ui, &mut old, fx, Self::completion_editor);
                        crate::OpenDocuments::put_document(store, installed.0, old);
                    }
                    None => self.hover.clear(),
                }
            }
        }
        let Some((id, editor)) = target else { return };

        let Some(point) = point else {
            if self.hover.open() {
                if let Some(mut document) = crate::OpenDocuments::document(store, id) {
                    self.hover
                        .retract(store, ui, &mut document, fx, Self::completion_editor);
                    crate::OpenDocuments::put_document(store, id, document);
                }
            }
            return;
        };

        let Some(location) = crate::OpenDocuments::location(store, id) else {
            return;
        };
        if location.is_synthetic() {
            return;
        }
        let Some(mut document) = crate::OpenDocuments::document(store, id) else {
            return;
        };
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        let byte = document.byte_at_point(editor, point.x, point.y, store, ui, &fonts, &theme);
        self.hover.sync(
            store,
            ui,
            &mut document,
            editor,
            byte,
            &location,
            Some((id, editor)),
            fx,
            Self::completion_editor,
        );
        crate::OpenDocuments::put_document(store, id, document);
    }

    pub(crate) fn tick_hover(
        &mut self,
        store: &mut Store,
        now: imba::anim::AnimationClock,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let Some((id, _editor)) = self.hover.installed() else {
            return;
        };

        let Some(document) = crate::OpenDocuments::document_ref(store, id) else {
            self.hover.clear();
            return;
        };
        self.hover.tick(document, now, fx, PanelCommand::Hover);
    }

    pub(crate) fn land_hover(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        found: crate::hover::HoverFound,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let Some((id, editor)) = self.hover.installed() else {
            return;
        };
        let Some(mut document) = crate::OpenDocuments::document(store, id) else {
            self.hover.clear();
            return;
        };
        self.hover.land(
            store,
            ui,
            &mut document,
            editor,
            found,
            fx,
            Self::completion_editor,
        );
        crate::OpenDocuments::put_document(store, id, document);
    }

    pub(crate) fn perform_find(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: crate::find::FindCommand,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        use crate::find::FindCommand;
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        match command {
            FindCommand::Input(command) => {
                if let Some(find) = &mut self.find {
                    fx.scope(PanelCommand::Find, |fx| {
                        find.perform_input(store, ui, command, fx)
                    });
                }
                self.sync_find(store, ui, fx);
            }
            FindCommand::Next | FindCommand::Previous => {
                let forward = matches!(command, FindCommand::Next);
                self.sync_find(store, ui, fx);
                if let Some(find) = &mut self.find {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.step(store, forward, ui, &fonts, &theme, fx),
                    );
                }
            }
            FindCommand::Close => {
                if let Some(mut find) = self.find.take() {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.uninstall(store, ui, &fonts, &theme, fx),
                    );
                }
            }
            FindCommand::Scanned(landed) => {
                let target = self.find_target();
                if let Some(find) = &mut self.find {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.adopt(store, target, &landed, ui, &fonts, &theme, fx),
                    );
                }
            }
        }
    }
}

pub enum NodeCommand {
    Leaf(PanelCommand),
    Split(Box<SplitCommand<NodeCommand, NodeCommand>>),
}

impl WorkbenchNode {
    pub fn editor_leaf(pane: EditorPane) -> Self {
        Self::Leaf(PaneSlot::of(Panel::Editor(pane)))
    }

    pub fn split_of(first: EditorPane, second: EditorPane, ratio: f32) -> Self {
        Self::Split(Box::new(
            SplitView::row(Self::editor_leaf(first), Self::editor_leaf(second)).with_ratio(ratio),
        ))
    }

    pub fn close_focused(&mut self) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split(split) => {
                let focused = split.focused();
                let focused_is_leaf = matches!(
                    match focused {
                        Pane::First => split.first(),
                        Pane::Second => split.second(),
                    },
                    Self::Leaf(_)
                );
                if !focused_is_leaf {
                    return match focused {
                        Pane::First => split.first_mut().close_focused(),
                        Pane::Second => split.second_mut().close_focused(),
                    };
                }

                let placeholder = Self::Leaf(PaneSlot::of(Panel::Plugin(Box::new(ClosedPanel))));
                let Self::Split(taken) = std::mem::replace(self, placeholder) else {
                    unreachable!("the enclosing match arm checked Split");
                };
                let (first, second) = taken.into_panes();
                *self = match focused {
                    Pane::First => second,
                    Pane::Second => first,
                };
                true
            }
        }
    }

    pub fn focused_pane(&self) -> &Panel {
        &self.focused_slot().panel
    }

    pub fn focused_pane_mut(&mut self) -> &mut Panel {
        &mut self.focused_slot_mut().panel
    }

    pub(crate) fn focused_slot(&self) -> &PaneSlot {
        match self {
            Self::Leaf(slot) => slot,
            Self::Split(split) => match split.focused() {
                Pane::First => split.first().focused_slot(),
                Pane::Second => split.second().focused_slot(),
            },
        }
    }

    pub(crate) fn focused_slot_mut(&mut self) -> &mut PaneSlot {
        match self {
            Self::Leaf(slot) => slot,
            Self::Split(split) => match split.focused() {
                Pane::First => split.first_mut().focused_slot_mut(),
                Pane::Second => split.second_mut().focused_slot_mut(),
            },
        }
    }

    pub fn for_each_pane(&self, visit: &mut impl FnMut(&Panel)) {
        match self {
            Self::Leaf(slot) => visit(&slot.panel),
            Self::Split(split) => {
                split.first().for_each_pane(visit);
                split.second().for_each_pane(visit);
            }
        }
    }

    pub(crate) fn for_each_slot_mut(&mut self, visit: &mut impl FnMut(&mut PaneSlot)) {
        match self {
            Self::Leaf(slot) => visit(slot),
            Self::Split(split) => {
                split.first_mut().for_each_slot_mut(visit);
                split.second_mut().for_each_slot_mut(visit);
            }
        }
    }

    pub fn for_each_pane_mut(&mut self, visit: &mut impl FnMut(&mut Panel)) {
        match self {
            Self::Leaf(slot) => visit(&mut slot.panel),
            Self::Split(split) => {
                split.first_mut().for_each_pane_mut(visit);
                split.second_mut().for_each_pane_mut(visit);
            }
        }
    }

    pub fn split_focused(&mut self, new_panel: Panel, arrangement: Arrangement, focus_new: bool) {
        match self {
            Self::Split(split) => match split.focused() {
                Pane::First => split
                    .first_mut()
                    .split_focused(new_panel, arrangement, focus_new),
                Pane::Second => split
                    .second_mut()
                    .split_focused(new_panel, arrangement, focus_new),
            },
            Self::Leaf(slot) => {
                let placeholder = new_panel.placeholder();
                let current = std::mem::replace(&mut slot.panel, placeholder);
                let history = (slot.back.clone(), slot.forward.clone());
                let mut current_slot = PaneSlot::of(current);
                current_slot.back = history.0.clone();
                current_slot.forward = history.1.clone();
                let mut new_slot = PaneSlot::of(new_panel);
                new_slot.back = history.0;
                new_slot.forward = history.1;
                let (first, second) = (Self::Leaf(current_slot), Self::Leaf(new_slot));
                let mut split = match arrangement {
                    Arrangement::Row => SplitView::row(first, second),
                    Arrangement::Column => SplitView::column(first, second),
                };
                if focus_new {
                    split.focus(Pane::Second);
                }
                *self = Self::Split(Box::new(split));
            }
        }
    }
}

impl View for WorkbenchNode {
    type Command = NodeCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, NodeCommand> {
        match self {
            Self::Leaf(slot) => slot.focus_data(store, ui).map(NodeCommand::Leaf),
            Self::Split(split) => split
                .focus_data(store, ui)
                .map(|command| NodeCommand::Split(Box::new(command))),
        }
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match (self, command) {
            (Self::Leaf(slot), NodeCommand::Leaf(PanelCommand::Find(command))) => fx
                .scope(NodeCommand::Leaf, |fx| {
                    slot.perform_find(store, ui, command, fx)
                }),
            (Self::Leaf(slot), NodeCommand::Leaf(PanelCommand::Completion(found))) => fx
                .scope(NodeCommand::Leaf, |fx| {
                    slot.land_completion(store, ui, found, fx)
                }),
            (Self::Leaf(slot), NodeCommand::Leaf(PanelCommand::Hover(found))) => fx
                .scope(NodeCommand::Leaf, |fx| {
                    slot.land_hover(store, ui, found, fx)
                }),

            (Self::Leaf(slot), NodeCommand::Leaf(PanelCommand::HoverTick(now))) => {
                fx.scope(NodeCommand::Leaf, |fx| slot.tick_hover(store, now, fx))
            }
            (Self::Leaf(slot), NodeCommand::Leaf(command)) => {
                if let (
                    Some(find),
                    PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                        ::editor::EditorCommand::Click { .. },
                    )),
                ) = (&mut slot.find, &command)
                {
                    find.focused = false;
                }

                if let PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                    ::editor::EditorCommand::Hover(point),
                )) = &command
                {
                    let point = *point;
                    return fx.scope(NodeCommand::Leaf, |fx| {
                        slot.sync_hover(store, ui, point, fx)
                    });
                }
                fx.scope(NodeCommand::Leaf, |fx| {
                    let Some(command) = slot.intercept_completion(store, ui, command, fx) else {
                        return;
                    };

                    let inserted = match &command {
                        PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                            ::editor::EditorCommand::InsertText { text },
                        )) => Some(text.clone()),
                        _ => None,
                    };
                    slot.panel.perform(store, ui, command, fx);
                    slot.sync_find(store, ui, fx);
                    slot.sync_completion(store, ui, inserted.as_deref(), fx);

                    slot.sync_hover(store, ui, None, fx);
                })
            }
            (Self::Split(split), NodeCommand::Split(command)) => fx.scope(
                |command| NodeCommand::Split(Box::new(command)),
                |fx| split.perform(store, ui, *command, fx),
            ),

            _ => {}
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let widget: imba::ThunkBox<'a, NodeCommand> = match self {
                Self::Leaf(slot) => match &slot.find {
                    None => imba::ThunkBox::new(
                        arena,
                        imba::Layout::layout(
                            slot.panel.display(arena, store, ui),
                            arena,
                            constraints,
                        )
                        .map(NodeCommand::Leaf),
                    ),

                    Some(find) => {
                        let size = constraints.max;
                        let chrome = ::editor::env::Themes::of(store).ui().search.clone();
                        let bar_height = crate::find::FindBar::height(&chrome).min(size.height);
                        let mut column = imba::container::container(arena, size);
                        column.place(
                            0.0,
                            bar_height,
                            imba::Layout::layout(
                                slot.panel.display(arena, store, ui),
                                arena,
                                Constraints::tight(skia_safe::Size::new(
                                    size.width,
                                    (size.height - bar_height).max(1.0),
                                )),
                            )
                            .map(NodeCommand::Leaf),
                        );
                        column.place(
                            0.0,
                            0.0,
                            find.layout(arena, store, ui, size.width)
                                .map(|command| NodeCommand::Leaf(PanelCommand::Find(command))),
                        );
                        imba::ThunkBox::new(arena, column)
                    }
                },

                Self::Split(split) => {
                    let divider = split.divider_rect(constraints.max);
                    let theme = ::editor::env::Themes::of(store);
                    let window = &theme.ui().window;
                    let color = window.divider.0;
                    let inset = window.divider_inset;
                    imba::ThunkBox::new(
                        arena,
                        imba::Layout::layout(split.display(arena, store, ui), arena, constraints)
                            .map(|command| NodeCommand::Split(Box::new(command)))
                            .paint_below(move |_arena, canvas, _| {
                                let mut paint = skia_safe::Paint::default();
                                paint.set_anti_alias(true);
                                paint.set_color(color);
                                canvas.draw_rect(divider.with_offset((-inset, 0.0)), &paint);
                            }),
                    )
                }
            };

            if matches!(self, Self::Leaf(slot) if slot.hover.armed()) {
                return imba::ThunkBox::new(
                    arena,
                    widget.event(|_arena, event, _size| match event {
                        imba::event::Event::AnimationClock { now } => {
                            imba::event::EventResult::Command(NodeCommand::Leaf(
                                PanelCommand::HoverTick(*now),
                            ))
                        }
                        _ => imba::event::EventResult::Ignored,
                    }),
                );
            }
            widget
        })
    }
}
