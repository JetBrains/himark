// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["groovy", "gradle"],
        module: "groovy",
        symbol: "tree_sitter_groovy",
        crate_name: "tree-sitter-groovy",
        parser_dir: "src",
        language: tree_sitter_groovy::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| {
            language
                .with_fold_kinds(&["class_declaration", "interface_declaration", "enum_declaration", "function_definition", "method_declaration", "constructor_declaration"])
                .with_outline_kinds(&["class_declaration", "interface_declaration", "enum_declaration", "function_definition", "method_declaration", "constructor_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
