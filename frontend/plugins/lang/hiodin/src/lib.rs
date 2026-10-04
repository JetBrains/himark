// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["odin"],
        module: "odin",
        symbol: "tree_sitter_odin",
        crate_name: "tree-sitter-odin",
        parser_dir: "src",
        language: tree_sitter_odin::LANGUAGE,
        highlights: tree_sitter_odin::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
