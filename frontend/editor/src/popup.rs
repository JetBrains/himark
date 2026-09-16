// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::arena::Arena;
use imba::constraints::Constraints;
use imba::store::Store;
use imba::thunk_ext::ThunkExt;
use imba::{Thunk, UiCtx};
use skia_safe::{Point, Rect, Size};

use crate::document::Document;
use crate::editor::EditorId;
use crate::editor_view::EditorCommand;
use crate::markup::{Inlay, InlayKey};

pub(crate) fn visible_popups<'a>(
    document: &Document,
    editor: EditorId,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &crate::theme::Theme,
    arena: &'a Arena,
    store: &'a Store,
    ui: &'a UiCtx,
    viewport: Rect,
    origin: Point,
) -> Vec<imba::overlay::Overlay<'a, EditorCommand>> {
    if !document.has_popups(editor) || viewport.height() <= 0.0 {
        return Vec::new();
    }

    let band = document.visible_byte_band(editor, viewport.top, viewport.bottom);
    let mut overlays = Vec::new();
    for (key, range, inlay, spec) in document.popups_in(editor, band) {
        let Some((x, y, width, height)) =
            document.caret_content_rect(editor, range.start, fonts, theme)
        else {
            continue;
        };
        let width = document
            .caret_content_rect(editor, range.end, fonts, theme)
            .filter(|(_, end_y, _, _)| *end_y == y)
            .map(|(end_x, _, _, _)| (end_x - x).max(width))
            .unwrap_or(width);
        if y + height < viewport.top || y > viewport.bottom {
            continue;
        }
        let seed = PopupSeed {
            inlay,
            key,
            position: spec.position,
            store,
            ui,
            arena,
        };
        overlays.push(imba::overlay::Overlay {
            host: spec.host,
            anchor: Rect::from_xywh(origin.x + x, origin.y + y, width.max(1.0), height),
            content: Box::new(move |host_size: Size, anchor: Rect| seed.layout(host_size, anchor)),
        });
    }
    overlays
}

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
            if matches!(interval.inlay.mode, InlayMode::Popup(_)) {
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

pub(crate) struct PopupSeed<'a> {
    pub inlay: Inlay,
    pub key: InlayKey,
    pub position: imba::overlay::fit::PreferredPosition,
    pub store: &'a Store,
    pub ui: &'a UiCtx,
    pub arena: &'a Arena,
}

impl<'a> PopupSeed<'a> {
    pub(crate) fn layout(
        self,
        host_size: Size,
        anchor: Rect,
    ) -> Vec<(Point, imba::ThunkBox<'a, EditorCommand>)> {
        let desired = self
            .inlay
            .layout(
                self.arena,
                self.store,
                self.ui,
                Constraints {
                    min: Size::default(),
                    max: Size::new(f32::INFINITY, f32::INFINITY),
                },
            )
            .size();
        let resolved = imba::overlay::fit::resolve(
            host_size,
            anchor,
            desired,
            Size::new(1.0, 1.0),
            self.position,
        );
        let key = self.key;
        let inlay: &Inlay = self.arena.alloc(self.inlay);
        let thunk = inlay
            .layout(
                self.arena,
                self.store,
                self.ui,
                Constraints::tight(Size::new(resolved.width(), resolved.height())),
            )
            .map(move |command| EditorCommand::Inlay { key, command });
        vec![(
            Point::new(resolved.left, resolved.top),
            imba::ThunkBox::new(self.arena, thunk),
        )]
    }
}
