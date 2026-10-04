// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use editor::reparse::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["xml", "svg", "xsd", "plist"],
        module: "xml",
        symbol: "tree_sitter_xml",
        crate_name: "tree-sitter-xml",
        parser_dir: "xml/src",
        language: tree_sitter_xml::LANGUAGE_XML,
        highlights: tree_sitter_xml::XML_HIGHLIGHT_QUERY,
        configure: |language| language,
    );
}

#[cfg(test)]
mod tests;
