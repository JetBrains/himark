// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["haskell", "hs"],
        module: "haskell",
        symbol: "tree_sitter_haskell",
        crate_name: "tree-sitter-haskell",
        parser_dir: "src",
        language: tree_sitter_haskell::LANGUAGE,
        highlights: tree_sitter_haskell::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_outline_kinds(&["function", "data_type", "class", "instance"])
        },
    );
}

#[cfg(test)]
mod tests;
