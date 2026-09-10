use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["lua"],
        module: "lua",
        symbol: "tree_sitter_lua",
        crate_name: "tree-sitter-lua",
        parser_dir: "src",
        language: tree_sitter_lua::LANGUAGE,
        highlights: tree_sitter_lua::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration"])
                .with_outline_kinds(&["function_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
