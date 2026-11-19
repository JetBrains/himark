# himark — the iOS app

> The macOS app is the Rust desktop shell (`apps/desktop`); this
> project is iOS only.

One Xcode project, one target (`himark-iOS`), over himark's C embedding
ABI (`frontend/himark-ffi`, linked as the `himark-plugins` staticlib).
The app owns its windows, the Metal layers and the display links;
himark is a headless engine that is handed events and a surface to
paint, and answers the platform's text-input protocol.

## Run it

```
brew install xcodegen        # one-time

./build.sh                   # build device + simulator libraries, generate the project
open himark.xcodeproj        # pick the himark-iOS scheme and ⌘R
```

`build.sh` builds the staticlib for `aarch64-apple-ios` and
`aarch64-apple-ios-sim`, resolves each platform's skia from the cargo
build-script JSON, writes `Generated-iOS.xcconfig`, and (re)generates
the project. The first run builds/downloads iOS skia and is slow.

## Sources

```
Sources/
  Shared/
    HimarkEngine.swift        the engine wrapper — create/render/events/IME
    SkiaMetalBridge/          host-side skia↔Metal
  iOS/                        UIKit views, the SwiftUI shell, text input
```

## The one build constraint: one shared skia per platform

The bridge creates the `SkCanvas`; `himark-api` paints into it. The
`SkCanvas*` crossing `himark_draw` must belong to one skia — so each target
links *the exact skia `himark-api` built for that platform*. `build.sh`
resolves it from the per-target cargo output (`target/release` for macOS,
`target/aarch64-apple-ios/release` for iOS) and points the xcconfig there.
`himark-api` builds skia with the `metal` feature so that libskia includes
the Metal backend the bridge needs. The skia frameworks differ by platform
(macOS `ApplicationServices` vs. iOS `CoreGraphics`/`CoreText`) and are read
from each build's own link declarations, so the xcconfigs are correct per
platform without hardcoding.

> **First iOS build is slow:** `cargo build --target aarch64-apple-ios`
> builds/downloads iOS skia the first time.

## The iOS shell's chrome

The phone (the iPhone artboards of the typographic redesign) draws
WORDS and HAIRLINES: a 44pt bar only where it says something (the
session and New over the chat; the way back over a pushed screen),
four section words along the bottom — Chat · Changes · History ·
Files — and the engine's content between. Every piece of
content is a Rust view in the phone's own theme pair
(`frontend/editor/assets/theme-ios.json`, `theme-ios-light.json`:
3x metrics, tap-target rows, the phone's insets), picked by the engine
once the shell declares `himark_set_form(HIMARK_FORM_PHONE)`; a pad
keeps the desktop pair. The native words read the same files
(`Sources/iOS/ShellTheme.swift`: the `ink` and the Swift-only `shell`
section) and set in the bundled Bricolage Grotesque, so nothing in
`RootView.swift` names a size or a color. The system appearance drives
both halves (`AppHost.follow(appearance:)`).
