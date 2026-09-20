// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Go-to reference peek (docs/ui/location-list.md §8): an
//! `InlayMode::Under` master–detail card under the caret's line —
//! the quick random-access interface. The master streams from an
//! `ahp-locations:/…` channel; the detail lazily fetches ONLY the
//! selected location's document and shows a bounded fragment around
//! the match. Enter navigates and dismisses; Escape dismisses; a
//! single-result stream navigates directly without showing the card.

use std::sync::Arc;

use imba::{
    arena::Arena,
    constraints::Constraints,
    effect::AnyEffect,
    event::{EventResult, Key as InputKey},
    store::Store,
    thunk_ext::ThunkExt,
    UiCtx, View,
};
use skia_safe::{Paint, Rect, Size};

use crate::locations::{resolve_batch, FoundLocation};
use crate::rows::{RowList, RowListCommand};
use crate::{
    AppRequests, Document, EditorCommand, EditorFocus, EditorView, Inlay, InlayKey, InlayMode,
    LocationsChannel,
};

const PEEK_HEIGHT: f32 = 280.0;

const FALLBACK_WIDTH: f32 = 600.0;

pub enum PeekCommand {
    Rows(RowListCommand),
    Select(isize),
    Pick,
    Close,
    Nothing,
    Preview(imba::scroll::ScrollCommand<EditorCommand>),
    Snapshot {
        outcome: Result<himark_ahp_ext_types::LocationList, String>,
    },
    Polled {
        batches: Vec<himark_ahp_ext_types::LocationList>,
    },
    Fetched {
        index: usize,
        text: Option<String>,
    },
    Built {
        index: usize,
        built: crate::BuiltDocument,
    },
}

#[derive(Clone)]
pub struct PeekView {
    /// The hosting document and the card's own key, baked in by the
    /// push → swap two-step so the card can remove itself.
    host: Option<crate::DocumentId>,
    key: Option<InlayKey>,
    width: f32,

    channel: Option<LocationsChannel>,
    poll_token: Option<imba::effect::CancellationToken>,
    fetch_token: Option<imba::effect::CancellationToken>,

    locations: rpds::VectorSync<FoundLocation>,
    done: bool,
    truncated: bool,

    list: RowList,
    preview: Option<imba::scroll::ScrollView<EditorView>>,
    preview_for: Option<usize>,
}

impl PeekView {
    fn new(host: Option<crate::DocumentId>, width: f32, channel: LocationsChannel) -> Self {
        Self {
            host,
            key: None,
            width,
            channel: Some(channel),
            poll_token: None,
            fetch_token: None,
            locations: rpds::VectorSync::new_sync(),
            done: false,
            truncated: false,
            list: RowList::new(),
            preview: None,
            preview_for: None,
        }
    }

    fn keyed(mut self, key: InlayKey) -> Self {
        self.key = Some(key);
        self
    }

    /// Fold landed batches; answers whether the stream still runs.
    fn land(
        &mut self,
        store: &Store,
        ui: &UiCtx,
        batches: Vec<himark_ahp_ext_types::LocationList>,
    ) -> bool {
        let Some(channel) = &self.channel else {
            return false;
        };
        for batch in batches {
            self.done |= batch.done;
            self.truncated |= batch.truncated;
            for found in resolve_batch(channel, batch) {
                self.locations.push_back_mut(found);
            }
        }
        let (labels, trails, note) = master_rows(self.locations.iter(), self.done, self.truncated);
        let selected = self.list.selected();
        self.list.set_with_trails(store, ui, &labels, &trails, note, selected);
        !self.done
    }

    fn relaunch_poll(&mut self, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        let Some(channel) = &self.channel else {
            return;
        };
        fx.relaunch_erased(
            &mut self.poll_token,
            AnyEffect::new(crate::higent::PollLocationsEffect {
                seat: Arc::clone(&channel.seat),
                channel: channel.channel.clone(),
            })
            .map(|batches| PeekCommand::Polled { batches }),
        );
    }

    fn drop_stream(&mut self, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        if let Some(token) = self.poll_token.take() {
            fx.cancel(token);
        }
        if let Some(token) = self.fetch_token.take() {
            fx.cancel(token);
        }
        if let Some(channel) = self.channel.take() {
            let _ = fx.push(
                AnyEffect::new(crate::higent::UnsubscribeLocationsEffect {
                    seat: channel.seat,
                    channel: channel.channel,
                })
                .map(|()| PeekCommand::Nothing),
            );
        }
    }

