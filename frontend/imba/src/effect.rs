use std::{
    any::{Any, TypeId},
    future::Future,
    pin::Pin,
};

pub trait Effect: Send + Sync + 'static {
    type Result;
}

#[allow(async_fn_in_trait)]
pub trait EffectHandler<E: Effect>: Send + Sync + 'static {
    async fn handle(&self, effect: E) -> E::Result;
}

pub type EffectFuture<R> = Pin<Box<dyn Future<Output = R> + 'static>>;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CancellationToken(std::num::NonZeroU64);

impl CancellationToken {
    fn fresh() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let raw = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(std::num::NonZeroU64::new(raw).expect("the counter starts at 1"))
    }
}

pub struct AnyEffect<R> {
    type_id: TypeId,

    name: &'static str,

    payload: Box<dyn Any + Send + Sync>,

    lift: Box<dyn FnOnce(Box<dyn Any + Send + Sync>) -> Option<R> + Send>,
}

impl<R: 'static> AnyEffect<R> {
    pub fn new<E: Effect<Result = R>>(effect: E) -> Self {
        Self {
            type_id: TypeId::of::<E>(),
            name: std::any::type_name::<E>(),
            payload: Box::new(effect),
            lift: Box::new(|outcome| {
                Some(
                    *outcome
                        .downcast::<E::Result>()
                        .expect("outcome typed by the dispatch registry"),
                )
            }),
        }
    }

    pub fn notification<E: Effect<Result = ()>>(effect: E) -> Self {
        Self {
            type_id: TypeId::of::<E>(),
            name: std::any::type_name::<E>(),
            payload: Box::new(effect),
            lift: Box::new(|_| None),
        }
    }

    pub fn is<E: Effect>(&self) -> bool {
        self.type_id == TypeId::of::<E>()
    }

    pub fn get<E: Effect>(&self) -> Option<&E> {
        self.payload.downcast_ref::<E>()
    }

    pub fn map<P: 'static>(self, wrap: impl FnOnce(R) -> P + Send + 'static) -> AnyEffect<P> {
        let lift = self.lift;
        AnyEffect {
            type_id: self.type_id,
            name: self.name,
            payload: self.payload,
            lift: Box::new(move |outcome| lift(outcome).map(wrap)),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn into_payload(self) -> EffectPayload<R> {
        EffectPayload {
            type_id: self.type_id,
            name: self.name,
            payload: self.payload,
            lift: self.lift,
        }
    }
}

pub struct EffectPayload<R> {
    type_id: TypeId,
    name: &'static str,
    payload: Box<dyn Any + Send + Sync>,
    lift: Box<dyn FnOnce(Box<dyn Any + Send + Sync>) -> Option<R> + Send>,
}

impl<R> EffectPayload<R> {
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn split(
        self,
    ) -> (
        Box<dyn Any + Send + Sync>,
        Box<dyn FnOnce(Box<dyn Any + Send + Sync>) -> Option<R> + Send>,
    ) {
        (self.payload, self.lift)
    }
}

pub enum Message<R> {
    Launch(CancellationToken, AnyEffect<R>),
    Cancel(CancellationToken),

    Relaunch(CancellationToken, CancellationToken, AnyEffect<R>),
}

pub struct Batch<R> {
    messages: Vec<Message<R>>,
}

impl<R: 'static> Batch<R> {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn effects(&mut self) -> Effects<'_, R> {
        Effects { sink: self }
    }

    pub fn launch<E: Effect<Result = R>>(&mut self, effect: E) -> CancellationToken {
        self.push(AnyEffect::new(effect))
    }

    pub fn push(&mut self, effect: AnyEffect<R>) -> CancellationToken {
        let token = CancellationToken::fresh();
        self.messages.push(Message::Launch(token, effect));
        token
    }

    pub fn cancel(&mut self, token: CancellationToken) {
        self.messages.push(Message::Cancel(token));
    }

    pub fn extend(&mut self, other: Batch<R>) {
        self.messages.extend(other.messages);
    }

    pub fn map<P: 'static>(self, wrap: impl Fn(R) -> P + Send + Clone + 'static) -> Batch<P> {
        Batch {
            messages: self
                .messages
                .into_iter()
                .map(|message| match message {
                    Message::Launch(token, effect) => {
                        let wrap = wrap.clone();
                        Message::Launch(token, effect.map(move |result| wrap(result)))
                    }
                    Message::Cancel(token) => Message::Cancel(token),
                    Message::Relaunch(previous, token, effect) => {
                        let wrap = wrap.clone();
                        Message::Relaunch(previous, token, effect.map(move |result| wrap(result)))
                    }
                })
                .collect(),
        }
    }

    pub fn surviving_launches(self) -> Vec<AnyEffect<R>> {
        let messages = self.drain();
        let cancelled: std::collections::HashSet<_> = messages
            .iter()
            .filter_map(|message| match message {
                Message::Cancel(token) => Some(*token),

                Message::Relaunch(previous, _, _) => Some(*previous),
                Message::Launch(..) => None,
            })
            .collect();
        messages
            .into_iter()
            .filter_map(|message| match message {
                Message::Launch(token, effect) | Message::Relaunch(_, token, effect)
                    if !cancelled.contains(&token) =>
                {
                    Some(effect)
                }
                _ => None,
            })
            .collect()
    }

    pub fn retain_launches(&mut self, mut keep: impl FnMut(&AnyEffect<R>) -> bool) {
        self.messages = std::mem::take(&mut self.messages)
            .into_iter()
            .filter_map(|message| match message {
                Message::Launch(_, ref effect) => keep(effect).then_some(message),
                Message::Cancel(_) => Some(message),
                Message::Relaunch(previous, _, ref effect) => match keep(effect) {
                    true => Some(message),
                    false => Some(Message::Cancel(previous)),
                },
            })
            .collect();
    }

    pub fn drain(self) -> Vec<Message<R>> {
        self.messages
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }
}

