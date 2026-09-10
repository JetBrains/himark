use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["yaml", "yml"],
        module: "yaml",
        symbol: "tree_sitter_yaml",
        crate_name: "tree-sitter-yaml",
        parser_dir: "src",
        language: tree_sitter_yaml::LANGUAGE,
        highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
