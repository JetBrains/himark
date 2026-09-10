use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["c", "h"],
        module: "c",
        symbol: "tree_sitter_c",
        crate_name: "tree-sitter-c",
        parser_dir: "src",
        language: tree_sitter_c::LANGUAGE,
        highlights: tree_sitter_c::HIGHLIGHT_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition"])
                .with_outline_kinds(&["function_definition", "type_definition"])
                .with_outline_kinds_with_body(&["struct_specifier", "enum_specifier", "union_specifier"])
        },
    );
}

#[cfg(test)]
mod tests;
