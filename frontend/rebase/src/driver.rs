// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver};

use crate::{Action, Dispatch, RebaseLog};

#[derive(Clone, Debug)]
pub struct Applied<I, A> {
    pub id: I,
    pub action: A,
}

#[derive(Debug)]
pub enum Local<A> {
    Edit(A),

    Took {
        seen_local: u64,
    },

    /// Fires once every local edit enqueued before it is committed —
    /// the moment the shared state is known to hold them all.
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Clone, Debug)]
pub struct Offer<S> {
    pub seen_local: u64,
    pub state: S,
}

pub async fn run<I, A>(
    mut log: RebaseLog<I, A>,
    mut local: UnboundedReceiver<Local<A>>,
    mut remote: Receiver<Applied<I, A>>,
    wire: Sender<Dispatch<I, A>>,
    offers: Sender<Offer<A::State>>,
    mut mint: impl FnMut() -> I,
) where
    I: Clone + Eq,
    A: Action,
{
    let mut seen_local: u64 = 0;

    let mut dirty = false;

    let mut outstanding: Option<u64> = None;

    let mut flushes: Vec<tokio::sync::oneshot::Sender<()>> = Vec::new();
    loop {
        tokio::select! {
            biased;

            said = local.recv() => {
                match said {
                    None => return,
                    Some(Local::Edit(edit)) => {
                        seen_local += 1;
                        if outstanding.take().is_some() {
                            dirty = true;
                        }
                        if let Some(dispatch) = log.local(mint(), edit) {
                            if wire.send(dispatch).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(Local::Took { seen_local: taken }) => {
                        if outstanding == Some(taken) {
                            outstanding = None;
                        }
                    }
                    Some(Local::Flush(done)) => flushes.push(done),
                }
            }

            applied = remote.recv() => {
                let Some(applied) = applied else { return };
                if !log.ack(&applied.id) {
                    log.remote(applied.id, applied.action);
                    dirty = true;
                }
            }

            _ = std::future::ready(()), if log.is_rebasing() => {
                if let Some(dispatch) = log.step() {
                    if wire.send(dispatch).await.is_err() {
                        return;
                    }
                }
            }
        }

        if dirty && !log.is_rebasing() && local.is_empty() {
            let offer = Offer {
                seen_local,
                state: log.display().clone(),
            };
            if offers.send(offer).await.is_err() {
                return;
            }
            dirty = false;
            outstanding = Some(seen_local);
        }

        if !flushes.is_empty() && log.is_settled() {
            for done in flushes.drain(..) {
                let _ = done.send(());
            }
        }
    }
}
