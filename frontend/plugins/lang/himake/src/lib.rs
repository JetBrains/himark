use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["make", "makefile", "mk"],
        module: "make",
        symbol: "tree_sitter_make",
        crate_name: "tree-sitter-make",
        parser_dir: "src",
        language: tree_sitter_make::LANGUAGE,
        highlights: tree_sitter_make::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
