// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

use documents::entity_view::EditorIdView;
use documents::FetchDocumentEffect;
use documents::OpenDocuments;
use editor::document::Document;
use editor::enrich::EnrichCx;
use editor::enrich::EnrichFuture;
use editor::enrich::EnrichInput;
use editor::enrich::Enricher;
use editor::enrich::EnricherId;
use editor::enrich::Enrichment;
use editor::location::ResourceLocation;
use editor::location::ResourceType;
use editor::markup::Inlay;
use editor::markup::InlayMode;
use editor::markup::InsteadKind;
use editor::markup::Markup;
use editor::reparse::SyntaxLanguages;
use editor::theme::StyleId;
use hisitter::TsTree;
use imba::{arena::Arena, constraints::Constraints, store::Store, ui::UiCtx, View};
use text::text::Text;

struct FenceRef {
    block: Range<u32>,
    language: String,
    path: String,

    lines: Option<(u32, u32)>,
}

const EMBED_WIDTH: f32 = 720.0;

struct Prepared {
    document: Document,
    layout: editor::document_layout::DocumentLayout,
}

#[derive(Clone)]
pub(crate) struct EmbedPending {
    location: ResourceLocation,
    content: String,
    language: String,
    lines: Option<(u32, u32)>,

    window: Option<Range<u32>>,
    prepared: std::sync::Arc<std::sync::Mutex<Option<Prepared>>>,
}

impl View for EmbedPending {
    type Command = std::convert::Infallible;
    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {}
    }
    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::fixed(imba::leaf::leaf(0.0, 0.0))
    }
}

#[derive(Clone, Copy)]
pub struct EmbedView {
    view: EditorIdView,
    height: f32,

    fragments: Option<editor::document::FragmentSetId>,
}

impl EmbedView {
    pub fn documents(&self) -> imba::store::Id<documents::OpenDocuments> {
        self.view.documents()
    }

    pub fn document(&self) -> documents::DocumentId {
        self.view.document()
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    pub fn editor(&self) -> editor::editor::EditorId {
        self.view.editor()
    }
}

impl View for EmbedView {
    type Command = editor::editor_view::EditorCommand;

    fn destroy(&mut self, store: &mut Store, fx: &mut imba::effect::Effects<'_, Self::Command>) {
        if let Some(set) = self.fragments.take() {
            if let Some(mut document) =
                OpenDocuments::document(store, self.view.documents(), self.view.document())
            {
                document.remove_fragment_set(set);
                OpenDocuments::put_document(
                    store,
                    self.view.documents(),
                    self.view.document(),
                    document,
                );
            }
        }
        View::destroy(&mut self.view, store, fx);
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        View::perform(&mut self.view, store, ui, command, fx);

        self.height = self.live_height(store).unwrap_or(self.height);
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        EmbedFrame {
            embed: self,
            store,
            ui,
        }
    }
}

impl EmbedView {
    fn live_height(&self, store: &Store) -> Option<f32> {
        let document =
            OpenDocuments::document_ref(store, self.view.documents(), self.view.document())?;
        document
            .has_editor(self.view.editor())
            .then(|| document.content_height(self.view.editor()))
    }
}

impl editor::markup::InlayEditing for EmbedView {
    fn take_edit(&mut self) -> Option<operation::operation::Operation> {
        None
    }

    fn set_range(&mut self, _range: Range<u32>) {}

    fn adopt_from(
        &mut self,
        previous: &Self,
        _store: &imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &editor::theme::Theme,
    ) -> bool {
        *self = *previous;
        true
    }

    fn passive(&self, command: &editor::markup::InlayCommand) -> bool {
        matches!(
            command.downcast_ref::<editor::editor_view::EditorCommand>(),
            Some(
                editor::editor_view::EditorCommand::ApplyRepair(_)
                    | editor::editor_view::EditorCommand::ApplyReparse(_)
                    | editor::editor_view::EditorCommand::ApplyEnrichment(_)
                    | editor::editor_view::EditorCommand::Retheme { .. }
                    | editor::editor_view::EditorCommand::Viewport { .. }
            )
        )
    }
}

pub struct FenceEmbedEnricher;

impl Enricher for FenceEmbedEnricher {
    fn id(&self) -> EnricherId {
        EnricherId("markdown-fence-embed")
    }

    fn derive<'a>(&'a self, input: &'a EnrichInput, cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
        Box::pin(async move { derive(input, cx).await })
    }

    fn install(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        replacement: &mut Markup,
        changed: &[Range<u32>],
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &editor::theme::Theme,
    ) {
        install(store, ui, replacement, changed, fonts, theme);
    }
}

