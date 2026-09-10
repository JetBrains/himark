use editor::SyntaxLanguages;

pub fn register(registry: &mut SyntaxLanguages) {
    hisitter::register_grammar!(
        registry,
        names: &["rust", "rs"],
        module: "rust",
        symbol: "tree_sitter_rust",
        crate_name: "tree-sitter-rust",
        parser_dir: "src",
        language: tree_sitter_rust::LANGUAGE,
        highlights: tree_sitter_rust::HIGHLIGHTS_QUERY,
        configure: |language| {
            language
                .with_fold_kinds(&["function_item"])
                .with_outline_kinds(&["function_item", "struct_item", "enum_item", "trait_item", "impl_item", "mod_item"])
        },
    );
}

#[cfg(test)]
trait RunReparse {
    fn run_reparse(self) -> editor::ReparseOutcome;
}

#[cfg(test)]
impl RunReparse for editor::ReparseWork {
    fn run_reparse(self) -> editor::ReparseOutcome {
        editor::ReparseHandler(editor::test_document::test_workshop(
            editor::Theme::embedded(),
        ))
        .reparse(self)
    }
}

#[cfg(test)]
fn languages() -> SyntaxLanguages {
    let mut registry = SyntaxLanguages::new();
    register(&mut registry);
    registry
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod assist_tests;

#[cfg(test)]
mod rust_root_tests;

#[cfg(test)]
mod folding;

#[cfg(test)]
mod utf8_field_repro;

#[cfg(test)]
mod typing_gate;
