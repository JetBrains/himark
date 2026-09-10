use std::{ops::Range, sync::Arc};

use operation::Operation;
use text::Text;

use crate::{
    document::Document,
    markup::{MarkupBuilder, Syntax},
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxSite {
    pub range: Range<u32>,

    pub language: String,
}

pub trait SyntaxTree: Send + Sync {
    fn clone_tree(&self) -> Box<dyn SyntaxTree>;

    fn edit(&mut self, operation: &Operation, view: &mut text::TextView, base: u32);

    fn changed_since(&self, old: &dyn SyntaxTree) -> Option<Vec<Range<u32>>>;

    fn as_any(&self) -> &dyn std::any::Any;
}

pub struct AssistRequest<'a> {
    pub kind: AssistKind,

    pub text: &'a Text,

    pub tree: &'a dyn SyntaxTree,

    pub range: Range<u32>,

    pub location: Range<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssistKind {
    Enter {
        soft: bool,
    },

    Indent,
    Outdent,

    Typed(char),
}

pub struct Assist {
    pub operation: Operation,
    pub caret: Option<u32>,
}

pub trait SyntaxLanguage: Send + Sync {
    fn parse(
        &self,
        text: &Text,
        range: Range<u32>,
        old: Option<&dyn SyntaxTree>,
    ) -> Option<Box<dyn SyntaxTree>>;

    #[allow(clippy::too_many_arguments)]
    fn markup_for_changes(
        &self,
        text: &Text,
        range: Range<u32>,
        tree: &dyn SyntaxTree,
        changed: &[Range<u32>],
        replacement: &mut MarkupBuilder,
        invalidated: &mut Vec<Range<u32>>,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    );

    fn sites(&self, _text: &Text, _range: Range<u32>, _tree: &dyn SyntaxTree) -> Vec<SyntaxSite> {
        Vec::new()
    }

    fn assist(&self, _request: &AssistRequest<'_>) -> Option<Assist> {
        None
    }
}

#[derive(Clone)]
pub struct SideGrammar {
    pub module: &'static str,
    pub symbol: &'static str,
    pub crate_name: &'static str,
    pub parser_dir: &'static str,
    #[cfg(not(target_os = "emscripten"))]
    pub highlights: Arc<dyn Fn() -> String + Send + Sync>,
}

pub struct LanguageEntry {
    names: Vec<&'static str>,
    slot: std::sync::OnceLock<Arc<dyn SyntaxLanguage>>,
    loader: Option<Arc<dyn Fn() -> Option<Arc<dyn SyntaxLanguage>> + Send + Sync>>,

    loading: std::sync::Mutex<()>,
    side: Option<SideGrammar>,
}

impl LanguageEntry {
    pub fn names(&self) -> &[&'static str] {
        &self.names
    }

    pub fn side(&self) -> Option<&SideGrammar> {
        self.side.as_ref()
    }
}

#[derive(Default, Clone)]
pub struct SyntaxLanguages {
    entries: Vec<Arc<LanguageEntry>>,
}

impl SyntaxLanguages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, names: &[&'static str], language: Arc<dyn SyntaxLanguage>) {
        let slot = std::sync::OnceLock::new();
        let _ = slot.set(language);
        self.entries.push(Arc::new(LanguageEntry {
            names: names.to_vec(),
            slot,
            loader: None,
            loading: std::sync::Mutex::new(()),
            side: None,
        }));
    }

    pub fn register_lazy(
        &mut self,
        names: &[&'static str],
        side: Option<SideGrammar>,
        loader: Arc<dyn Fn() -> Option<Arc<dyn SyntaxLanguage>> + Send + Sync>,
    ) {
        self.entries.push(Arc::new(LanguageEntry {
            names: names.to_vec(),
            slot: std::sync::OnceLock::new(),
            loader: Some(loader),
            loading: std::sync::Mutex::new(()),
            side,
        }));
    }

    pub fn knows(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }

    fn entry(&self, name: &str) -> Option<&Arc<LanguageEntry>> {
        self.entries
            .iter()
            .find(|entry| entry.names.contains(&name))
    }

    pub fn entries(&self) -> impl Iterator<Item = &Arc<LanguageEntry>> {
        self.entries.iter()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn SyntaxLanguage>> {
        self.entry(name)?.slot.get()
    }

    pub fn ensure(&self, name: &str) -> Option<Arc<dyn SyntaxLanguage>> {
        let entry = self.entry(name)?;
        if let Some(resident) = entry.slot.get() {
            return Some(resident.clone());
        }
        let loader = entry.loader.as_ref()?;

        let _guard = entry.loading.lock().ok()?;
        if let Some(resident) = entry.slot.get() {
            return Some(resident.clone());
        }
        let loaded = loader()?;
        let _ = entry.slot.set(loaded.clone());
        Some(loaded)
    }

    pub fn parse_syntax(
        &self,
        name: &str,
        text: &Text,
        range: Range<u32>,
        old: Option<&Syntax>,
        edited: &[Range<u32>],
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> Option<(Syntax, Vec<Range<u32>>, Vec<SyntaxSite>)> {
        let language = self.ensure(name)?;
        let byte_count = range.end.saturating_sub(range.start);
        let old_tree = old.and_then(|old| old.tree.as_deref());
        let tree = language.parse(text, range.clone(), old_tree)?;

        let mut changed: Vec<Range<u32>> = match old_tree {
            Some(old_tree) => tree
                .changed_since(old_tree)
                .map(|ranges| {
                    ranges
                        .into_iter()
                        .map(|changed| changed.start.min(byte_count)..changed.end.min(byte_count))
                        .collect()
                })

                .unwrap_or_else(|| vec![0..byte_count]),
            None => vec![0..byte_count],
        };
        changed.extend(edited.iter().cloned());

        let mut invalidated = changed.clone();
        let mut replacement = crate::markup::Markup::builder();
        if !changed.is_empty() {
            language.markup_for_changes(
                text,
                range.clone(),
                tree.as_ref(),
                &changed,
                &mut replacement,
                &mut invalidated,
                fonts,
                theme,
            );
        }

        let (fold_pushes, outline_pushes) = replacement.take_channels();
        let mut markup = old.map(|old| old.markup.clone()).unwrap_or_default();
        markup.splice(&invalidated, replacement, fonts, theme);
        let mut folds = old
            .map(|old| old.folds.clone())
            .unwrap_or_else(intervals::Intervals::new);
        crate::markup::splice_intervals(
            &mut folds,
            &changed,
            fold_pushes.into_iter().map(|range| (range, ())),
        );
        let mut outline = old
            .map(|old| old.outline.clone())
            .unwrap_or_else(intervals::Intervals::new);
        crate::markup::splice_intervals(&mut outline, &changed, outline_pushes);
        let sites = language.sites(text, range, tree.as_ref());
        Some((
            Syntax {
                language: name.to_owned(),
                tree: Some(tree),
                markup,
                folds,
                outline,
            },
            invalidated,
            sites,
        ))
    }
}

pub struct ReparseWork {
    token: crate::document::DocumentToken,
    revision: u64,
    text: Text,

    root: Syntax,

    edited: Vec<Range<u32>>,

    anchor: Option<crate::editor::EditorId>,
    parsers: Arc<SyntaxLanguages>,
}

impl ReparseWork {
    pub fn capture(document: &Document, parsers: Arc<SyntaxLanguages>) -> Option<Self> {
        Some(Self {
            token: document.token(),
            revision: document.revision(),
            text: document.text().clone(),
            root: document.syntax()?.clone(),
            edited: document.edited_since_parse().to_vec(),
            anchor: document.editors.keys().next().copied(),
            parsers,
        })
    }
}

pub struct ReparseOutcome {
    token: crate::document::DocumentToken,
    revision: u64,
    anchor: Option<crate::editor::EditorId>,
    invalidated: Vec<Range<u32>>,

    root: Syntax,

    descended: bool,
}

impl ReparseOutcome {
    pub fn invalidated(&self) -> &[Range<u32>] {
        &self.invalidated
    }

    pub fn anchor(&self) -> Option<crate::editor::EditorId> {
        self.anchor
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        crate::document::DocumentToken,
        u64,
        Vec<Range<u32>>,
        Syntax,
        bool,
    ) {
        (
            self.token,
            self.revision,
            self.invalidated,
            self.root,
            self.descended,
        )
    }
}

pub struct ReparseEffect {
    work: ReparseWork,
}

impl ReparseEffect {
    pub fn new(work: ReparseWork) -> Self {
        Self { work }
    }
}

impl imba::effect::Effect for ReparseEffect {
    type Result = crate::editor_view::EditorCommand;
}

pub struct ReparseHandler(pub std::sync::Arc<crate::env::Workshop>);

impl imba::effect::EffectHandler<ReparseEffect> for ReparseHandler {
    async fn handle(&self, effect: ReparseEffect) -> crate::editor_view::EditorCommand {
        crate::editor_view::EditorCommand::ApplyReparse(self.reparse(effect.work))
    }
}

impl ReparseHandler {
    pub fn reparse(&self, work: ReparseWork) -> ReparseOutcome {
        work.run(&self.0.fonts(), &self.0.theme())
    }
}

impl ReparseWork {
    pub fn run(
        self,
        fonts: &skia_safe::textlayout::FontCollection,
        theme: &crate::theme::Theme,
    ) -> ReparseOutcome {
        let work = self;
        {
            let byte_count = work.text.byte_count().min(u32::MAX as usize) as u32;
            let Some((mut root, invalidated, sites)) = work.parsers.parse_syntax(
                &work.root.language,
                &work.text,
                0..byte_count,
                Some(&work.root),
                &work.edited,
                fonts,
                theme,
            ) else {
                return ReparseOutcome {
                    token: work.token,
                    revision: work.revision,
                    anchor: work.anchor,
                    invalidated: Vec::new(),
                    root: work.root,
                    descended: false,
                };
            };

            let fresh = root
                .markup
                .reconcile_syntaxes_in(&work.root.markup, &invalidated, &sites);

            let mut plan: Vec<(crate::markup::SyntaxId, Range<u32>)> = fresh
                .iter()
                .map(|block| (block.key, block.range.clone()))
                .collect();
            for range in &invalidated {
                for (key, marker) in root.markup.syntax_in(range.clone()) {
                    if !plan.iter().any(|(planned, _)| *planned == key) {
                        plan.push((key, marker));
                    }
                }
            }

            for site in &sites {
                for (key, marker) in root.markup.syntax_in(site.range.clone()) {
                    if marker == site.range
                        && !plan.iter().any(|(planned, _)| *planned == key)
                        && root
                            .markup
                            .syntax(key)
                            .is_some_and(|syntax| syntax.tree.is_none())
                    {
                        plan.push((key, marker));
                    }
                }
            }

            let descended = !plan.is_empty();
            let mut invalidated = invalidated;
            for (key, marker) in plan {
                let Some(old) = root.markup.syntax(key).cloned() else {
                    continue;
                };
                if old.language.is_empty() {
                    continue;
                }
                let language = old.language.clone();

                let edited: Vec<Range<u32>> = work
                    .edited
                    .iter()
                    .filter(|range| range.start <= marker.end && range.end >= marker.start)
                    .map(|range| {
                        range.start.max(marker.start) - marker.start
                            ..range.end.min(marker.end) - marker.start
                    })
                    .collect();
                if let Some((child, child_invalidated, _)) = work.parsers.parse_syntax(
                    &language,
                    &work.text,
                    marker.clone(),

                    Some(&old).filter(|old| old.tree.is_some()),
                    &edited,
                    fonts,
                    theme,
                ) {
                    invalidated.extend(child_invalidated.into_iter().map(|range| {
                        marker.start.saturating_add(range.start)
                            ..marker.start.saturating_add(range.end).min(marker.end)
                    }));
                    root.markup.set_syntax(key, child);
                }
            }

            ReparseOutcome {
                token: work.token,
                revision: work.revision,
                anchor: work.anchor,
                invalidated,
                root,
                descended,
            }
        }
    }
}
