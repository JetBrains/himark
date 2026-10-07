// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The diff RENDERING machinery (docs/editor/diff-canvas.md §4,
//! docs/model-view.md step 1): build a tracked pair over registered
//! documents, gather the per-ask facade, perform against the
//! store-held view with effects routed home BY ID, re-dress at the
//! batch tail, tear down, rewrap. The faces over it — the split-diff
//! pane, the diff canvas — live with their features; no window and
//! no panel is known here.

use imba::effect::{Effect, Effects};
use imba::store::Store;
use imba::View as _;

use crate::diffs::{DiffView, DiffViewId};
use crate::{DocumentsCommand, OpenDocuments};
use editor::location::ResourceLocation;

/// The ONE off-thread step both diff roads share: ensure each side is
/// a REGISTERED document. An OPEN side passes through by id (no
/// fetch, no build); a CLOSED side is fetched and built here and
/// registered at the landing — the standard open road. It does NOT
/// diff: the view's normalize lane computes the diff from the
/// registered documents (docs/no-diff-on-ui-thread). The canvas has
/// no business with Texts, parses, or operations.
pub struct OpenDiffPairEffect {
    pub old: DiffSideInput,
    pub new: DiffSideInput,
    /// The half width to lay the editors at (the canvas's content width).
    pub width: f32,
}

impl std::fmt::Display for OpenDiffPairEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("open diff pair")
    }
}

impl Effect for OpenDiffPairEffect {
    type Result = OpenedDiffPair;
}

/// One side to open, resolved on the UI thread at launch — a
/// reference, never content.
pub enum DiffSideInput {
    /// Already a registered document — use it as-is.
    Open(crate::DocumentId),
    /// Closed — the handler fetches and builds it, the landing registers.
    Fetch(ResourceLocation),
}

impl DiffSideInput {
    pub fn resolve(
        store: &Store,
        documents: imba::store::Id<OpenDocuments>,
        location: ResourceLocation,
    ) -> Self {
        match OpenDocuments::by_location(store, documents, &location) {
            Some(document) => DiffSideInput::Open(document),
            None => DiffSideInput::Fetch(location),
        }
    }
}

#[derive(Clone)]
pub struct OpenedDiffPair {
    pub old: DiffSide,
    pub new: DiffSide,
    pub width: f32,
    /// Both sides gone — the caller reports instead of mounting.
    pub failed: bool,
}

/// What the landing does with a side: reuse the registered document,
/// or register the freshly-built one at its location
/// (register-at-display — the standard `BuiltDocument` payload).
#[derive(Clone)]
pub enum DiffSide {
    Open(crate::DocumentId),
    Built {
        location: ResourceLocation,
        document: crate::BuiltDocument,
    },
}

/// Assemble the per-ask facade over a tracked pair: live documents +
/// the store-held `DiffViewState` (attached on first gather).
pub fn gather_diff_view(
    pair: &DiffView,
    store: &Store,
    documents: imba::store::Id<OpenDocuments>,
) -> Option<editor::unified_diff::UnifiedDiffView> {
    let left_view = editor::editor_view::EditorView {
        document: OpenDocuments::document(store, documents, pair.left.document())?,
        editor: pair.left.editor(),
        reports_geometry: true,
        location: OpenDocuments::location(store, documents, pair.left.document()),
        gutter_width: 0.0,
        base: None,
    };
    let right_view = editor::editor_view::EditorView {
        document: OpenDocuments::document(store, documents, pair.right.document())?,
        editor: pair.right.editor(),
        reports_geometry: true,
        location: OpenDocuments::location(store, documents, pair.right.document()),
        gutter_width: 0.0,
        base: None,
    };
    let state = match &pair.state {
        Some(state) => state.clone(),
        None => editor::split_diff::DiffViewState::attach(
            pair.diff,
            &left_view.document,
            &right_view.document,
            OpenDocuments::diff_handle(store, documents, pair.diff)?.base_markup,
            pair.right_extras,
            None,
        )?,
    };
    Some(editor::unified_diff::UnifiedDiffView::new(
        editor::split_diff::SplitDiffView::new(left_view, right_view, state),
    ))
}

