// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["powershell", "ps1", "psm1"],
        module: "powershell",
        symbol: "tree_sitter_powershell",
        crate_name: "tree-sitter-powershell",
        parser_dir: "src",
        language: tree_sitter_powershell::LANGUAGE,
        highlights: tree_sitter_powershell::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
