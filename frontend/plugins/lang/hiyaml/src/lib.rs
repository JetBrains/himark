// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["yaml", "yml"],
        module: "yaml",
        symbol: "tree_sitter_yaml",
        crate_name: "tree-sitter-yaml",
        parser_dir: "src",
        language: tree_sitter_yaml::LANGUAGE,
        highlights: tree_sitter_yaml::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
