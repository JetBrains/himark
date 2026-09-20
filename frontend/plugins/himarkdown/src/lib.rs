// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

mod assist;
mod checkbox;
mod fence_embed;
pub mod image;
mod table;

pub use fence_embed::{EmbedView, FenceEmbedEnricher};

pub use table::{
    CellAlign, InsertTable, TableCommand, TableEditor, TableRelayoutEffect, TableRelayoutHandler,
};

use himark::{
    Document, Markup, MarkupBuilder, StyleId, SyntaxLanguage, SyntaxTree, TextDecorationInterval,
};
use hisitter::TsTree;
use text::Text;
use tree_sitter::{Node, Tree};

pub fn document_from_markdown(
    source: &str,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Document {
    let text = Text::from_string_exact(source);
    let tree = parse_markdown(&text);
    document_from_tree_text(text, tree,
                store, ui, fonts, theme)
}

pub fn markdown_languages(mut languages: himark::SyntaxLanguages) -> himark::SyntaxLanguages {
    languages.register(&["markdown"], std::sync::Arc::new(MarkdownLanguage));
    languages
}

pub fn markdown_enrichers(mut enrichers: himark::Enrichers) -> himark::Enrichers {
    enrichers.register(std::sync::Arc::new(TableEnricher));
    enrichers.register(std::sync::Arc::new(fence_embed::FenceEmbedEnricher));
    enrichers.register(std::sync::Arc::new(image::ImageEnricher));
    enrichers
}

fn builder_enrichers() -> himark::Enrichers {
    markdown_enrichers(himark::Enrichers::new())
}

pub struct TableEnricher;

impl himark::Enricher for TableEnricher {
    fn id(&self) -> himark::EnricherId {
        himark::EnricherId("markdown-tables")
    }

    fn derive<'a>(
        &'a self,
        input: &'a himark::EnrichInput,
        cx: &'a himark::EnrichCx<'a>,
    ) -> himark::EnrichFuture<'a> {
        let mut builder = Markup::builder();
        let mut changed: Vec<std::ops::Range<u32>> = Vec::new();

        for range in &input.changed {
            if !input.previous.all_inlays_in(range.clone()).is_empty() && !changed.contains(range) {
                changed.push(range.clone());
            }
        }
        if input.syntax.language == "markdown" {
            if let Some(tree) = input.syntax.tree.as_deref().and_then(TsTree::of) {
                visit_markdown_blocks_where(
                    &input.text,
                    tree,
                    |range| {
                        input
                            .changed
                            .iter()
                            .any(|changed| range.start <= changed.end && changed.start <= range.end)
                    },
                    |block| {
                        let Some(source) = &block.table else {
                            return;
                        };
                        builder.push_inlay(
                            block.range.clone(),
                            himark::Inlay::editing(
                                himark::InlayMode::Instead(himark::InsteadKind::FullLine),
                                cx.measure.with_ctx(|store, ui| {
                                    table::TableEditor::new(
                                        source.clone(),
                                        store,
                                        ui,
                                        cx.fonts,
                                        cx.theme,
                                    )
                                }),
                            ),
                        );
                        if !changed.contains(&block.range) {
                            changed.push(block.range.clone());
                        }
                    },
                );
            }
        }
        changed.sort_by_key(|range| range.start);
        himark::enrich_ready(himark::Enrichment {
            replacement: builder,
            changed,
        })
    }
}

