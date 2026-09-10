use std::ops::Range;

use editor::{MarkupBuilder, StyleId, SyntaxLanguage, SyntaxTree};

mod assist;
mod caret_passes;
pub use caret_passes::{register_caret_enrichers, BraceMatchPass, OccurrencePass};
#[cfg(target_os = "emscripten")]
mod side;
use operation::{Op, Operation};
#[cfg(target_os = "emscripten")]
pub use side::fetch_side_grammar;
use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, Tree};

pub struct TsTree(pub Tree);

impl TsTree {
    pub fn of(tree: &dyn SyntaxTree) -> Option<&Tree> {
        tree.as_any().downcast_ref::<Self>().map(|tree| &tree.0)
    }
}

impl SyntaxTree for TsTree {
    fn clone_tree(&self) -> Box<dyn SyntaxTree> {
        Box::new(Self(self.0.clone()))
    }

    fn edit(&mut self, operation: &Operation, view: &mut text::TextView, base: u32) {
        fn point_at(view: &mut text::TextView, at: u32) -> (usize, usize) {
            let at = (at as usize).min(view.byte_count());
            let row = view.line_at(at).0;
            (row, at - view.line_start_offset(text::LineNumber(row)))
        }
        let base_row = point_at(view, base).0;
        let local = |view: &mut text::TextView, offset: u32| -> Point {
            let (row, column) = point_at(view, base + offset);
            Point::new(row.saturating_sub(base_row), column)
        };

        let mut offset = 0u32;
        for op in operation.iter() {
            match op {
                Op::Retain(len) => offset = offset.saturating_add(len),
                Op::Insert(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let start = local(view, offset);
                    self.0.edit(&InputEdit {
                        start_byte: offset as usize,
                        old_end_byte: offset as usize,
                        new_end_byte: (offset + len) as usize,
                        start_position: start,
                        old_end_position: start,

                        new_end_position: local(view, offset + len),
                    });
                    offset = offset.saturating_add(len);
                }
                Op::Delete(text) => {
                    let len = text.len().min(u32::MAX as usize) as u32;
                    let start = local(view, offset);
                    self.0.edit(&InputEdit {
                        start_byte: offset as usize,
                        old_end_byte: (offset + len) as usize,
                        new_end_byte: offset as usize,
                        start_position: start,
                        old_end_position: advanced_over(start, &text),
                        new_end_position: start,
                    });
                }
            }
        }
    }

