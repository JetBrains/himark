// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["scala", "sc"],
        module: "scala",
        symbol: "tree_sitter_scala",
        crate_name: "tree-sitter-scala",
        parser_dir: "src",
        language: tree_sitter_scala::LANGUAGE,
        highlights: tree_sitter_scala::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition", "class_definition", "object_definition", "trait_definition"])
                .with_outline_kinds(&["function_definition", "class_definition", "object_definition", "trait_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
