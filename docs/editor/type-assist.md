# Type Assist

Structural help while typing: the right indentation on Enter, indent
steps on Tab/Shift-Tab, balanced auto-closing braces — and in markdown
lists, Obsidian's editing model. The editor core knows NONE of the
rules: the language whose syntax encloses the caret is handed the
tree, the text and the location, and answers with an [`Operation`]
(and, optionally, where the caret lands). No assist means the plain
behavior.

## 1. The behavior

Everywhere:

- **Enter** inserts a newline plus indentation: the current line's
  leading whitespace, bumped one unit after an opener (`{`, `(`, `[`)
  — and the classic double break between a just-opened brace and its
  closer (`{\n    |\n}`).
- **Tab** increases indentation; **Shift-Tab** decreases it. With a
  selection, every touched line steps together.
- **Brace/quote openers auto-close** (`(` → `(|)`) — but only while
  the surrounding code is BALANCED: with an unmatched `(` already
  dangling, typing `(` inserts just the `(` (closing it would "fix"
  the wrong spot).

In markdown lists, specifically:

1. **Enter** continues the list: a new item at the same nesting, the
   marker minted by kind — `-`/`*` verbatim, ordered markers
   incremented, task items as a fresh unchecked `- [ ]`. Text after
   the caret moves into the new item. Enter on an EMPTY item
   terminates the list instead: the empty item's marker is removed
   (the Obsidian/VSCode convention).
