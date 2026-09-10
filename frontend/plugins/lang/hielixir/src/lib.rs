use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["elixir", "ex", "exs"],
        module: "elixir",
        symbol: "tree_sitter_elixir",
        crate_name: "tree-sitter-elixir",
        parser_dir: "src",
        language: tree_sitter_elixir::LANGUAGE,
        highlights: tree_sitter_elixir::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
