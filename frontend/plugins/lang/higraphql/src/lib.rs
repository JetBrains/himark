// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["graphql", "gql"],
        module: "graphql",
        symbol: "tree_sitter_graphql",
        crate_name: "tree-sitter-graphql",
        parser_dir: "src",
        language: tree_sitter_graphql::LANGUAGE,
        highlights: include_str!("highlights.scm"),
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
