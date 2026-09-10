use super::*;

#[test]
fn the_embedded_theme_parses_and_covers_the_styles() {
    let theme = Theme::embedded();

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
            theme.attributes(id),
            &TextAttributes::default(),
            "theme entry missing for {id:?}"
        );
    }
    assert!(theme.base().color.is_some(), "base text has a color");
}

#[test]
fn the_light_theme_parses_covers_and_matches_dark_geometry() {
    let dark = Theme::embedded();
    let light = Theme::light();
    assert_eq!(dark.name(), "dark");
    assert_eq!(light.name(), "light");
    for id in StyleId::all_slots() {
        if matches!(id, StyleId::Composing | StyleId::Indent(_)) {
            continue;
        }
        assert_ne!(
            light.attributes(id),
            &TextAttributes::default(),
            "light theme entry missing for {id:?}"
        );
    }

    assert_eq!(
        light.base().font_size.unwrap(),
        dark.base().font_size.unwrap(),
        "light body type matches dark"
    );
    assert!(
        light.attributes(StyleId::Header(1)).font_size.unwrap()
            == dark.attributes(StyleId::Header(1)).font_size.unwrap()
    );

    let light_combo = &light.ui().combo;
    let dark_combo = &dark.ui().combo;
    assert_eq!(light_combo.pad, dark_combo.pad, "combo geometry matches");
    assert_eq!(
        light_combo.menu_row_height, dark_combo.menu_row_height,
        "menu geometry matches"
    );
    assert_eq!(
        light_combo.value_color.0,
        Color::from_argb(0xff, 0x20, 0x24, 0x2b),
        "closed values use dark ink"
    );
    assert_eq!(
        light_combo.menu_fill.0,
        Color::from_argb(0xff, 0xf8, 0xf9, 0xfb),
        "expanded menus use a light surface"
    );
    assert_eq!(
        light_combo.menu_border.0,
        Color::from_argb(0xff, 0xb8, 0xbe, 0xc9),
        "closed cells and expanded menus have a visible edge"
    );
    assert_eq!(
        light_combo.menu_text.0,
        Color::from_argb(0xff, 0x20, 0x24, 0x2b),
        "expanded menu labels use dark ink"
    );
}

#[test]
fn merging_layers_inline_over_block() {
    let theme = Theme::embedded();
    let merged = theme.resolve([StyleId::CodeBlock, StyleId::Keyword]);
    assert!(merged.font_families.is_some(), "code face survives");
    assert_eq!(
        merged.color,
        theme.attributes(StyleId::Keyword).color,
        "token color wins over the block's"
    );
}

#[test]
fn style_slots_match_their_array_positions() {
    for (position, id) in StyleId::all_slots().into_iter().enumerate() {
        assert_eq!(id.slot(), position, "{id:?} out of slot order");
    }
}
