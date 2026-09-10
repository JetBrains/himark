use super::*;

#[derive(Clone)]
struct Stub;

impl crate::View for Stub {
    type Command = ();

    fn perform(
        &mut self,
        _store: &mut crate::store::Store,
        _ui: &crate::UiCtx,
        _command: (),
        _fx: &mut crate::effect::Effects<'_, ()>,
    ) {
    }

    fn layout<'a>(
        &'a self,
        _arena: &'a crate::arena::Arena,
        _store: &'a crate::store::Store,
        _ui: &'a crate::UiCtx,
        _constraints: crate::constraints::Constraints,
    ) -> impl crate::Thunk<'a, ()> + 'a {
        crate::leaf::leaf::<()>(10.0, 30.0)
    }
}

fn list(heights: &[f32]) -> ListView<Stub> {
    ListView::from_measured(heights.iter().map(|height| (Stub, *height)))
}

#[test]
fn extent_of_reads_metric_spans() {
    let rows = list(&[20.0, 30.0, 10.0]);
    assert_eq!(rows.extent_of(0..1), 20.0);
    assert_eq!(rows.extent_of(1..3), 40.0);
    assert_eq!(rows.extent_of(0..3), 60.0);
    assert_eq!(rows.extent_of(2..3), 10.0);
    assert_eq!(list(&[20.0]).extent_of(0..1), 20.0);
}

#[test]
fn probe_unroll_is_monotone() {
    use crate::store::Store;
    use crate::UiCtx;
    let mut rows = list(&[30.0, 30.0, 30.0]);

    rows.splice_animated(1..2, (0..5).map(|_| (Stub, 30.0)));
    let store = Store::new();
    let ui = UiCtx::new();
    let heights = |rows: &ListView<Stub>| -> Vec<f32> {
        rows.items.iter().map(|element| element.height).collect()
    };
    let mut previous = heights(&rows);
    eprintln!("[probe] t=0 {previous:?}");
    for ms in (0..200).step_by(16) {
        let mut batch = crate::effect::Batch::new();
        rows.perform(
            &mut Store::clone(&store),
            &ui,
            ListCommand::Animate(crate::anim::AnimationClock::from_millis(ms as f64)),
            &mut batch.effects(),
        );
        let current = heights(&rows);
        eprintln!("[probe] t={ms} {current:?}");
        assert_eq!(current[0], 30.0, "the row above never moves");
        assert_eq!(
            *current.last().expect("tail"),
            30.0,
            "the row below never moves"
        );
        for (index, (now, before)) in current.iter().zip(&previous).enumerate() {
            assert!(
                now + 0.01 >= *before,
                "row {index} shrank mid-unroll: {before} -> {now}"
            );
        }
        previous = current;
    }
}