    /// Lazily fetch the selected location's document — the ONE fetch
    /// this surface ever makes before navigation.
    fn ensure_preview(&mut self, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        let index = self.list.selected();
        if self.preview_for == Some(index) || index >= self.locations.len() {
            return;
        }
        self.preview_for = Some(index);
        self.preview = None;
        let Some(location) = self.locations.get(index).map(|found| found.location.clone()) else {
            return;
        };
        fx.relaunch_erased(
            &mut self.fetch_token,
            AnyEffect::new(crate::FetchDocumentEffect { location })
                .map(move |text| PeekCommand::Fetched { index, text }),
        );
    }

    /// The detail pane, VSCode/Fleet style: the WHOLE document in
    /// its own scrollable editor, scrolled to the match, the match
    /// washed `StyleId::Match`. Nested scrolls are fine; the card's
    /// fixed height clips.
    fn install_preview(
        &mut self,
        store: &Store,
        built: crate::BuiltDocument,
        index: usize,
        fx: &mut imba::effect::Effects<'_, PeekCommand>,
    ) {
        let Some(found) = self.locations.get(index) else {
            return;
        };
        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let mut document = built.document;

        let (hit, target) = {
            let mut view = document.text().view();
            let hit = crate::offset_at(&mut view, found.target().start) as u32;
            (hit, hit..hit + found.length.max(1))
        };

        let markup = document.add_markup();
        let mut tints = crate::Markup::new();
        tints.push_styled(target.clone(), crate::theme::StyleId::Match);
        let scoped = |command| PeekCommand::Preview(imba::scroll::ScrollCommand::Content(command));
        fx.scope(scoped, |fx| {
            document.replace_markup(markup, tints, &[target], &fonts, &theme, fx)
        });

        let detail = (self.width * 0.6 - 1.0).max(120.0);
        let editor = fx.scope(scoped, |fx| {
            document.add_editor(
                detail,
                None,
                ::editor::EditorBuild::Complete,
                &[],
                &fonts,
                &theme,
                fx,
            )
        });
        document.show_markup(editor, markup);
        let reveal = document
            .caret_content_rect(editor, hit, &fonts, &theme)
            .map(|(_, y, _, _)| (y - PEEK_HEIGHT / 3.0).max(0.0))
            .unwrap_or(0.0);
        let mut view = EditorView {
            document,
            editor,
            reports_geometry: false,
            location: None,
            gutter_width: 0.0,
            base: None,
        };
        view.set_caret(hit);
        view.blur();
        let mut preview = imba::scroll::ScrollView::new(view);
        preview.set_scroll_y(reveal);
        self.preview = Some(preview);
    }

    fn navigate(&mut self, store: &mut Store, index: usize) {
        let Some(found) = self.locations.get(index) else {
            return;
        };
        AppRequests::push(
            store,
            Arc::new(crate::hisearch::OpenFoundLocation {
                location: found.location.clone(),
                target: found.target(),
            }),
        );
    }

    fn close(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, PeekCommand>) {
        self.drop_stream(fx);
        if let (Some(host), Some(key)) = (self.host, self.key) {
            AppRequests::push(store, Arc::new(RemovePeek { document: host, key }));
        }
    }
}

impl View for PeekView {
    type Command = PeekCommand;

