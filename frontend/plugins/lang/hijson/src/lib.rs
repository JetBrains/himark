// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["json"],
        module: "json",
        symbol: "tree_sitter_json",
        crate_name: "tree-sitter-json",
        parser_dir: "src",
        language: tree_sitter_json::LANGUAGE,
        highlights: tree_sitter_json::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
