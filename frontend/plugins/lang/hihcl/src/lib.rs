use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["hcl", "tf", "terraform"],
        module: "hcl",
        symbol: "tree_sitter_hcl",
        crate_name: "tree-sitter-hcl",
        parser_dir: "src",
        language: tree_sitter_hcl::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
