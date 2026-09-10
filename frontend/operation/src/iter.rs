use rope::CursorIter;

use crate::measure::OperationMeasure;
use crate::op::Op;

pub struct Iter {
    pub(crate) iter: CursorIter<Op, OperationMeasure>,
}

impl Iterator for Iter {
    type Item = Op;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
