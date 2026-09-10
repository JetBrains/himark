use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["csharp", "cs", "c#"],
        module: "csharp",
        symbol: "tree_sitter_c_sharp",
        crate_name: "tree-sitter-c-sharp",
        parser_dir: "src",
        language: tree_sitter_c_sharp::LANGUAGE,
        highlights: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["class_declaration", "interface_declaration", "struct_declaration", "enum_declaration", "record_declaration", "method_declaration", "constructor_declaration", "namespace_declaration"])
                .with_outline_kinds(&["class_declaration", "interface_declaration", "struct_declaration", "enum_declaration", "record_declaration", "method_declaration", "constructor_declaration", "namespace_declaration", "property_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
