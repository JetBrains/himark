// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The split-diff PANE faces: `PairPane` (the embedded face a canvas
//! row wears) and `DiffPanelView` (the standalone workbench panel),
//! both reference views over store-held `DiffView` records. Moved in
//! from the dissolved hidiff plugin (docs/model-view.md stage C).

use documents::{EditorIdView, OpenDocuments};
use editor::{split_diff::DiffViewState, split_diff::SplitDiffCommand, unified_diff::UnifiedDiffCommand, unified_diff::UnifiedDiffView};
use imba::{arena::Arena, constraints::Constraints, scroll::ScrollView, store::Store, ui::UiCtx, View, Widget};

/// The pane holds IDS (docs/entities.md): the collection its pair
/// lives in, and the pair's key within it.
#[derive(Clone, Copy)]
pub struct PairPane {
    documents: imba::store::Id<documents::OpenDocuments>,
    id: documents::diffs::DiffViewId,
}

impl PairPane {
    pub fn over(
        documents: imba::store::Id<documents::OpenDocuments>,
        id: documents::diffs::DiffViewId,
    ) -> Self {
        Self { documents, id }
    }

    pub fn id(&self) -> documents::diffs::DiffViewId {
        self.id
    }

    pub fn documents(&self) -> imba::store::Id<documents::OpenDocuments> {
        self.documents
    }
}

fn gathered(
    pair: &documents::diffs::DiffView,
    store: &Store,
    documents: imba::store::Id<documents::OpenDocuments>,
) -> Option<UnifiedDiffView> {
    documents::diff_views::gather_diff_view(pair, store, documents)
}

pub fn gathered_view(
    store: &Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    id: documents::diffs::DiffViewId,
) -> Option<UnifiedDiffView> {
    gathered(
        documents::OpenDocuments::diff_view_ref(store, documents, id)?,
        store,
        documents,
    )
}

impl View for PairPane {
    type Command = UnifiedDiffCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, UnifiedDiffCommand> {
        pane_focus_data(self.documents, self.id, store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: UnifiedDiffCommand,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut pair) =
            documents::OpenDocuments::take_diff_view(store, self.documents, self.id)
        else {
            return;
        };
        let Some(mut view) = gathered(&pair, store, self.documents) else {
            documents::OpenDocuments::put_diff_view(store, self.documents, self.id, pair);
            return;
        };

        view.perform(store, ui, command, fx);

        OpenDocuments::put_document(
            store,
            self.documents,
            pair.left.document(),
            view.split.left.document,
        );
        OpenDocuments::put_document(
            store,
            self.documents,
            pair.right.document(),
            view.split.right.document,
        );
        pair.state = Some(view.split.state);
        documents::OpenDocuments::put_diff_view(store, self.documents, self.id, pair);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        // A real THUNK: the gathered view moves into the arena, and
        // `realize` lays it ONCE, bounded by the honest viewport —
        // the widget closes over the result and every later ask
        // (events, focus, keys, IME) is a read. The old shape was a
        // pre-built widget behind `eager` that re-laid the whole
        // split per ask and invented viewports for `focus_data`
        // (the DiffCanvas.trace lesson).
        imba::layout::laid(
            move |arena: &'a Arena, constraints: Constraints| GatheredThunk {
                view: documents::OpenDocuments::diff_view_ref(store, self.documents, self.id)
                    .and_then(|pair| gathered(pair, store, self.documents))
                    .map(|view| &*arena.alloc(view)),
                store,
                ui,
                arena,
                constraints,
                laid: std::cell::Cell::new(None),
            },
        )
    }
}

struct GatheredThunk<'a> {
    view: Option<&'a UnifiedDiffView>,
    store: &'a Store,
    ui: &'a UiCtx,
    arena: &'a Arena,
    constraints: Constraints,

    laid: std::cell::Cell<Option<skia_safe::Size>>,
}

impl<'a> imba::Thunk<'a, UnifiedDiffCommand> for GatheredThunk<'a> {
    fn size(&self) -> skia_safe::Size {
        if let Some(size) = self.laid.get() {
            return size;
        }
        let size = match self.view {
            Some(view) => {
                let inner = imba::layout::Layout::layout(
                    view.display(self.arena, self.store, self.ui),
                    self.arena,
                    self.constraints,
                );
                skia_safe::Size::new(self.constraints.max.width, imba::Thunk::size(&inner).height)
            }
            None => skia_safe::Size::default(),
        };
        self.laid.set(Some(size));
        size
    }

    fn realize(
        self,
        arena: &'a Arena,
        viewport: skia_safe::Rect,
    ) -> imba::WidgetBox<'a, UnifiedDiffCommand> {
        let inner = self.view.map(|view| {
            imba::layout::Layout::layout(
                view.display(self.arena, self.store, self.ui),
                self.arena,
                self.constraints,
            )
            .realize(arena, viewport)
        });
        imba::WidgetBox::new(
            arena,
            GatheredSplit {
                view: self.view,
                inner,
            },
        )
    }
}

