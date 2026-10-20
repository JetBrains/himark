// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

pub mod inlay;
pub mod tree_demo;

use editor::document::Document;

const SAMPLE: &str = include_str!("../sample.md");
const SAMPLE_REPETITIONS: usize = 1_409;

pub fn monster_document(
    store: &imba::store::Store,
    ui: &imba::ui::UiCtx,
    theme: &editor::theme::Theme,
) -> Document {
    let (mut document, blocks) =
        himarkdown::markdown_document(&SAMPLE.repeat(SAMPLE_REPETITIONS), store, ui, theme);

    inlay::add_badges(&mut document, &blocks, store, ui, theme);
    document
}

pub fn wall_of_text_document(
    _store: &imba::store::Store,
    _ui: &imba::ui::UiCtx,
    _theme: &editor::theme::Theme,
) -> Document {
    wall_of_text(1_000_000)
}

pub fn demo_location(name: &str) -> editor::location::ResourceLocation {
    editor::location::ResourceLocation::new(
        editor::location::ResourceType::document(),
        editor::location::Authority::new("demo"),
        vec![name.to_owned()],
    )
}

/// The demo documents as a plugin: the three palette commands.
pub struct DemoPlugin;

impl himark::plugin::Plugin for DemoPlugin {
    fn install(&self, app: &mut himark::app::Himark, _cx: &himark::plugin::PluginContext) {
        use himark::app_ext::AppExt;
        app.register_command(std::sync::Arc::new(tree_demo::OpenTreeDemo));
        app.register_command(std::sync::Arc::new(OpenMonsterDemo));
        app.register_command(std::sync::Arc::new(OpenWallOfTextDemo));
    }
}

pub struct OpenMonsterDemo;

impl himark::commands::HimarkCommand for OpenMonsterDemo {
    fn id(&self) -> &'static str {
        "demo.open-document"
    }
    fn name(&self) -> String {
        "Open Demo Document".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let documents = himark::workspace::session_state(store, window)
            .expect("a demo document opens into a window with a session")
            .documents();
        fx.push(himark::app::open_effect(
            window,
            documents,
            "torture sample".to_owned(),
            true,
            Some(demo_location("torture sample")),
            Box::new(monster_document),
        ));
    }
}

pub struct OpenWallOfTextDemo;

impl himark::commands::HimarkCommand for OpenWallOfTextDemo {
    fn id(&self) -> &'static str {
        "demo.open-wall-of-text"
    }
    fn name(&self) -> String {
        "Open Demo Wall of Text".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        bound: &imba::focus::Bindings,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(window) = himark::commands::bound_window(self.id(), bound) else {
            return;
        };
        let documents = himark::workspace::session_state(store, window)
            .expect("a demo document opens into a window with a session")
            .documents();
        fx.push(himark::app::open_effect(
            window,
            documents,
            "wall of text".to_owned(),
            true,
            Some(demo_location("wall of text")),
            Box::new(wall_of_text_document),
        ));
    }
}

pub(crate) fn wall_of_text(lines: usize) -> Document {
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
        text::text::Text::from_string_exact(&source),
        editor::markup::Markup::new(),
    )
}

#[cfg(test)]
mod tests;
