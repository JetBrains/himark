// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["html", "htm"],
        module: "html",
        symbol: "tree_sitter_html",
        crate_name: "tree-sitter-html",
        parser_dir: "src",
        language: tree_sitter_html::LANGUAGE,
        highlights: tree_sitter_html::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
