// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use rope::cursor::CursorIter;

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
