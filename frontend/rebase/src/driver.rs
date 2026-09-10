use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver};

use crate::{Action, Dispatch, RebaseLog};

#[derive(Clone, Debug)]
pub struct Applied<I, A> {
    pub id: I,
    pub action: A,
}

#[derive(Clone, Debug)]
pub enum Local<A> {
    Edit(A),

    Took { seen_local: u64 },
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
    }
}
