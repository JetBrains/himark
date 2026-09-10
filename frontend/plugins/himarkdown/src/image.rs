use std::ops::Range;
use std::sync::Arc;

use himark::{
    EnrichCx, EnrichFuture, EnrichInput, Enricher, EnricherId, Enrichment,
    FetchResourceBytesEffect, Inlay, InlayMode, Markup,
};
use hisitter::TsTree;
use imba::{
    arena::Arena, constraints::Constraints, image::ImageView, store::Store, Thunk, UiCtx, View,
};

#[derive(Clone, Debug, PartialEq)]
struct ImageRef {
    range: Range<u32>,
    reference: String,
}

#[derive(Clone)]
pub struct ImageInlay {
    reference: Arc<str>,
    view: ImageView,
}

impl ImageInlay {
    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub fn intrinsic(&self) -> skia_safe::Size {
        self.view.intrinsic()
    }
}

impl View for ImageInlay {
    type Command = imba::image::ImageCommand;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
        match command {}
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        self.view.layout(arena, store, ui, constraints)
    }
}

pub struct ImageEnricher;

impl Enricher for ImageEnricher {
    fn id(&self) -> EnricherId {
        EnricherId("markdown-image")
    }

    fn derive<'a>(&'a self, input: &'a EnrichInput, cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
        Box::pin(async move { derive(input, cx).await })
    }
}

async fn derive(input: &EnrichInput, cx: &EnrichCx<'_>) -> Enrichment {
    let Some(origin) = input.base.clone() else {
        return Enrichment::none();
    };
    let mut builder = Markup::builder();
    let mut changed: Vec<Range<u32>> = Vec::new();

    for range in &input.changed {
        if !input.previous.all_inlays_in(range.clone()).is_empty() && !changed.contains(range) {
            changed.push(range.clone());
        }
    }
    for image in image_refs(input) {
        let view = match loaded(&input.previous, &image) {
            Some(view) => view,
            None => {
                let bytes = match cx
                    .caller
                    .call(FetchResourceBytesEffect {
                        origin: origin.clone(),
                        reference: image.reference.clone(),
                    })
                    .await
                {
                    Some(Some(bytes)) => bytes,
                    _ => continue,
                };

                let Some(view) = ImageView::new(bytes) else {
                    continue;
                };
                ImageInlay {
                    reference: image.reference.as_str().into(),
                    view,
                }
            }
        };
        builder.push_inlay(image.range.clone(), Inlay::new(InlayMode::Under, view));
        if !changed.contains(&image.range) {
            changed.push(image.range);
        }
    }
    changed.sort_by_key(|range| range.start);
    Enrichment {
        replacement: builder,
        changed,
    }
}

fn loaded(previous: &Markup, image: &ImageRef) -> Option<ImageInlay> {
    previous
        .all_inlays_in(image.range.clone())
        .into_iter()
        .find_map(|interval| {
            let inlay = interval.inlay.view_as::<ImageInlay>()?;
            (&*inlay.reference == image.reference.as_str()).then(|| inlay.clone())
        })
}

fn image_refs(input: &EnrichInput) -> Vec<ImageRef> {
    let Some(tree) = input.syntax.tree.as_deref().and_then(TsTree::of) else {
        return Vec::new();
    };
    let byte_count = input.text.byte_count().min(u32::MAX as usize) as u32;
    let mut view = input.text.view();
    let mut refs = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "inline" {
            let start = (node.start_byte() as u32).min(byte_count);
            let end = (node.end_byte() as u32).min(byte_count);
            let touched = input
                .changed
                .iter()
                .any(|range| start <= range.end && range.start <= end);
            if touched && start < end {
                let text = view.byte_string(start as usize, end as usize);
                refs.extend(scan_images(&text, start));
            }
            continue;
        }
        for index in 0..node.child_count() {
            if let Some(child) = node.child(index as u32) {
                stack.push(child);
            }
        }
    }
    refs.sort_by_key(|image| image.range.start);
    refs
}

fn scan_images(text: &str, base: u32) -> Vec<ImageRef> {
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut at = 0usize;
    while let Some(bang) = text[at..].find("![").map(|index| at + index) {
        let Some(close) = text[bang + 2..].find("](").map(|index| bang + 2 + index) else {
            break;
        };
        let target_start = close + 2;
        let Some(paren) = text[target_start..]
            .find(')')
            .map(|index| target_start + index)
        else {
            break;
        };
        let target = target(&text[target_start..paren]);
        if !target.is_empty() {
            refs.push(ImageRef {
                range: base + bang as u32..base + paren as u32 + 1,
                reference: target,
            });
        }
        at = paren + 1;
        if at >= bytes.len() {
            break;
        }
    }
    refs
}

fn target(destination: &str) -> String {
    let trimmed = destination.trim();
    let bare = match trimmed.strip_prefix('<') {
        Some(rest) => rest.split('>').next().unwrap_or(rest),
        None => trimmed.split_whitespace().next().unwrap_or(""),
    };
    bare.trim().to_owned()
}

#[cfg(test)]
mod tests;
