// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["nix"],
        module: "nix",
        symbol: "tree_sitter_nix",
        crate_name: "tree-sitter-nix",
        parser_dir: "src",
        language: tree_sitter_nix::LANGUAGE,
        highlights: tree_sitter_nix::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
