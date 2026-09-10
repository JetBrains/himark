use std::ops::Range;

use editor::{
    enrich_ready, EnrichCx, EnrichFuture, EnrichInput, Enricher, EnricherId, Enrichment, Interest,
    Markup, StyleId,
};
use tree_sitter::Node;

use crate::TsTree;

const PAIRS: [(&str, &str); 3] = [("(", ")"), ("[", "]"), ("{", "}")];

const OCCURRENCE_CAP: usize = 512;

pub fn register_caret_enrichers(enrichers: &mut editor::Enrichers) {
    enrichers.register(std::sync::Arc::new(BraceMatchPass));
    enrichers.register(std::sync::Arc::new(OccurrencePass));
}

fn styled(previous: &Markup, fresh: Vec<Range<u32>>, style: StyleId) -> Enrichment {
    let mut changed = previous.styled_ranges_in(0..u32::MAX);
    changed.extend(fresh.iter().cloned());
    if changed.is_empty() {
        return Enrichment::none();
    }
    changed.sort_by_key(|range| (range.start, range.end));
    changed.dedup();
    let mut builder = Markup::builder();
    for range in fresh {
        builder.push_styled(range, style);
    }
    Enrichment {
        replacement: builder,
        changed,
    }
}

fn tree_at<'a>(
    input: &'a EnrichInput,
    offset: u32,
) -> Option<(&'a tree_sitter::Tree, u32)> {
    let byte_count = input.text.byte_count().min(u32::MAX as usize) as u32;
    let (syntax, range) = input.syntax.enclosing_at(offset, 0..byte_count);
    let tree = TsTree::of(syntax.tree.as_deref()?)?;
    Some((tree, range.start))
}

fn leaf_beside<'t>(
    root: Node<'t>,
    rel: u32,
    pick: impl Fn(&Node<'t>) -> bool,
) -> Option<Node<'t>> {
    let mut probes = vec![rel as usize];
    if rel > 0 {
        probes.push(rel as usize - 1);
    }
    for probe in probes {
        let Some(node) = root.descendant_for_byte_range(probe, probe.saturating_add(1)) else {
            continue;
        };
        if node.child_count() == 0 && pick(&node) {
            return Some(node);
        }
    }
    None
}

pub struct BraceMatchPass;

fn bracket_kind(kind: &str) -> Option<(&'static str, &'static str, bool)> {
    for (open, close) in PAIRS {
        if kind == open {
            return Some((open, close, true));
        }
        if kind == close {
            return Some((open, close, false));
        }
    }
    None
}

fn partner<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let (open, close, opens) = bracket_kind(node.kind())?;
    let parent = node.parent()?;
    let mut depth = 0i32;
    if opens {
        let mut cursor = parent.walk();
        let mut seen = false;
        for child in parent.children(&mut cursor) {
            if child == node {
                seen = true;
                depth = 1;
                continue;
            }
            if !seen {
                continue;
            }
            if child.kind() == open {
                depth += 1;
            } else if child.kind() == close {
                depth -= 1;
                if depth == 0 {
                    return Some(child);
                }
            }
        }
    } else {
        let mut cursor = parent.walk();
        let children: Vec<Node<'t>> = parent.children(&mut cursor).collect();
        let at = children.iter().position(|child| *child == node)?;
        for child in children[..at].iter().rev() {
            if child.kind() == close {
                depth += 1;
            } else if child.kind() == open {
                if depth == 0 {
                    return Some(*child);
                }
                depth -= 1;
            }
        }
    }
    None
}

impl Enricher for BraceMatchPass {
    fn id(&self) -> EnricherId {
        EnricherId("ts-brace-match")
    }

    fn interest(&self) -> Interest {
        Interest {
            syntax: false,
            carets: true,
        }
    }

    fn derive<'a>(&'a self, input: &'a EnrichInput, _cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
        let mut fresh = Vec::new();
        if let Some(caret) = &input.caret {
            if let Some((tree, base)) = tree_at(input, caret.offset) {
                let rel = caret.offset.saturating_sub(base);
                let bracket = leaf_beside(tree.root_node(), rel, |node| {
                    bracket_kind(node.kind()).is_some()
                });
                if let Some(bracket) = bracket {
                    if let Some(partner) = partner(bracket) {
                        for node in [bracket, partner] {
                            let range = node.byte_range();
                            fresh.push(base + range.start as u32..base + range.end as u32);
                        }
                        fresh.sort_by_key(|range| range.start);
                    }
                }
            }
        }
        enrich_ready(styled(&input.previous, fresh, StyleId::BraceMatch))
    }
}

pub struct OccurrencePass;

fn is_identifier(node: &Node<'_>) -> bool {
    node.is_named() && node.child_count() == 0 && node.kind().contains("identifier")
}

fn occurrences_of(
    tree: &tree_sitter::Tree,
    word: &str,
    slice: impl Fn(Range<u32>) -> String,
) -> Vec<Range<u32>> {
    let mut found = Vec::new();
    let mut cursor = tree.root_node().walk();
    'walk: loop {
        let node = cursor.node();
        if is_identifier(&node)
            && node.byte_range().len() == word.len()
            && slice(node.start_byte() as u32..node.end_byte() as u32) == word
        {
            found.push(node.start_byte() as u32..node.end_byte() as u32);
            if found.len() >= OCCURRENCE_CAP {
                break;
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    found
}

impl Enricher for OccurrencePass {
    fn id(&self) -> EnricherId {
        EnricherId("ts-occurrences")
    }

    fn interest(&self) -> Interest {
        Interest {
            syntax: false,
            carets: true,
        }
    }

    fn derive<'a>(&'a self, input: &'a EnrichInput, _cx: &'a EnrichCx<'a>) -> EnrichFuture<'a> {
        let mut fresh = Vec::new();
        if let Some(caret) = &input.caret {
            if let Some((tree, base)) = tree_at(input, caret.offset) {
                let rel = caret.offset.saturating_sub(base);
                let slice = |range: Range<u32>| {
                    input.text.byte_string(
                        (base + range.start) as usize,
                        (range.end - range.start) as usize,
                    )
                };
                if let Some(under) = leaf_beside(tree.root_node(), rel, is_identifier) {
                    let word = slice(under.start_byte() as u32..under.end_byte() as u32);
                    fresh = occurrences_of(tree, &word, slice)
                        .into_iter()
                        .map(|range| base + range.start..base + range.end)
                        .collect();
                }
            }
        }
        enrich_ready(styled(&input.previous, fresh, StyleId::Occurrence))
    }
}

#[cfg(test)]
mod tests;
