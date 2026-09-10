use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["julia", "jl"],
        module: "julia",
        symbol: "tree_sitter_julia",
        crate_name: "tree-sitter-julia",
        parser_dir: "src",
        language: tree_sitter_julia::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
