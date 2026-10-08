// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::arena::Arena;
use imba::constraints::Constraints;
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{ui::UiCtx, Thunk};
use skia_safe::{Point, Rect, Size};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::markup::Inlay;

/// Inlays that target an overlay host (`Inlay::over`) — fold strips,
/// before-cards — emitted exactly like popups: fully interactive,
/// anchored at the slot they reserve in the flow, and laid by the
/// HOST at its width, so a strip spans the whole pane (or, hosted
/// above a split, both panes at once). The editor's own container
/// keeps holes in their places. Line geometry comes from the frame's
/// `EditorViewport` — derived once, queried here.
pub(crate) fn projected_overlays<'a>(
    document: &Document,
    editor: EditorId,
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    data: &crate::viewport::EditorViewport,
    origin: Point,
) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
    use crate::markup::{inlay_anchors_line, InlayMode, OverlaidMarkup};

    let extras = document.extras_keyed(editor);
    let markups = OverlaidMarkup::new(document.markup(), &extras);
    if !markups.has_inlays() {
        return Vec::new();
    }
    let width = data.layout_width.max(1.0);
    let constraints = Constraints {
        min: Size::default(),
        max: Size::new(width, f32::MAX),
    };
    let mut overlays = Vec::new();
    for line in &data.lines {
        let line_range = line.byte_start..line.byte_end;
        let hits = markups.all_inlays_in(line_range.clone());
        if !hits.iter().any(|interval| interval.inlay.overlay.is_some()) {
            continue;
        }
        let content_top = line.text_top;
        let content_height = line.inlays.content_height_from_total(line.height);
        // The slot walk mirrors the container pass, counting EVERY
        // inlay so projected and inline neighbours keep their
        // stacking order.
        let mut above_y = line.top;
        let mut under_y = content_top + content_height;
        for interval in &hits {
            if !inlay_anchors_line(interval.inlay.mode, &interval.range, &line_range) {
                continue;
            }
            let size = interval.inlay.layout(arena, store, ui, constraints).size();
            let y = match interval.inlay.mode {
                InlayMode::Above => {
                    let y = above_y;
                    above_y += size.height;
                    y
                }
                InlayMode::Under => {
                    let y = under_y;
                    under_y += size.height;
                    y
                }
                _ => content_top + (content_height - size.height).max(0.0) * 0.5,
            };
            let Some((host, projection)) = interval.inlay.overlay else {
                continue;
            };
            let key = interval.key;
            let inlay = interval.inlay.clone();
            overlays.push(imba::overlay::Overlay {
                host,
                // The anchor's left edge is the MINTING EDITOR's left
                // edge (0 here; translations on the way up carry it
                // into host coordinates).
                anchor: Rect::from_xywh(0.0, origin.y + y, width, size.height.max(1.0)),
                content: Box::new(move |host_size: Size, anchor: Rect| {
                    use crate::markup::InlayProjection;
                    let left = match projection {
                        InlayProjection::Span => 0.0,
                        InlayProjection::Aligned => anchor.left.max(0.0),
                    };
                    // A real THUNK, not a pre-built widget: the host
                    // realizes it with its own clipped viewport
                    // (`place_realized`), so the content widget is
                    // born closed over the bounded result — every
                    // later ask (events, focus) is a read. The old
                    // deferring PopupWidget re-laid per ask and had
                    // to invent a full-extent viewport for
                    // `focus_data` (the DiffCanvas.trace lesson).
                    let size = Size::new((host_size.width - left).max(1.0), anchor.height());
                    // The inlay moves into the arena so the thunk it
                    // lays can borrow it for the frame's lifetime.
                    let inlay: &Inlay = arena.alloc(inlay);
                    let thunk = inlay
                        .layout(arena, store, ui, Constraints::tight(size))
                        .map(move |command| EditorCommand::Inlay { key, command });
                    vec![(
                        Point::new(left, anchor.top),
                        imba::ThunkBox::new(arena, thunk),
                    )]
                }),
            });
        }
    }
    overlays
}