async fn derive(input: &EnrichInput, cx: &EnrichCx<'_>) -> Enrichment {
    let Some(base) = input.base.clone() else {
        return Enrichment::none();
    };
    let refs = fence_refs(input);
    let mut builder = Markup::builder();
    let mut changed: Vec<Range<u32>> = Vec::new();

    for range in &input.changed {
        if !input.previous.all_inlays_in(range.clone()).is_empty() && !changed.contains(range) {
            changed.push(range.clone());
        }
    }
    for fence in refs {
        let Some(location) = resolve(&base, &fence.path) else {
            continue;
        };

        let content = match cx
            .caller
            .call(FetchDocumentEffect {
                location: location.clone(),
            })
            .await
        {
            Some(Some(content)) => content,
            _ => continue,
        };

        let document = cx.measure.with_ctx(|store, ui| {
            build_document(
                &content,
                &fence.language,
                cx.languages.as_deref(),
                store,
                ui,
                cx.fonts,
                cx.theme,
            )
        });
        let window = fence
            .lines
            .and_then(|(from, to)| line_window(document.text(), from, to));
        let layout = {
            let globals: Vec<(editor::markup::MarkupId, &Markup)> =
                document.document_scoped_markups().collect();
            cx.measure.measure(EMBED_WIDTH, |measure| {
                editor::document_layout::DocumentLayout::build_complete(
                    document.text(),
                    editor::markup::OverlaidMarkup::new(document.markup(), &globals),
                    measure,
                    cx.fonts,
                    cx.theme,
                    window.clone(),
                )
            })
        };
        builder.push_inlay(
            fence.block.clone(),
            Inlay::new(
                InlayMode::Instead(InsteadKind::FullLine),
                EmbedPending {
                    location,
                    content,
                    language: fence.language,
                    lines: fence.lines,
                    window,
                    prepared: std::sync::Arc::new(std::sync::Mutex::new(Some(Prepared {
                        document,
                        layout,
                    }))),
                },
            ),
        );
        if !changed.contains(&fence.block) {
            changed.push(fence.block);
        }
    }
    changed.sort_by_key(|range| range.start);
    Enrichment {
        replacement: builder,
        changed,
    }
}

fn install(
    store: &mut Store,
    ui: &imba::ui::UiCtx,
    replacement: &mut Markup,
    changed: &[Range<u32>],
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &editor::theme::Theme,
) {
    let pending: Vec<(editor::markup::InlayKey, Range<u32>, EmbedPending)> = changed
        .iter()
        .flat_map(|range| replacement.all_inlays_in(range.clone()))
        .filter_map(|interval| {
            let embed = interval.inlay.view_as::<EmbedPending>()?.clone();
            Some((interval.key, interval.range.clone(), embed))
        })
        .collect();
    let languages = ::editor::env::Parsers::of(store);
    for (key, range, embed) in pending {
        // The embed's temp documents file under the location's owner
        // (synthetic embed locations fall to the local session).
        let home = ahp_wire::SessionId::of_location(store, &embed.location);
        let documents = ahp_session::session::state::Hosts::ensure_state(store, &home).documents();
        let (id, prebuilt, carried_window) =
            match OpenDocuments::by_location(store, documents, &embed.location) {
                Some(id) => (id, None, None),
                None => {
                    let prepared = embed
                        .prepared
                        .lock()
                        .ok()
                        .and_then(|mut prepared| prepared.take());
                    match prepared {
                        Some(Prepared { document, layout }) => {
                            let revision = document.revision();
                            let id = OpenDocuments::register(
                                store,
                                documents,
                                document,
                                Some(embed.location.clone()),
                                embed.location.name().to_owned(),
                                revision,
                            );
                            (id, Some(layout), embed.window.clone())
                        }

                        None => {
                            let document = build_document(
                                &embed.content,
                                &embed.language,
                                languages.as_deref(),
                                store,
                                ui,
                                fonts,
                                theme,
                            );
                            let revision = document.revision();
                            let id = OpenDocuments::register(
                                store,
                                documents,
                                document,
                                Some(embed.location.clone()),
                                embed.location.name().to_owned(),
                                revision,
                            );
                            (id, None, None)
                        }
                    }
                }
            };

        let Some(mut document) = OpenDocuments::document(store, documents, id) else {
            continue;
        };
        let window = match (&prebuilt, carried_window) {
            (Some(_), carried) => carried,
            (None, _) => embed
                .lines
                .and_then(|(from, to)| line_window(document.text(), from, to)),
        };
        let (bounds, fragments) = match window {
            Some(window) => {
                let set = document.add_fragment_set();
                let fragment = document.add_fragment(set, window);
                (Some(fragment), Some(set))
            }
            None => (None, None),
        };
        let build = match prebuilt {
            Some(layout) => editor::document::EditorBuild::Prebuilt(layout),
            None => editor::document::EditorBuild::Complete,
        };
        let mut batch = imba::effect::Batch::new();
        let editor = document.add_editor(
            EMBED_WIDTH,
            bounds,
            build,
            &[],
            store,
            ui,
            fonts,
            theme,
            &mut batch.effects(),
        );
        let height = document.content_height(editor);
        OpenDocuments::put_document(store, documents, id, document);
        let view = EmbedView {
            view: EditorIdView::new(documents, id, editor),
            height,
            fragments,
        };
        replacement.replace_inlay_at(
            key,
            range,
            Inlay::editing(InlayMode::Instead(InsteadKind::FullLine), view),
        );
    }
}

