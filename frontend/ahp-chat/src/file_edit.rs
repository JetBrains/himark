// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use ahp_types::common::Uri;
use ahp_types::state::{ContentRef, ToolResultFileEditContent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnapshotRef {
    pub uri: Uri,
    pub content: ContentRef,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffCounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FileEditRefs {
    pub before: Option<FileSnapshotRef>,
    pub after: Option<FileSnapshotRef>,
    pub counts: DiffCounts,
}

impl FileEditRefs {
    pub fn parse(edit: &ToolResultFileEditContent) -> Option<Self> {
        let side = |value: &Option<serde_json::Value>| -> Option<FileSnapshotRef> {
            value
                .as_ref()
                .and_then(|value| serde_json::from_value(value.clone()).ok())
        };
        let before = side(&edit.before);
        let after = side(&edit.after);
        if before.is_none() && after.is_none() {
            return None;
        }
        let counts = edit
            .diff
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        Some(Self {
            before,
            after,
            counts,
        })
    }

    pub fn to_content(&self) -> ToolResultFileEditContent {
        ToolResultFileEditContent {
            before: self
                .before
                .as_ref()
                .map(|side| serde_json::to_value(side).expect("a snapshot serializes")),
            after: self
                .after
                .as_ref()
                .map(|side| serde_json::to_value(side).expect("a snapshot serializes")),
            diff: Some(serde_json::to_value(self.counts).expect("counts serialize")),
        }
    }

    pub fn display_name(&self) -> String {
        let uri = self
            .after
            .as_ref()
            .or(self.before.as_ref())
            .map(|side| side.uri.as_str())
            .unwrap_or("file");
        uri.rsplit('/').next().unwrap_or(uri).to_owned()
    }
}

/// Both sides of a file edit, BUILT: two documents with their syntax,
/// the diff operation installed and normalized on the after side, the
/// hunk markup, and the prepared marks (washes, word tints, fold
/// strips). Everything the seeded pair road needs EXCEPT the editors —
/// layout belongs to the mount, at the cell's own width.
///
/// Built off the UI thread by `BuildFileEditEffect`: a chat turn brings
/// hundreds of edits, and parsing two files plus diffing them per edit
/// is not frame work (docs/model-view.md, the open road's rule).
#[derive(Clone)]
pub struct BuiltFileEdit {
    pub before: editor::Document,
    pub after: editor::Document,
    pub diff: ::editor::diff::DiffId,
    /// The after side's hunk markup, minted by the diff install.
    pub hunks: ::editor::MarkupId,
    /// The before side's wash markup, already filled in.
    pub left_marks: ::editor::MarkupId,
    pub prepared: ::editor::PreparedMarks,
}

/// The seeded pair recipe, off-thread. `name` names the language (the
/// edited file's own name); `parsers` and `differ` are passed in because
/// the build context is the workshop's bare store, not the app's.
pub fn build_file_edit(
    name: &str,
    contents: &ahp_wire::client::FileEditContents,
    parsers: &std::sync::Arc<editor::SyntaxLanguages>,
    differ: &std::sync::Arc<dyn ::editor::diff::DiffPolicy>,
    store: &imba::store::Store,
    ui: &imba::UiCtx,
    fonts: &skia_safe::textlayout::FontCollection,
    theme: &editor::Theme,
) -> BuiltFileEdit {
    let before_text = editor::Text::from_string_exact(contents.before.as_deref().unwrap_or(""));
    let after_text = editor::Text::from_string_exact(contents.after.as_deref().unwrap_or(""));
    let extension = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let mut before = crate::cell::side_document(
        before_text.clone(),
        &extension,
        parsers,
        store,
        ui,
        fonts,
        theme,
    );
    let mut after = crate::cell::side_document(
        after_text, &extension, parsers, store, ui, fonts, theme,
    );

    let operation = differ.diff(&before_text, after.text(), None);
    let prepared = ::editor::prepare_marks(&operation, before.text());
    let diff = after.add_diff(operation.clone(), before.revision());
    after.install_normalized_diff(diff, operation, before.revision());
    let hunks = after.diff(diff).expect("just added").markup();

    // The before side's washes and fold spacers, settled here too — the
    // mount only hands the markup to its editor.
    let mut throwaway = imba::effect::Batch::new();
    let quiet = &mut throwaway.effects();
    let left_marks = before.add_markup();
    before.replace_markup(
        left_marks,
        prepared.left.clone(),
        &[],
        store,
        ui,
        fonts,
        theme,
        quiet,
    );

    BuiltFileEdit {
        before,
        after,
        diff,
        hunks,
        left_marks,
        prepared,
    }
}

pub fn snapshot(uri: &str, content_uri: &str) -> FileSnapshotRef {
    FileSnapshotRef {
        uri: uri.to_owned(),
        content: ContentRef {
            uri: content_uri.to_owned(),
            size_hint: None,
            content_type: None,
            nonce: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_round_trip_the_wire_shape() {
        let refs = FileEditRefs {
            before: Some(snapshot("src/scroll.rs", "ahp-content:/b1")),
            after: Some(snapshot("src/scroll.rs", "ahp-content:/a1")),
            counts: DiffCounts {
                added: Some(7),
                removed: Some(2),
            },
        };
        let parsed = FileEditRefs::parse(&refs.to_content()).expect("parses back");
        assert_eq!(parsed.after.unwrap().content.uri, "ahp-content:/a1");
        assert_eq!(parsed.before.unwrap().uri, "src/scroll.rs");
        assert_eq!(parsed.counts.added, Some(7));
        assert_eq!(refs.display_name(), "scroll.rs");
    }

    #[test]
    fn unrecognizable_sides_parse_to_none() {
        let junk = ToolResultFileEditContent {
            before: Some(serde_json::json!({ "weird": true })),
            after: None,
            diff: None,
        };
        assert!(FileEditRefs::parse(&junk).is_none());
    }
}

/// Both sides of a file edit, fetched AND built (two documents with
/// syntax, the diff, the prepared marks) off the UI thread — the chat's
/// diff cell only mounts the result. A coding turn brings hundreds of
/// edits; none of this is frame work.
pub struct BuildFileEditEffect {
    pub client: std::sync::Arc<dyn ahp_wire::client::ChatClient>,
    pub before: Option<Uri>,
    pub after: Option<Uri>,
    /// The edited file's name — it names the language.
    pub name: String,
}

impl std::fmt::Display for BuildFileEditEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "build file edit {}", self.name)
    }
}

impl imba::effect::Effect for BuildFileEditEffect {
    type Result = Result<BuiltFileEdit, String>;
}
