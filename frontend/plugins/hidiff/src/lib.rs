// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use himark::{
    Application, DiffState, EditorIdView, EditorView, OpenDocuments, SplitDiffCommand,
    SplitDiffView, UnifiedDiffCommand, UnifiedDiffView,
};
use imba::{
    arena::Arena, constraints::Constraints, scroll::ScrollView, store::Store, UiCtx, View, Widget,
};

const OPEN_HALF_WIDTH: f32 = 420.0;

pub mod canvas;
pub use canvas::{canvas_sync_observer, CanvasNavigator, Canvases, DiffCanvasView};

#[derive(Clone, Copy)]
pub struct PairPane {
    id: himark::DiffViewId,
}

impl PairPane {
    pub fn over(id: himark::DiffViewId) -> Self {
        Self { id }
    }

    pub fn id(&self) -> himark::DiffViewId {
        self.id
    }
}

fn gathered(pair: &himark::DiffView, store: &Store) -> Option<UnifiedDiffView> {
    {
        let left = pair.left;
        let right = pair.right;
        let left_view = EditorView {
            document: OpenDocuments::document(store, left.document())?,
            editor: left.editor(),
            reports_geometry: true,

            location: OpenDocuments::location(store, left.document()),
            gutter_width: 0.0,
            base: None,
        };
        let right_view = EditorView {
            document: OpenDocuments::document(store, right.document())?,
            editor: right.editor(),
            reports_geometry: true,
            location: OpenDocuments::location(store, right.document()),
            gutter_width: 0.0,
            base: None,
        };
        let state = match &pair.state {
            Some(state) => state.clone(),

            None => DiffState::attach(
                pair.diff,
                &left_view.document,
                &right_view.document,
                OpenDocuments::diff_handle(store, pair.diff)?.base_markup,
                pair.right_extras,
                None,
                himark::env::Differ::of(store),
            )?,
        };
        Some(UnifiedDiffView::new(SplitDiffView::new(
            left_view, right_view, state,
        )))
    }
}

/// Reconstruct a tracked pair's `UnifiedDiffView` from the store — the
/// read side of the registered-diff mechanism (documents live in
/// `OpenDocuments`, the view is gathered per ask). Used by the diff
/// canvas rows for layout/probe reads.
pub fn gathered_view(store: &Store, id: himark::DiffViewId) -> Option<UnifiedDiffView> {
    gathered(himark::OpenDocuments::diff_view_ref(store, id)?, store)
}

/// Tear down a tracked pair: drop the store-held `DiffView`, remove the
/// pair's editors from the (possibly shared) registered documents, and
/// untrack the diff from the Diffs subsystem. Does NOT close the
/// documents — they may be open elsewhere. Shared by `DiffPanelView`
/// and the diff canvas.
pub fn teardown_diff_view(store: &mut Store, id: himark::DiffViewId) {
    // Teardown-only road (dismantle/destroy/retire carry no UiCtx);
    // the release may reshape a surviving base document's markup once.
    let ui = &imba::UiCtx::dont_use_too_slow();
    let Some(pair) = himark::OpenDocuments::take_diff_view(store, id) else {
        return;
    };
    if let Some(inline) = pair.state.as_ref().and_then(|state| state.inline_editor()) {
        if let Some(mut document) = OpenDocuments::document(store, pair.right.document()) {
            document.remove_editor(inline);
            OpenDocuments::put_document(store, pair.right.document(), document);
        }
    }
    for entity in [pair.left, pair.right] {
        if let Some(mut document) = OpenDocuments::document(store, entity.document()) {
            document.remove_editor(entity.editor());
            OpenDocuments::put_document(store, entity.document(), document);
        }
    }
    himark::OpenDocuments::untrack_diff(
        store, ui,
        pair.diff,
        &mut imba::effect::Batch::<UnifiedDiffCommand>::new().effects(),
    );
}

