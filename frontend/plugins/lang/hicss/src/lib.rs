use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["css"],
        module: "css",
        symbol: "tree_sitter_css",
        crate_name: "tree-sitter-css",
        parser_dir: "src",
        language: tree_sitter_css::LANGUAGE,
        highlights: tree_sitter_css::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
