use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["ruby", "rb"],
        module: "ruby",
        symbol: "tree_sitter_ruby",
        crate_name: "tree-sitter-ruby",
        parser_dir: "src",
        language: tree_sitter_ruby::LANGUAGE,
        highlights: tree_sitter_ruby::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["method", "singleton_method", "class", "module"])
                .with_outline_kinds(&["method", "singleton_method", "class", "module"])
        },
    );
}

#[cfg(test)]
mod tests;