    fn changed_since(&self, old: &dyn SyntaxTree) -> Option<Vec<Range<u32>>> {
        let old = TsTree::of(old)?;
        Some(
            old.changed_ranges(&self.0)
                .map(|changed| changed.start_byte as u32..changed.end_byte as u32)
                .collect(),
        )
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn advanced_over(from: Point, text: &str) -> Point {
    match text.rfind('\n') {
        Some(last_newline) => Point::new(
            from.row + text.bytes().filter(|byte| *byte == b'\n').count(),
            text.len() - last_newline - 1,
        ),
        None => Point::new(from.row, from.column + text.len()),
    }
}

const MAX_TOKENS_PER_BLOCK: usize = 50_000;

pub struct TreeSitterLanguage {
    language: Language,
    query: Query,

    themes: Vec<Option<StyleId>>,

    fold_kinds: Vec<&'static str>,

    outline_kinds: Vec<(&'static str, bool)>,
}

impl TreeSitterLanguage {
    pub fn new(language: Language, highlights: &str) -> Self {
        let query = Query::new(&language, highlights).expect("a valid highlights query");
        let themes = query
            .capture_names()
            .iter()
            .map(|name| theme_for_capture(name))
            .collect();
        Self {
            language,
            query,
            themes,
            fold_kinds: Vec::new(),
            outline_kinds: Vec::new(),
        }
    }

    pub fn with_fold_kinds(mut self, kinds: &[&'static str]) -> Self {
        self.fold_kinds = kinds.to_vec();
        self
    }

    pub fn with_outline_kinds(mut self, kinds: &[&'static str]) -> Self {
        self.outline_kinds
            .extend(kinds.iter().map(|kind| (*kind, false)));
        self
    }

    pub fn with_outline_kinds_with_body(mut self, kinds: &[&'static str]) -> Self {
        self.outline_kinds
            .extend(kinds.iter().map(|kind| (*kind, true)));
        self
    }
}

impl SyntaxLanguage for TreeSitterLanguage {
    fn parse(
        &self,
        text: &text::Text,
        range: Range<u32>,
        old: Option<&dyn SyntaxTree>,
    ) -> Option<Box<dyn SyntaxTree>> {
        let end = (range.end as usize).min(text.byte_count());
        let start = (range.start as usize).min(end);
        let mut parser = Parser::new();
        parser.set_language(&self.language).ok()?;

        let mut view = text.view();
        let tree = parser.parse_with_options(
            &mut |offset, _point| page_from(&mut view, start + offset, end),
            old.and_then(TsTree::of),
            None,
        )?;
        Some(Box::new(TsTree(tree)))
    }

    fn markup_for_changes(
        &self,
        text: &text::Text,
        range: Range<u32>,
        tree: &dyn SyntaxTree,
        changed: &[Range<u32>],
        replacement: &mut MarkupBuilder,
        invalidated: &mut Vec<Range<u32>>,
        _fonts: &skia_safe::textlayout::FontCollection,
        _theme: &editor::Theme,
    ) {
        let tree = TsTree::of(tree).expect("a tree-sitter language parses tree-sitter trees");
        let end = (range.end as usize).min(text.byte_count());
        let start = (range.start as usize).min(end);

        let mut spans: Vec<(Range<u32>, StyleId)> = Vec::new();
        let mut by_range = std::collections::HashMap::<(u32, u32), (usize, usize)>::new();
        let mut cursor = QueryCursor::new();
        for change in changed {
            cursor.set_byte_range(change.start as usize..change.end as usize);
            let mut matches = cursor.matches(
                &self.query,
                tree.root_node(),
                RopePages::over(text, start, end),
            );
            'matches: while let Some(matched) = matches.next() {
                if !self
                    .query
                    .general_predicates(matched.pattern_index)
                    .is_empty()
                {
                    continue;
                }
                for capture in matched.captures {
                    let Some(theme) = self.themes[capture.index as usize] else {
                        continue;
                    };
                    let span = capture.node.start_byte() as u32..capture.node.end_byte() as u32;
                    if span.start >= span.end {
                        continue;
                    }
                    match by_range.entry((span.start, span.end)) {
                        std::collections::hash_map::Entry::Occupied(mut slot) => {
                            let (at, pattern) = *slot.get();
                            if matched.pattern_index >= pattern {
                                spans[at].1 = theme;
                                slot.insert((at, matched.pattern_index));
                            }
                        }
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            if spans.len() >= MAX_TOKENS_PER_BLOCK {
                                break 'matches;
                            }
                            slot.insert((spans.len(), matched.pattern_index));
                            spans.push((span, theme));
                        }
                    }
                }
            }
        }

        for (span, theme) in spans {
            invalidated.push(span.clone());
            replacement.push_styled(span, theme);
        }

        if !self.outline_kinds.is_empty() {
            let mut items: Vec<(Range<u32>, Range<u32>, Range<u32>)> = Vec::new();
            for change in changed {
                collect_outline(tree.root_node(), change, &self.outline_kinds, &mut items);
            }
            items.sort_by_key(|(node, _, _)| (node.start, node.end));
            items.dedup();
            let mut view = text.view();
            for (node, title, name) in items {
                let title = outline_title(&mut view, start, title);
                if !title.is_empty() {
                    replacement.push_outline(node, editor::OutlineItem { title });
                    if name.start < name.end {
                        replacement.push_styled(name, StyleId::DeclarationName);
                    }
                }
            }
        }

        if !self.fold_kinds.is_empty() {
            let mut foldables: Vec<Range<u32>> = Vec::new();
            for change in changed {
                collect_foldables(tree.root_node(), change, &self.fold_kinds, &mut foldables);
            }
            foldables.sort_by(|a, b| (a.start, a.end).cmp(&(b.start, b.end)));
            foldables.dedup();
            for span in foldables {
                replacement.push_foldable(span);
            }
        }
    }

    fn assist(&self, request: &editor::AssistRequest<'_>) -> Option<editor::Assist> {
        assist::assist(request)
    }
}

fn collect_foldables(
    node: tree_sitter::Node,
    range: &Range<u32>,
    kinds: &[&'static str],
    out: &mut Vec<Range<u32>>,
) {
    let start = node.start_byte() as u32;
    let end = node.end_byte() as u32;
    if end <= range.start || start >= range.end {
        return;
    }
    if kinds.contains(&node.kind()) {
        if let Some(interior) = fold_interior(node) {
            out.push(interior);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_foldables(child, range, kinds, out);
    }
}

fn collect_outline(
    node: tree_sitter::Node,
    range: &Range<u32>,
    kinds: &[(&'static str, bool)],
    out: &mut Vec<(Range<u32>, Range<u32>, Range<u32>)>,
) {
    let start = node.start_byte() as u32;
    let end = node.end_byte() as u32;
    if end <= range.start || start >= range.end {
        return;
    }
    if let Some((_, requires_body)) = kinds.iter().find(|(kind, _)| *kind == node.kind()) {
        if !requires_body || body_of(node).is_some() {
            if let Some(named) = title_node(node) {
                out.push((
                    start..end,
                    start..named.end_byte() as u32,
                    named.start_byte() as u32..named.end_byte() as u32,
                ));
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_outline(child, range, kinds, out);
    }
}

fn title_node(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("declarator"))
        .or_else(|| node.child_by_field_name("type"))
        .or_else(|| {
            let mut cursor = node.walk();
            let named = node
                .children(&mut cursor)
                .find(|child| child.is_named() && child.kind().contains("identifier"));
            named
        })
}

fn outline_title(view: &mut text::TextView, syntax_start: usize, title: Range<u32>) -> String {
    let count = view.byte_count();
    let from = (syntax_start + title.start as usize).min(count);
    let to = (syntax_start + title.end as usize).min(count);
    if from >= to {
        return String::new();
    }
    let raw = view.byte_string(from, to);
    let line = raw.lines().next().unwrap_or("").trim();
    let mut capped: String = line.chars().take(80).collect();
    if capped.len() < line.len() {
        capped.push('…');
    }
    capped
}

fn fold_interior(node: tree_sitter::Node) -> Option<Range<u32>> {
    let body = body_of(node)?;
    if body.start_position().row == body.end_position().row {
        return None;
    }
    let count = body.child_count();
    if count >= 2 && !body.child(0)?.is_named() {
        let start = body.child(0)?.end_byte() as u32;
        let end = body.child((count - 1) as u32)?.start_byte() as u32;
        return (start < end).then_some(start..end);
    }
    let start = body
        .prev_sibling()
        .map(|prev| prev.end_byte())
        .unwrap_or_else(|| node.start_byte()) as u32;
    let end = body.end_byte() as u32;
    (start < end).then_some(start..end)
}

fn body_of(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    node.child_by_field_name("body").or_else(|| {
        let mut cursor = node.walk();
        let body = node
            .children(&mut cursor)
            .filter(|child| child.is_named() && child.kind().ends_with("_body"))
            .last();
        body
    })
}

fn theme_for_capture(name: &str) -> Option<StyleId> {
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "keyword" => StyleId::Keyword,
        "string" | "character" => StyleId::String,
        "comment" => StyleId::Comment,
        "number" | "float" | "integer" => StyleId::Number,
        "type" | "enum" | "struct" | "trait" => StyleId::Type,
        "function" | "method" | "constructor" | "macro" => StyleId::Function,
        "variable" | "parameter" | "property" | "field" | "identifier" => StyleId::Variable,
        "constant" | "boolean" => StyleId::Constant,
        "operator" => StyleId::Operator,
        "punctuation" | "bracket" | "delimiter" => StyleId::Punctuation,
        "attribute" | "label" | "lifetime" | "escape" | "tag" => StyleId::Attribute,
        "embedded" | "injection" => StyleId::Embedded,
        _ => return None,
    })
}

fn page_from(view: &mut text::TextView, at: usize, end: usize) -> Vec<u8> {
    let end = end.min(view.byte_count());
    if at >= end {
        return Vec::new();
    }
    let to = (at + 8 * 1024).min(end);
    let mut page = Vec::with_capacity(to - at);
    view.byte_range_into(at, to, &mut page);
    page
}

struct RopePages {
    view: std::rc::Rc<std::cell::RefCell<text::TextView>>,
    base: usize,
    end: usize,
}

impl RopePages {
    fn over(text: &text::Text, base: usize, end: usize) -> Self {
        Self {
            view: std::rc::Rc::new(std::cell::RefCell::new(text.view())),
            base,
            end,
        }
    }
}

struct RopePageIter {
    view: std::rc::Rc<std::cell::RefCell<text::TextView>>,
    at: usize,
    end: usize,
}

impl Iterator for RopePageIter {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        let page = page_from(&mut self.view.borrow_mut(), self.at, self.end);
        if page.is_empty() {
            return None;
        }
        self.at += page.len();
        Some(page)
    }
}

impl tree_sitter::TextProvider<Vec<u8>> for RopePages {
    type I = RopePageIter;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        RopePageIter {
            view: std::rc::Rc::clone(&self.view),
            at: (self.base + node.start_byte()).min(self.end),
            end: (self.base + node.end_byte()).min(self.end),
        }
    }
}

#[macro_export]
macro_rules! register_grammar {
    (
        $registry:expr,
        names: $names:expr,
        module: $module:literal,
        symbol: $symbol:literal,
        crate_name: $crate_name:literal,
        parser_dir: $parser_dir:literal,
        language: $language:expr,
        highlights: $highlights:expr,
        configure: $configure:expr $(,)?
    ) => {{
        #[cfg(not(target_os = "emscripten"))]
        {
            let highlights: ::std::sync::Arc<dyn Fn() -> String + Send + Sync> =
                ::std::sync::Arc::new(|| ::std::string::String::from($highlights));
            let loader_highlights = ::std::sync::Arc::clone(&highlights);
            let configure: fn($crate::TreeSitterLanguage) -> $crate::TreeSitterLanguage =
                $configure;
            $registry.register_lazy(
                $names,
                Some(editor::SideGrammar {
                    module: $module,
                    symbol: $symbol,
                    crate_name: $crate_name,
                    parser_dir: $parser_dir,
                    highlights,
                }),
                ::std::sync::Arc::new(move || {
                    Some(
                        ::std::sync::Arc::new(configure($crate::TreeSitterLanguage::new(
                            ($language).into(),
                            &loader_highlights(),
                        ))) as ::std::sync::Arc<dyn editor::SyntaxLanguage>,
                    )
                }),
            );
        }
        #[cfg(target_os = "emscripten")]
        {
            let configure: fn($crate::TreeSitterLanguage) -> $crate::TreeSitterLanguage =
                $configure;
            $registry.register_lazy(
                $names,
                Some(editor::SideGrammar {
                    module: $module,
                    symbol: $symbol,
                    crate_name: $crate_name,
                    parser_dir: $parser_dir,
                }),
                ::std::sync::Arc::new(move || {
                    let (language, query) = $crate::fetch_side_grammar($module, $symbol)?;
                    Some(
                        ::std::sync::Arc::new(configure($crate::TreeSitterLanguage::new(
                            language, &query,
                        ))) as ::std::sync::Arc<dyn editor::SyntaxLanguage>,
                    )
                }),
            );
        }
    }};
}
