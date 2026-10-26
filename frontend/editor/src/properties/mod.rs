// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Generative properties over the editor. Each file holds one
//! layer; the oracle throughout is a FRESH build of the same thing
//! (`EditorView::complete`, `DocumentLayout::build`), never a
//! stored golden — goldens rot with the theme, a fresh build
//! cannot.

macro_rules! fx {
    () => {
        &mut imba::effect::Batch::new().effects()
    };
}

mod editors;
mod gen;
mod layout;
mod render;
mod width_tree;
