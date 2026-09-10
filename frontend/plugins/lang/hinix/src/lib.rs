use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["nix"],
        module: "nix",
        symbol: "tree_sitter_nix",
        crate_name: "tree-sitter-nix",
        parser_dir: "src",
        language: tree_sitter_nix::LANGUAGE,
        highlights: tree_sitter_nix::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
