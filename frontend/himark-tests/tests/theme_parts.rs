// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The open theme parses LAZILY — a malformed section only panics on
//! its first lookup. This test is the eager pass the old monolithic
//! deserialize used to be: every part of every shipped theme, forced.

use imba::theme::Theme;

fn force_every_part(theme: &Theme) {
    // editor's parts
    use ::editor::theme as et;
    let _ = theme.part(et::Styles::KEY);
    let _ = theme.part(et::CaretChrome::KEY);
    let _ = theme.part(et::CodePanelChrome::KEY);
    let _ = theme.part(et::EditorGutterChrome::KEY);
    let _ = theme.part(et::FoldChipChrome::KEY);
    let _ = theme.part(et::GutterChrome::KEY);
    let _ = theme.part(et::InkChrome::KEY);
    let _ = theme.part(et::PanelChrome::KEY);
    let _ = theme.part(et::RuleChrome::KEY);
    let _ = theme.part(et::ScrollStripeChrome::KEY);
    let _ = theme.part(et::ScrollbarChrome::KEY);
    let _ = theme.part(et::SearchChrome::KEY);
    let _ = theme.part(et::StickyChrome::KEY);
    let _ = theme.part(et::TypeChrome::KEY);
    let _ = theme.part(et::WindowChrome::KEY);
    // the editor's slices of hikit-owned sections
    let _ = theme.part(et::PopupChrome::KEY);
    let _ = theme.part(et::TreeWell::KEY);
    let _ = theme.part(et::DiffFoldChrome::KEY);
    let _ = theme.part(et::HoverSheet::KEY);
    // the subsystems' parts
    let _ = theme.part(::hikit::chrome::ToolbarChrome::KEY);
    let _ = theme.part(::hikit::chrome::ChatChrome::KEY);
    let _ = theme.part(::hikit::chrome::ComboChrome::KEY);
    let _ = theme.part(::hikit::chrome::DiffChrome::KEY);
    let _ = theme.part(::hikit::chrome::PeekerChrome::KEY);
    let _ = theme.part(::hikit::chrome::TreeChrome::KEY);
    let _ = theme.part(::changesview::chrome::HistoryChrome::KEY);
    let _ = theme.part(::filetree::chrome::FilesChrome::KEY);
    let _ = theme.part(::locations::chrome::FindChrome::KEY);
    let _ = theme.part(::comments::chrome::CommentChrome::KEY);
    let _ = theme.part(::terminals::chrome::TerminalChrome::KEY);
    let _ = theme.part(::ahp_session::chrome::SessionsChrome::KEY);
    let _ = theme.part(::himark::stats_chrome::StatsChrome::KEY);
    let _ = theme.part(::toc::contents_chrome::ContentsChrome::KEY);
    let _ = theme.part(::himarkdown::chrome::TableChrome::KEY);
    let _ = theme.part(::himarkdown::chrome::CheckboxChrome::KEY);
}

#[test]
fn every_part_of_the_dark_theme_parses() {
    force_every_part(&::editor::theme::embedded());
}

#[test]
fn every_part_of_the_light_theme_parses() {
    force_every_part(&::editor::theme::light());
}

#[test]
fn every_part_of_the_ios_theme_parses() {
    force_every_part(&::editor::theme::ios());
}

#[test]
fn every_part_of_the_ios_light_theme_parses() {
    force_every_part(&::editor::theme::ios_light());
}

/// The phone pair is the desktop pair's SHAPE — every section and
/// key the desktop theme names, the phone theme names too; a metric
/// added to one must be set on the other.
#[test]
fn the_ios_pair_mirrors_the_desktop_pairs_keys() {
    fn keys(value: &serde_json::Value, at: &str, out: &mut Vec<String>) {
        if let serde_json::Value::Object(map) = value {
            for (key, inner) in map {
                let path = format!("{at}/{key}");
                keys(inner, &path, out);
                out.push(path);
            }
        }
    }
    let shape = |json: &str| {
        let value: serde_json::Value = serde_json::from_str(json).expect("the theme is JSON");
        let mut out = Vec::new();
        keys(&value, "", &mut out);
        out.sort();
        out
    };
    for (desktop, phone) in [
        ("../editor/assets/theme.json", "../editor/assets/theme-ios.json"),
        ("../editor/assets/theme-light.json", "../editor/assets/theme-ios-light.json"),
    ] {
        let desktop_keys = shape(&std::fs::read_to_string(desktop).expect(desktop));
        let phone_keys = shape(&std::fs::read_to_string(phone).expect(phone));
        let missing: Vec<&String> =
            desktop_keys.iter().filter(|key| !phone_keys.contains(key)).collect();
        assert!(missing.is_empty(), "{phone} lacks {missing:?}");
    }
}