pub fn markdown_document(
    source: &str,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> (Document, Vec<MarkdownBlock>) {
    let text = Text::from_string_exact(source);
    let tree = parse_markdown(&text);
    let blocks = markdown_blocks(&text, &tree);
    let document = document_from_tree_text(text, tree,
                store, ui, fonts, theme);
    (document, blocks)
}

pub fn document_from_tree(
    source: &str,
    tree: &Tree,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Document {
    document_from_tree_text(Text::from_string_exact(source), tree.clone(),
                store, ui, fonts, theme)
}

fn document_from_tree_text(
    text: Text,
    tree: Tree,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Document {
    let markup = markup_from_tree(&text, &tree, fonts, theme);
    let sites = MarkdownLanguage.sites_impl(&text, &tree);

    let mut syntax = himark::Syntax::new("markdown", Some(Box::new(TsTree(tree.clone()))), markup);
    {
        let mut sections: Vec<(Range<u32>, Range<u32>)> = Vec::new();
        let full = 0..text.byte_count().min(u32::MAX as usize) as u32;
        collect_sections(tree.root_node(), &full, &mut sections);
        sections.sort_by_key(|(node, _)| (node.start, node.end));
        sections.dedup();
        for (node, title) in sections {
            if let Some(item) = section_outline_item(&text, &title) {
                syntax.push_outline_item(node, item);
            }
        }
    }
    let mut document = Document::new(text, Markup::new()).with_syntax(syntax, &sites);

    document.enrich_now(&builder_enrichers(),
                store, ui, fonts, theme);
    document
}

fn parse_markdown_incremental(
    text: &Text,
    range: &std::ops::Range<u32>,
    old: Option<&Tree>,
) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&mdparser::markdown_language())
        .expect("the markdown grammar must load");
    let end = (range.end as usize).min(text.byte_count());
    let start = (range.start as usize).min(end);

    let virtual_newline = end > start && {
        let mut last = Vec::with_capacity(1);
        text.view().byte_range_into(end - 1, end, &mut last);
        last != b"\n"
    };

    let mut view = text.view();
    parser
        .parse_with_options(
            &mut |offset, _point| {
                let at = start + offset;
                if at < end {
                    let to = (at + 8 * 1024).min(end);
                    let mut page = Vec::with_capacity(to - at);
                    view.byte_range_into(at, to, &mut page);
                    return page;
                }
                if virtual_newline && at == end {
                    return b"\n".to_vec();
                }
                Vec::new()
            },
            old,
            None,
        )
        .expect("reparse must produce a tree")
}

pub struct MarkdownLanguage;

impl SyntaxLanguage for MarkdownLanguage {
    fn parse(
        &self,
        text: &Text,
        range: std::ops::Range<u32>,
        old: Option<&dyn SyntaxTree>,
    ) -> Option<Box<dyn SyntaxTree>> {
        let old = old.and_then(TsTree::of);
        Some(Box::new(TsTree(parse_markdown_incremental(
            text, &range, old,
        ))))
    }

    fn markup_for_changes(
        &self,
        text: &Text,
        _range: std::ops::Range<u32>,
        tree: &dyn SyntaxTree,
        changed: &[std::ops::Range<u32>],
        replacement: &mut MarkupBuilder,
        invalidated: &mut Vec<std::ops::Range<u32>>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        let tree = TsTree::of(tree).expect("markdown parses tree-sitter trees");
        self.markup_impl(text, tree, changed, replacement, invalidated, fonts, theme);

        let mut sections: Vec<(std::ops::Range<u32>, std::ops::Range<u32>)> = Vec::new();
        for change in changed {
            collect_sections(tree.root_node(), change, &mut sections);
        }
        sections.sort_by_key(|(node, _)| (node.start, node.end));
        sections.dedup();
        for (node, title) in sections {
            if let Some(item) = section_outline_item(text, &title) {
                replacement.push_outline(node, item);
            }
        }
    }

    fn sites(
        &self,
        text: &Text,
        _range: std::ops::Range<u32>,
        tree: &dyn SyntaxTree,
    ) -> Vec<himark::SyntaxSite> {
        let tree = TsTree::of(tree).expect("markdown parses tree-sitter trees");
        self.sites_impl(text, tree)
    }

    fn assist(&self, request: &himark::AssistRequest<'_>) -> Option<himark::Assist> {
        let tree = TsTree::of(request.tree)?;
        assist::assist(
            request.text,
            tree,
            request.range.start,
            request.kind,
            &request.location,
        )
    }
}

impl MarkdownLanguage {
    fn sites_impl(&self, text: &Text, tree: &Tree) -> Vec<himark::SyntaxSite> {
        if std::env::var_os("HIMARK_NO_INJECT").is_some() {
            return Vec::new();
        }
        let mut sites = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.kind() == "fenced_code_block" {
                let mut language = None;
                let mut content = None;
                for index in 0..node.child_count() {
                    let Some(child) = node.child(index as u32) else {
                        continue;
                    };
                    match child.kind() {
                        "info_string" => {
                            language = Some(text.byte_string(
                                child.start_byte(),
                                child.end_byte() - child.start_byte(),
                            ));
                        }
                        "code_fence_content" => {
                            content = Some(child.start_byte() as u32..child.end_byte() as u32);
                        }
                        _ => {}
                    }
                }
                if let (Some(language), Some(range)) = (language, content) {
                    let byte_count = text.byte_count().min(u32::MAX as usize) as u32;
                    let range = range.start.min(byte_count)..range.end.min(byte_count);
                    let language = language.split_whitespace().next().unwrap_or("").to_owned();
                    if !language.is_empty() && range.start < range.end {
                        sites.push(himark::SyntaxSite { range, language });
                    }
                }
                continue;
            }
            for index in 0..node.child_count() {
                if let Some(child) = node.child(index as u32) {
                    stack.push(child);
                }
            }
        }
        sites
    }

    #[allow(clippy::too_many_arguments)]
    fn markup_impl(
        &self,
        text: &Text,
        tree: &Tree,
        changed: &[std::ops::Range<u32>],
        replacement: &mut MarkupBuilder,
        invalidated: &mut Vec<std::ops::Range<u32>>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &himark::Theme,
    ) {
        visit_markdown_blocks_where(
            text,
            tree,
            |range| {
                changed
                    .iter()
                    .any(|changed| range.start <= changed.end && changed.start <= range.end)
            },
            |block| {
                invalidated.push(block.range.clone());
                push_markup_for_block(replacement, &block, fonts, theme);
            },
        );
    }
}

