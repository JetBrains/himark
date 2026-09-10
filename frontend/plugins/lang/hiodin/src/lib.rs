use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["odin"],
        module: "odin",
        symbol: "tree_sitter_odin",
        crate_name: "tree-sitter-odin",
        parser_dir: "src",
        language: tree_sitter_odin::LANGUAGE,
        highlights: tree_sitter_odin::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
