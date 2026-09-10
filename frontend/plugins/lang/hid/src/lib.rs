use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["d"],
        module: "d",
        symbol: "tree_sitter_d",
        crate_name: "tree-sitter-d",
        parser_dir: "src",
        language: tree_sitter_d::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