pub fn parse_markdown(text: &Text) -> Tree {
    let mut source = text.byte_string(0, text.byte_count());

    if !source.ends_with('\n') && !source.is_empty() {
        source.push('\n');
    }
    mdparser::block_tree(&source)
}

pub fn markup_from_tree(
    text: &Text,
    tree: &Tree,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> Markup {
    markup_builder_from_tree(text, tree, fonts, theme).finish()
}

pub fn markup_builder_from_tree(
    text: &Text,
    tree: &Tree,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> MarkupBuilder {
    let mut markup = Markup::builder();
    visit_markdown_blocks(text, tree, |block| {
        push_markup_for_block(&mut markup, &block, fonts, theme);
    });
    markup
}

pub fn markup_builder_from_blocks(
    blocks: &[MarkdownBlock],
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) -> MarkupBuilder {
    let mut markup = Markup::builder();

    for block in blocks {
        push_markup_for_block(&mut markup, block, fonts, theme);
    }

    markup
}

pub fn markdown_blocks(text: &Text, tree: &Tree) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    visit_markdown_blocks(text, tree, |block| blocks.push(block));
    blocks
}

fn visit_markdown_blocks(text: &Text, tree: &Tree, visit: impl FnMut(MarkdownBlock)) {
    visit_markdown_blocks_where(text, tree, |_| true, visit);
}

fn visit_markdown_blocks_where(
    text: &Text,
    tree: &Tree,
    keep: impl Fn(&std::ops::Range<u32>) -> bool,
    mut visit: impl FnMut(MarkdownBlock),
) {
    let mut view = text.view();
    let byte_count = text.byte_count().min(u32::MAX as usize) as u32;

    for block in BlockIter::new(tree.root_node()) {
        let block = ParsedBlock {
            range: block.range.start.min(byte_count)..block.range.end.min(byte_count),
            marks: block.marks,
        };
        if !keep(&block.range) {
            continue;
        }
        let mut display = None;
        let mut hidden = Vec::new();
        let mut inline = Vec::new();
        let mut checkbox = None;

        if block.marks.code {
            let raw = view.byte_string(block.range.start as usize, block.range.end as usize);
            hidden = fence_hidden_ranges(&raw)
                .into_iter()
                .map(|range| absolute(block.range.start, range))
                .collect();
        } else if !block.marks.horizontal_line {
            let raw = view.byte_string(block.range.start as usize, block.range.end as usize);
            let plain_paragraph =
                block.marks.header.is_none() && !block.marks.list_item && !block.marks.quote;
            if plain_paragraph {
                if let Some(source) = table::parse_table(&raw) {
                    visit(MarkdownBlock {
                        range: block.range,
                        marks: block.marks,
                        display: Some(inline_markup_text(block.marks, &raw)),
                        hidden: Vec::new(),
                        inline: Vec::new(),
                        table: Some(source),
                        checkbox: None,
                    });
                    continue;
                }
            }
            if block.marks.header.is_some() {
                let prefix = heading_prefix_len(&raw);
                if prefix > 0 {
                    hidden.push(absolute(block.range.start, 0..prefix));
                }
            }
            if block.marks.quote {
                for range in quote_hidden_ranges(&raw) {
                    hidden.push(absolute(block.range.start, range));
                }
            }
            if block.marks.list_item {
                checkbox = checkbox::checkbox_marker(&raw)
                    .map(|(range, checked)| (absolute(block.range.start, range), checked));
            }
            for token in inline_decorations(&raw) {
                inline.push(TextDecorationInterval {
                    range: absolute(
                        block.range.start,
                        token.decoration.range.start as usize..token.decoration.range.end as usize,
                    ),
                    id: token.decoration.id,
                });

                for span in token.syntax {
                    hidden.push(absolute(block.range.start, span));
                }
            }
            display = Some(inline_markup_text(block.marks, &raw));
        }

        visit(MarkdownBlock {
            range: block.range,
            marks: block.marks,
            display,
            hidden,
            inline,
            table: None,
            checkbox,
        });
    }
}

fn push_markup_for_block(
    markup: &mut MarkupBuilder,
    block: &MarkdownBlock,

    _fonts: &skia_safe::textlayout::FontCollection,
    theme: &himark::Theme,
) {
    if block.table.is_some() {
        return;
    }
    markup.push_block_styles(block.range.clone(), block.marks.style_ids());

    match block.marks.header {
        Some(1) => markup.push_alignment(block.range.clone(), himark::TextAlignment::Right),
        Some(2) => markup.push_alignment(block.range.clone(), himark::TextAlignment::Center),
        _ => {}
    }

    if let Some((range, checked)) = &block.checkbox {
        markup.push_inlay(
            range.clone(),
            himark::Inlay::editing(
                himark::InlayMode::Left,
                checkbox::CheckboxView::new(*checked, theme),
            ),
        );
        markup.push_hidden(range.clone());
    }

    for hidden in &block.hidden {
        markup.push_hidden(hidden.clone());
    }

    for decoration in &block.inline {
        markup.push_inline(decoration.range.clone(), decoration.id);
    }
}

fn absolute(block_start: u32, range: Range<usize>) -> Range<u32> {
    block_start.saturating_add(range.start.min(u32::MAX as usize) as u32)
        ..block_start.saturating_add(range.end.min(u32::MAX as usize) as u32)
}

fn heading_prefix_len(raw: &str) -> usize {
    let mut start = 0;
    while start < raw.len() {
        let ch = raw[start..].chars().next().expect("text is not empty");
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    while start < raw.len() {
        let ch = raw[start..].chars().next().expect("text is not empty");
        if ch != '#' {
            break;
        }
        start += ch.len_utf8();
    }
    while start < raw.len() {
        let ch = raw[start..].chars().next().expect("text is not empty");
        if !ch.is_whitespace() {
            break;
        }
        start += ch.len_utf8();
    }
    start
}

fn fence_hidden_ranges(raw: &str) -> Vec<Range<usize>> {
    let content = raw.trim_start();
    let trimmed_start = raw.len() - content.len();
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    if !first.trim_start().starts_with("```") && !first.trim_start().starts_with("~~~") {
        return Vec::new();
    }

    let content_start = trimmed_start + first.len();
    let mut ranges = vec![0..content_start];
    let mut offset = content_start;
    for line in lines {
        let line_start = offset;
        offset += line.len();

        let marker = line.trim_end_matches('\n').trim_start();
        if marker.starts_with("```") || marker.starts_with("~~~") {
            let close_start = if line_start > content_start {
                line_start - 1
            } else {
                line_start
            };
            ranges.push(close_start..raw.len());
            break;
        }
    }

    ranges
}

#[derive(Clone, Copy, Default)]
pub struct BlockMarks {
    pub header: Option<u8>,
    pub list_item: bool,
    pub code: bool,
    pub horizontal_line: bool,
    pub quote: bool,
    pub indent: u8,
}

impl BlockMarks {
    fn style_ids(&self) -> Vec<StyleId> {
        let mut ids = Vec::new();
        if let Some(level) = self.header {
            ids.push(StyleId::Header(level));
        }
        if self.list_item {
            ids.push(StyleId::ListItem);
        }
        if self.code {
            ids.push(StyleId::CodeBlock);
        }
        if self.horizontal_line {
            ids.push(StyleId::HorizontalLine);
        }
        if self.quote {
            ids.push(StyleId::Quote);
        }
        if self.indent > 0 {
            ids.push(StyleId::Indent(self.indent));
        }
        ids
    }
}

fn inline_markup_text(marks: BlockMarks, raw: &str) -> String {
    let mut text = String::new();
    if marks.header.is_some() {
        clean_heading(raw, &mut text);
    } else {
        clean_plain_text(raw, &mut text);
    }
    text
}

pub struct MarkdownBlock {
    pub range: Range<u32>,
    pub marks: BlockMarks,

    pub display: Option<String>,

    pub hidden: Vec<Range<u32>>,

    pub inline: Vec<TextDecorationInterval>,

    pub(crate) table: Option<table::TableSource>,

    pub(crate) checkbox: Option<(Range<u32>, bool)>,
}

struct ParsedBlock {
    range: Range<u32>,
    marks: BlockMarks,
}

struct BlockIter<'tree> {
    stack: Vec<Node<'tree>>,
}

impl<'tree> BlockIter<'tree> {
    fn new(root: Node<'tree>) -> Self {
        Self { stack: vec![root] }
    }

    fn emit(&self, node: Node<'_>, marks: BlockMarks) -> ParsedBlock {
        let range = node.byte_range();
        ParsedBlock {
            range: range.start.min(u32::MAX as usize) as u32
                ..range.end.min(u32::MAX as usize) as u32,
            marks,
        }
    }
}

impl Iterator for BlockIter<'_> {
    type Item = ParsedBlock;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.stack.pop()?;
            match node.kind() {
                "document" | "section" | "list" => {
                    push_children_rev(&mut self.stack, node);
                }
                "atx_heading" => {
                    let mut marks = BlockMarks::default();
                    marks.header = Some(heading_level(node));
                    return Some(self.emit(node, marks));
                }
                "paragraph" | "pipe_table" => {
                    return Some(self.emit(node, BlockMarks::default()));
                }
                "list_item" => {
                    let mut marks = BlockMarks::default();
                    marks.list_item = true;

                    let mut nested = None;
                    for index in 0..node.child_count() {
                        let Some(child) = node.child(index as u32) else {
                            continue;
                        };
                        if child.kind() == "list" {
                            nested = Some(index);
                            break;
                        }
                    }
                    let Some(nested) = nested else {
                        return Some(self.emit(node, marks));
                    };

                    for index in (nested..node.child_count()).rev() {
                        if let Some(child) = node.child(index as u32) {
                            self.stack.push(child);
                        }
                    }
                    let start = node.byte_range().start;
                    let end = node
                        .child(nested as u32)
                        .map(|child| child.byte_range().start)
                        .unwrap_or(start);
                    if start >= end {
                        continue;
                    }
                    return Some(ParsedBlock {
                        range: start.min(u32::MAX as usize) as u32
                            ..end.min(u32::MAX as usize) as u32,
                        marks,
                    });
                }
                "fenced_code_block" | "indented_code_block" => {
                    let mut marks = BlockMarks::default();
                    marks.code = true;
                    return Some(self.emit(node, marks));
                }
                "thematic_break" => {
                    let mut marks = BlockMarks::default();
                    marks.horizontal_line = true;
                    return Some(self.emit(node, marks));
                }
                "block_quote" => {
                    let mut marks = BlockMarks::default();
                    marks.quote = true;
                    return Some(self.emit(node, marks));
                }
                _ => push_children_rev(&mut self.stack, node),
            }
        }
    }
}

