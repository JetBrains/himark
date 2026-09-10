use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["go", "golang"],
        module: "go",
        symbol: "tree_sitter_go",
        crate_name: "tree-sitter-go",
        parser_dir: "src",
        language: tree_sitter_go::LANGUAGE,
        highlights: tree_sitter_go::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "method_declaration"])
                .with_outline_kinds(&["function_declaration", "method_declaration", "type_spec"])
        },
    );
}

#[cfg(test)]
mod tests;
