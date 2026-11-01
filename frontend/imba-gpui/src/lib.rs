// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! imba on gpui: the INPUT bridge and nothing more — gpui
//! keystrokes, pointers and cursor styles onto `imba::event`, both
//! directions nameable. Rendering is `gfx-gpui`'s business; what
//! hangs a workbench off the bridge (an engine, windows, a
//! clipboard) is the shell's.

pub mod events;