/// Re-wrap the inline face of a tracked pair to `width` (the row-level
/// rewrap, docs/editor/diff-canvas.md §4): gather, resize the half + inline
/// editors on the registered documents, resync, write back. The split
/// face owns its half widths and is left alone.
pub fn rewrap_pair(
    store: &mut Store,
    ui: &UiCtx,
    id: himark::DiffViewId,
    width: f32,
    fx: &mut imba::effect::Effects<'_, UnifiedDiffCommand>,
) {
    let Some(mut pair) = himark::OpenDocuments::take_diff_view(store, id) else {
        return;
    };
    let Some(mut view) = gathered(&pair, store) else {
        himark::OpenDocuments::put_diff_view(store, id, pair);
        return;
    };
    if view.layout == himark::DiffLayout::Split {
        himark::OpenDocuments::put_diff_view(store, id, pair);
        return;
    }
    let fonts = himark::env::Fonts::of(store)();
    let theme = himark::env::Themes::of(store);
    let left_editor = view.split.left.editor;
    let right_editor = view.split.right.editor;
    let inline = view.inline_editor;
    fx.scope(
        |c: himark::EditorCommand| UnifiedDiffCommand::Split(himark::SplitDiffCommand::Left(c)),
        |fx| {
            view.split
                .left
                .document
                .resize(left_editor, width, 0,
                store, ui, &fonts, &theme, fx)
        },
    );
    fx.scope(
        |c: himark::EditorCommand| UnifiedDiffCommand::Split(himark::SplitDiffCommand::Right(c)),
        |fx| {
            view.split
                .right
                .document
                .resize(right_editor, width, 0,
                store, ui, &fonts, &theme, fx)
        },
    );
    if let Some(inline) = inline {
        fx.scope(
            |c: himark::EditorCommand| UnifiedDiffCommand::Inline(c),
            |fx| {
                view.split
                    .right
                    .document
                    .resize(inline, width, 0,
                store, ui, &fonts, &theme, fx)
            },
        );
    }
    view.perform(
        store,
        ui,
        UnifiedDiffCommand::Split(himark::SplitDiffCommand::Resync),
        fx,
    );
    OpenDocuments::put_document(store, pair.left.document(), view.split.left.document);
    OpenDocuments::put_document(store, pair.right.document(), view.split.right.document);
    pair.state = Some(view.split.state);
    himark::OpenDocuments::put_diff_view(store, id, pair);
}