fn push_children_rev<'tree>(stack: &mut Vec<Node<'tree>>, node: Node<'tree>) {
    for index in (0..node.child_count()).rev() {
        if let Some(child) = node.child(index as u32) {
            stack.push(child);
        }
    }
}

fn section_outline_item(text: &Text, title: &Range<u32>) -> Option<himark::OutlineItem> {
    let raw = text.byte_string(title.start as usize, (title.end - title.start) as usize);
    let line = raw.lines().next().unwrap_or("").trim();
    let mut capped: String = line.chars().take(80).collect();
    if capped.len() < line.len() {
        capped.push('…');
    }
    (!capped.is_empty()).then_some(himark::OutlineItem { title: capped })
}

fn collect_sections(node: Node<'_>, range: &Range<u32>, out: &mut Vec<(Range<u32>, Range<u32>)>) {
    let start = node.start_byte() as u32;
    let end = node.end_byte() as u32;
    if end <= range.start || start >= range.end {
        return;
    }
    if node.kind() == "section" {
        let heading = (0..node.child_count())
            .filter_map(|index| node.child(index as u32))
            .find(|child| child.kind().ends_with("_heading"));
        if let Some(heading) = heading {
            let title = (0..heading.child_count())
                .filter_map(|index| heading.child(index as u32))
                .find(|child| child.kind() == "inline")
                .map(|inline| inline.start_byte() as u32..inline.end_byte() as u32)
                .unwrap_or(heading.start_byte() as u32..heading.end_byte() as u32);
            out.push((start..end, title));
        }
    }
    for index in 0..node.child_count() {
        if let Some(child) = node.child(index as u32) {
            collect_sections(child, range, out);
        }
    }
}

