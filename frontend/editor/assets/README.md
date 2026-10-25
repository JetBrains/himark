# Native Test Font

`NotoSans-Regular.ttf` is from Google Noto Fonts and is distributed under the
SIL Open Font License 1.1. It remains the hermetic font for native test
harnesses; the web target uses its separately served JetBrains Mono WOFF2.

Source:
https://github.com/notofonts/noto-fonts/blob/main/hinted/ttf/NotoSans/NotoSans-Regular.ttf

# Bundled Code Font

The `JetBrainsMono-*.ttf` cuts (Regular, Bold, Italic, BoldItalic,
v2.304) are the application code font, registered with both text
stacks at startup so the family resolves identically everywhere
without a system install. SIL Open Font License 1.1.

Source:
https://github.com/JetBrains/JetBrainsMono/releases/tag/v2.304

# Bundled Chrome Font

`BricolageGrotesque-Variable.ttf` (axes opsz 12–96, wdth 75–100,
wght 200–800) is the application CHROME font — every label, title,
tab, query and transcript line outside the editor's code shapes with
it (docs/Design.md, the typographic redesign). Registered with both
text stacks at startup like the code font. SIL Open Font License 1.1.

Source:
https://github.com/google/fonts/tree/main/ofl/bricolagegrotesque

# The iOS pair

`theme-ios.json` / `theme-ios-light.json` are the phone's themes:
the desktop pair's inks and sections, with the PHONE's metrics —
physical pixels for a 3x iPhone screen (a 17pt title is `51.0`, a
44pt row `132.0`), rows that are tap targets, the phone artboards'
insets (no voice gutter in the chat, a 20pt text edge, a 36pt
line-number margin). The engine picks the pair when the shell
declares the phone form (`himark_set_form`, `HIMARK_FORM_PHONE`);
the SwiftUI chrome reads the same files' `ink` and its own `shell`
section, which no Rust part reads, so the native words share the
engine's ramp. A key added to the desktop pair must be added here
too — himark-tests/tests/theme_parts.rs holds the pair to that.