impl View for PairPane {
    type Command = UnifiedDiffCommand;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
    ) -> imba::focus::FocusData<'w, UnifiedDiffCommand> {
        pane_focus_data(self.id, store, ui)
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: UnifiedDiffCommand,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        let Some(mut pair) = himark::OpenDocuments::take_diff_view(store, self.id) else {
            return;
        };
        let Some(mut view) = gathered(&pair, store) else {
            himark::OpenDocuments::put_diff_view(store, self.id, pair);
            return;
        };

        view.perform(store, ui, command, fx);

        OpenDocuments::put_document(store, pair.left.document(), view.split.left.document);
        OpenDocuments::put_document(store, pair.right.document(), view.split.right.document);
        pair.state = Some(view.split.state);
        himark::OpenDocuments::put_diff_view(store, self.id, pair);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        // A real THUNK: the gathered view moves into the arena, and
        // `realize` lays it ONCE, bounded by the honest viewport —
        // the widget closes over the result and every later ask
        // (events, focus, keys, IME) is a read. The old shape was a
        // pre-built widget behind `eager` that re-laid the whole
        // split per ask and invented viewports for `focus_data`
        // (the DiffCanvas.trace lesson).
        imba::laid(
            move |arena: &'a Arena, constraints: Constraints| GatheredThunk {
                view: himark::OpenDocuments::diff_view_ref(store, self.id)
                    .and_then(|pair| gathered(pair, store))
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
                let inner = imba::Layout::layout(
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
            imba::Layout::layout(
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
    id: himark::DiffViewId,
    store: &'w Store,
    ui: &'w imba::UiCtx,
) -> imba::focus::FocusData<'w, UnifiedDiffCommand> {
    use imba::event::EventResult;
    use imba::focus::FocusData;
    let mint = move || {
        himark::OpenDocuments::diff_view_ref(store, id).and_then(|pair| gathered(pair, store))
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
    let wrap: Option<fn(himark::EditorCommand) -> UnifiedDiffCommand> = match view.layout {
        himark::DiffLayout::Inline => {
            if view.inline_editor.is_some_and(|editor| {
                view.split.right.document.focus(editor) != himark::EditorFocus::None
            }) {
                Some(UnifiedDiffCommand::Inline)
            } else {
                None
            }
        }
        himark::DiffLayout::Split => {
            if view.split.left.focus() != himark::EditorFocus::None {
                Some(|command| UnifiedDiffCommand::Split(SplitDiffCommand::Left(command)))
            } else if view.split.right.focus() != himark::EditorFocus::None {
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
            wrap(himark::EditorCommand::Dynamic {
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
        let (Some(view), Some(inner)) = (self.view, &self.inner) else {
            return imba::event::EventResult::Ignored;
        };

        if let imba::event::Event::UserEvent(payload) = event {
            if let Some(changed) = payload.downcast_ref::<himark::DiffChanged>() {
                if changed.diff == view.split.state.diff_id() {
                    return imba::event::EventResult::Command(UnifiedDiffCommand::Split(
                        SplitDiffCommand::Resync,
                    ));
                }
                return imba::event::EventResult::Ignored;
            }
        }
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
    pub fn over(id: himark::DiffViewId) -> Self {
        Self {
            pane: ScrollView::new(PairPane { id }),
        }
    }

    pub fn pair(&self) -> himark::DiffViewId {
        self.pane.content().id
    }

    pub fn diff_state<'a>(&self, store: &'a Store) -> Option<&'a DiffState> {
        himark::OpenDocuments::diff_view_ref(store, self.pane.content().id)?
            .state
            .as_ref()
    }

    #[doc(hidden)]
    pub fn halves(&self, store: &Store) -> (EditorIdView, EditorIdView) {
        let pair = himark::OpenDocuments::diff_view_ref(store, self.pane.content().id)
            .expect("the pane's family row");
        (pair.left, pair.right)
    }

    pub fn new(
        store: &mut Store,
        left: EditorIdView,
        right: EditorIdView,
        handle: himark::DiffHandle,
        right_extras: himark::MarkupId,
        state: Option<DiffState>,
    ) -> Self {
        let id = himark::DiffViewId::mint();
        himark::OpenDocuments::put_diff_view(
            store,
            id,
            himark::DiffView {
                left,
                right,
                diff: handle.id,
                right_extras,
                state,
            },
        );
        Self {
            pane: ScrollView::new(PairPane { id }),
        }
    }
}

impl View for DiffPanelView {
    type Command = imba::scroll::ScrollCommand<UnifiedDiffCommand>;

    fn focus_data<'w>(
        &'w self,
        store: &'w Store,
        ui: &'w imba::UiCtx,
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
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        self.pane.display(arena, store, ui)
    }
}

#[derive(Clone, PartialEq)]
pub struct DiffPlace {
    pub old: himark::ResourceLocation,
    pub new: himark::ResourceLocation,
}

impl himark::Place for DiffPlace {}

impl himark::PanelView for DiffPanelView {
    type Place = DiffPlace;

    fn family_row(&self) -> Option<himark::FamilyRow> {
        Some(himark::FamilyRow::Pair(self.pane.content().id))
    }

    fn navigation_location(&self, store: &Store) -> Option<DiffPlace> {
        let pair = himark::OpenDocuments::diff_view_ref(store, self.pane.content().id)?;
        Some(DiffPlace {
            old: OpenDocuments::location(store, pair.left.document())?,
            new: OpenDocuments::location(store, pair.right.document())?,
        })
    }

    fn navigate_to(
        &mut self,
        store: &mut Store,
        place: &DiffPlace,
        _fx: &mut himark::AppFx<'_>,
    ) -> bool {
        let Some(pair) = himark::OpenDocuments::diff_view_ref(store, self.pane.content().id) else {
            return false;
        };
        let (left, right) = (pair.left, pair.right);
        OpenDocuments::location(store, left.document()).as_ref() == Some(&place.old)
            && OpenDocuments::location(store, right.document()).as_ref() == Some(&place.new)
    }

    fn title(&self, _store: &Store) -> String {
        "Diff".to_owned()
    }

    fn dismantle(&mut self, store: &mut Store) {
        teardown_diff_view(store, self.pane.content().id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn open_diff_documents(
    store: &mut Store,
    ui: &imba::UiCtx,
    window: himark::WindowId,
    left: himark::DocumentId,
    right: himark::DocumentId,
    prep: Option<DiffPrep>,
    fx: &mut himark::AppFx<'_>,
) -> bool {
    let Some(panel) = diff_panel(store, ui, left, right, prep) else {
        return false;
    };
    let mut entity = himark::Windows::window(store, window).expect("the window entity");
    let opened = entity.open_panel(store, ui, Box::new(panel), fx);
    himark::Windows::put(store, window, entity);
    opened
}

#[derive(Clone)]
pub struct DiffPrep {
    pub operation: himark::Operation,
    pub marks: himark::PreparedMarks,
}

pub fn diff_panel(
    store: &mut Store,
    ui: &imba::UiCtx,
    left: himark::DocumentId,
    right: himark::DocumentId,
    prep: Option<DiffPrep>,
) -> Option<DiffPanelView> {
    let id = build_diff_view(store, ui, left, right, prep, OPEN_HALF_WIDTH)?;
    Some(DiffPanelView::over(id))
}

/// Register a tracked diff over two ALREADY-REGISTERED documents and
/// mint the store-held `DiffView` for it: track it through the Diffs
/// subsystem (so the normalize lane runs and edits compose), add a
/// bounded editor per half, seed the prepared markups, and attach the
/// `DiffState`. Returns the `DiffViewId` for a `PairPane` to render.
/// The reusable core of `diff_panel`; the diff canvas drives it per
/// row so its rows are registered documents, not throwaway snapshots
/// (docs/editor/diff-canvas.md §7, docs/editor/diff-canvas.md §7).
pub fn build_diff_view(
    store: &mut Store,
    ui: &imba::UiCtx,
    left: himark::DocumentId,
    right: himark::DocumentId,
    prep: Option<DiffPrep>,
    half_width: f32,
) -> Option<himark::DiffViewId> {
    let fonts = himark::env::Fonts::of(store)();
    let theme = himark::env::Themes::of(store);

    let prep = match OpenDocuments::pair_tracked(store, left, right) {
        true => None,
        false => prep.or_else(|| {
            let base = OpenDocuments::document_ref(store, left)?;
            let target = OpenDocuments::document_ref(store, right)?;
            let operation = himark::env::Differ::of(store).diff(base.text(), target.text(), None);
            let marks = himark::prepare_marks(&operation, base.text());
            Some(DiffPrep { operation, marks })
        }),
    };

    let diff = OpenDocuments::track_diff(
        store,
        left,
        right,
        false,
        prep.as_ref().map(|prep| prep.operation.clone()),
    )?;
    let handle = OpenDocuments::diff_handle(store, diff)?;
    let target_markup = OpenDocuments::document_ref(store, right)
        .and_then(|document| document.diff(diff).map(|entry| entry.markup()))?;

    fn seed(
        store: &mut Store,
        ui: &imba::UiCtx,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
        document_id: himark::DocumentId,
        marks: himark::MarkupId,
        markup: &himark::Markup,
    ) {
        let Some(mut document) = OpenDocuments::document(store, document_id) else {
            return;
        };
        document.replace_markup(
            marks,
            markup.clone(),
            &[],
                store, ui,
            fonts,
            theme,
            &mut imba::effect::Batch::new().effects(),
        );
        OpenDocuments::put_document(store, document_id, document);
    }
    if let Some(prep) = &prep {
        seed(
            store, ui,
            &fonts,
            &theme,
            left,
            handle.base_markup,
            &prep.marks.left,
        );
    }
    let mut open =
        |document_id: himark::DocumentId, marks: himark::MarkupId| -> Option<EditorIdView> {
            let mut document = OpenDocuments::document(store, document_id)?;

            let editor = document.add_editor(
                half_width,
                None,
                himark::EditorBuild::Bounded,
                &[marks],
                store, ui,
                &fonts,
                &theme,
                &mut imba::effect::Batch::new().effects(),
            );

            document.manage_repairs_in_pair(editor);

            OpenDocuments::put_document(store, document_id, document);
            Some(EditorIdView::new(document_id, editor))
        };
    let (Some(left_view), Some(right_view)) =
        (open(left, handle.base_markup), open(right, target_markup))
    else {
        return None;
    };
    // The pane's own right-half extras (word tints + fold strips) —
    // editor-owned, dying with the half. THE diff markup
    // (`target_markup`, the hunk washes) stays the diff machinery's.
    let right_extras = {
        let mut document = OpenDocuments::document(store, right)?;
        let id = document.add_owned_markup(right_view.editor());
        OpenDocuments::put_document(store, right, document);
        id
    };
    if let Some(prep) = &prep {
        seed(
            store, ui,
            &fonts,
            &theme,
            right,
            right_extras,
            &prep.marks.right,
        );
    }

    let state = {
        let left_document = OpenDocuments::document_ref(store, left)?;
        let right_document = OpenDocuments::document_ref(store, right)?;
        DiffState::attach(
            diff,
            left_document,
            right_document,
            handle.base_markup,
            right_extras,
            prep.map(|prep| prep.marks.window),
            himark::env::Differ::of(store),
        )
    };
    let id = himark::DiffViewId::mint();
    himark::OpenDocuments::put_diff_view(
        store,
        id,
        himark::DiffView {
            left: left_view,
            right: right_view,
            diff: handle.id,
            right_extras,
            state,
        },
    );
    Some(id)
}

pub fn row_minter() -> std::sync::Arc<himark::RowMinter> {
    std::sync::Arc::new(|_store, row| match row {
        himark::FamilyRow::Pair(id) => {
            Some(Box::new(DiffPanelView::over(*id)) as Box<dyn himark::DynPanelView>)
        }
        // Canvases open through the NAVIGATION road (CanvasNavigator)
        // — reuse is a store lookup, not a mint.
        _ => None,
    })
}

pub struct OpenDiff;

impl himark::DynamicCommand for OpenDiff {
    fn id(&self) -> &'static str {
        "diff.open"
    }
    fn name(&self) -> String {
        "Diff Two Recent Documents".to_owned()
    }
    fn perform(
        &self,
        app: &mut Application,
        store: &mut Store,
        window: himark::WindowId,
        _fx: &mut himark::AppFx<'_>,
    ) {
        let ui = &app.ui_ctx();
        let recent = OpenDocuments::list_recent(store);
        let (Some(newest), Some(older)) = (recent.first(), recent.get(1)) else {
            return;
        };
        let _ = open_diff_documents(store, ui, window, older.0, newest.0, None, _fx);
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod monster_probe;
