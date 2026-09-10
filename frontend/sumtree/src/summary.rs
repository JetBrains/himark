pub trait Summary: Clone {
    fn empty() -> Self;
    fn add(&mut self, other: &Self);
}

pub trait Item: Clone {
    type Summary: Summary;
    fn summary(&self) -> Self::Summary;
}

pub trait Dimension<S>: Copy + Default + PartialOrd {
    fn from_summary(summary: &S) -> Self;
    fn add(&mut self, other: Self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    Left,

    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seek<D> {
    pub index: usize,
    pub start: D,
}