impl<R: 'static> Default for Batch<R> {
    fn default() -> Self {
        Self::new()
    }
}

trait Sink<R> {
    fn forward(&mut self, message: Message<R>);
}

impl<R: 'static> Sink<R> for Batch<R> {
    fn forward(&mut self, message: Message<R>) {
        self.messages.push(message);
    }
}

struct Scoped<'p, C, R, W: Fn(C) -> R> {
    parent: &'p mut dyn Sink<R>,
    wrap: W,
    _wraps: std::marker::PhantomData<fn(C) -> R>,
}

impl<C: 'static, R: 'static, W: Fn(C) -> R + Send + Clone + 'static> Sink<C>
    for Scoped<'_, C, R, W>
{
    fn forward(&mut self, message: Message<C>) {
        match message {
            Message::Launch(token, effect) => {
                let wrap = self.wrap.clone();
                self.parent.forward(Message::Launch(
                    token,
                    effect.map(move |result| wrap(result)),
                ));
            }
            Message::Cancel(token) => self.parent.forward(Message::Cancel(token)),
            Message::Relaunch(previous, token, effect) => {
                let wrap = self.wrap.clone();
                self.parent.forward(Message::Relaunch(
                    previous,
                    token,
                    effect.map(move |result| wrap(result)),
                ));
            }
        }
    }
}

struct Filtered<'p, C, K: Fn(&AnyEffect<C>) -> bool> {
    parent: &'p mut dyn Sink<C>,
    keep: K,
}

impl<C: 'static, K: Fn(&AnyEffect<C>) -> bool> Sink<C> for Filtered<'_, C, K> {
    fn forward(&mut self, message: Message<C>) {
        match message {
            Message::Launch(token, effect) => {
                if (self.keep)(&effect) {
                    self.parent.forward(Message::Launch(token, effect));
                }
            }
            Message::Cancel(token) => self.parent.forward(Message::Cancel(token)),

            Message::Relaunch(previous, token, effect) => match (self.keep)(&effect) {
                true => self
                    .parent
                    .forward(Message::Relaunch(previous, token, effect)),
                false => self.parent.forward(Message::Cancel(previous)),
            },
        }
    }
}

pub struct Effects<'a, R> {
    sink: &'a mut dyn Sink<R>,
}