    fn focus_data<'w>(
        &'w self,
        _store: &'w Store,
        _ui: &'w UiCtx,
    ) -> imba::focus::FocusData<'w, PeekCommand> {
        use imba::focus::FocusData;
        let rows = self.list.len();
        FocusData {
            on_key: Some(Box::new(move |key, _mods| match key {
                InputKey::Up => EventResult::Command(PeekCommand::Select(-1)),
                InputKey::Down => EventResult::Command(PeekCommand::Select(1)),
                InputKey::Enter if rows > 0 => EventResult::Command(PeekCommand::Pick),
                InputKey::Escape => EventResult::Command(PeekCommand::Close),
                _ => EventResult::Ignored,
            })),
            ..FocusData::default()
        }
    }

    fn destroy(&mut self, _store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        self.drop_stream(fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {
            PeekCommand::Rows(command) => {
                if let Some(index) = self.list.picked(&command) {
                    self.list.select(index);
                    self.navigate(store, index);
                    self.close(store, fx);
                    return;
                }
                fx.scope(PeekCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
                self.ensure_preview(fx);
            }
            PeekCommand::Select(delta) => {
                let index = self.list.selected();
                let stepped = match delta < 0 {
                    true => index.saturating_sub(delta.unsigned_abs()),
                    false => index.saturating_add(delta as usize),
                };
                self.list.select(stepped);
                self.ensure_preview(fx);
            }
            PeekCommand::Pick => {
                let index = self.list.selected();
                self.navigate(store, index);
                self.close(store, fx);
            }
            PeekCommand::Close => self.close(store, fx),
            PeekCommand::Nothing => {}
            PeekCommand::Preview(command) => {
                if let Some(preview) = &mut self.preview {
                    fx.scope(PeekCommand::Preview, |fx| {
                        View::perform(preview, store, ui, command, fx)
                    });
                }
            }
            PeekCommand::Snapshot { outcome } => match outcome {
                Ok(snapshot) => {
                    let running = self.land(store, ui, vec![snapshot]);
                    // The trivial case shows no card: one known
                    // result navigates directly.
                    if self.done && self.locations.len() == 1 {
                        self.navigate(store, 0);
                        self.close(store, fx);
                        return;
                    }
                    if running {
                        self.relaunch_poll(fx);
                    }
                    self.ensure_preview(fx);
                }
                Err(_) => {
                    self.done = true;
                    self.truncated = true;
                    let _ = self.land(store, ui, Vec::new());
                }
            },
            PeekCommand::Polled { batches } => {
                let had = self.locations.len();
                let running = self.land(store, ui, batches);
                if self.done && self.locations.len() == 1 && had <= 1 {
                    self.navigate(store, 0);
                    self.close(store, fx);
                    return;
                }
                if running {
                    self.relaunch_poll(fx);
                }
                self.ensure_preview(fx);
            }
            PeekCommand::Fetched { index, text } => {
                if self.preview_for != Some(index) {
                    return;
                }
                let Some(text) = text else {
                    return;
                };
                let Some(found) = self.locations.get(index) else {
                    return;
                };
                let location = found.location.clone();
                fx.relaunch_erased(
                    &mut self.fetch_token,
                    AnyEffect::new(crate::BuildDocumentEffect {
                        location,
                        text,
                    })
                    .map(move |built| PeekCommand::Built { index, built }),
                );
            }
            PeekCommand::Built { index, built } => {
                if self.preview_for != Some(index) {
                    return;
                }
                self.install_preview(store, built, index, fx);
            }
        }
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let width = constraints.max.width.min(self.width).max(120.0);
            let size = Size::new(width, PEEK_HEIGHT);
            let chrome = crate::env::Themes::of(store).ui().peeker.clone();
            let fill = chrome.background.0;
            let rule = chrome.rule.0;
            let master = (width * 0.4).max(120.0);

            let mut card = imba::container::container(arena, size);
            let backdrop = imba::leaf::leaf::<PeekCommand>(width, PEEK_HEIGHT).paint_instead(
                move |_arena, canvas, rect| {
                    let mut paint = Paint::default();
                    paint.set_color(fill);
                    canvas.draw_rect(rect, &paint);
                    paint.set_color(rule);
                    for y in [rect.top, rect.bottom - 1.0] {
                        canvas.draw_rect(Rect::from_xywh(rect.left, y, rect.width(), 1.0), &paint);
                    }
                    canvas.draw_rect(
                        Rect::from_xywh(rect.left + master, rect.top, 1.0, rect.height()),
                        &paint,
                    );
                },
            );
            card.place(0.0, 0.0, backdrop);
            card.place(
                0.0,
                1.0,
                imba::Layout::layout(
                    self.list.display(arena, store, ui),
                    arena,
                    Constraints::tight(Size::new(master, PEEK_HEIGHT - 2.0)),
                )
                .map(PeekCommand::Rows),
            );
            if let Some(preview) = &self.preview {
                card.place(
                    master + 1.0,
                    1.0,
                    imba::Layout::layout(
                        preview.display(arena, store, ui),
                        arena,
                        Constraints {
                            min: Size::default(),
                            max: Size::new((width - master - 1.0).max(1.0), PEEK_HEIGHT - 2.0),
                        },
                    )
                    .map(PeekCommand::Preview),
                );
            }
            card
        })
    }
}

/// The master list's rows: the context as the label, `name:line` as
/// the trail chip, and the stream's state as the trailing note.
fn master_rows<'a>(
    locations: impl Iterator<Item = &'a FoundLocation>,
    done: bool,
    truncated: bool,
) -> (Vec<String>, Vec<Option<String>>, Option<String>) {
    let mut labels: Vec<String> = Vec::new();
    let mut trails: Vec<Option<String>> = Vec::new();
    for found in locations {
        labels.push(found.context.trim().to_owned());
        trails.push(Some(format!(
            "{}:{}",
            found.location.name(),
            found.line + 1
        )));
    }
    let note = match (done, truncated, labels.is_empty()) {
        (false, _, _) => Some("searching…".to_owned()),
        (true, true, _) => Some("cut off".to_owned()),
        (true, false, true) => Some("no references".to_owned()),
        _ => None,
    };
    (labels, trails, note)
}