fn line_window(text: &Text, from: u32, to: u32) -> Option<Range<u32>> {
    let from = from.max(1) as usize;
    let to = (to as usize).max(from);
    let whole = text.byte_string(0, text.byte_count());
    let mut start = None;
    let mut end = whole.len();
    let mut line = 1usize;
    let mut offset = 0usize;
    for segment in whole.split_inclusive('\n') {
        if line == from {
            start = Some(offset);
        }
        if line == to {
            end = offset + segment.trim_end_matches(['\n', '\r']).len();
            break;
        }
        offset += segment.len();
        line += 1;
    }
    let start = start? as u32;
    Some(start..(end as u32).max(start))
}

fn fence_refs(input: &EnrichInput) -> Vec<FenceRef> {
    let Some(tree) = input.syntax.tree.as_deref().and_then(TsTree::of) else {
        return Vec::new();
    };
    let byte_count = input.text.byte_count().min(u32::MAX as usize) as u32;
    let mut view = input.text.view();
    let mut refs = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "fenced_code_block" {
            let block = (node.start_byte() as u32).min(byte_count)
                ..(node.end_byte() as u32).min(byte_count);
            let touched = input
                .changed
                .iter()
                .any(|range| block.start <= range.end && range.start <= block.end);
            if touched {
                if let Some(info) = fence_info(&mut view, node) {
                    let mut words = info.split_whitespace();
                    if let (Some(language), Some(spec)) = (words.next(), words.next()) {
                        let (path, lines) = split_fragment(spec);
                        if !path.is_empty() {
                            refs.push(FenceRef {
                                block,
                                language: language.to_owned(),
                                path: path.to_owned(),
                                lines,
                            });
                        }
                    }
                }
            }
            continue;
        }
        for index in 0..node.child_count() {
            if let Some(child) = node.child(index as u32) {
                stack.push(child);
            }
        }
    }
    refs
}

fn fence_info(view: &mut text::text_view::TextView, node: tree_sitter::Node) -> Option<String> {
    for index in 0..node.child_count() {
        let child = node.child(index as u32)?;
        if child.kind() == "info_string" {
            return Some(view.byte_string(child.start_byte(), child.end_byte()));
        }
    }
    None
}

fn split_fragment(spec: &str) -> (&str, Option<(u32, u32)>) {
    let Some((path, fragment)) = spec.split_once('#') else {
        return (spec, None);
    };
    let digits = fragment.trim_start_matches(['L', 'l']);
    let lines = match digits.split_once('-') {
        Some((a, b)) => a.parse::<u32>().ok().zip(b.parse::<u32>().ok()),
        None => digits.parse::<u32>().ok().map(|a| (a, a)),
    };
    (path, lines)
}

fn resolve(base: &ResourceLocation, path: &str) -> Option<ResourceLocation> {
    let mut segments: Vec<String> = base.path().to_vec();
    segments.pop();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            part => segments.push(part.to_owned()),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(ResourceLocation::new(
        ResourceType::document(),
        base.authority().clone(),
        segments,
    ))
}

fn build_document(
    content: &str,
    language: &str,
    languages: Option<&SyntaxLanguages>,
    store: &imba::store::Store,
    ui: &imba::ui::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &editor::theme::Theme,
) -> Document {
    let text = Text::from_string_exact(content);
    if let Some(languages) = languages {
        if languages.knows(language) {
            return Document::from_language(text, language, languages, store, ui, fonts, theme);
        }
    }

    let byte_count = text.byte_count().min(u32::MAX as usize) as u32;
    let mut markup = Markup::new();
    markup.push_styled_covering(0..byte_count, StyleId::SourceCode);
    Document::new(text, markup)
}

#[cfg(test)]
mod tests;

/// The fenced-code embed pane, reified: the live document height,
/// the pane clamped to the incoming width.
struct EmbedFrame<'a> {
    embed: &'a EmbedView,
    store: &'a Store,
    ui: &'a UiCtx,
}

impl imba::layout::LayoutValue for EmbedFrame<'_> {}

impl<'a> imba::layout::Layout<'a, editor::editor_view::EditorCommand> for EmbedFrame<'a> {
    fn layout(
        self,
        arena: &'a Arena,
        constraints: Constraints,
    ) -> imba::ThunkBox<'a, editor::editor_view::EditorCommand> {
        let EmbedFrame { embed, store, ui } = self;
        let height = embed.live_height(store).unwrap_or(embed.height).max(1.0);
        let width = match constraints.max.width.is_finite() {
            true => constraints.max.width,
            false => constraints.min.width,
        }
        .max(60.0);
        let mut pane = imba::container::container(arena, skia_safe::Size::new(width, height));
        pane.place(
            0.0,
            0.0,
            imba::layout::Layout::layout(embed.view.display(arena, store, ui), arena, constraints),
        );
        imba::ThunkBox::new(arena, pane)
    }
}
