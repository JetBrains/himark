// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

mod inlay;
mod tree_demo;

pub use inlay::add_badges;
pub use tree_demo::{OpenTreeDemo, TreeDemoView};

use himark::Document;

const SAMPLE: &str = include_str!("../sample.md");
const SAMPLE_REPETITIONS: usize = 1_409;

pub fn monster_document(
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Document {
    let (mut document, blocks) =
        himarkdown::markdown_document(&SAMPLE.repeat(SAMPLE_REPETITIONS), store, ui, fonts, theme);

    add_badges(&mut document, &blocks, store, ui, fonts, theme);
    document
}

pub fn wall_of_text_document(
    _store: &imba::store::Store,
    _ui: &imba::UiCtx,
    _fonts: &skia_safe::textlayout::FontCollection,
    _theme: &himark::Theme,
) -> Document {
    wall_of_text(1_000_000)
}

pub fn demo_location(name: &str) -> himark::ResourceLocation {
    himark::ResourceLocation::new(
        himark::ResourceType::document(),
        himark::Authority::new("demo"),
        vec![name.to_owned()],
    )
}

pub struct OpenMonsterDemo;

impl himark::DynamicCommand for OpenMonsterDemo {
    fn id(&self) -> &'static str {
        "demo.open-document"
    }
    fn name(&self) -> String {
        "Open Demo Document".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        _store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        fx.push(himark::open_effect(
            window,
            "torture sample".to_owned(),
            true,
            Some(demo_location("torture sample")),
            Box::new(monster_document),
        ));
    }
}

pub struct OpenWallOfTextDemo;

impl himark::DynamicCommand for OpenWallOfTextDemo {
    fn id(&self) -> &'static str {
        "demo.open-wall-of-text"
    }
    fn name(&self) -> String {
        "Open Demo Wall of Text".to_owned()
    }
    fn perform(
        &self,
        _app: &mut himark::Application,
        _store: &mut imba::store::Store,
        window: himark::WindowId,
        fx: &mut himark::AppFx<'_>,
    ) {
        fx.push(himark::open_effect(
            window,
            "wall of text".to_owned(),
            true,
            Some(demo_location("wall of text")),
            Box::new(wall_of_text_document),
        ));
    }
}

pub fn wall_of_text(lines: usize) -> Document {
    let unit = "0000 :: lorem ipsum dolor sit amet :: 00 ";
    let mut block = String::new();
    let lengths: [usize; 16] = [1, 3, 1, 7, 2, 1, 12, 1, 3, 2, 24, 1, 2, 6, 1, 48];
    for (index, repeats) in lengths.iter().enumerate() {
        block.push_str(&format!("line {index:04} "));
        for _ in 0..*repeats {
            block.push_str(unit);
        }
        block.push('\n');
    }
    let source = block.repeat(lines.max(lengths.len()) / lengths.len());
    Document::new(
        himark::Text::from_string_exact(&source),
        himark::Markup::new(),
    )
}

#[cfg(test)]
mod tests;