/// Which side of the pair a routed editor command addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairSide {
    Inline,
    Left,
    Right,
}

/// A cmd-click into a half is a caret set first; the side it hit
/// comes back so the pane can run the link follower AFTER the click
/// performed (`EditorIdView` does the same for a full pane).
pub fn link_click(
    command: editor::unified_diff::UnifiedDiffCommand,
) -> (editor::unified_diff::UnifiedDiffCommand, Option<PairSide>) {
    use editor::editor_view::{ClickKind, EditorCommand};
    use editor::split_diff::SplitDiffCommand;
    use editor::unified_diff::UnifiedDiffCommand;
    let set = |point| EditorCommand::Click {
        point,
        kind: ClickKind::Set,
    };
    match command {
        UnifiedDiffCommand::Inline(EditorCommand::Click {
            point,
            kind: ClickKind::Link,
        }) => (
            UnifiedDiffCommand::Inline(set(point)),
            Some(PairSide::Inline),
        ),
        UnifiedDiffCommand::Split(SplitDiffCommand::Left(EditorCommand::Click {
            point,
            kind: ClickKind::Link,
        })) => (
            UnifiedDiffCommand::Split(SplitDiffCommand::Left(set(point))),
            Some(PairSide::Left),
        ),
        UnifiedDiffCommand::Split(SplitDiffCommand::Right(EditorCommand::Click {
            point,
            kind: ClickKind::Link,
        })) => (
            UnifiedDiffCommand::Split(SplitDiffCommand::Right(set(point))),
            Some(PairSide::Right),
        ),
        command => (command, None),
    }
}

/// The cmd-click tail for a diff half: if the fresh caret sits on a
/// linkable span, the one registered link follower performs there
/// with the half's ids in hand.
pub fn follow_link(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    documents: imba::store::Id<OpenDocuments>,
    id: DiffViewId,
    side: PairSide,
    fx: &mut Effects<'_, editor::unified_diff::UnifiedDiffCommand>,
) {
    use editor::split_diff::SplitDiffCommand;
    use editor::unified_diff::UnifiedDiffCommand;
    let Some(entry) = crate::dynamic::DocumentCommands::of(store)
        .link_follower(documents)
        .cloned()
    else {
        return;
    };
    let sides = {
        let Some(pair) = OpenDocuments::diff_view_ref(store, documents, id) else {
            return;
        };
        match side {
            PairSide::Inline => pair
                .state
                .as_ref()
                .and_then(|state| state.inline_editor())
                .map(|editor| (pair.right.document(), editor)),
            PairSide::Left => Some((pair.left.document(), pair.left.editor())),
            PairSide::Right => Some((pair.right.document(), pair.right.editor())),
        }
    };
    let Some((document_id, editor)) = sides else {
        return;
    };
    let Some(location) = OpenDocuments::location(store, documents, document_id) else {
        return;
    };
    if !entry.offers_at(&location) {
        return;
    }
    let Some(mut document) = OpenDocuments::document(store, documents, document_id) else {
        return;
    };
    if document
        .link_range_at(document.caret_byte(editor))
        .is_none()
    {
        return;
    }
    fx.scope(
        move |command| match side {
            PairSide::Inline => UnifiedDiffCommand::Inline(command),
            PairSide::Left => UnifiedDiffCommand::Split(SplitDiffCommand::Left(command)),
            PairSide::Right => UnifiedDiffCommand::Split(SplitDiffCommand::Right(command)),
        },
        |fx| {
            entry.perform(
                store,
                ui,
                documents,
                document_id,
                &mut document,
                editor,
                &location,
                None,
                fx,
            );
        },
    );
    OpenDocuments::put_document(store, documents, document_id, document);
}

