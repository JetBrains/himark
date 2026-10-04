// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["bash", "sh", "shell", "zsh"],
        module: "bash",
        symbol: "tree_sitter_bash",
        crate_name: "tree-sitter-bash",
        parser_dir: "src",
        language: tree_sitter_bash::LANGUAGE,
        highlights: tree_sitter_bash::HIGHLIGHT_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_definition"])
                .with_outline_kinds(&["function_definition"])
        },
    );
}

#[cfg(test)]
mod tests;
