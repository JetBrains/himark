// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["swift"],
        module: "swift",
        symbol: "tree_sitter_swift",
        crate_name: "tree-sitter-swift",
        parser_dir: "src",
        language: tree_sitter_swift::LANGUAGE,
        highlights: tree_sitter_swift::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration", "class_declaration", "protocol_declaration", "init_declaration"])
                .with_outline_kinds(&["function_declaration", "class_declaration", "protocol_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