struct RemovePeek {
    document: crate::DocumentId,
    key: InlayKey,
}

impl crate::DynamicCommand for RemovePeek {
    fn id(&self) -> &'static str {
        "peek.remove"
    }

    fn name(&self) -> String {
        "Close Reference Peek".to_owned()
    }

    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        _window: crate::WindowId,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(mut doc) = crate::OpenDocuments::document(store, self.document) else {
            return;
        };
        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let document = self.document;
        fx.scope(
            move |command| crate::AppCommand::Entity(document, command),
            |fx| doc.remove_inlay(self.key, &fonts, &theme, fx),
        );
        crate::OpenDocuments::put_document(store, self.document, doc);
    }
}

fn peek_markup() -> crate::MarkupId {
    static ID: std::sync::OnceLock<crate::MarkupId> = std::sync::OnceLock::new();
    *ID.get_or_init(crate::MarkupId::mint)
}

pub struct GoToReference;

impl crate::DynamicEditorCommand for GoToReference {
    fn id(&self) -> &'static str {
        "code.go-to-reference"
    }

    fn name(&self) -> String {
        "Go to Reference".to_owned()
    }

    fn perform(
        &self,
        store: &mut Store,
        document: &mut Document,
        editor: crate::EditorId,
        location: &crate::ResourceLocation,
        payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        fx: &mut imba::effect::Effects<'_, EditorCommand>,
    ) {
        let Some(payload) = payload else {
            let caret = document.caret_byte(editor) as usize;
            let mut view = document.text().view();
            let position = crate::line_col_at(&mut view, caret);
            let _ = fx.push(
                AnyEffect::new(crate::LspLocationsEffect {
                    location: location.clone(),
                    position,
                    kind: crate::LspLocationsKind::References,
                })
                .map(move |outcome| EditorCommand::Dynamic {
                    id: "code.go-to-reference",
                    payload: Some(Box::new(outcome)),
                }),
            );
            return;
        };
        let Ok(outcome) = payload.downcast::<Result<LocationsChannel, String>>() else {
            return;
        };
        let Ok(channel) = *outcome else {
            return;
        };

        let fonts = crate::env::Fonts::of(store)();
        let theme = crate::env::Themes::of(store);
        let width = match document.layout_width(editor) {
            width if width > 1.0 => width,
            _ => FALLBACK_WIDTH,
        };
        let host = crate::OpenDocuments::by_location(store, location);
        let caret = document.caret_byte(editor);
        let anchor = caret..caret;

        let view = PeekView::new(host, width, channel.clone());
        let markup = peek_markup();
        document.ensure_document_markup(markup);
        let key = document.push_inlay(
            markup,
            anchor.clone(),
            Inlay::new(InlayMode::Under, view.clone()),
            &fonts,
            &theme,
            fx,
        );
        document.swap_inlay(key, anchor, Inlay::new(InlayMode::Under, view.keyed(key)));
        document.set_focus(editor, EditorFocus::Inlay(key));

        // The stream lands into the card by its key.
        let _ = fx.push(
            AnyEffect::new(crate::higent::SubscribeLocationsEffect {
                seat: channel.seat,
                channel: channel.channel,
            })
            .map(move |outcome| EditorCommand::Inlay {
                key,
                command: Box::new(PeekCommand::Snapshot { outcome }),
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, line: u32, context: &str) -> FoundLocation {
        FoundLocation {
            location: crate::ResourceLocation::new(
                crate::ResourceType::document(),
                crate::Authority::new("local"),
                vec!["work".to_owned(), name.to_owned()],
            ),
            line,
            column: 2,
            length: 3,
            context: context.to_owned(),
            context_column_start: 0,
        }
    }

    #[test]
    fn master_rows_carry_context_position_and_stream_state() {
        let rows = [found("a.rs", 4, "  let x = y;"), found("b.rs", 0, "fn b()")];

        let (labels, trails, note) = master_rows(rows.iter(), false, false);
        assert_eq!(labels, ["let x = y;", "fn b()"], "contexts trimmed");
        assert_eq!(
            trails,
            [Some("a.rs:5".to_owned()), Some("b.rs:1".to_owned())],
            "1-based positions"
        );
        assert_eq!(note.as_deref(), Some("searching…"));

        let (_, _, note) = master_rows(rows.iter(), true, false);
        assert_eq!(note, None, "a settled stream needs no note");
        let (_, _, note) = master_rows(rows.iter(), true, true);
        assert_eq!(note.as_deref(), Some("cut off"));
        let (labels, _, note) = master_rows([].iter(), true, false);
        assert!(labels.is_empty());
        assert_eq!(note.as_deref(), Some("no references"));
    }
}