/// Land a code-navigation target INSIDE a tracked diff: the right
/// half takes the caret at `byte`, a fold hiding it lifts
/// (`SplitDiffCommand::GoTo`), through the store-held road.
pub fn go_to(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    id: DiffViewId,
    byte: u32,
    fx: &mut Effects<'_, DocumentsCommand>,
) {
    perform_diff_view(
        store,
        documents,
        ui,
        id,
        editor::unified_diff::UnifiedDiffCommand::GoTo { byte },
        fx,
    );
}

/// Collection-scoped document commands (comments.add among them)
/// dispatch HERE with the pair's ids in hand — the gathered editors
/// are bare `EditorView`s and would drop a Dynamic command on the
/// floor (`EditorIdView` does the same for full panes). Returns the
/// command back when it is not a dynamic one.
pub fn dynamic_diff_command(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    documents: imba::store::Id<OpenDocuments>,
    id: DiffViewId,
    command: editor::unified_diff::UnifiedDiffCommand,
    fx: &mut Effects<'_, editor::unified_diff::UnifiedDiffCommand>,
) -> Option<editor::unified_diff::UnifiedDiffCommand> {
    use editor::editor_view::EditorCommand;
    use editor::split_diff::SplitDiffCommand;
    use editor::unified_diff::UnifiedDiffCommand;
    let (side, command_id) = match &command {
        UnifiedDiffCommand::Inline(EditorCommand::Dynamic { id, .. }) => (PairSide::Inline, *id),
        UnifiedDiffCommand::Split(SplitDiffCommand::Left(EditorCommand::Dynamic {
            id, ..
        })) => (PairSide::Left, *id),
        UnifiedDiffCommand::Split(SplitDiffCommand::Right(EditorCommand::Dynamic {
            id, ..
        })) => (PairSide::Right, *id),
        _ => return Some(command),
    };
    let Some(entry) = crate::dynamic::DocumentCommands::of(store)
        .find(documents, command_id)
        .cloned()
    else {
        return Some(command);
    };
    let sides = {
        let pair = OpenDocuments::diff_view_ref(store, documents, id)?;
        match side {
            // The inline face reads the right document through the
            // inline editor — that is where the selection lives.
            PairSide::Inline => pair
                .state
                .as_ref()
                .and_then(|state| state.inline_editor())
                .map(|editor| (pair.right.document(), editor)),
            PairSide::Left => Some((pair.left.document(), pair.left.editor())),
            PairSide::Right => Some((pair.right.document(), pair.right.editor())),
        }
    };
    let Some((document_id, editor)) = sides else {
        return None;
    };
    let payload = match command {
        UnifiedDiffCommand::Inline(EditorCommand::Dynamic { payload, .. })
        | UnifiedDiffCommand::Split(SplitDiffCommand::Left(EditorCommand::Dynamic {
            payload,
            ..
        }))
        | UnifiedDiffCommand::Split(SplitDiffCommand::Right(EditorCommand::Dynamic {
            payload,
            ..
        })) => payload,
        _ => unreachable!("matched above"),
    };
    let location = OpenDocuments::location(store, documents, document_id)?;
    let mut document = OpenDocuments::document(store, documents, document_id)?;
    fx.scope(
        move |command| match side {
            PairSide::Inline => UnifiedDiffCommand::Inline(command),
            PairSide::Left => UnifiedDiffCommand::Split(SplitDiffCommand::Left(command)),
            PairSide::Right => UnifiedDiffCommand::Split(SplitDiffCommand::Right(command)),
        },
        |fx| {
            entry.perform(
                store,
                ui,
                documents,
                document_id,
                &mut document,
                editor,
                &location,
                payload.and_then(editor::dynamic::DynPayload::take),
                fx,
            );
        },
    );
    OpenDocuments::put_document(store, documents, document_id, document);
    None
}

