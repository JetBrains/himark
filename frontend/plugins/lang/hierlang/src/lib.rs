use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["erlang", "erl", "hrl"],
        module: "erlang",
        symbol: "tree_sitter_erlang",
        crate_name: "tree-sitter-erlang",
        parser_dir: "src",
        language: tree_sitter_erlang::LANGUAGE,
        highlights: tree_sitter_erlang::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_clause"])
                .with_outline_kinds(&["function_clause", "record_decl"])
        },
    );
}

#[cfg(test)]
mod tests;
