use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["fsharp", "fs", "fsx"],
        module: "fsharp",
        symbol: "tree_sitter_fsharp",
        crate_name: "tree-sitter-fsharp",
        parser_dir: "fsharp/src",
        language: tree_sitter_fsharp::LANGUAGE_FSHARP,
        highlights: tree_sitter_fsharp::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
