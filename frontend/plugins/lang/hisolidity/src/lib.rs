use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["solidity", "sol"],
        module: "solidity",
        symbol: "tree_sitter_solidity",
        crate_name: "tree-sitter-solidity",
        parser_dir: "src",
        language: tree_sitter_solidity::LANGUAGE,
        highlights: tree_sitter_solidity::HIGHLIGHT_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["contract_declaration", "interface_declaration", "library_declaration", "function_definition", "modifier_definition", "constructor_definition"])
                .with_outline_kinds(&["contract_declaration", "interface_declaration", "library_declaration", "function_definition", "modifier_definition", "enum_declaration", "event_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