2. **Shift-Enter** breaks the line WITHOUT a new item: a newline
   indented to the item's content column — a soft continuation inside
   the same item (Obsidian's block-linebreak).
3. **Tab / Shift-Tab** nest / un-nest the item: the item's lines
   re-indent by one list level; Shift-Tab at top level changes
   nothing.

## 2. The seam: the language answers

Type assist is a third (defaulted) job on [`SyntaxLanguage`] — beside
`parse` and `markup_for_changes`, registered the same way, so the
inversion holds: the editor core stays language-blind, himarkdown and
the tree-sitter languages bring their own rules.

```rust
/// One assist ask: the caret'd syntax's tree and place, the whole
/// text, and what the user did.
pub struct AssistRequest<'a> {
    /// What was typed.
    pub kind: AssistKind,
    /// The whole document text (the rope — never materialized).
    pub text: &'a Text,
    /// The enclosing syntax's retained parse.
    pub tree: &'a dyn SyntaxTree,
    /// The syntax's document range — `tree` coordinates are relative
    /// to `range.start` (the injection contract).
    pub range: Range<u32>,
    /// The caret's selection (collapsed = caret), document bytes.
    pub location: Range<u32>,
}

pub enum AssistKind {
    /// Enter; `soft` is Shift-Enter (the in-item linebreak).
    Enter { soft: bool },
    /// Tab / Shift-Tab.
    Indent,
    Outdent,
    /// A typed character an assist may want to complete (`(`, `[`,
    /// `{`, `"`). The character is NOT yet in the text.
    Typed(char),
}

/// The answer: the edit, in DOCUMENT coordinates (the language adds
/// its `range.start` base), and where the caret lands (post-edit
/// bytes; `None` = the operation's natural transform).
pub struct Assist {
    pub operation: Operation,
    pub caret: Option<u32>,
}

// On SyntaxLanguage, defaulted — languages without opinions change
// nothing:
fn assist(&self, _request: &AssistRequest<'_>) -> Option<Assist> {
    None
}
```

**Which language is asked.** The innermost syntax whose interval
contains the caret — `Document::syntax_enclosing`, over the injection
machinery's syntax intervals (`Markup::child_syntax_at`, boundaries
inclusive): a caret inside a rust fence asks rust; outside it,
markdown. A cold syntax (`tree: None`) or an unregistered language
answers nothing and the plain behavior applies.

**`Option` all the way down.** `None` from the plugin means "no
opinion here" — the core falls back: Enter inserts `"\n"`, Tab
inserts the indent unit, Typed inserts the character; Outdent has NO
fallback and does nothing. An answered no-op is different from
declining: both implementations answer Outdent-at-column-0 with
`Some` carrying an EMPTY operation, so the caret is handled without
touching the text. The plugin never re-implements plain typing.

## 3. The flow through the core

The machinery lives in `frontend/editor/src/assist.rs`
(`Document::syntax_enclosing`, `Document::assist_at_carets`). The
keymap (`keymap.json`) binds Enter to `EditorCommand::Enter { soft }`
(plain and Shift — `editor.newline`), Tab/Shift-Tab to
`EditorCommand::Indent`/`Outdent` (`editor.indent`/`editor.outdent`).
`InsertText` of a single non-control character consults
[`AssistKind::Typed`] first — never while IME-composing.
`EditorCommand::Paste` NEVER consults assists: a pasted `(` must not
auto-close. Table cells rewrite Enter to plain `InsertText` and drop
Indent/Outdent, so assists never run inside a cell.

`Document::assist_at_carets(store, editor, kind, …) -> bool` asks per
caret and answers `false` when NO caret produced an assist — the
command arm then runs the untouched plain path. Otherwise:

- **Multi-caret**: one ask per caret (each may sit in a different
  syntax), the answers merged into ONE bulk operation through an
  `OperationBuilder` and applied through one `Document::edit` — one
  revision, one undo entry, one change notification, the repairs
  riding the same effects as any edit. A caret whose language
  declined gets the fallback edit for that key (`fallback_edit`);
  answered carets land where their assist said, repositioned by the
  running shift of the edits before them. An assist whose edit would
  overlap a previous caret's is silently dropped — only its anchor
  shifts.
- **IME**: composing text never consults assists — marked-text
  replacement stays exactly as it is.
- **Purity**: the ask runs synchronously on the UI thread against the
  CURRENT tree (edit-stepped, possibly a reparse behind the text —
  the same staleness every tree consumer tolerates). Assists must be
  cheap: locate the caret's node, read a few lines. Nothing launches,
  nothing blocks.

The indent unit is four spaces — a private `INDENT_UNIT` const,
duplicated in the editor core (the fallback) and hisitter (the
implementation).

## 4. The implementations

**himarkdown** (the markdown tree is tree-sitter — `list`,
`list_item`, `list_marker_*`, `task_list_marker_*`, `paragraph`
nodes):

- `Typed` → `None`, always: markdown has NO auto-pairs.
- `Enter` in a `list_item`: mint the next marker from the item's
  (kind, indent) — bullets verbatim, ordered number + 1, task items
  re-emitting `[ ] `; an empty item deletes its own marker instead
  (list termination). Ordered markers renumber only the NEW item.
- `Enter { soft: true }` in a `list_item`: `"\n"` + spaces to the
  item's content column.
- `Indent`/`Outdent` in a `list_item`: ± one level (two spaces per
  level, matching the parser's nesting rules) applied to the item's
  own lines; a nested `list` node begins AFTER the child line's
  indent (the indent rides the paragraph's `block_continuation`), so
  re-indenting caps at the line before the nested list's line.
  `Outdent` at column 0 answers the empty no-op operation.
- Elsewhere: `None` (plain behavior).

**hisitter** ([`TreeSitterLanguage`] — every tree-sitter language
gets this free):

- `Enter`: copy the current line's leading whitespace run, bump one
  indent unit after a trailing opener (`{`, `(`, `[`), and double the
  break when the matching closer is the next thing — `{\n    |\n}`.
  Grammar-agnostic text rules, no node-depth computation; `soft` is
  ignored (Shift-Enter behaves like Enter in code).
- `Indent`/`Outdent`: ± one indent unit at the touched lines' starts;
  `Outdent` on unindented lines answers the empty no-op operation.
- `Typed('(' | '[' | '{' | '"')` — and ONLY those four; backticks are
  never auto-closed: close iff balanced, judged by counting the
  pair's characters over a bounded window — the largest enclosing
  node no bigger than 16 KiB, else ±8 KiB around the caret. Balanced
  → insert both and park the caret between; unbalanced → `None` and
  the plain character inserts.

## 5. Deliberately absent (choices, not gaps)

- Overtype of a just-auto-closed `)` and delete-the-pair backspace
  (needs per-editor "just closed" state).
- Wrapping a selection in quotes/braces.
- Paste re-indentation; whole-list renumbering.
- Blockquote/fence continuation on Enter.
- Table cell Tab-navigation (tables own Tab someday — the keymap arm
  is where that decision will live).
