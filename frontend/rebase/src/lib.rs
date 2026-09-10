use std::collections::VecDeque;

mod driver;
pub use driver::{run, Applied, Local, Offer};

pub trait Action: Clone {
    type State: Clone;

    fn apply(self, state: &mut Self::State) -> Option<Self>;
}

#[derive(Clone, Debug)]
struct Entry<I, A: Action> {
    id: I,
    action: A,
    after: A::State,
}

#[derive(Clone, Debug)]
pub struct Dispatch<I, A: Action> {
    pub id: I,
    pub base: I,
    pub action: A,
    pub before: A::State,
}

#[derive(Clone, Debug)]
pub struct RebaseLog<I, A: Action> {
    committed: A::State,
    version: I,
    speculation: Vec<Entry<I, A>>,

    rebasing: VecDeque<(I, A)>,
}

impl<I: Clone + Eq, A: Action> RebaseLog<I, A> {
    pub fn new(state: A::State, version: I) -> Self {
        Self {
            committed: state,
            version,
            speculation: Vec::new(),
            rebasing: VecDeque::new(),
        }
    }

    pub fn display(&self) -> &A::State {
        match self.speculation.last() {
            Some(entry) => &entry.after,
            None => &self.committed,
        }
    }

    pub fn committed(&self) -> &A::State {
        &self.committed
    }

    pub fn version(&self) -> &I {
        &self.version
    }

    pub fn is_rebasing(&self) -> bool {
        !self.rebasing.is_empty()
    }

    pub fn is_settled(&self) -> bool {
        self.speculation.is_empty() && self.rebasing.is_empty()
    }

    pub fn pending(&self) -> usize {
        self.speculation.len() + self.rebasing.len()
    }

    pub fn local(&mut self, id: I, action: A) -> Option<Dispatch<I, A>> {
        if self.is_rebasing() {
            self.rebasing.push_back((id, action));
            return None;
        }
        self.push(id, action)
    }

    pub fn remote(&mut self, version: I, action: A) {
        let _ = action.apply(&mut self.committed);
        self.version = version;
        for entry in self.speculation.drain(..).rev() {
            self.rebasing.push_front((entry.id, entry.action));
        }
    }

    pub fn ack(&mut self, id: &I) -> bool {
        let Some(head) = self.speculation.first() else {
            return false;
        };
        if head.id != *id {
            return false;
        }
        let head = self.speculation.remove(0);
        self.committed = head.after;
        self.version = head.id;
        true
    }

    pub fn step(&mut self) -> Option<Dispatch<I, A>> {
        while let Some((id, action)) = self.rebasing.pop_front() {
            if let Some(dispatch) = self.push(id, action) {
                return Some(dispatch);
            }
        }
        None
    }

    fn push(&mut self, id: I, action: A) -> Option<Dispatch<I, A>> {
        let base = match self.speculation.last() {
            Some(entry) => entry.id.clone(),
            None => self.version.clone(),
        };
        let before = self.display().clone();
        let mut after = before.clone();
        let action = action.apply(&mut after)?;
        self.speculation.push(Entry {
            id: id.clone(),
            action: action.clone(),
            after,
        });
        Some(Dispatch {
            id,
            base,
            action,
            before,
        })
    }
}

#[cfg(test)]
mod tests;
