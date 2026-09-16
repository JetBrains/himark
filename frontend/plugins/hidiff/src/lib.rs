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
pub use canvas::{CanvasPlace, DiffCanvasView};

#[derive(Clone, Copy)]
pub struct PairPane {
    id: himark::DiffViewId,
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
            )?,
        };
        Some(UnifiedDiffView::new(SplitDiffView::new(
            left_view, right_view, state,
        )))
    }
}

impl View for PairPane {
    type Command = UnifiedDiffCommand;

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
                let inner = view.layout(self.arena, self.store, self.ui, self.constraints);
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
            view.layout(self.arena, self.store, self.ui, self.constraints)
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

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, UnifiedDiffCommand>
    where
        'a: 'w,
    {
        use imba::focus::FocusData;
        let (Some(view), Some(inner)) = (self.view, &mut self.inner) else {
            return FocusData::default();
        };
        let wrap: Option<fn(himark::EditorCommand) -> UnifiedDiffCommand> =
            if view.split.left.focus() != himark::EditorFocus::None {
                Some(|command| UnifiedDiffCommand::Split(SplitDiffCommand::Left(command)))
            } else if view.split.right.focus() != himark::EditorFocus::None {
                Some(|command| UnifiedDiffCommand::Split(SplitDiffCommand::Right(command)))
            } else if view.inline_editor.is_some_and(|editor| {
                view.split.right.document.focus(editor) != himark::EditorFocus::None
            }) {
                Some(UnifiedDiffCommand::Inline)
            } else {
                None
            };
        let mut data = inner.focus_data();
        let injected = data
            .commands
            .iter()
            .any(|presentable| presentable.id == "workbench.open-in-full");
        if let (false, Some(wrap)) = (injected, wrap) {
            data.commands.push(imba::PresentableCommand::new(
                "workbench.open-in-full",
                "Open Working Copy",
                wrap(himark::EditorCommand::Dynamic {
                    id: "workbench.open-in-full",
                    payload: None,
                }),
            ));
        }
        data
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
        let Some(pair) = himark::OpenDocuments::take_diff_view(store, self.pane.content().id)
        else {
            return;
        };
        let diff = pair.diff;

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
            store,
            diff,
            &mut imba::effect::Batch::<UnifiedDiffCommand>::new().effects(),
        );
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn open_diff_documents(
    store: &mut Store,
    window: himark::WindowId,
    left: himark::DocumentId,
    right: himark::DocumentId,
    prep: Option<DiffPrep>,
    fx: &mut himark::AppFx<'_>,
) -> bool {
    let Some(panel) = diff_panel(store, left, right, prep) else {
        return false;
    };
    let mut entity = himark::Windows::window(store, window).expect("the window entity");
    let opened = entity.open_panel(store, Box::new(panel), fx);
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
    left: himark::DocumentId,
    right: himark::DocumentId,
    prep: Option<DiffPrep>,
) -> Option<DiffPanelView> {
    let fonts = himark::env::Fonts::of(store)();
    let theme = himark::env::Themes::of(store);

    let prep = match OpenDocuments::pair_tracked(store, left, right) {
        true => None,
        false => prep.or_else(|| {
            let base = OpenDocuments::document_ref(store, left)?;
            let target = OpenDocuments::document_ref(store, right)?;
            let operation = himark::diff::diff(base.text(), target.text());
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
            fonts,
            theme,
            &mut imba::effect::Batch::new().effects(),
        );
        OpenDocuments::put_document(store, document_id, document);
    }
    if let Some(prep) = &prep {
        seed(
            store,
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
                OPEN_HALF_WIDTH,
                None,
                himark::EditorBuild::Bounded,
                &[marks],
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
            store,
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
        )
    };
    Some(DiffPanelView::new(
        store,
        left_view,
        right_view,
        handle,
        right_extras,
        state,
    ))
}

pub fn row_minter() -> std::sync::Arc<himark::RowMinter> {
    std::sync::Arc::new(|_store, row| match row {
        himark::FamilyRow::Pair(id) => {
            Some(Box::new(DiffPanelView::over(*id)) as Box<dyn himark::DynPanelView>)
        }
        himark::FamilyRow::Canvas(source) => {
            Some(Box::new(DiffCanvasView::fresh(source.clone())) as Box<dyn himark::DynPanelView>)
        }
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
        _app: &mut Application,
        store: &mut Store,
        window: himark::WindowId,
        _fx: &mut himark::AppFx<'_>,
    ) {
        let recent = OpenDocuments::list_recent(store);
        let (Some(newest), Some(older)) = (recent.first(), recent.get(1)) else {
            return;
        };
        let _ = open_diff_documents(store, window, older.0, newest.0, None, _fx);
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod monster_probe;
