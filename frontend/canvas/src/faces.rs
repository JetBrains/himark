// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The canvas FACES (docs/editor/diff-canvas.md): the two panels that
//! stand a HEAD over the bare diff canvas — the working copy's commit
//! control (`changes_face`), a commit's block (`commit_face`) — and
//! the navigators that pick one by the place's source. The canvas
//! lists diffs and nothing else: the head is the face's, the scroll
//! is the shell's (docs/mounted-parity.md), and one Column stacks the
//! head over the list under that one scroller.

use imba::event::{Event, EventResult};
use imba::{arena::Arena, store::Store, ui::UiCtx, Widget};
use skia_safe::{Rect, Size};

use crate::canvas::DiffCanvasView;
use crate::changes_face::ChangesFace;
use crate::commit_face::CommitFace;
use crate::diff_canvas::{CanvasPlace, Totals};
use changesview::hichanges::CanvasSource;

/// Which of a face's two areas holds the keyboard: the head's own
/// editors, or a row of the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaceFocus {
    Head,
    Canvas,
}

/// A press anywhere in an area claims the keyboard for it: the
/// widget forwards the event and rides the area's ask on every
/// MouseDown that reached it (the container already hit-tested).
pub(crate) struct FocusOn<Inner, Command> {
    pub inner: Inner,
    pub ask: Command,
}

impl<'a, Command: Clone + 'a, Inner: Widget<'a, Command>> Widget<'a, Command>
    for FocusOn<Inner, Command>
{
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, Command>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<Command> {
        let result = self.inner.handle_event(arena, event, viewport);
        match event {
            Event::MouseDown { .. } => result.merge(EventResult::command(self.ask.clone())),
            _ => result,
        }
    }

    fn layout_data<'w>(
        &'w mut self,
        target: imba::focus::SeatKey,
    ) -> imba::focus::LayoutData<'w, Command>
    where
        'a: 'w,
    {
        self.inner.layout_data(target)
    }
}

/// The BYLINE a change set stands under (Main.dc.html, Commit.dc.html):
/// its author when known, the branch when known, the file count, the
/// totals — brief words on one baseline.
pub(crate) fn byline_row<'a, Command: 'a>(
    arena: &'a Arena,
    store: &Store,
    ui: &UiCtx,
    author: Option<&str>,
    branch: Option<&str>,
    totals: Totals,
) -> imba::layout::Row<'a, Command> {
    let theme = editor::env::Themes::of(store);
    let diff = theme.part(::hikit::chrome::DiffChrome::KEY);
    let ink = theme.part(::editor::theme::InkChrome::KEY);
    let brief = hikit::ui::brief(store, ui);
    let mut row = imba::layout::Row::new(arena).gap(diff.byline_gap);
    let author = author.filter(|author| !author.is_empty());
    if let Some(author) = author {
        row = row.child_by_baseline(hikit::ui::text(&brief.clone().colored(ink.ink_2.0), author));
    }
    if let Some(branch) = branch {
        row = row.child_by_baseline(hikit::ui::text(&brief, format!("on {branch}")));
    }
    if author.is_some() || branch.is_some() {
        row = row.child_by_baseline(hikit::ui::text(&brief, "·"));
    }
    row = row.child_by_baseline(hikit::ui::text(
        &brief,
        match totals.files {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        },
    ));
    if totals.added > 0 {
        row = row.child_by_baseline(hikit::ui::text(
            &brief.clone().colored(ink.added.0),
            format!("+{}", totals.added),
        ));
    }
    if totals.removed > 0 {
        row = row.child_by_baseline(hikit::ui::text(
            &brief.clone().colored(ink.removed.0),
            format!("−{}", totals.removed),
        ));
    }
    row
}

/// The canvas view a panel stands over — either face's — for the
/// roads that reach a canvas through its panel (the diff landing,
/// the tests' probes).
pub fn canvas_of(panel: &dyn std::any::Any) -> Option<&DiffCanvasView> {
    panel
        .downcast_ref::<ChangesFace>()
        .map(ChangesFace::canvas)
        .or_else(|| panel.downcast_ref::<CommitFace>().map(CommitFace::canvas))
}

/// The face for a place: the source's canvas — found or created, the
/// reveal armed — under the head its source calls for.
fn face_over(store: &mut Store, ui: &UiCtx, place: &CanvasPlace) -> Face {
    let canvas = DiffCanvasView::over(store, place.changes, place.source.clone());
    if let Some(key) = &place.reveal {
        canvas.arm_reveal(store, key.clone());
    }
    match place.source {
        CanvasSource::WorkingCopy { .. } => Face::Changes(ChangesFace::over(store, ui, canvas)),
        CanvasSource::Commit { .. } => Face::Commit(CommitFace::over(store, canvas)),
    }
}

enum Face {
    Changes(ChangesFace),
    Commit(CommitFace),
}

/// Answers `CanvasPlace` navigations with the source's face over its
/// store-held canvas. Reuse is the lookup; nothing is ever rebuilt
/// for a second open. BARE — the mounted road's answer: the host's
/// scroller owns the physics (docs/mounted-parity.md).
pub struct CanvasNavigator;

impl hikit::navigation::Navigator for CanvasNavigator {
    type Place = CanvasPlace;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        place: &CanvasPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::panel::DynPanelView>> {
        Some(match face_over(store, ui, place) {
            Face::Changes(face) => Box::new(face),
            Face::Commit(face) => Box::new(face),
        })
    }
}

/// The WINDOWED road's navigator: the same face, wrapped in the
/// shared top scroller — the workbench pane's half of the
/// descrollering law, exactly like every other panel.
pub struct ScrolledCanvasNavigator;

impl hikit::navigation::Navigator for ScrolledCanvasNavigator {
    type Place = CanvasPlace;

    fn navigate(
        &self,
        store: &mut Store,
        ui: &UiCtx,
        place: &CanvasPlace,
        _fx: &mut imba::command::Fx<'_>,
    ) -> Option<Box<dyn hikit::panel::DynPanelView>> {
        Some(match face_over(store, ui, place) {
            Face::Changes(face) => Box::new(hikit::modal::Scrolled::new(face)),
            Face::Commit(face) => Box::new(hikit::modal::Scrolled::new(face)),
        })
    }
}