/// Perform one command against a STORE-HELD diff view — the
/// panel-free road (docs/model-view.md step 1): take the record,
/// gather, perform with effects routed home BY ID
/// (`DocumentsCommand::DiffView` over the `At` road), put the
/// documents and the state back. Panels keep their own routed
/// perform for interaction; the dressing flows through here.
pub fn perform_diff_view(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    id: DiffViewId,
    command: editor::unified_diff::UnifiedDiffCommand,
    fx: &mut Effects<'_, DocumentsCommand>,
) {
    let mut passthrough = None;
    fx.scope(
        move |command: editor::unified_diff::UnifiedDiffCommand| {
            DocumentsCommand::DiffView(id, Box::new(command))
        },
        |fx| passthrough = dynamic_diff_command(store, ui, documents, id, command, fx),
    );
    let Some(command) = passthrough else {
        return;
    };
    let Some(mut pair) = OpenDocuments::take_diff_view(store, documents, id) else {
        return;
    };
    let Some(mut view) = gather_diff_view(&pair, store, documents) else {
        OpenDocuments::put_diff_view(store, documents, id, pair);
        return;
    };
    fx.scope(
        move |command: editor::unified_diff::UnifiedDiffCommand| {
            DocumentsCommand::DiffView(id, Box::new(command))
        },
        |fx| view.perform(store, ui, command, fx),
    );
    OpenDocuments::put_document(
        store,
        documents,
        pair.left.document(),
        view.split.left.document,
    );
    OpenDocuments::put_document(
        store,
        documents,
        pair.right.document(),
        view.split.right.document,
    );
    pair.state = Some(view.split.state);
    OpenDocuments::put_diff_view(store, documents, id, pair);
    OpenDocuments::note_dressed(store, documents, id);
}

/// The batch-tail DRESSING sweep (docs/model-view.md step 1): any
/// tracked view whose basis lags its pair — a normalize landed, or
/// another editor moved a shared document — resyncs NOW, id-routed,
/// no paint probe. Runs right after the diff lanes, so a landing and
/// its re-dress share a batch. O(touched views): the write doors
/// queue side-document writes, and the queue names exactly the views
/// whose state can lag (docs/perf-issue.md §1b) — never a walk of
/// every view a canvas ever dressed.
pub fn sync_diff_dressing(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    fx: &mut Effects<'_, DocumentsCommand>,
) {
    for id in OpenDocuments::take_stale_view_candidates(store, documents) {
        let stale = OpenDocuments::diff_view_ref(store, documents, id).is_some_and(|pair| {
            let Some(state) = &pair.state else {
                // Never gathered: no face was built, nothing owes.
                return false;
            };
            let Some(left) = OpenDocuments::document_ref(store, documents, pair.left.document())
            else {
                return false;
            };
            let Some(right) = OpenDocuments::document_ref(store, documents, pair.right.document())
            else {
                return false;
            };
            state.stale(left, right)
        });
        if !stale {
            continue;
        }
        perform_diff_view(
            store,
            documents,
            ui,
            id,
            editor::unified_diff::UnifiedDiffCommand::Split(
                editor::split_diff::SplitDiffCommand::Resync,
            ),
            fx,
        );
    }
}

/// The standalone pane's half width — shared by every pair build.
pub const OPEN_HALF_WIDTH: f32 = 420.0;

