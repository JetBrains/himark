use rope::Cursor;

use crate::measure::OperationMeasure;
use crate::op::{byte_len, slice_bytes, Op};
use crate::Operation;

#[derive(Clone)]
pub(crate) struct Reader {
    cursor: Option<Cursor<Op, OperationMeasure>>,
    offset_bytes: u32,
}

impl Reader {
    pub(crate) fn new(operation: &Operation) -> Self {
        Self {
            cursor: (!operation.is_empty()).then(|| operation.rope.cursor()),
            offset_bytes: 0,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.cursor.is_none()
    }

    pub(crate) fn current(&self) -> &Op {
        self.cursor.as_ref().expect("reader is exhausted").element()
    }

    pub(crate) fn remaining_old_len(&self) -> u32 {
        self.current().old_len() - self.offset_bytes
    }

    pub(crate) fn remaining_new_len(&self) -> u32 {
        self.current().new_len() - self.offset_bytes
    }

    pub(crate) fn consume_old(&mut self, len: u32) {
        match self.current() {
            Op::Retain(total) => self.advance_within(*total, len),
            Op::Delete(text) => self.advance_within(byte_len(text), len),
            Op::Insert(_) => panic!("cannot consume old length from insert"),
        }
    }

    pub(crate) fn consume_new(&mut self, len: u32) {
        match self.current() {
            Op::Retain(total) => self.advance_within(*total, len),
            Op::Insert(text) => self.advance_within(byte_len(text), len),
            Op::Delete(_) => panic!("cannot consume new length from delete"),
        }
    }

    pub(crate) fn take_old_piece(&mut self, len: u32) -> Op {
        let piece = match self.current() {
            Op::Retain(_) => Op::Retain(len),
            Op::Delete(text) => Op::Delete(slice_bytes(text, self.offset_bytes, len)),
            Op::Insert(_) => panic!("cannot take old piece from insert"),
        };
        self.consume_old(len);
        piece
    }

    pub(crate) fn take_new_piece(&mut self, len: u32) -> Op {
        let piece = match self.current() {
            Op::Retain(_) => Op::Retain(len),
            Op::Insert(text) => Op::Insert(slice_bytes(text, self.offset_bytes, len)),
            Op::Delete(_) => panic!("cannot take new piece from delete"),
        };
        self.consume_new(len);
        piece
    }

    pub(crate) fn remaining_insert_text(&self) -> Option<String> {
        match self.current() {
            Op::Insert(text) => Some(slice_bytes(
                text,
                self.offset_bytes,
                self.remaining_new_len(),
            )),
            _ => None,
        }
    }

    fn advance_within(&mut self, total: u32, len: u32) {
        assert!(
            len <= total - self.offset_bytes,
            "consuming beyond current op"
        );
        self.offset_bytes += len;
        if self.offset_bytes == total {
            self.advance_to_next_op();
        }
    }

    fn advance_to_next_op(&mut self) {
        self.offset_bytes = 0;
        let next = match self.cursor.as_mut() {
            Some(cursor) => cursor.advance(),
            None => false,
        };
        if !next {
            self.cursor = None;
        }
    }
}