/// The pane's semantic focus: the unified view is MINTED from the
/// store per ask (documents are persistent, the clone is cheap);
/// handlers re-mint per call. The standing "open in full" command
/// rides whichever side of the pair is focused.
fn pane_focus_data<'w>(
    documents: imba::store::Id<documents::OpenDocuments>,
    id: documents::diffs::DiffViewId,
    store: &'w Store,
    ui: &'w imba::ui::UiCtx,
) -> imba::focus::FocusData<'w, UnifiedDiffCommand> {
    use imba::event::EventResult;
    use imba::focus::FocusData;
    let mint = move || {
        documents::OpenDocuments::diff_view_ref(store, documents, id)
            .and_then(|pair| gathered(pair, store, documents))
    };
    let Some(view) = mint() else {
        return FocusData::default();
    };
    let (mut commands, location, seat) = {
        let mut data = view.focus_data(store, ui);
        (
            std::mem::take(&mut data.commands),
            data.location.take(),
            data.seat.take(),
        )
    };
    // Route by the ACTIVE face: in the inline face only the inline
    // editor is on screen — the split editors' focus flags can be
    // stale-true from before a face toggle, and checking them first
    // sent commands (cmd-enter's open-in-full among them) to an
    // editor whose caret was never placed.
    let wrap: Option<fn(editor::editor_view::EditorCommand) -> UnifiedDiffCommand> = match view.layout {
        editor::unified_diff::DiffLayout::Inline => {
            if view.inline_editor.is_some_and(|editor| {
                view.split.right.document.focus(editor) != editor::editor_view::EditorFocus::None
            }) {
                Some(UnifiedDiffCommand::Inline)
            } else {
                None
            }
        }
        editor::unified_diff::DiffLayout::Split => {
            if view.split.left.focus() != editor::editor_view::EditorFocus::None {
                Some(|command| UnifiedDiffCommand::Split(SplitDiffCommand::Left(command)))
            } else if view.split.right.focus() != editor::editor_view::EditorFocus::None {
                Some(|command| UnifiedDiffCommand::Split(SplitDiffCommand::Right(command)))
            } else {
                None
            }
        }
    };
    let injected = commands
        .iter()
        .any(|presentable| presentable.id == "workbench.open-in-full");
    if let (false, Some(wrap)) = (injected, wrap) {
        commands.push(imba::PresentableCommand::new(
            "workbench.open-in-full",
            "Open Working Copy",
            wrap(editor::editor_view::EditorCommand::Dynamic {
                id: "workbench.open-in-full",
                payload: None,
            }),
        ));
    }
    let with_view = move |f: &mut dyn FnMut(
        imba::focus::FocusData<'_, UnifiedDiffCommand>,
    ) -> EventResult<UnifiedDiffCommand>| {
        match mint() {
            Some(view) => f(view.focus_data(store, ui)),
            None => EventResult::Ignored,
        }
    };
    FocusData {
        commands,
        on_key: Some(Box::new(move |key, mods| {
            with_view(&mut |mut data| data.key(key, mods))
        })),
        on_text: Some(Box::new(move |text| {
            with_view(&mut |mut data| data.text(text))
        })),
        clipboard: Some(Box::new(move |visit| {
            with_view(&mut |mut data| match data.clipboard.as_mut() {
                Some(seat) => seat(visit),
                None => EventResult::Ignored,
            })
        })),
        location,
        seat,
    }
}

struct GatheredSplit<'a> {
    view: Option<&'a UnifiedDiffView>,
    inner: Option<imba::WidgetBox<'a, UnifiedDiffCommand>>,
}

impl<'a> Widget<'a, UnifiedDiffCommand> for GatheredSplit<'a> {
    fn size(&self) -> skia_safe::Size {
        self.inner
            .as_ref()
            .map(|inner| Widget::size(inner))
            .unwrap_or_default()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, UnifiedDiffCommand>> {
        self.inner
            .as_mut()
            .map(|inner| inner.overlays())
            .unwrap_or_default()
    }

    fn blocks_pointer(&self, point: skia_safe::Point) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.blocks_pointer(point))
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &imba::event::Event<'_>,
        viewport: skia_safe::Rect,
    ) -> imba::event::EventResult<UnifiedDiffCommand> {
        let (Some(_view), Some(inner)) = (self.view, &self.inner) else {
            return imba::event::EventResult::Ignored;
        };
        // No staleness probe here anymore: the batch-tail DRESSING
        // sweep (crate::diffs::sync_diff_dressing) resyncs a lagging
        // basis in the same batch that moved it — id-routed, no paint
        // (docs/model-view.md step 1).
        inner.handle_event(arena, event, viewport)
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, UnifiedDiffCommand>
    where
        'a: 'w,
    {
        match &mut self.inner {
            Some(inner) => inner.layout_data(target),
            None => imba::focus::LayoutData::default(),
        }
    }
}

#[derive(Clone)]
pub struct DiffPanelView {
    pane: ScrollView<PairPane>,
}

