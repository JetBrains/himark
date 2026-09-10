use super::*;
use editor::StyleId;

trait RunReparse {
    fn run_reparse(self) -> editor::ReparseOutcome;
}

impl RunReparse for editor::ReparseWork {
    fn run_reparse(self) -> editor::ReparseOutcome {
        editor::ReparseHandler(editor::test_document::test_workshop(
            editor::Theme::embedded(),
        ))
        .reparse(self)
    }
}

fn languages() -> editor::SyntaxLanguages {
    let mut registry = editor::SyntaxLanguages::new();
    register(&mut registry);
    registry
}

fn parsed() -> editor::Document {
    let fonts = editor::embedded_fonts::source()();
    let theme = editor::Theme::embedded();
    let registry = std::sync::Arc::new(languages());
    let mut document = editor::Document::from_language(
        editor::Text::from_string_exact(SOURCE),
        EXT,
        &registry,
        &fonts,
        &theme,
    );
    let outcome = editor::ReparseWork::capture(&document, registry)
        .expect("the fixture parses")
        .run_reparse();
    document.apply_reparse_outcome(
        outcome,
        &fonts,
        &theme,
        &mut imba::effect::Batch::new().effects(),
    );
    document
}

fn style_at(document: &editor::Document, needle: &str) -> Option<StyleId> {
    let at = SOURCE.find(needle).expect("the needle is in the fixture") as u32;
    let mut inline = Vec::new();
    let mut hidden = Vec::new();
    document
        .markup()
        .marks_inline_hidden_in(0..SOURCE.len() as u32, &mut inline, &mut hidden);
    inline
        .iter()
        .filter(|interval| {
            matches!(
                interval.id,
                StyleId::Keyword
                    | StyleId::String
                    | StyleId::Comment
                    | StyleId::Number
                    | StyleId::Type
                    | StyleId::Function
                    | StyleId::Variable
                    | StyleId::Constant
                    | StyleId::Operator
                    | StyleId::Punctuation
                    | StyleId::Attribute
                    | StyleId::Embedded
            )
        })
        .filter(|interval| interval.range.start <= at && at < interval.range.end)
        .map(|interval| interval.id)
        .last()
}

const EXT: &str = "mk";
const SOURCE: &str = r#"# note
all: main.c
	cc -o app main.c
"#;

#[test]
fn highlights() {
    let document = parsed();
    assert_eq!(
        style_at(&document, "# note"),
        Some(StyleId::Comment),
        "# note"
    );
    assert!(style_at(&document, "all").is_some(), "all is styled");
}

#[test]
fn outline() {
    let document = parsed();
    let items = document.outline_items();
    let titles: Vec<&str> = items
        .iter()
        .map(|(_, _, _, item)| item.title.as_str())
        .collect();
    assert!(
        titles.is_empty(),
        "no outline kinds are declared: {titles:?}"
    );
}

#[test]
fn folds() {
    let document = parsed();
    let folds = document.foldables_in(0..SOURCE.len() as u32);
    let texts: Vec<&str> = folds
        .iter()
        .map(|range| &SOURCE[range.start as usize..range.end as usize])
        .collect();
    assert!(texts.is_empty(), "no fold kinds are declared: {texts:?}");
}
