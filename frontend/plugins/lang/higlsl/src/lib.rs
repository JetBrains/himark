use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["glsl", "vert", "frag"],
        module: "glsl",
        symbol: "tree_sitter_glsl",
        crate_name: "tree-sitter-glsl",
        parser_dir: "src",
        language: tree_sitter_glsl::LANGUAGE_GLSL,
        highlights: tree_sitter_glsl::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition"])
                .with_outline_kinds(&["function_definition"])
                .with_outline_kinds_with_body(&["struct_specifier"])
        },
    );
}

#[cfg(test)]
mod tests;
