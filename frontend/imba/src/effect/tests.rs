use super::*;

struct Probe(u32);

impl Effect for Probe {
    type Result = u32;
}

#[derive(Debug, PartialEq)]
enum Root {
    A(u32),
    B(u32),
}

struct RootProbe(#[allow(dead_code)] u32);

impl Effect for RootProbe {
    type Result = Root;
}

#[test]
fn map_composes_lifts_with_the_payload_untouched() {
    let effect = AnyEffect::new(Probe(7))
        .map(|result| result * 2)
        .map(|result| result + 1);

    assert!(effect.is::<Probe>(), "type identity rides every map");
    let (value, lift) = effect.into_payload().split();
    let probe = value.downcast::<Probe>().expect("payload stays concrete");
    assert_eq!(probe.0, 7, "the effect value rode untouched");

    assert_eq!(lift(Box::new(9u32)), Some(9 * 2 + 1));
}

#[test]
fn scopes_lift_to_the_root_and_preserve_tokens_and_order() {
    let mut batch: Batch<Root> = Batch::new();
    let mut fx = batch.effects();
    let first = fx.launch(RootProbe(1));
    let second = fx.scope(Root::A, |fx| fx.launch(Probe(2)));
    fx.cancel(first);

    let third = fx.scope(Root::B, |fx| {
        fx.scope(|n: u32| n + 1, |fx| fx.launch(Probe(3)))
    });
    assert_ne!(first, second);
    assert_ne!(second, third);

    let mut messages = batch.drain();
    assert_eq!(messages.len(), 4, "program order, nothing merged");
    assert!(
        matches!(&messages[0], Message::Launch(token, effect)
            if *token == first && effect.is::<RootProbe>()),
        "the root launch first"
    );
    assert!(
        matches!(&messages[1], Message::Launch(token, effect)
            if *token == second && effect.is::<Probe>()),
        "the scoped launch keeps its payload type"
    );
    assert!(matches!(&messages[2], Message::Cancel(token) if *token == first));
    let Message::Launch(token, effect) = messages.remove(3) else {
        panic!("the nested launch last");
    };
    assert_eq!(token, third);

    let (_, lift) = effect.into_payload().split();
    assert_eq!(lift(Box::new(9u32)), Some(Root::B(10)));
}

#[test]
fn a_dropped_batch_is_no_transaction() {
    let mut batch: Batch<Root> = Batch::new();
    let mut fx = batch.effects();
    let token = fx.launch(RootProbe(1));
    fx.cancel(token);
    assert_eq!(batch.len(), 2);
    drop(batch);
}

#[test]
fn batch_map_preserves_tokens_and_cancels() {
    let mut batch: Batch<u32> = Batch::new();
    let token = batch.launch(Probe(4));
    batch.cancel(token);
    let mapped: Batch<Root> = batch.map(Root::A);
    let messages = mapped.drain();
    assert!(matches!(&messages[0], Message::Launch(kept, effect)
            if *kept == token && effect.is::<Probe>()));
    assert!(matches!(&messages[1], Message::Cancel(kept) if *kept == token));
}

#[test]
fn effect_caller_calls_registered_types_and_answers_none_otherwise() {
    let caller = EffectCaller::new(std::sync::Arc::new(|type_id, payload| {
        (type_id == TypeId::of::<Probe>()).then(|| {
            let probe = payload.downcast::<Probe>().expect("keyed by TypeId");
            Box::pin(async move { Box::new(probe.0 * 3) as Box<dyn Any + Send + Sync> })
                as EffectFuture<Box<dyn Any + Send + Sync>>
        })
    }));

    struct Unregistered;
    impl Effect for Unregistered {
        type Result = u32;
    }

    fn drive<E: Effect>(caller: &EffectCaller, effect: E) -> Option<E::Result>
    where
        E::Result: 'static,
    {
        let caller = caller.clone();
        block_on(Box::pin(async move { caller.call(effect).await }))
    }

    assert_eq!(drive(&caller, Probe(7)), Some(21));
    assert_eq!(drive(&caller, Unregistered), None);
    assert_eq!(drive(&EffectCaller::disconnected(), Probe(7)), None);
}

#[test]
fn a_relaunch_is_one_message_with_both_tokens() {
    let mut batch: Batch<Root> = Batch::new();
    let mut fx = batch.effects();
    let mut slot = None;
    fx.scope(Root::A, |fx| fx.relaunch(&mut slot, Probe(1)));
    let first = slot.expect("the launch token is stored");
    fx.scope(Root::A, |fx| fx.relaunch(&mut slot, Probe(2)));
    let second = slot.expect("the fresh token is stored");
    assert_ne!(first, second);

    let mut messages = batch.drain();
    assert_eq!(messages.len(), 2);
    assert!(
        matches!(&messages[0], Message::Launch(token, _) if *token == first),
        "an empty slot launches plain"
    );
    let Message::Relaunch(previous, token, effect) = messages.remove(1) else {
        panic!("a full slot supersedes as one message");
    };
    assert_eq!(previous, first);
    assert_eq!(token, second);
    assert!(effect.is::<Probe>(), "type identity rides the lift");
    let (_, lift) = effect.into_payload().split();
    assert_eq!(
        lift(Box::new(9u32)),
        Some(Root::A(9)),
        "the scope's wrap composed"
    );
}

#[test]
fn a_filtered_out_relaunch_degrades_to_its_cancel() {
    let mut batch: Batch<Root> = Batch::new();
    let mut fx = batch.effects();
    let mut slot = None;
    fx.scope(Root::A, |fx| fx.relaunch(&mut slot, Probe(1)));
    let previous = slot.expect("the launch token is stored");
    fx.scope_filtered(
        Root::A,
        |effect| !effect.is::<Probe>(),
        |fx| fx.relaunch(&mut slot, Probe(2)),
    );

    let messages = batch.drain();
    assert_eq!(messages.len(), 2, "the launch, then the degraded cancel");
    assert!(matches!(&messages[1], Message::Cancel(token) if *token == previous));
}

#[test]
fn retain_launches_degrades_a_rejected_relaunch_to_its_cancel() {
    let mut batch: Batch<u32> = Batch::new();
    let mut fx = batch.effects();
    let mut slot = None;
    fx.relaunch(&mut slot, Probe(1));
    let previous = slot.expect("the launch token is stored");
    fx.relaunch(&mut slot, Probe(2));

    batch.retain_launches(|_| false);
    let messages = batch.drain();
    assert_eq!(messages.len(), 1, "both launches dropped, the cancel stays");
    assert!(matches!(&messages[0], Message::Cancel(token) if *token == previous));
}
