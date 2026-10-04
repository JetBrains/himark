// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["zig"],
        module: "zig",
        symbol: "tree_sitter_zig",
        crate_name: "tree-sitter-zig",
        parser_dir: "src",
        language: tree_sitter_zig::LANGUAGE,
        highlights: tree_sitter_zig::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_declaration"])
                .with_outline_kinds(&["function_declaration"])
        },
    );
}

#[cfg(test)]
mod tests;
