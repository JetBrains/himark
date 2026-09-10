use ::editor::{bridge, EditIdentity, EditLog};
use operation::Operation;
use text::Text;

#[derive(Clone, Debug)]
pub struct SyncState {
    pub text: Text,
    pub log: EditLog,
}

impl SyncState {
    pub fn new(text: Text, log: EditLog) -> Self {
        Self { text, log }
    }

    pub fn slice_from(&self, base: &EditLog) -> Option<Operation> {
        bridge(base, &self.log)
    }
}

pub type Resolve = std::sync::Arc<dyn Fn(&Text) -> Option<Operation> + Send + Sync>;

#[derive(Clone)]
pub enum SyncEdit {
    Ours {
        base: EditLog,
        op: Operation,

        identity: EditIdentity,
    },

    Theirs { resolve: Resolve },
}

impl std::fmt::Debug for SyncEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ours { base, identity, .. } => {
                write!(f, "Ours({base:?}, {identity:?})")
            }
            Self::Theirs { .. } => write!(f, "Theirs(unresolved)"),
        }
    }
}

impl SyncEdit {
    pub fn captured(base: EditLog, op: Operation, identity: EditIdentity) -> Self {
        Self::Ours {
            base,
            op,
            identity,
        }
    }

    pub fn operation(&self) -> Option<&Operation> {
        match self {
            Self::Ours { op, .. } => Some(op),
            Self::Theirs { .. } => None,
        }
    }

    pub fn identity(&self) -> Option<EditIdentity> {
        match self {
            Self::Ours { identity, .. } => Some(*identity),
            Self::Theirs { .. } => None,
        }
    }
}

impl rebase::Action for SyncEdit {
    type State = SyncState;

    fn apply(self, state: &mut SyncState) -> Option<Self> {
        let base = state.log.clone();
        let (op, identity) = match self {
            Self::Theirs { resolve } => (resolve(&state.text)?, EditIdentity::mint()),

            Self::Ours {
                base: made_on,
                op,
                identity,
            } => match made_on.head() == state.log.head() {
                true => (op, identity),

                false => {
                    let arrow = bridge(&made_on, &state.log)
                        .filter(|arrow| arrow.old_len() == op.old_len())?;
                    (op.transform(&arrow), EditIdentity::mint())
                }
            },
        };
        if op.is_empty() || op.iter().all(|step| matches!(step, operation::Op::Retain(_))) {
            return None;
        }
        let old_len = state.text.byte_count().min(u32::MAX as usize) as u32;
        state.text = state.text.edit(&op);
        state.log.record_as(identity, &op, old_len);
        Some(Self::Ours {
            base,
            op,
            identity,
        })
    }
}

#[cfg(test)]
mod tests;
