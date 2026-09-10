use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["kotlin", "kt", "kts"],
        module: "kotlin",
        symbol: "tree_sitter_kotlin",
        crate_name: "tree-sitter-kotlin-sg",
        parser_dir: "src",
        language: tree_sitter_kotlin_sg::LANGUAGE,
        highlights: tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "class_declaration", "object_declaration"])
                .with_outline_kinds(&["function_declaration", "class_declaration", "object_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
