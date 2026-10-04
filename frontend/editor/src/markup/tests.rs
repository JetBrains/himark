// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Clone)]
struct NullInlay;

impl View for NullInlay {
    type Command = std::convert::Infallible;

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: Self::Command,
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn display<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
    ) -> impl imba::layout::Layout<'a, Self::Command> + imba::layout::LayoutValue + 'a {
        imba::layout::laid(move |_arena: &'a Arena, _constraints: Constraints| imba::leaf::leaf(10.0, 10.0))
    }
}

#[test]
fn nested_syntax_inlays_reach_the_host_flag() {
    let mut derived = Markup::builder();
    derived.push_inlay(
        0..4,
        Inlay::new(
            InlayMode::Instead(crate::markup::InsteadKind::FullLine),
            NullInlay,
        ),
    );
    let entry = Syntax::new("markdown", None, derived.finish());

    let mut host = Markup::new();
    host.add_syntax(0..10, entry.clone());
    assert!(host.has_inlays(), "the install bubbles the flag");

    let mut host = Markup::new();
    let root = {
        let empty = Syntax::new("markdown", None, Markup::new());
        host.add_syntax(0..10, empty)
    };
    assert!(!host.has_inlays(), "no inlays anywhere yet");
    assert!(host.set_syntax(root, entry));
    assert!(host.has_inlays(), "the landing bubbles the flag");
}
