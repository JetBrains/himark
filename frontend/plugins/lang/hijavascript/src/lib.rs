use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["javascript", "js", "jsx", "mjs", "cjs"],
        module: "javascript",
        symbol: "tree_sitter_javascript",
        crate_name: "tree-sitter-javascript",
        parser_dir: "src",
        language: tree_sitter_javascript::LANGUAGE,
        highlights: format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "method_definition"])
                .with_outline_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "method_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
