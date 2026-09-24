// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;
use std::sync::Arc;

use himark::{
    Enricher, EnricherId, Enrichers, Enrichment, Inlay, InlayMode, SyntaxLanguage, SyntaxLanguages,
    SyntaxTree,
};
use imba::{
    arena::Arena, constraints::Constraints, store::Store, svg::SvgView, thunk_ext::ThunkExt, UiCtx,
    View,
};
use skia_safe::{Canvas, Rect, Size};
use text::Text;

pub fn register(registry: &mut SyntaxLanguages) {
    registry.register(&["mermaid", "mmd"], Arc::new(MermaidLanguage));
}

pub fn register_enricher(enrichers: &mut Enrichers) {
    enrichers.register(Arc::new(MermaidEnricher));
}

const MERMAID_NAMES: [&str; 2] = ["mermaid", "mmd"];

pub struct MermaidEnricher;

impl Enricher for MermaidEnricher {
    fn id(&self) -> EnricherId {
        EnricherId("mermaid")
    }

    fn derive<'a>(
        &'a self,
        input: &'a himark::EnrichInput,
        _cx: &'a himark::EnrichCx<'a>,
    ) -> himark::EnrichFuture<'a> {
        let mut builder = himark::Markup::builder();

        let mut changed: Vec<Range<u32>> = Vec::new();
        let byte_count = input.text.byte_count().min(u32::MAX as usize) as u32;
        let mut subjects: Vec<Range<u32>> = Vec::new();
        if MERMAID_NAMES.contains(&input.syntax.language.as_str()) {
            if byte_count > 0 {
                subjects.push(0..byte_count);
            }
        } else {
            for range in &input.changed {
                for (key, marker) in input.syntax.markup.syntax_in(range.clone()) {
                    let mermaid = input
                        .syntax
                        .markup
                        .syntax(key)
                        .is_some_and(|child| MERMAID_NAMES.contains(&child.language.as_str()));
                    if mermaid && marker.start < marker.end && !subjects.contains(&marker) {
                        subjects.push(marker);
                    }
                }
            }
        }
        for range in &input.changed {
            if !input.previous.all_inlays_in(range.clone()).is_empty() && !changed.contains(range) {
                changed.push(range.clone());
            }
        }
        let mut view = input.text.view();
        for subject in subjects {
            let source = view.byte_string(subject.start as usize, subject.end as usize);
            builder.push_inlay(
                subject.clone(),
                Inlay::new(InlayMode::Under, MermaidView::render(&source)),
            );
            if !changed.contains(&subject) {
                changed.push(subject);
            }
        }
        changed.sort_by_key(|range| range.start);
        himark::enrich_ready(Enrichment {
            replacement: builder,
            changed,
        })
    }
}

struct MermaidLanguage;

struct MermaidParse;

impl SyntaxTree for MermaidParse {
    fn clone_tree(&self) -> Box<dyn SyntaxTree> {
        Box::new(MermaidParse)
    }

    fn edit(&mut self, _operation: &operation::Operation, _view: &mut text::TextView, _base: u32) {}

    fn changed_since(&self, _old: &dyn SyntaxTree) -> Option<Vec<Range<u32>>> {
        Some(Vec::new())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl SyntaxLanguage for MermaidLanguage {
    fn parse(
        &self,
        _text: &Text,
        _range: Range<u32>,
        _old: Option<&dyn SyntaxTree>,
    ) -> Option<Box<dyn SyntaxTree>> {
        Some(Box::new(MermaidParse))
    }

    fn markup_for_changes(
        &self,
        _text: &Text,
        _range: Range<u32>,
        _tree: &dyn SyntaxTree,
        _changed: &[Range<u32>],
        _replacement: &mut himark::MarkupBuilder,
        _invalidated: &mut Vec<Range<u32>>,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &himark::Theme,
    ) {
    }
}

#[derive(Clone)]
pub struct MermaidView {
    outcome: Outcome,
}

#[derive(Clone)]
enum Outcome {
    Diagram(SvgView),

    Error(Arc<str>),
}

#[cfg(test)]
static RENDER_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

const ERROR_STRIP_HEIGHT: f32 = 24.0;
const DIAGRAM_PAD: f32 = 8.0;

const DIAGRAM_SCALE: f32 = 2.0;

impl MermaidView {
    pub fn render(source: &str) -> Self {
        #[cfg(test)]
        RENDER_LOG
            .lock()
            .expect("render log")
            .push(source.to_owned());

        let renderer = merman::svg::HeadlessRenderer::new()
            .with_svg_pipeline(merman::svg::SvgPipeline::resvg_safe());
        let outcome = match renderer.render_svg_sync(source) {
            Ok(Some(svg)) => {
                let intrinsic = SvgView::intrinsic_size(&svg).unwrap_or(Size::new(320.0, 180.0));
                Outcome::Diagram(SvgView::new(svg, intrinsic).with_scale_limit(DIAGRAM_SCALE))
            }
            Ok(None) => Outcome::Error("mermaid: unrecognized diagram type".into()),
            Err(error) => Outcome::Error(format!("mermaid: {error}").into()),
        };
        Self { outcome }
    }

    pub fn is_diagram(&self) -> bool {
        matches!(self.outcome, Outcome::Diagram(_))
    }

    pub fn svg(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Diagram(svg) => Some(svg.svg()),
            Outcome::Error(_) => None,
        }
    }

    fn scaled(&self, width: f32) -> Size {
        match &self.outcome {
            Outcome::Diagram(svg) => {
                let drawn = svg.scaled((width - DIAGRAM_PAD * 2.0).max(60.0));
                Size::new(
                    drawn.width + DIAGRAM_PAD * 2.0,
                    drawn.height + DIAGRAM_PAD * 2.0,
                )
            }
            Outcome::Error(_) => Size::new(width.max(60.0), ERROR_STRIP_HEIGHT),
        }
    }

    fn paint(&self, ui: &UiCtx, canvas: &Canvas, rect: Rect) {
        match &self.outcome {
            Outcome::Diagram(svg) => {
                let inner = Rect::from_xywh(
                    rect.left + DIAGRAM_PAD,
                    rect.top + DIAGRAM_PAD,
                    (rect.width() - DIAGRAM_PAD * 2.0).max(1.0),
                    (rect.height() - DIAGRAM_PAD * 2.0).max(1.0),
                );
                svg.paint(canvas, inner);
            }
            Outcome::Error(message) => {
                let font = himark::fonts::ui_text_font(ui, 12.0);
                imba::TextShaper::of(ui).draw(
                    canvas,
                    &font,
                    message.as_ref(),
                    skia_safe::Color::from_argb(0xA0, 0x80, 0x80, 0x80),
                    0.0,
                    rect.left + DIAGRAM_PAD,
                    rect.top + ERROR_STRIP_HEIGHT * 0.5 + 4.0,
                );
            }
        }
    }
}

pub enum MermaidCommand {}

impl View for MermaidView {
    type Command = MermaidCommand;
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
        ui: &'a UiCtx,
    ) -> impl imba::Layout<'a, Self::Command> + imba::LayoutValue + 'a {
        imba::laid(move |_arena: &'a Arena, constraints: Constraints| {
            let size = self.scaled(constraints.max.width);
            imba::leaf::leaf(size.width, size.height)
                .paint_instead(move |_, canvas, rect| self.paint(ui, canvas, rect))
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod render_cost_probe;

#[cfg(test)]
mod bounded_tail_probe;

#[cfg(test)]
mod svg_shape_probe;

#[cfg(test)]
mod diagram_pixels;

#[cfg(test)]
mod incremental_renders;
