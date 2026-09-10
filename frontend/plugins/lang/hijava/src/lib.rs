use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["java"],
        module: "java",
        symbol: "tree_sitter_java",
        crate_name: "tree-sitter-java",
        parser_dir: "src",
        language: tree_sitter_java::LANGUAGE,
        highlights: tree_sitter_java::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["class_declaration", "interface_declaration", "enum_declaration", "record_declaration", "method_declaration", "constructor_declaration"])
                .with_outline_kinds(&["class_declaration", "interface_declaration", "enum_declaration", "record_declaration", "method_declaration", "constructor_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
