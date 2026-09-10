# himark — Apple apps (macOS + iOS)

One Xcode project, two targets (`himark-macOS`, `himark-iOS`), over himark's
C embedding ABI exported by `crates/himark-api`. Each app owns its window, the Metal
layer, the display link — and skia and the GPU surface. himark is a headless
engine: handed an `SkCanvas` and events, answering
the platform's text-input protocol for input methods.

## Run it

```
brew install xcodegen        # one-time

./build.sh mac               # macOS: build + launch (fast — host skia only)
./build.sh ios               # iOS: build the target (first run builds iOS skia — slow)
./build.sh gen               # both: generate the project for use in Xcode
open himark.xcodeproj        # pick a scheme (himark-macOS / himark-iOS) and ⌘R
```

`build.sh mac` only touches the host (macOS) skia, so it's quick; `ios` and
`gen` trigger `cargo build --target aarch64-apple-ios`, whose **first** run
builds/downloads iOS skia and is slow. On its first run, the script also
checks out the pinned Skia headers under `target/` for the Objective-C++ Metal
bridge. For each platform it builds
`himark-api`, resolves *that platform's* skia from the cargo build-script
JSON, writes the matching xcconfig, and (re)generates the two-target project.
The macOS path targets the machine's native architecture, even when the active
Rust toolchain runs under Rosetta; the script installs that Rust target on
first use.

## What's shared vs. per-platform

```
Sources/
  Shared/
    HimarkEngine.swift        the engine wrapper — create/render/events/IME,
                              platform-neutral (CAMetalLayer + Metal only)
    SkiaMetalBridge/          host-side skia↔Metal (compiles for both)
  macOS/  HimarkView.swift    NSView + NSTextInputClient + CVDisplayLink; main.swift
  iOS/    HimarkView.swift    UIView + UIKeyInput + CADisplayLink; AppDelegate.swift
```

The engine, the ABI, and the skia bridge are identical across platforms —
`CAMetalLayer`/`MTLDevice` and himark's C ABI are all cross-platform. Only
the view adapter differs: AppKit's `NSTextInputClient` and `CVDisplayLink`
vs. UIKit's text input and `CADisplayLink`.

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

## Status of the iOS target

The macOS app is the complete reference (full IME via `NSTextInputClient`,
mouse, scroll, live-resize). The iOS view renders, takes touches (→ click),
a pan (→ scroll), and basic typing via `UIKeyInput`. Full `UITextInput`/IME
on iOS is a follow-up — the *engine* already supports marked text; only the
iOS adapter is basic for now.

`crates/ios` (the old Rust iOS staticlib shell) is now redundant — the iOS
target consumes `himark-api` like macOS does.