impl DiffPanelView {
    pub fn over(
        documents: imba::store::Id<documents::OpenDocuments>,
        id: documents::diffs::DiffViewId,
    ) -> Self {
        Self {
            pane: ScrollView::new(PairPane { documents, id }),
        }
    }

    pub fn pair(&self) -> documents::diffs::DiffViewId {
        self.pane.content().id
    }

    pub fn documents(&self) -> imba::store::Id<documents::OpenDocuments> {
        self.pane.content().documents
    }

    pub fn diff_state<'a>(&self, store: &'a Store) -> Option<&'a DiffViewState> {
        documents::OpenDocuments::diff_view_ref(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        )?
        .state
        .as_ref()
    }

    #[doc(hidden)]
    pub fn halves(&self, store: &Store) -> (EditorIdView, EditorIdView) {
        let pair = documents::OpenDocuments::diff_view_ref(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        )
        .expect("the pane's state row");
        (pair.left, pair.right)
    }

    pub fn new(
        store: &mut Store,
        left: EditorIdView,
        right: EditorIdView,
        handle: documents::diffs::DiffHandle,
        right_extras: editor::markup::MarkupId,
        state: Option<DiffViewState>,
    ) -> Self {
        let documents = left.documents();
        let id = documents::diffs::DiffViewId::mint();
        documents::OpenDocuments::put_diff_view(
            store,
            documents,
            id,
            documents::diffs::DiffView {
                left,
                right,
                diff: handle.id,
                right_extras,
                state,
                embedded: false,
            },
        );
        Self {
            pane: ScrollView::new(PairPane { documents, id }),
        }
    }
}

impl View for DiffPanelView {
    type Command = imba::scroll::ScrollCommand<UnifiedDiffCommand>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::ui::UiCtx,
    ) -> imba::focus::FocusData<'w, Self::Command> {
        self.pane.focus_data(store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        self.pane.perform(store, ui, command, fx)
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        self.pane.display(arena, store, ui)
    }
}

#[derive(Clone, PartialEq)]
pub struct DiffPlace {
    pub old: editor::location::ResourceLocation,
    pub new: editor::location::ResourceLocation,
}

impl hikit::Place for DiffPlace {}

impl hikit::PanelView for DiffPanelView {
    type Place = DiffPlace;

    fn pane_row(&self) -> Option<hikit::PaneRow> {
        let pane = self.pane.content();
        Some(hikit::PaneRow::new(crate::PairRow(
            pane.documents,
            pane.id,
        )))
    }

    fn navigation_location(&self, store: &Store) -> Option<DiffPlace> {
        let pair = documents::OpenDocuments::diff_view_ref(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        )?;
        Some(DiffPlace {
            old: OpenDocuments::location(store, pair.left.documents(), pair.left.document())?,
            new: OpenDocuments::location(store, pair.left.documents(), pair.right.document())?,
        })
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &DiffPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> bool {
        let Some(pair) = documents::OpenDocuments::diff_view_ref(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        ) else {
            return false;
        };
        let (left, right) = (pair.left, pair.right);
        OpenDocuments::location(store, left.documents(), left.document()).as_ref()
            == Some(&place.old)
            && OpenDocuments::location(store, right.documents(), right.document()).as_ref()
                == Some(&place.new)
    }

    fn title(&self, store: &Store) -> String {
        let named = documents::OpenDocuments::diff_view_ref(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        )
        .and_then(|pair| {
            OpenDocuments::location(store, pair.left.documents(), pair.right.document())
        });
        match named {
            Some(location) => format!("Diff: {}", location.name()),
            None => "Diff".to_owned(),
        }
    }

    fn dismantle(&mut self, store: &mut Store) {
        documents::diff_views::teardown_diff_view(
            store,
            self.pane.content().documents,
            self.pane.content().id,
        );
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn diff_panel(
    store: &mut Store,
    documents: imba::store::Id<documents::OpenDocuments>,
    ui: &imba::ui::UiCtx,
    left: documents::DocumentId,
    right: documents::DocumentId,
) -> Option<DiffPanelView> {
    let id = documents::diff_views::build_diff_view(
        store,
        documents,
        ui,
        left,
        right,
        documents::diff_views::OPEN_HALF_WIDTH,
        false,
    )?;
    Some(DiffPanelView::over(documents, id))
}

pub fn pair_row_minter() -> std::sync::Arc<hikit::RowMinter> {
    // The row carries its collection: the pane is minted off the ids
    // while the pair still stands. Canvases open through the
    // NAVIGATION road (CanvasNavigator) — reuse is a store lookup,
    // not a mint.
    std::sync::Arc::new(|store, row| {
        let crate::PairRow(documents, id) = *row.row::<crate::PairRow>()?;
        documents::OpenDocuments::diff_view_ref(store, documents, id)
            .map(|_| Box::new(DiffPanelView::over(documents, id)) as Box<dyn hikit::DynPanelView>)
    })
}
