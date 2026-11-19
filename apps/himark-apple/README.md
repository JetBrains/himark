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

`build.sh` builds the staticlib (`himark-plugins`, feature
`metal-shell`) for `aarch64-apple-ios` and `aarch64-apple-ios-sim`,
writes `Generated-iOS.xcconfig`, and (re)generates the project.

Bridges draw through gpui's Metal renderer: each bridge view owns a
`CAMetalLayer`, the engine paints into a scene and the renderer
presents it (`frontend/himark-ffi/src/surface.rs`). No skia is linked.

## Sources

```
Sources/
  Shared/
    HimarkEngine.swift        the engine wrapper — create/render/events/IME
  iOS/                        UIKit views, the SwiftUI shell, text input
```

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
