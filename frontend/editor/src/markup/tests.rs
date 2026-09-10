use super::*;

#[derive(Clone)]
struct NullInlay;

impl View for NullInlay {
    type Command = ();

    fn perform(
        &mut self,
        _store: &mut Store,
        _ui: &UiCtx,
        _command: (),
        _fx: &mut imba::effect::Effects<'_, Self::Command>,
    ) {
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a Arena,
        _store: &'a Store,
        _ui: &'a UiCtx,
        _constraints: Constraints,
    ) -> impl imba::Thunk<'a, Self::Command> + 'a {
        imba::leaf::leaf(10.0, 10.0)
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
