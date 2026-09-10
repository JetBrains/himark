use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["perl", "pl", "pm"],
        module: "perl",
        symbol: "tree_sitter_perl",
        crate_name: "tree-sitter-perl",
        parser_dir: "src",
        language: tree_sitter_perl::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition"])
                .with_outline_kinds(&["function_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