fn heading_level(node: Node<'_>) -> u8 {
    node.child(0)
        .map(|marker| marker.byte_range().len())
        .unwrap_or(1)
        .clamp(1, 6) as u8
}

fn quote_hidden_ranges(raw: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let mut end = indent;
        let bytes = line.as_bytes();
        while end < line.len() && bytes[end] == b'>' {
            end += 1;
            if end < line.len() && bytes[end] == b' ' {
                end += 1;
            }
        }
        if end > indent {
            ranges.push(offset + indent..offset + end);
        }
        offset += line.len();
    }
    ranges
}

fn clean_heading(text: &str, out: &mut String) {
    let text = text.trim_start().trim_start_matches('#').trim_start();
    clean_plain_text(text, out);
}

fn clean_plain_text(text: &str, out: &mut String) {
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
}

struct InlineToken {
    decoration: TextDecorationInterval,
    syntax: Vec<Range<usize>>,
}

fn inline_decorations(text: &str) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    delimited_decorations(text, "`", StyleId::InlineCode, &mut tokens);
    delimited_decorations(text, "~~", StyleId::Strikethrough, &mut tokens);
    delimited_decorations(text, "**", StyleId::Strong, &mut tokens);
    delimited_decorations(text, "__", StyleId::Strong, &mut tokens);
    delimited_decorations(text, "*", StyleId::Emphasis, &mut tokens);
    delimited_decorations(text, "_", StyleId::Emphasis, &mut tokens);
    link_decorations(text, &mut tokens);
    tokens.sort_by_key(|token| token.decoration.range.start);
    keep_non_overlapping(tokens)
}