/// Tear down a tracked pair: drop the store-held `DiffView`, remove
/// the pair's editors from the (possibly shared) registered
/// documents, and untrack the diff from the Diffs subsystem. Does NOT
/// close the documents — they may be open elsewhere.
pub fn teardown_diff_view(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    id: DiffViewId,
) {
    // Teardown-only road (dismantle/destroy/retire carry no UiCtx);
    // the release may reshape a surviving base document's markup once.
    let ui = &imba::ui::UiCtx::dont_use_too_slow();
    let Some(pair) = OpenDocuments::take_diff_view(store, documents, id) else {
        return;
    };
    if let Some(inline) = pair.state.as_ref().and_then(|state| state.inline_editor()) {
        if let Some(mut document) = OpenDocuments::document(store, documents, pair.right.document())
        {
            document.remove_editor(inline);
            OpenDocuments::put_document(store, documents, pair.right.document(), document);
        }
    }
    for entity in [pair.left, pair.right] {
        if let Some(mut document) = OpenDocuments::document(store, documents, entity.document()) {
            document.remove_editor(entity.editor());
            OpenDocuments::put_document(store, documents, entity.document(), document);
        }
    }
    OpenDocuments::untrack_diff(
        store,
        documents,
        ui,
        pair.diff,
        &mut imba::effect::Batch::<editor::unified_diff::UnifiedDiffCommand>::new().effects(),
    );
}

/// Re-wrap the inline face of a tracked pair to `width` (the
/// row-level rewrap, docs/editor/diff-canvas.md §4): gather, resize
/// the half + inline editors on the registered documents, resync,
/// write back. The split face owns its half widths and is left alone.
pub fn rewrap_pair(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    id: DiffViewId,
    width: f32,
    fx: &mut Effects<'_, editor::unified_diff::UnifiedDiffCommand>,
) {
    let Some(mut pair) = OpenDocuments::take_diff_view(store, documents, id) else {
        return;
    };
    let Some(mut view) = gather_diff_view(&pair, store, documents) else {
        OpenDocuments::put_diff_view(store, documents, id, pair);
        return;
    };
    if view.layout == editor::unified_diff::DiffLayout::Split {
        OpenDocuments::put_diff_view(store, documents, id, pair);
        return;
    }
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);
    let left_editor = view.split.left.editor;
    let right_editor = view.split.right.editor;
    let inline = view.inline_editor;
    fx.scope(
        |c: editor::editor_view::EditorCommand| {
            editor::unified_diff::UnifiedDiffCommand::Split(
                editor::split_diff::SplitDiffCommand::Left(c),
            )
        },
        |fx| {
            view.split
                .left
                .document
                .resize(left_editor, width, 0, store, ui, &fonts, &theme, fx)
        },
    );
    fx.scope(
        |c: editor::editor_view::EditorCommand| {
            editor::unified_diff::UnifiedDiffCommand::Split(
                editor::split_diff::SplitDiffCommand::Right(c),
            )
        },
        |fx| {
            view.split
                .right
                .document
                .resize(right_editor, width, 0, store, ui, &fonts, &theme, fx)
        },
    );
    if let Some(inline) = inline {
        fx.scope(
            |c: editor::editor_view::EditorCommand| {
                editor::unified_diff::UnifiedDiffCommand::Inline(c)
            },
            |fx| {
                view.split
                    .right
                    .document
                    .resize(inline, width, 0, store, ui, &fonts, &theme, fx)
            },
        );
    }
    view.perform(
        store,
        ui,
        editor::unified_diff::UnifiedDiffCommand::Split(
            editor::split_diff::SplitDiffCommand::Resync,
        ),
        fx,
    );
    OpenDocuments::put_document(
        store,
        documents,
        pair.left.document(),
        view.split.left.document,
    );
    OpenDocuments::put_document(
        store,
        documents,
        pair.right.document(),
        view.split.right.document,
    );
    pair.state = Some(view.split.state);
    OpenDocuments::put_diff_view(store, documents, id, pair);
}

fn register_or_reuse(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    side: DiffSide,
) -> crate::DocumentId {
    match side {
        DiffSide::Open(id) => id,
        DiffSide::Built { location, document } => {
            match OpenDocuments::by_location(store, documents, &location) {
                Some(id) => id,
                None => {
                    let revision = document.document.revision();
                    OpenDocuments::register(
                        store,
                        documents,
                        document.document,
                        Some(location.clone()),
                        location.name().to_owned(),
                        revision,
                    )
                }
            }
        }
    }
}

