// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::theme::ThemeStyles as _;

#[test]
fn the_embedded_theme_parses_and_covers_the_styles() {
    let theme = crate::theme::embedded();

    let all = [
        StyleId::Emphasis,
        StyleId::Strong,
        StyleId::InlineCode,
        StyleId::Link,
        StyleId::Strikethrough,
        StyleId::Composing,
        StyleId::Header(1),
        StyleId::Header(2),
        StyleId::Header(3),
        StyleId::Header(4),
        StyleId::ListItem,
        StyleId::CodeBlock,
        StyleId::Quote,
        StyleId::HorizontalLine,
        StyleId::Keyword,
        StyleId::String,
        StyleId::Comment,
        StyleId::Number,
        StyleId::Type,
        StyleId::Function,
        StyleId::Variable,
        StyleId::Constant,
        StyleId::Operator,
        StyleId::Punctuation,
        StyleId::Attribute,
        StyleId::Embedded,
    ];
    for id in all {
        assert_ne!(
            theme.styles().attributes(id),
            &TextAttributes::default(),
            "theme entry missing for {id:?}"
        );
    }
    assert!(
        theme.styles().base().color.is_some(),
        "base text has a color"
    );
}

#[test]
fn the_light_theme_parses_covers_and_matches_dark_geometry() {
    let dark = crate::theme::embedded();
    let light = crate::theme::light();
    assert_eq!(dark.name(), "dark");
    assert_eq!(light.name(), "light");
    for id in StyleId::all_slots() {
        // Reference is deliberately attribute-less: the link look is
        // the editor's hovered-link state, not the span's.
        if matches!(
            id,
            StyleId::Composing | StyleId::Indent(_) | StyleId::Reference
        ) {
            continue;
        }
        assert_ne!(
            light.styles().attributes(id),
            &TextAttributes::default(),
            "light theme entry missing for {id:?}"
        );
    }

    assert_eq!(
        light.styles().base().font_size.unwrap(),
        dark.styles().base().font_size.unwrap(),
        "light body type matches dark"
    );
    assert!(
        light
            .styles()
            .attributes(StyleId::Header(1))
            .font_size
            .unwrap()
            == dark
                .styles()
                .attributes(StyleId::Header(1))
                .font_size
                .unwrap()
    );
}
// The combo chrome's light/dark geometry and ink assertions moved to
// hikit::chrome::tests with the ComboChrome struct.

#[test]
fn merging_layers_inline_over_block() {
    let theme = crate::theme::embedded();
    let merged = theme.resolve([StyleId::CodeBlock, StyleId::Keyword]);
    assert!(merged.font_families.is_some(), "code face survives");
    assert_eq!(
        merged.color,
        theme.styles().attributes(StyleId::Keyword).color,
        "token color wins over the block's"
    );
}

#[test]
fn style_slots_match_their_array_positions() {
    for (position, id) in StyleId::all_slots().into_iter().enumerate() {
        assert_eq!(id.slot(), position, "{id:?} out of slot order");
    }
}
