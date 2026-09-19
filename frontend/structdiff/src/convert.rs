// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! tree-sitter tree → difftastic `Syntax` tree (docs/editor/structural-diff.md).
//!
//! Leaves become atoms, interior nodes become delimiter-less lists.
//! Whitespace between tokens is not represented; the derivation's gap
//! coalescing recovers it as retains.

use difftastic_core::parse::syntax::{AtomKind, StringKind, Syntax};
use line_numbers::LinePositions;
use tree_sitter::Node;
use typed_arena::Arena;

pub(crate) struct Converted<'a> {
    pub(crate) roots: Vec<&'a Syntax<'a>>,
    pub(crate) total_nodes: u32,
    pub(crate) error_nodes: u32,
}

pub(crate) fn convert<'a>(
    arena: &'a Arena<Syntax<'a>>,
    tree: &tree_sitter::Tree,
    src: &str,
) -> Converted<'a> {
    let mut converter = Converter {
        arena,
        src,
        lines: LinePositions::from(src),
        total_nodes: 0,
        error_nodes: 0,
    };
    let root = tree.root_node();
    let mut roots = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if let Some(node) = converter.node(child) {
            roots.push(node);
        }
    }
    Converted {
        roots,
        total_nodes: converter.total_nodes,
        error_nodes: converter.error_nodes,
    }
}

struct Converter<'a, 's> {
    arena: &'a Arena<Syntax<'a>>,
    src: &'s str,
    lines: LinePositions,
    total_nodes: u32,
    error_nodes: u32,
}

impl<'a> Converter<'a, '_> {
    fn node(&mut self, node: Node) -> Option<&'a Syntax<'a>> {
        self.total_nodes += 1;
        if node.is_error() || node.is_missing() {
            self.error_nodes += 1;
        }
        let range = node.byte_range();
        if range.is_empty() {
            return None;
        }

        if node.child_count() == 0 {
            return Some(self.atom(node));
        }

        // Children don't necessarily cover the node's text — mdparser's
        // `inline` nodes leave paragraph prose outside any child. Each
        // uncovered run becomes a synthetic atom (whitespace-trimmed;
        // interstitial whitespace stays with the derivation's gap
        // coalescing), otherwise that text would be invisible to the
        // engine and every paragraph would diff as novel.
        let mut children = Vec::with_capacity(node.child_count());
        let mut covered_to = range.start;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let child_range = child.byte_range();
            if child_range.start > covered_to {
                self.gap_atom(covered_to..child_range.start, &mut children);
            }
            covered_to = covered_to.max(child_range.end);
            if let Some(converted) = self.node(child) {
                children.push(converted);
            }
        }
        if range.end > covered_to {
            self.gap_atom(covered_to..range.end, &mut children);
        }
        if children.is_empty() {
            // All children were empty (e.g. missing nodes): fall back
            // to one atom over the node's own text so it stays diffable.
            return Some(self.atom(node));
        }
        Some(Syntax::new_list(
            self.arena,
            "",
            Vec::new(),
            children,
            "",
            Vec::new(),
        ))
    }

    fn atom(&mut self, node: Node) -> &'a Syntax<'a> {
        let range = node.byte_range();
        let content = self.src[range.clone()].to_owned();
        let position = self.lines.from_region(range.start, range.end);
        Syntax::new_atom(self.arena, position, content, atom_kind(node))
    }

    fn gap_atom(&mut self, range: std::ops::Range<usize>, into: &mut Vec<&'a Syntax<'a>>) {
        let text = &self.src[range.clone()];
        let trimmed = text.trim_matches(|c: char| c.is_ascii_whitespace());
        if trimmed.is_empty() {
            return;
        }
        let start = range.start + (trimmed.as_ptr() as usize - text.as_ptr() as usize);
        let end = start + trimmed.len();
        let position = self.lines.from_region(start, end);
        into.push(Syntax::new_atom(
            self.arena,
            position,
            trimmed.to_owned(),
            AtomKind::Normal,
        ));
    }
}

fn atom_kind(node: Node) -> AtomKind {
    if node.is_error() {
        return AtomKind::TreeSitterError;
    }
    let kind = node.kind();
    if kind.contains("comment") {
        // Unlocks difftastic's fuzzy comment matching (ReplacedComment).
        AtomKind::Comment
    } else if kind.contains("string") || kind == "code_fence_content" {
        AtomKind::String(StringKind::StringLiteral)
    } else {
        AtomKind::Normal
    }
}
