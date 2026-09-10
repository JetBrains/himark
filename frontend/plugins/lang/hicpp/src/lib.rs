use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["cpp", "c++", "cc", "cxx", "hpp", "hxx", "hh"],
        module: "cpp",
        symbol: "tree_sitter_cpp",
        crate_name: "tree-sitter-cpp",
        parser_dir: "src",
        language: tree_sitter_cpp::LANGUAGE,
        highlights: format!("{}\n{}", tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY),
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition", "namespace_definition"])
                .with_outline_kinds(&["function_definition", "type_definition", "namespace_definition"])
                .with_outline_kinds_with_body(&["class_specifier", "struct_specifier", "enum_specifier", "union_specifier"])
        },
    );
}

#[cfg(test)]
mod tests;
