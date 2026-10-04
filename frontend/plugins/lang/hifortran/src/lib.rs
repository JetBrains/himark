// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["fortran", "f90", "f95", "f03", "f08"],
        module: "fortran",
        symbol: "tree_sitter_fortran",
        crate_name: "tree-sitter-fortran",
        parser_dir: "src",
        language: tree_sitter_fortran::LANGUAGE,
        highlights: tree_sitter_fortran::HIGHLIGHTS_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
