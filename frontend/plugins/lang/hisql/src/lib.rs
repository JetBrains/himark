// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["sql"],
        module: "sql",
        symbol: "tree_sitter_sql",
        crate_name: "tree-sitter-sequel",
        parser_dir: "src",
        language: tree_sitter_sequel::LANGUAGE,
        highlights: tree_sitter_sequel::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