/// Install an opened pair: register/reuse both sides, then
/// `build_diff_view` (which tracks the diff — the normalize lane
/// computes and dresses it — and mounts the `UnifiedDiffView`). The
/// one landing behind the split-diff pane AND the diff canvas; each
/// wraps the returned id in its own face.
pub fn install_opened_pair(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    pair: OpenedDiffPair,
    embedded: bool,
) -> Option<DiffViewId> {
    let old_id = register_or_reuse(store, documents, pair.old);
    let new_id = register_or_reuse(store, documents, pair.new);
    let half_width = match embedded {
        true => {
            let gutter = editor::env::Themes::of(store).ui().editor_gutter.width;
            (pair.width - gutter).max(120.0)
        }
        false => OPEN_HALF_WIDTH,
    };
    build_diff_view(store, documents, ui, old_id, new_id, half_width, embedded)
}

/// Make a diff view over two ALREADY-REGISTERED documents and mint
/// the store-held `DiffView`: track the diff through the Diffs
/// subsystem (which SEEDS it — the normalize lane computes the real
/// diff from the documents and dresses it), add a bounded editor per
/// half, and attach the `DiffViewState`. No diff is computed here
/// (docs/no-diff-on-ui-thread). The reusable core the split-diff pane
/// and the diff canvas both mount.
pub fn build_diff_view(
    store: &mut Store,
    documents: imba::store::Id<OpenDocuments>,
    ui: &imba::ui::UiCtx,
    left: crate::DocumentId,
    right: crate::DocumentId,
    half_width: f32,
    embedded: bool,
) -> Option<DiffViewId> {
    let fonts = editor::env::Fonts::of(store)();
    let theme = editor::env::Themes::of(store);

    let diff = OpenDocuments::track_diff(store, documents, left, right, false)?;
    let handle = OpenDocuments::diff_handle(store, documents, diff)?;
    let target_markup = OpenDocuments::document_ref(store, documents, right)
        .and_then(|document| document.diff(diff).map(|entry| entry.markup()))?;

    let mut open = |document_id: crate::DocumentId,
                    marks: editor::markup::MarkupId|
     -> Option<crate::entity_view::EditorIdView> {
        let mut document = OpenDocuments::document(store, documents, document_id)?;

        let editor = document.add_editor(
            half_width,
            None,
            editor::document::EditorBuild::Bounded,
            &[marks],
            store,
            ui,
            &fonts,
            &theme,
            &mut imba::effect::Batch::new().effects(),
        );

        document.manage_repairs_in_pair(editor);

        OpenDocuments::put_document(store, documents, document_id, document);
        Some(crate::entity_view::EditorIdView::new(
            documents,
            document_id,
            editor,
        ))
    };
    let (Some(left_view), Some(right_view)) =
        (open(left, handle.base_markup), open(right, target_markup))
    else {
        return None;
    };
    // The pane's own right-half extras (word tints + fold strips) —
    // editor-owned, dying with the half; derived by the marks job on
    // settle. THE diff markup (`target_markup`, the hunk washes) stays
    // the diff machinery's — seeded by `track_diff`, minimized by the
    // normalize lane.
    let right_extras = {
        let mut document = OpenDocuments::document(store, documents, right)?;
        let id = document.add_owned_markup(right_view.editor());
        OpenDocuments::put_document(store, documents, right, document);
        id
    };

    let state = {
        let left_document = OpenDocuments::document_ref(store, documents, left)?;
        let right_document = OpenDocuments::document_ref(store, documents, right)?;
        editor::split_diff::DiffViewState::attach(
            diff,
            left_document,
            right_document,
            handle.base_markup,
            right_extras,
            None,
        )
    };
    let id = DiffViewId::mint();
    OpenDocuments::put_diff_view(
        store,
        documents,
        id,
        DiffView {
            left: left_view,
            right: right_view,
            diff: handle.id,
            right_extras,
            state,
            embedded,
        },
    );
    Some(id)
}
