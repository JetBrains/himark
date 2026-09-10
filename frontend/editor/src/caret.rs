use std::ops::Range;
use std::sync::Arc;

use operation::{Bias, Operation};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DragOrigin {
    Char(u32),
    Word(std::ops::Range<u32>),
    Line(std::ops::Range<u32>),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Caret {
    start: u32,
    end: u32,
    offset: u32,
    goal_x: Option<f32>,
}

impl Caret {
    pub fn at(offset: u32) -> Self {
        Self {
            start: offset,
            end: offset,
            offset,
            goal_x: None,
        }
    }

    pub fn selecting(anchor: u32, offset: u32) -> Self {
        Self {
            start: anchor.min(offset),
            end: anchor.max(offset),
            offset,
            goal_x: None,
        }
    }

    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn goal_x(&self) -> Option<f32> {
        self.goal_x
    }

    pub fn with_goal(mut self, x: f32) -> Self {
        self.goal_x = Some(x);
        self
    }

    pub fn selection(&self) -> Range<u32> {
        self.start..self.end
    }

    pub fn has_selection(&self) -> bool {
        self.start < self.end
    }

    pub fn anchor(&self) -> u32 {
        if self.offset == self.start {
            self.end
        } else {
            self.start
        }
    }

    pub fn moved_to(&self, offset: u32, select: bool) -> Self {
        match select {
            true => Self::selecting(self.anchor(), offset),
            false => Self::at(offset),
        }
    }

    fn clamped(&self, window: &Range<u32>) -> Self {
        let start = self.start.clamp(window.start, window.end);
        let end = self.end.clamp(window.start, window.end);
        Self {
            start,
            end,
            offset: self.offset.clamp(start, end),
            goal_x: self.goal_x,
        }
    }

    fn transformed(&self, operation: &Operation) -> Self {
        let start = operation.transform_offset(self.start, Bias::Left);
        let end = operation.transform_offset(self.end, Bias::Left).max(start);
        Self {
            start,
            end,
            offset: operation
                .transform_offset(self.offset, Bias::Left)
                .clamp(start, end),

            goal_x: self.goal_x,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct MultiCaret {
    carets: Arc<Vec<Caret>>,
    primary: usize,
}

impl MultiCaret {
    pub fn single(offset: u32) -> Self {
        Self {
            carets: Arc::new(vec![Caret::at(offset)]),
            primary: 0,
        }
    }

    pub fn one(caret: Caret) -> Self {
        Self {
            carets: Arc::new(vec![caret]),
            primary: 0,
        }
    }

    pub fn normalized(carets: Vec<Caret>, primary: usize) -> Self {
        if carets.is_empty() {
            return Self::single(0);
        }

        let mut indexed: Vec<(usize, Caret)> = carets.into_iter().enumerate().collect();
        indexed.sort_by_key(|(_, caret)| (caret.start, caret.end, caret.offset));

        let mut merged: Vec<Caret> = Vec::with_capacity(indexed.len());
        let mut primary_out = 0usize;
        for (input_index, caret) in indexed {
            let absorbed = match merged.last_mut() {
                Some(last) if caret.start <= last.end => {
                    last.end = last.end.max(caret.end);
                    last.offset = caret.offset.clamp(last.start, last.end);
                    last.goal_x = caret.goal_x;
                    true
                }
                _ => {
                    merged.push(caret);
                    false
                }
            };
            if input_index == primary {
                primary_out = merged.len() - 1;
            }
            let _ = absorbed;
        }
        Self {
            carets: Arc::new(merged),
            primary: primary_out,
        }
    }

    pub fn carets(&self) -> &[Caret] {
        &self.carets
    }

    pub fn len(&self) -> usize {
        self.carets.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn primary(&self) -> Caret {
        self.carets[self.primary.min(self.carets.len() - 1)]
    }

    pub fn primary_index(&self) -> usize {
        self.primary.min(self.carets.len() - 1)
    }

    pub fn has_selection(&self) -> bool {
        self.carets.iter().any(Caret::has_selection)
    }

    pub fn map(&self, mut f: impl FnMut(&Caret) -> Caret) -> Self {
        Self::normalized(
            self.carets.iter().map(|caret| f(caret)).collect(),
            self.primary,
        )
    }

    pub fn with_added(&self, caret: Caret) -> Self {
        let mut carets = self.carets.as_ref().clone();
        carets.push(caret);
        Self::normalized(carets, self.carets.len())
    }

    pub fn with_removed_at(&self, offset: u32) -> Option<Self> {
        if self.carets.len() <= 1 {
            return None;
        }
        let index = self.carets.iter().position(|caret| {
            caret.offset == offset || (caret.start <= offset && offset < caret.end)
        })?;
        let mut carets = self.carets.as_ref().clone();
        carets.remove(index);
        let primary = match self.primary {
            primary if primary > index => primary - 1,
            primary => primary.min(carets.len() - 1),
        };
        Self::normalized(carets, primary).into()
    }

    pub fn collapsed_to_primary(&self) -> Self {
        Self::single(self.primary().offset)
    }

    pub fn collapsed_selections(&self) -> Self {
        self.map(|caret| Caret::at(caret.offset))
    }

    pub fn clamped(&self, window: &Range<u32>) -> Self {
        if self
            .carets
            .iter()
            .all(|caret| window.start <= caret.start && caret.end <= window.end)
        {
            return self.clone();
        }
        self.map(|caret| caret.clamped(window))
    }

    pub fn transformed(&self, operation: &Operation) -> Self {
        self.map(|caret| caret.transformed(operation))
    }
}

#[cfg(test)]
mod tests;
