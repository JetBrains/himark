# Air UI reference assets

The gallery and shared controls follow the Air UI source at
`JetBrains/jcp-frontend`, revision `bca58ddb3f277265c479da7379d8a80572822985`,
under `libs/ui-kits/air-ui`. The reference was located through the migration
note in `JetBrains/jcp-air` at `4cacf447d0db9a205da4bfe0d09fe389657f734c`.
The Air Cloud consumer currently pins the published kit to `0.1.26`; these
styles were read from the identified source revision, not that npm release.

The Inter and JetBrains Mono Latin variable fonts are the kit's WOFF2 assets,
decompressed to TrueType with `wawoff2@2.0.1`, with no outline changes. The
original copyright and OFL licenses are included. The font weights and
optical size axes are retained, including the light theme's +20 weight
adjustment. System font fallback supplies characters outside these subsets.

Mappings:

- `label`: Text/default, Inter 13/16, 480 dark / 500 light, 0.004em tracking.
- `caption`: Text/medium, 12/16, 500 / 520, 0.005em tracking.
- `heading`: Heading/h2-semibold, 19/24, 600 / 620.
- `caps`: Heading/h5-semibold, 10/14, 700 / 720, 0.1em tracking.
- `key_hint`: Text/small, 10/14, 500 / 520, 0.006em tracking.
- `code`: Text/code, JetBrains Mono 13/22, 400 / 420.
- Buttons: Button/primary, Button/secondary and ButtonGhost/off, default size.
- Checkbox: 16px control, 14px box, 1px border, 2px radius; exact checked
  and indeterminate icon paths from the kit. Focus rings sit outside the box.
- List/tree rows: List.Item's 24px height, 8px left / 4px right padding;
  trees use Air Cloud ArtifactTree's 24px nesting and the kit's 16px chevron.
- Surfaces: 8px Card radius, 12px content padding, Card fill and general
  border tokens; bordered and outline are compositions of those primitives.

Colors and typography are resolved through `ui.air` in the editor's
`theme.json` and `theme-light.json`, alongside the existing application theme.
The `air-ui` crate consumes these typed theme fields. Inter and JetBrains Mono
are registered as `Air Inter` and `Air JetBrains Mono` in the shared font
collection, so they do not replace existing editor font families. It caches weight and optical
size variants for both the editor and UI text. Focus outlines use Imba's
existing window overlay host and do not change layout or hit bounds.
