use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["python", "py", "python3"],
        module: "python",
        symbol: "tree_sitter_python",
        crate_name: "tree-sitter-python",
        parser_dir: "src",
        language: tree_sitter_python::LANGUAGE,
        highlights: tree_sitter_python::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition", "class_definition"])
                .with_outline_kinds(&["function_definition", "class_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
