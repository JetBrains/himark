use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["toml"],
        module: "toml",
        symbol: "tree_sitter_toml",
        crate_name: "tree-sitter-toml-ng",
        parser_dir: "src",
        language: tree_sitter_toml_ng::LANGUAGE,
        highlights: tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
