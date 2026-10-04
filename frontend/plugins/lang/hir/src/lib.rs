// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["r"],
        module: "r",
        symbol: "tree_sitter_r",
        crate_name: "tree-sitter-r",
        parser_dir: "src",
        language: tree_sitter_r::LANGUAGE,
        highlights: tree_sitter_r::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
