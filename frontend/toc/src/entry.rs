// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! One line of the Contents outline, set like a book's table of
//! contents: the KIND word in a right-aligned margin, the name, a
//! dotted leader, the line number. A top-level container (a markdown
//! section, a `mod`) heads its members as a caps GROUP label instead.
//! The entry the caret is in is CURRENT: a size up, in the live ink.

use imba::{
    arena::Arena,
    effect::Effects,
    event::{Event, EventResult},
    layout::LayoutExt as _,
    store::Store,
    ui::UiCtx,
    View,
};

use hikit::tree_item::{TreeItemCommand, TreeLabelCommand};

use crate::chrome::{LeaderLine, Oblique};

/// Declaration keywords an outline title may lead with — the KIND
/// the drawer sets in its margin.
const KINDS: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "impl",
    "trait",
    "mod",
    "type",
    "union",
    "class",
    "interface",
    "object",
    "func",
    "function",
    "def",
    "namespace",
    "module",
    "record",
    "mixin",
    "extension",
    "instance",
];

/// Words that may stand before the keyword and say nothing about
/// what is declared.
const MODIFIERS: &[&str] = &[
    "pub", "async", "const", "unsafe", "extern", "default", "static", "export", "public",
    "private", "protected", "internal", "open", "abstract", "final", "override", "sealed",
    "inline", "suspend", "virtual", "data",
];

/// Kinds that name a TYPE — their names stand a rank up.
const TYPE_KINDS: &[&str] = &[
    "struct",
    "enum",
    "union",
    "trait",
    "class",
    "interface",
    "type",
    "record",
    "object",
];

/// Kinds whose members a top-level instance heads as a group.
const GROUP_KINDS: &[&str] = &["mod", "namespace", "module"];

/// What an outline title DECLARES: the keyword it leads with (past
/// any modifiers) and the name after it. A title with no keyword
/// (a markdown heading) is all name.
pub(crate) fn split_kind(title: &str) -> (Option<&str>, &str) {
    let mut rest = title.trim_start();
    loop {
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..word_end];
        let tail = rest[word_end..].trim_start();
        if tail.is_empty() {
            return (None, title.trim());
        }
        if KINDS.contains(&word) {
            return (Some(word), tail);
        }
        let modifier = MODIFIERS.contains(&word) || word.starts_with("pub(") || word.starts_with('"');
        if !modifier {
            return (None, title.trim());
        }
        rest = tail;
    }
}

pub(crate) fn is_type_kind(kind: &str) -> bool {
    TYPE_KINDS.contains(&kind)
}

pub(crate) fn is_group_kind(kind: &str) -> bool {
    GROUP_KINDS.contains(&kind)
}

/// Where an entry stands in the drawer.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A caps group label heading its members; `first` sits flush
    /// under the column's top air.
    Group { first: bool },
    /// An entry: a type (`strong`) or not, nested `indent` levels
    /// under a member-holding entry (an `impl`, a subsection).
    Entry { strong: bool, indent: u8 },
}

/// One outline line's facts, resolved at landing.
#[derive(Clone, Debug)]
pub struct ContentsEntry {
    pub kind: Option<String>,
    pub name: String,
    pub line: u32,
    pub place: Place,
    /// The caret is in this entry (the innermost one holding it).
    pub current: bool,
    /// Prose outlines (markdown) set their names in the row role,
    /// code outlines in the mono face.
    pub prose: bool,
}

/// The outline's list row. It mints no commands of its own — a press
/// is the LIST's to answer (`Select` then `Activate`) — but speaks
/// the tree item's command type so the forest's fold decoders read
/// it.
#[derive(Clone)]
pub struct ContentsRow {
    entry: ContentsEntry,
}

impl ContentsRow {
    pub fn new(entry: ContentsEntry) -> Self {
        Self { entry }
    }

    pub fn entry(&self) -> &ContentsEntry {
        &self.entry
    }
}