fn delimited_decorations(
    text: &str,
    delimiter: &str,
    style: StyleId,
    tokens: &mut Vec<InlineToken>,
) {
    let mut search_from = 0;

    while let Some(open) = find_from(text, delimiter, search_from) {
        let content_start = open + delimiter.len();
        let Some(close) = find_from(text, delimiter, content_start) else {
            break;
        };

        if content_start < close {
            tokens.push(InlineToken {
                decoration: TextDecorationInterval::new(content_start..close, style),
                syntax: vec![open..content_start, close..close + delimiter.len()],
            });
        }
        search_from = close + delimiter.len();
    }
}

fn link_decorations(text: &str, tokens: &mut Vec<InlineToken>) {
    let mut search_from = 0;

    while let Some(open) = find_from(text, "[", search_from) {
        let content_start = open + 1;
        let Some(close) = find_from(text, "]", content_start) else {
            break;
        };

        if content_start < close {
            let mut syntax = vec![open..content_start, close..close + 1];
            let mut consumed = close + 1;
            if text[close + 1..].starts_with('(') {
                if let Some(url_end) = find_from(text, ")", close + 1) {
                    syntax.push(close + 1..url_end + 1);
                    consumed = url_end + 1;
                }
            }
            tokens.push(InlineToken {
                decoration: TextDecorationInterval::new(content_start..close, StyleId::Link),
                syntax,
            });
            search_from = consumed;
            continue;
        }
        search_from = close + 1;
    }
}

