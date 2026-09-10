mod inlay;
mod tree_demo;

pub use inlay::add_badges;
pub use tree_demo::{OpenTreeDemo, TreeDemoView};

use himark::Document;

const SAMPLE: &str = include_str!("../sample.md");
const SAMPLE_REPETITIONS: usize = 1_409;

pub fn monster_document(
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Document {
    let (mut document, blocks) =
        himarkdown::markdown_document(&SAMPLE.repeat(SAMPLE_REPETITIONS), fonts, theme);

    add_badges(&mut document, &blocks, fonts, theme);
    document
}

pub fn wall_of_text_document(
    _fonts: &skia_safe::textlayout::FontCollection,
    _theme: &himark::Theme,
) -> Document {
    wall_of_text(1_000_000)
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