impl<'a, R: 'static> Effects<'a, R> {
    pub fn launch<E: Effect<Result = R>>(&mut self, effect: E) -> CancellationToken {
        self.push(AnyEffect::new(effect))
    }

    pub fn push(&mut self, effect: AnyEffect<R>) -> CancellationToken {
        let token = CancellationToken::fresh();
        self.sink.forward(Message::Launch(token, effect));
        token
    }

    pub fn notify<E: Effect<Result = ()>>(&mut self, effect: E) -> CancellationToken {
        self.push(AnyEffect::notification(effect))
    }

    pub fn cancel(&mut self, token: CancellationToken) {
        self.sink.forward(Message::Cancel(token));
    }

    pub fn relaunch<E: Effect<Result = R>>(
        &mut self,
        slot: &mut Option<CancellationToken>,
        effect: E,
    ) {
        self.relaunch_erased(slot, AnyEffect::new(effect));
    }

    pub fn relaunch_erased(&mut self, slot: &mut Option<CancellationToken>, effect: AnyEffect<R>) {
        let token = CancellationToken::fresh();
        match slot.take() {
            Some(previous) => self
                .sink
                .forward(Message::Relaunch(previous, token, effect)),
            None => self.sink.forward(Message::Launch(token, effect)),
        }
        *slot = Some(token);
    }

    pub fn scope<C: 'static, T>(
        &mut self,
        wrap: impl Fn(C) -> R + Send + Clone + 'static,
        f: impl FnOnce(&mut Effects<'_, C>) -> T,
    ) -> T {
        let mut scoped = Scoped {
            parent: &mut *self.sink,
            wrap,
            _wraps: std::marker::PhantomData,
        };
        f(&mut Effects { sink: &mut scoped })
    }

    pub fn scope_filtered<C: 'static, T>(
        &mut self,
        wrap: impl Fn(C) -> R + Send + Clone + 'static,
        keep: impl Fn(&AnyEffect<C>) -> bool,
        f: impl FnOnce(&mut Effects<'_, C>) -> T,
    ) -> T {
        let mut scoped = Scoped {
            parent: &mut *self.sink,
            wrap,
            _wraps: std::marker::PhantomData,
        };
        let mut filtered = Filtered {
            parent: &mut scoped,
            keep,
        };
        f(&mut Effects {
            sink: &mut filtered,
        })
    }

    pub fn merge(&mut self, batch: Batch<R>) {
        for message in batch.messages {
            self.sink.forward(message);
        }
    }

    pub fn forward(&mut self, message: Message<R>) {
        self.sink.forward(message);
    }
}

pub struct EffectCaller {
    #[allow(clippy::type_complexity)]
    dispatch: std::sync::Arc<
        dyn Fn(
                TypeId,
                Box<dyn Any + Send + Sync>,
            ) -> Option<EffectFuture<Box<dyn Any + Send + Sync>>>
            + Send
            + Sync,
    >,
}

impl Clone for EffectCaller {
    fn clone(&self) -> Self {
        Self {
            dispatch: std::sync::Arc::clone(&self.dispatch),
        }
    }
}

impl EffectCaller {
    #[allow(clippy::type_complexity)]
    pub fn new(
        dispatch: std::sync::Arc<
            dyn Fn(
                    TypeId,
                    Box<dyn Any + Send + Sync>,
                ) -> Option<EffectFuture<Box<dyn Any + Send + Sync>>>
                + Send
                + Sync,
        >,
    ) -> Self {
        Self { dispatch }
    }

    pub fn disconnected() -> Self {
        Self::new(std::sync::Arc::new(|_, _| None))
    }

    pub async fn call<E: Effect>(&self, effect: E) -> Option<E::Result>
    where
        E::Result: 'static,
    {
        let future = (self.dispatch)(TypeId::of::<E>(), Box::new(effect))?;
        let outcome = future.await;
        Some(
            *outcome
                .downcast::<E::Result>()
                .expect("outcome typed by the dispatch registry"),
        )
    }
}

pub fn block_on<T>(mut future: EffectFuture<T>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(std::sync::Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests;
