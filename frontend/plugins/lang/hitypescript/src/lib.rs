use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["typescript", "ts", "mts", "cts"],
        module: "typescript",
        symbol: "tree_sitter_typescript",
        crate_name: "tree-sitter-typescript",
        parser_dir: "typescript/src",
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        highlights: format!("{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "abstract_class_declaration", "method_definition", "interface_declaration", "enum_declaration", "internal_module"])
                .with_outline_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "abstract_class_declaration", "method_definition", "interface_declaration", "enum_declaration", "internal_module", "type_alias_declaration"])
        },
    );
    hisitter::register_grammar!(
        registry,
        names: &["tsx"],
        module: "tsx",
        symbol: "tree_sitter_tsx",
        crate_name: "tree-sitter-typescript",
        parser_dir: "tsx/src",
        language: tree_sitter_typescript::LANGUAGE_TSX,
        highlights: format!("{}\n{}\n{}", tree_sitter_javascript::HIGHLIGHT_QUERY, tree_sitter_javascript::JSX_HIGHLIGHT_QUERY, tree_sitter_typescript::HIGHLIGHTS_QUERY),
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "abstract_class_declaration", "method_definition", "interface_declaration", "enum_declaration", "internal_module"])
                .with_outline_kinds(&["function_declaration", "generator_function_declaration", "class_declaration", "abstract_class_declaration", "method_definition", "interface_declaration", "enum_declaration", "internal_module", "type_alias_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
