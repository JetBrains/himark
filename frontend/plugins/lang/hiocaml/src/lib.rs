use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["ocaml", "ml"],
        module: "ocaml",
        symbol: "tree_sitter_ocaml",
        crate_name: "tree-sitter-ocaml",
        parser_dir: "grammars/ocaml/src",
        language: tree_sitter_ocaml::LANGUAGE_OCAML,
        highlights: tree_sitter_ocaml::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
