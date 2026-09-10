use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["php"],
        module: "php",
        symbol: "tree_sitter_php",
        crate_name: "tree-sitter-php",
        parser_dir: "php/src",
        language: tree_sitter_php::LANGUAGE_PHP,
        highlights: tree_sitter_php::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition", "method_declaration", "class_declaration", "interface_declaration", "trait_declaration", "enum_declaration"])
                .with_outline_kinds(&["function_definition", "method_declaration", "class_declaration", "interface_declaration", "trait_declaration", "enum_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
