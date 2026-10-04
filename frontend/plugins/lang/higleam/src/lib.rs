// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["gleam"],
        module: "gleam",
        symbol: "tree_sitter_gleam",
        crate_name: "tree-sitter-gleam",
        parser_dir: "src",
        language: tree_sitter_gleam::LANGUAGE,
        highlights: tree_sitter_gleam::HIGHLIGHT_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function"])
                .with_outline_kinds(&["function", "external_function"])
        },
    );
}

#[cfg(test)]
mod tests;
