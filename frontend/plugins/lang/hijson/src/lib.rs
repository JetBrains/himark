use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["json"],
        module: "json",
        symbol: "tree_sitter_json",
        crate_name: "tree-sitter-json",
        parser_dir: "src",
        language: tree_sitter_json::LANGUAGE,
        highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