fn find_from(text: &str, needle: &str, from: usize) -> Option<usize> {
    text.get(from..)
        .and_then(|text| text.find(needle))
        .map(|index| from + index)
}

fn keep_non_overlapping(tokens: Vec<InlineToken>) -> Vec<InlineToken> {
    let mut kept: Vec<InlineToken> = Vec::with_capacity(tokens.len());
    let mut end = 0;

    for token in tokens {
        let start = token
            .syntax
            .first()
            .map_or(token.decoration.range.start as usize, |span| span.start);
        if start >= end {
            end = token
                .syntax
                .last()
                .map_or(token.decoration.range.end as usize, |span| span.end);
            kept.push(token);
        }
    }

    kept
}

pub fn register_handlers(app: &mut himark::Application) {
    app.register_handler::<TableRelayoutEffect>(TableRelayoutHandler(std::sync::Arc::clone(
        app.workshop(),
    )));
}

#[cfg(test)]
trait RunReparse {
    fn run_reparse(self) -> himark::ReparseOutcome;
}

#[cfg(test)]
impl RunReparse for himark::ReparseWork {
    fn run_reparse(self) -> himark::ReparseOutcome {
        let workshop = std::sync::Arc::new(himark::Workshop::new(
            himark::embedded_fonts::source(),
            himark::Theme::embedded(),
        ));
        himark::ReparseHandler(workshop).reparse(self)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod injection_tests;

#[cfg(test)]
mod convergence_probe;

#[cfg(test)]
mod reveal_tests;

#[cfg(test)]
mod utf8_boundary_repro;