impl View for ContentsRow {
    type Command = TreeItemCommand<TreeLabelCommand>;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        let theme = editor::env::Themes::of(store);
        let chrome = theme.part(crate::contents_chrome::ContentsChrome::KEY);
        let ink = theme.part(::editor::theme::InkChrome::KEY);
        let entry = &self.entry;
        let current = entry.current;
        let body: imba::layout::LayoutBox<'a, Self::Command> = match entry.place {
            Place::Group { first } => {
                let caps = hikit::ui::caps(store, ui);
                let caps = match current {
                    true => caps.colored(ink.live.0),
                    false => caps,
                };
                imba::layout::LayoutBox::new(
                    arena,
                    hikit::ui::text(&caps, entry.name.to_uppercase()).pad_insets(
                        imba::layout::Insets {
                            left: chrome.text_edge + chrome.kind_column,
                            top: match first {
                                true => 0.0,
                                false => chrome.group_gap,
                            },
                            right: chrome.text_edge,
                            bottom: chrome.group_after,
                        },
                    ),
                )
            }
            Place::Entry { strong, indent } => {
                let member = indent > 0;
                // The margin word: the kind, in the label ink (live
                // when current). Members leave it blank — their
                // holder already said it.
                let kind_style = hikit::ui::brief(store, ui)
                    .sized(chrome.kind_size)
                    .colored(match current {
                        true => ink.live.0,
                        false => ink.ink_3.0,
                    });
                let kind_word = match member {
                    true => String::new(),
                    false => entry.kind.clone().unwrap_or_default(),
                };
                let kind = imba::layout::Row::new(arena)
                    .weighted(1.0, imba::layout::Fill::new().height(0.0))
                    .child_by_baseline(Oblique {
                        inner: hikit::ui::text(&kind_style, kind_word),
                        slant: chrome.kind_slant,
                    })
                    .pad_insets(imba::layout::Insets {
                        right: chrome.kind_pad,
                        ..Default::default()
                    })
                    .width(chrome.kind_column);

                let name_style = match entry.prose {
                    true => match (current, strong) {
                        (true, _) => hikit::ui::row_strong(store, ui)
                            .sized(chrome.current_name_size)
                            .colored(ink.declaration.0),
                        (false, true) => hikit::ui::row_strong(store, ui),
                        (false, false) => hikit::ui::row(store, ui),
                    },
                    false => {
                        let mut style = hikit::ui::margin(store, ui);
                        let (size, color, bold) = match (current, strong, member) {
                            (true, _, _) => {
                                (chrome.current_name_size, ink.declaration.0, true)
                            }
                            (false, true, _) => (chrome.type_size, chrome.name_color.0, true),
                            (false, false, true) => {
                                (chrome.name_size, chrome.member_color.0, false)
                            }
                            (false, false, false) => {
                                (chrome.name_size, chrome.name_color.0, false)
                            }
                        };
                        style.font.set_embolden(bold);
                        style.sized(size).colored(color)
                    }
                };
                let name = hikit::ui::text(&name_style, entry.name.clone()).pad_insets(
                    imba::layout::Insets {
                        left: f32::from(indent) * chrome.member_indent,
                        ..Default::default()
                    },
                );
                let (accent, number_ink) = match current {
                    true => (ink.live.0, ink.live.0),
                    false => (chrome.leader_color.0, ink.ink_3.0),
                };
                let number_style = hikit::ui::brief(store, ui)
                    .sized(chrome.number_size)
                    .colored(number_ink);
                let line = LeaderLine {
                    name: imba::layout::LayoutBox::new(arena, name),
                    number: hikit::ui::text(&number_style, entry.line.to_string()),
                    number_min_width: chrome.number_min_width,
                    color: accent,
                    min: chrome.leader_min,
                    dot: chrome.leader_dot,
                    rise: chrome.leader_rise,
                    gap: chrome.gap,
                };
                let height = match current {
                    true => chrome.current_row_height,
                    false => chrome.row_height,
                };
                imba::layout::LayoutBox::new(
                    arena,
                    imba::layout::Row::new(arena)
                        .gap(chrome.gap)
                        .child_by_baseline(kind)
                        .child_by_baseline(line)
                        .pad_insets(imba::layout::Insets {
                            left: chrome.text_edge,
                            right: chrome.text_edge,
                            ..Default::default()
                        })
                        .align(imba::layout::Alignment::CenterStart)
                        .height(height),
                )
            }
        };
        // The row body consumes nothing: an unclaimed click is the
        // LIST's to answer — `Select` then `Activate(Click)`
        // (docs/ui/list-keyboard.md §2).
        body.on_event(
            move |_arena: &Arena, event: &Event<'_>, _size| match event {
                Event::MouseDown { .. } => EventResult::handled(),
                _ => EventResult::Ignored,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::split_kind;

    #[test]
    fn titles_split_into_kind_and_name() {
        assert_eq!(split_kind("pub fn annotations_session"), (Some("fn"), "annotations_session"));
        assert_eq!(split_kind("pub(crate) struct HostConfig"), (Some("struct"), "HostConfig"));
        assert_eq!(
            split_kind("impl Default for HostConfig"),
            (Some("impl"), "Default for HostConfig")
        );
        assert_eq!(split_kind("pub async unsafe fn run"), (Some("fn"), "run"));
        assert_eq!(split_kind("extern \"C\" fn hook"), (Some("fn"), "hook"));
        assert_eq!(split_kind("Getting started"), (None, "Getting started"));
        assert_eq!(split_kind("fn"), (None, "fn"), "a bare keyword is a name");
        assert_eq!(split_kind("Foo"), (None, "Foo"));
    }
}
