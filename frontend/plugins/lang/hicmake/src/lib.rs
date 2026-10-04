// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["cmake"],
        module: "cmake",
        symbol: "tree_sitter_cmake",
        crate_name: "tree-sitter-cmake",
        parser_dir: "src",
        language: tree_sitter_cmake::LANGUAGE,
        highlights: tree_sitter_cmake::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
