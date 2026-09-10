use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["objc", "m", "mm", "objective-c"],
        module: "objc",
        symbol: "tree_sitter_objc",
        crate_name: "tree-sitter-objc",
        parser_dir: "src",
        language: tree_sitter_objc::LANGUAGE,
        highlights: format!("{}\n{}", tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_objc::HIGHLIGHTS_QUERY),
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition"])
                .with_outline_kinds(&["function_definition", "class_interface", "class_implementation"])
        },
    );
}

#[cfg(test)]
mod tests;
