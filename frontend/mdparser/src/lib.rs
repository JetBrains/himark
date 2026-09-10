use tree_sitter::{Language, Parser, Tree};
use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_markdown() -> *const ();
}

pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_markdown) };

pub fn markdown_language() -> Language {
    LANGUAGE.into()
}

pub fn block_tree(source: &str) -> Tree {
    block_tree_incremental(source, None)
}

pub fn block_tree_incremental(source: &str, old_tree: Option<&Tree>) -> Tree {
    let mut parser = Parser::new();
    let language = markdown_language();
    parser
        .set_language(&language)
        .expect("failed to load markdown block grammar");
    parser
        .parse(source, old_tree)
        .expect("failed to parse markdown")
}

pub fn block_sexp(source: &str) -> String {
    block_tree(source).root_node().to_sexp()
}

#[cfg(test)]
mod tests {
    use super::block_sexp;

    #[test]
    fn parses_heading_and_paragraph() {
        let parsed = block_sexp("# Title\n\nhello world\n");

        assert!(parsed.contains("atx_heading"));
        assert!(parsed.contains("paragraph"));
    }

    #[test]
    fn parses_unordered_list_items() {
        let parsed = block_sexp("- first\n- second\n- third\n");

        assert!(parsed.contains("list"));
        assert!(parsed.contains("list_item"));
    }

    #[test]
    fn parses_fenced_code_block() {
        let parsed = block_sexp("```rust\nfn main() {}\n```\n");

        assert!(parsed.contains("fenced_code_block"));
        assert!(parsed.contains("info_string"));
        assert!(parsed.contains("code_fence_content"));
    }

    #[test]
    fn an_empty_celled_row_does_not_restart_the_table() {
        let cases = [
            ("| a | b |\n|---|---|\n| 1 | 2 |\n|   |   |\n\n---\n\nafter\n", 1),
            (
                "| a | b |\n|---|---|\n| 1 | 2 |\n|   |   |\n\n## H\n\n| x | y |\n|---|---|\n| 3 | 4 |\n",
                2,
            ),
            ("| a | b |\n|---|---|\n| 1 | 2 |\n| | |\n\nafter\n", 1),
        ];
        for (source, tables) in cases {
            let parsed = block_sexp(source);
            assert!(
                !parsed.contains("ERROR"),
                "clean parse for {source:?}: {parsed}"
            );
            assert_eq!(
                parsed.matches("pipe_table_header").count(),
                tables,
                "table count for {source:?}: {parsed}"
            );
        }

        let real = block_sexp("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(real.contains("pipe_table_delimiter_row"), "{real}");
    }
}
