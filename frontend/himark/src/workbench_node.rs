// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use hikit::{panel::DynPanelView, panel::PanelRequest, panel::PanelView, panel::WidgetOrigin};

use editor::editor_view::EditorCommand;

use documents::entity_view::EditorIdView;
use imba::{arena::Arena, constraints::Constraints, scroll::{ScrollCommand, ScrollView}, split::{Arrangement, Pane, SplitCommand, SplitView}, store::Store, thunk_ext::ThunkExt, ui::UiCtx, View};


#[derive(Clone)]
pub(crate) struct ClosedPanel;

impl imba::View for ClosedPanel {
    type Command = imba::dyn_view::DynCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &imba::ui::UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a imba::arena::Arena,
        _store: &'a Store,
        _ui: &'a imba::ui::UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::Fill::new()
    }
}

impl PanelView for ClosedPanel {
    type Place = hikit::navigation::NoPlace;

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

#[derive(Clone)]
pub enum PanelCommand {
    Editor(PaneCommand),
    Plugin(imba::dyn_view::DynCommand),

    Find(crate::find::FindCommand),

    Completion(ahp_chat::completion::CompletionFound),

    Hover(documents::hover::HoverFound),

    HoverTick(imba::anim::AnimationClock),
}

impl std::fmt::Display for PanelCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelCommand::Editor(command) => command.fmt(out),
            PanelCommand::Plugin(command) => command.fmt(out),
            PanelCommand::Find(command) => command.fmt(out),
            PanelCommand::Completion(_) => out.write_str("completion found"),
            PanelCommand::Hover(_) => out.write_str("hover found"),
            PanelCommand::HoverTick(_) => out.write_str("hover tick"),
        }
    }
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

    /// The panel left its slot and is about to be dropped (see
    /// `PanelView::displaced`).
    pub fn displaced(&mut self, store: &mut Store) {
        if let Self::Plugin(view) = self {
            view.displaced(store);
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
                let documents = pane.content().documents();
                let document = pane.content().document();
                let name = documents::OpenDocuments::name(store, documents, document)
                    .unwrap_or_else(|| "untitled".to_owned());
                // The unsaved mark rides the omnibox title.
                match documents::OpenDocuments::entity(store, documents, document)
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
        window: crate::window::WindowId,
    ) -> Option<Box<dyn hikit::modal::ModalView>> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location =
                    documents::OpenDocuments::location(store, view.documents(), view.document())?;

                let document =
                    documents::OpenDocuments::document_ref(store, view.documents(), view.document())?;
                if !document.has_outline() {
                    return None;
                }
                let jump = std::sync::Arc::new(move |place| {
                    hikit::modal::ModalRequest::Perform(crate::app::shell_verb(crate::app::AppCommand::Dynamic(
                        window,
                        std::sync::Arc::new(crate::toc::NavigateToPlace { place }),
                    )))
                });
                Some(Box::new(toc::OutlineView::new(
                    store,
                    ui,
                    view.documents(),
                    view.document(),
                    location,
                    jump,
                )) as Box<dyn hikit::modal::ModalView>)
            }
            Self::Plugin(view) => view.drawer_view_dyn(store, ui),
        }
    }

    pub(crate) fn navigation_location(&self, store: &Store) -> Option<hikit::navigation::NavigationLocation> {
        match self {
            Self::Editor(pane) => {
                let view = pane.content();
                let location =
                    documents::OpenDocuments::location(store, view.documents(), view.document())?;
                let document =
                    documents::OpenDocuments::document_ref(store, view.documents(), view.document())?;

                if documents::is_scratch(&location) && document.revision() == 0 {
                    return None;
                }
                let caret = document.caret_byte(view.editor());
                Some(hikit::navigation::NavigationLocation::new(hikit::navigation::EditorPlace {
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
        ui: &imba::ui::UiCtx,
        target: &hikit::navigation::NavigationLocation,
        fx: &mut crate::app::AppFx<'_>,
    ) -> bool {
        match self {
            Self::Editor(pane) => {
                let Some(place) = target.place::<hikit::navigation::EditorPlace>() else {
                    return false;
                };
                let view = *pane.content();
                if documents::OpenDocuments::location(store, view.documents(), view.document()).as_ref()
                    != Some(&place.location)
                {
                    return false;
                }
                let Some(mut document) =
                    documents::OpenDocuments::document(store, view.documents(), view.document())
                else {
                    return false;
                };
                let fonts = ::editor::env::Fonts::of(store)();
                let theme = ::editor::env::Themes::of(store);
                let (documents, target) = (view.documents(), view.document());
                fx.scope(
                    move |command| {
                        crate::app::AppCommand::at(
                            documents,
                            documents::DocumentsCommand::Editor(target, command),
                        )
                    },
                    |fx| {
                        document.reveal_at(
                            view.editor(),
                            place.caret,
                            store,
                            ui,
                            &fonts,
                            &theme,
                            fx,
                        )
                    },
                );
                documents::OpenDocuments::put_document(
                    store,
                    view.documents(),
                    view.document(),
                    document,
                );
                pane.set_scroll_y(place.scroll_y);
                true
            }
            Self::Plugin(view) => fx.scope(crate::app::AppCommand::Verb, |fx| {
                view.navigate_to_dyn(store, target, fx)
            }),
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
                        documents::OpenDocuments::document_ref(store, view.documents(), view.document())
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
                            documents::OpenDocuments::document(store, view.documents(), view.document())
                        {
                            document.cancel_reveal(view.editor());
                            documents::OpenDocuments::put_document(
                                store,
                                view.documents(),
                                view.document(),
                                document,
                            );
                        }
                    }
                }
                if let Some(anchor) = anchor {
                    {
                        let view = *pane.content();
                        if let Some(document) = documents::OpenDocuments::document_ref(
                            store,
                            view.documents(),
                            view.document(),
                        ) {
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let content: imba::ThunkBox<'a, PanelCommand> = match self {
                Self::Editor(pane) => imba::ThunkBox::new(
                    arena,
                    imba::layout::Layout::layout(pane.display(arena, store, ui), arena, constraints)
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

/// A workbench-minted identity for the panel occupying a leaf. Effects a
/// panel launches are routed by leaf PATH, so an async result can land
/// after a swap put a different panel in that leaf; the id the command
/// was tagged with lets the delivery drop a stale command instead of
/// mis-delivering it to whoever now sits there (docs/editor/diff-canvas.md).
/// Minted fresh whenever a leaf's occupant is replaced; it rides the slot
/// (and so travels with the panel) when the tree reshapes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(u64);

impl PanelId {
    fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// A panel bundled with its workbench identity, so the two are always
/// replaced together (a `PaneSlot` outlives its occupants — its history
/// persists across navigations — so keeping `id` beside `panel` as
/// separate fields would risk them drifting out of sync). Derefs to the
/// `Panel` so read access reads through transparently; use `PaneSlot::
/// replace_panel` to swap the occupant, which mints a fresh id.
#[derive(Clone)]
pub struct PanelWithId {
    pub(crate) id: PanelId,
    pub(crate) panel: Panel,
}

impl PanelWithId {
    fn new(panel: Panel) -> Self {
        Self {
            id: PanelId::mint(),
            panel,
        }
    }
}

impl std::ops::Deref for PanelWithId {
    type Target = Panel;
    fn deref(&self) -> &Panel {
        &self.panel
    }
}

impl std::ops::DerefMut for PanelWithId {
    fn deref_mut(&mut self) -> &mut Panel {
        &mut self.panel
    }
}

#[derive(Clone)]
pub struct PaneSlot {
    pub(crate) panel: PanelWithId,
    pub(crate) back: rpds::VectorSync<hikit::navigation::NavigationLocation>,
    pub(crate) forward: rpds::VectorSync<hikit::navigation::NavigationLocation>,

    pub(crate) pending: Option<PendingWalk>,

    pub(crate) find: Option<crate::find::FindBar>,

    pub(crate) completion: ahp_chat::completion::Completion,

    pub(crate) hover: documents::hover::Hover,
}

#[derive(Clone)]
pub(crate) struct PendingWalk {
    pub(crate) target: hikit::navigation::NavigationLocation,
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
            // panel content; its input editor supplies the text client.
            Some(find) if find.focused => find
                .focus_data(store, ui)
                .map(PanelCommand::Find)
                .merge_over(panel),
            _ => panel,
        }
    }

    pub(crate) fn of(panel: Panel) -> Self {
        Self {
            panel: PanelWithId::new(panel),
            back: rpds::VectorSync::new_sync(),
            forward: rpds::VectorSync::new_sync(),
            pending: None,
            find: None,
            completion: ahp_chat::completion::Completion::new(),
            hover: documents::hover::Hover::new(),
        }
    }

    pub(crate) fn panel_id(&self) -> PanelId {
        self.panel.id
    }

    /// Swap in a new occupant, minting it a fresh `PanelId` so any effect
    /// still in flight for the departing panel is dropped on delivery
    /// rather than mis-routed to the newcomer. Returns the displaced panel.
    pub(crate) fn replace_panel(&mut self, panel: Panel) -> Panel {
        std::mem::replace(&mut self.panel, PanelWithId::new(panel)).panel
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn history_depths(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    pub(crate) fn find_target(&self) -> Option<(documents::DocumentId, ::editor::editor::EditorId)> {
        self.panel
            .editor()
            .map(|pane| (pane.content().document(), pane.content().editor()))
    }

    /// The collection this leaf's editor reads through — the pane
    /// holds the id (docs/entities.md law 3).
    pub(crate) fn documents_id(&self) -> Option<imba::store::Id<documents::OpenDocuments>> {
        self.panel.editor().map(|pane| pane.content().documents())
    }

    pub(crate) fn sync_find(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let target = self
            .panel
            .editor()
            .map(|pane| (pane.content().document(), pane.content().editor()));
        let target_documents = self.documents_id();
        let Some(find) = &mut self.find else {
            return;
        };
        let documents = match target_documents {
            Some(documents) => documents,
            None => return,
        };
        let fonts = ::editor::env::ui_collection(store, ui);
        let theme = ::editor::env::Themes::of(store);
        fx.scope(
            |command| PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command)),
            |fx| find.sync(store, documents, target, ui, &fonts, &theme, fx),
        );

        let Some(find) = &mut self.find else {
            return;
        };
        fx.scope(PanelCommand::Find, |fx| {
            find.launch(
                store,
                documents,
                target,
                fx,
                crate::find::FindCommand::Scanned,
            )
        });
    }

    fn completion_editor(command: ::editor::editor_view::EditorCommand) -> PanelCommand {
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
        let PanelCommand::Editor(ScrollCommand::Content(::editor::editor_view::EditorCommand::Inlay {
            key,
            command: inlay,
        })) = command
        else {
            return Some(command);
        };
        let rewrap = |inlay| {
            PanelCommand::Editor(ScrollCommand::Content(::editor::editor_view::EditorCommand::Inlay {
                key,
                command: inlay,
            }))
        };
        if Some(key) != self.completion.inlay_key() {
            return Some(rewrap(inlay));
        }
        let popup = match inlay.downcast_ref::<ahp_chat::completion::CompletionCommand>() {
            Some(_) => inlay
                .downcast::<ahp_chat::completion::CompletionCommand>()
                .expect("probed above"),
            None => return Some(rewrap(inlay)),
        };
        use ahp_chat::completion::CompletionCommand;
        let Some((id, editor)) = self.completion.installed() else {
            return None;
        };
        let Some(documents) = self.documents_id() else {
            return None;
        };
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
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
        documents::OpenDocuments::put_document(store, documents, id, document);
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
        let Some(documents) = self.documents_id() else {
            return;
        };

        if let Some(installed) = self.completion.installed() {
            if target != Some(installed) {
                match documents::OpenDocuments::document(store, documents, installed.0) {
                    Some(mut old) => {
                        self.completion.drop_state(
                            &mut old,
                            store,
                            ui,
                            fx,
                            Self::completion_editor,
                        );
                        documents::OpenDocuments::put_document(store, documents, installed.0, old);
                    }
                    None => self.completion.clear(),
                }
            }
        }
        let Some((id, editor)) = target else { return };
        if !self.completion.open() && inserted.is_none() {
            return;
        }
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
            return;
        };

        let markdown = document.syntax().map(|syntax| syntax.language.as_str()) == Some("markdown");
        if markdown {
            let Some((session, state)) = ahp_session::session::state::Hosts::home_of_documents(store, documents)
            else {
                documents::OpenDocuments::put_document(store, documents, id, document);
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
                std::sync::Arc::new(ahp_session::session::folders::session_folders(store, &session)),
                state.recents(),
                Some((id, editor)),
                fx,
                PanelCommand::Completion,
                Self::completion_editor,
            );
        } else {
            match documents::OpenDocuments::location(store, documents, id) {
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
        documents::OpenDocuments::put_document(store, documents, id, document);
    }

    pub(crate) fn land_completion(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        found: ahp_chat::completion::CompletionFound,
        _fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let _ = ui;
        let Some((id, editor)) = self.completion.installed() else {
            return;
        };
        let Some(documents) = self.documents_id() else {
            return;
        };
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
            self.completion.clear();
            return;
        };
        self.completion
            .land(store, ui, &mut document, editor, found);
        documents::OpenDocuments::put_document(store, documents, id, document);
    }

    pub(crate) fn sync_hover(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        point: Option<skia_safe::Point>,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let target = self.find_target();
        let Some(documents) = self.documents_id() else {
            return;
        };

        if let Some(installed) = self.hover.installed() {
            if target != Some(installed) {
                match documents::OpenDocuments::document(store, documents, installed.0) {
                    Some(mut old) => {
                        self.hover
                            .retract(store, ui, &mut old, fx, Self::completion_editor);
                        documents::OpenDocuments::put_document(store, documents, installed.0, old);
                    }
                    None => self.hover.clear(),
                }
            }
        }
        let Some((id, editor)) = target else { return };

        let Some(point) = point else {
            if self.hover.open() {
                if let Some(mut document) = documents::OpenDocuments::document(store, documents, id) {
                    self.hover
                        .retract(store, ui, &mut document, fx, Self::completion_editor);
                    documents::OpenDocuments::put_document(store, documents, id, document);
                }
            }
            return;
        };

        let Some(location) = documents::OpenDocuments::location(store, documents, id) else {
            return;
        };
        if location.is_synthetic() {
            return;
        }
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
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
        documents::OpenDocuments::put_document(store, documents, id, document);
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
        let Some(documents) = self.documents_id() else {
            return;
        };
        let Some(document) = documents::OpenDocuments::document_ref(store, documents, id) else {
            self.hover.clear();
            return;
        };
        self.hover.tick(document, now, fx, PanelCommand::Hover);
    }

    pub(crate) fn land_hover(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        found: documents::hover::HoverFound,
        fx: &mut imba::effect::Effects<'_, PanelCommand>,
    ) {
        let Some((id, editor)) = self.hover.installed() else {
            return;
        };
        let Some(documents) = self.documents_id() else {
            return;
        };
        let Some(mut document) = documents::OpenDocuments::document(store, documents, id) else {
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
        documents::OpenDocuments::put_document(store, documents, id, document);
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
                let Some(documents) = self.documents_id() else {
                    return;
                };
                if let Some(find) = &mut self.find {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.step(store, documents, forward, ui, &fonts, &theme, fx),
                    );
                }
            }
            FindCommand::Close => {
                let Some(documents) = self.documents_id() else {
                    return;
                };
                if let Some(mut find) = self.find.take() {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.uninstall(store, documents, ui, &fonts, &theme, fx),
                    );
                }
            }
            FindCommand::Scanned(landed) => {
                let target = self.find_target();
                let Some(documents) = self.documents_id() else {
                    return;
                };
                if let Some(find) = &mut self.find {
                    fx.scope(
                        |command| {
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(command))
                        },
                        |fx| find.adopt(store, documents, target, &landed, ui, &fonts, &theme, fx),
                    );
                }
            }
        }
    }
}

#[derive(Clone)]
pub enum NodeCommand {
    Leaf {
        /// The occupant this command was addressed to; a leaf drops it if
        /// its current occupant no longer carries this id (a stale route
        /// after a panel swap).
        target: PanelId,
        command: PanelCommand,
    },
    Split(Box<SplitCommand<NodeCommand, NodeCommand>>),
}

impl std::fmt::Display for NodeCommand {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeCommand::Leaf { command, .. } => command.fmt(out),
            NodeCommand::Split(command) => command.fmt(out),
        }
    }
}

/// Tag a leaf's outgoing commands with the occupant they belong to.
fn wrap_leaf(target: PanelId) -> impl Fn(PanelCommand) -> NodeCommand + Copy {
    move |command| NodeCommand::Leaf { target, command }
}

impl WorkbenchNode {
    pub fn editor_leaf(pane: EditorPane) -> Self {
        Self::Leaf(PaneSlot::of(Panel::Editor(pane)))
    }

    /// A tree with nothing open: the sole leaf holds the blank panel.
    pub fn vacant() -> Self {
        Self::Leaf(PaneSlot::of(Panel::blank()))
    }

    /// Nothing open — the chat column (when present) owns the whole
    /// workbench, and closing the last panel brings the chat back.
    pub fn is_vacant(&self) -> bool {
        matches!(self, Self::Leaf(slot) if slot.panel.panel.is_blank())
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
        &self.focused_slot().panel.panel
    }

    pub fn focused_pane_mut(&mut self) -> &mut Panel {
        &mut self.focused_slot_mut().panel.panel
    }

    /// Swap the focused leaf's occupant, minting it a fresh `PanelId`.
    pub(crate) fn replace_focused_panel(&mut self, panel: Panel) -> Panel {
        self.focused_slot_mut().replace_panel(panel)
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
            Self::Leaf(slot) => visit(&slot.panel.panel),
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
            Self::Leaf(slot) => visit(&mut slot.panel.panel),
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
                let current = std::mem::replace(&mut slot.panel, PanelWithId::new(placeholder));
                let history = (slot.back.clone(), slot.forward.clone());
                let mut current_slot = PaneSlot::of(current.panel);
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
            Self::Leaf(slot) => slot.focus_data(store, ui).map(wrap_leaf(slot.panel_id())),
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
            (Self::Leaf(slot), NodeCommand::Leaf { target, command }) => {
                // Stale route: an async result addressed to a panel that
                // has since left this leaf (a swap put another there).
                // Drop it — its panel, if still alive elsewhere, re-derives.
                if slot.panel_id() != target {
                    return;
                }
                let wrap = wrap_leaf(target);
                match command {
                    PanelCommand::Find(command) => {
                        fx.scope(wrap, |fx| slot.perform_find(store, ui, command, fx))
                    }
                    PanelCommand::Completion(found) => {
                        fx.scope(wrap, |fx| slot.land_completion(store, ui, found, fx))
                    }
                    PanelCommand::Hover(found) => {
                        fx.scope(wrap, |fx| slot.land_hover(store, ui, found, fx))
                    }
                    PanelCommand::HoverTick(now) => {
                        fx.scope(wrap, |fx| slot.tick_hover(store, now, fx))
                    }
                    command => {
                        if let (
                            Some(find),
                            PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                                ::editor::editor_view::EditorCommand::Click { .. },
                            )),
                        ) = (&mut slot.find, &command)
                        {
                            find.focused = false;
                        }

                        if let PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                            ::editor::editor_view::EditorCommand::Hover(point),
                        )) = &command
                        {
                            let point = *point;
                            return fx.scope(wrap, |fx| slot.sync_hover(store, ui, point, fx));
                        }
                        fx.scope(wrap, |fx| {
                            let Some(command) = slot.intercept_completion(store, ui, command, fx)
                            else {
                                return;
                            };

                            let inserted = match &command {
                                PanelCommand::Editor(imba::scroll::ScrollCommand::Content(
                                    ::editor::editor_view::EditorCommand::InsertText { text },
                                )) => Some(text.clone()),
                                _ => None,
                            };
                            slot.panel.perform(store, ui, command, fx);
                            slot.sync_find(store, ui, fx);
                            slot.sync_completion(store, ui, inserted.as_deref(), fx);

                            slot.sync_hover(store, ui, None, fx);
                        })
                    }
                }
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
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let widget: imba::ThunkBox<'a, NodeCommand> = match self {
                Self::Leaf(slot) => match &slot.find {
                    None => imba::ThunkBox::new(
                        arena,
                        imba::layout::Layout::layout(
                            slot.panel.display(arena, store, ui),
                            arena,
                            constraints,
                        )
                        .map(wrap_leaf(slot.panel_id())),
                    ),

                    Some(find) => {
                        let leaf = wrap_leaf(slot.panel_id());
                        let size = constraints.max;
                        let chrome = ::editor::env::Themes::of(store).ui().search.clone();
                        let bar_height = crate::find::FindBar::height(&chrome).min(size.height);
                        let mut column = imba::container::container(arena, size);
                        column.place(
                            0.0,
                            bar_height,
                            imba::layout::Layout::layout(
                                slot.panel.display(arena, store, ui),
                                arena,
                                Constraints::tight(skia_safe::Size::new(
                                    size.width,
                                    (size.height - bar_height).max(1.0),
                                )),
                            )
                            .map(leaf),
                        );
                        column.place(
                            0.0,
                            0.0,
                            find.layout(arena, store, ui, size.width)
                                .map(move |command| leaf(PanelCommand::Find(command))),
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
                        imba::layout::Layout::layout(split.display(arena, store, ui), arena, constraints)
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

            if let Self::Leaf(slot) = self {
                if slot.hover.armed() {
                    let leaf = wrap_leaf(slot.panel_id());
                    return imba::ThunkBox::new(
                        arena,
                        widget.event(move |_arena, event, _size| match event {
                            imba::event::Event::AnimationClock { now } => {
                                imba::event::EventResult::Command(leaf(PanelCommand::HoverTick(
                                    *now,
                                )))
                            }
                            _ => imba::event::EventResult::Ignored,
                        }),
                    );
                }
            }
            widget
        })
    }
}
