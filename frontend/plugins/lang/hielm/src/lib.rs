use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["elm"],
        module: "elm",
        symbol: "tree_sitter_elm",
        crate_name: "tree-sitter-elm",
        parser_dir: "src",
        language: tree_sitter_elm::LANGUAGE,
        highlights: tree_sitter_elm::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["value_declaration"])
                .with_outline_kinds(&["type_declaration", "type_alias_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
