// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["css"],
        module: "css",
        symbol: "tree_sitter_css",
        crate_name: "tree-sitter-css",
        parser_dir: "src",
        language: tree_sitter_css::LANGUAGE,
        highlights: tree_sitter_css::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
