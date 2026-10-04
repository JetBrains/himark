// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["dart"],
        module: "dart",
        symbol: "tree_sitter_dart",
        crate_name: "tree-sitter-dart",
        parser_dir: "src",
        language: tree_sitter_dart::LANGUAGE,
        highlights: tree_sitter_dart::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["class_declaration", "mixin_declaration", "extension_declaration", "enum_declaration"])
                .with_outline_kinds(&["class_declaration", "mixin_declaration", "extension_declaration", "enum_declaration", "function_signature", "getter_signature", "setter_signature"])
        },
    );
}

#[cfg(test)]
mod tests;
