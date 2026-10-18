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
