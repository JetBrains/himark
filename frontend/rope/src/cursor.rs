use crate::metrics::MetricId;
use crate::rope::Rope;
use crate::zipper::Zipper;
use crate::Measure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeekMode {
    Before,
    After,
}

pub struct Cursor<T: Clone, M: Measure<T>> {
    zipper: Zipper<T, M>,
}

pub struct CursorIter<T: Clone, M: Measure<T>> {
    cursor: Cursor<T, M>,
    exhausted: bool,
}

impl<T: Clone, M: Measure<T>> Clone for Cursor<T, M> {
    fn clone(&self) -> Self {
        Self {
            zipper: self.zipper.clone(),
        }
    }
}

impl<T: Clone, M: Measure<T>> Cursor<T, M> {
    pub(crate) fn from_zipper(mut zipper: Zipper<T, M>) -> Option<Self> {
        while zipper.move_down() {}

        if zipper.is_leaf() {
            Some(Self { zipper })
        } else {
            None
        }
    }

    pub fn seek(&mut self, metric_id: MetricId, metric_value: u32, mode: SeekMode) -> bool {
        self.seek_metric(metric_id, metric_value, mode)
    }

    pub fn seek_to_index(&mut self, index: u32) -> bool {
        self.seek_index(index)
    }

    fn seek_metric(&mut self, metric_id: MetricId, metric_value: u32, mode: SeekMode) -> bool {
        loop {
            self.move_to_first_sibling();
            if self.metric_is_before_sibling_set(metric_id, metric_value) {
                if !self.zipper.move_up() {
                    break self.zipper.metrics().length != 0;
                }
                continue;
            }

            while self.metric_is_before(metric_id, metric_value, mode) {
                if !self.zipper.move_right() {
                    break;
                }
            }

            if !self.metric_is_before(metric_id, metric_value, mode) {
                break self.descend_metric(metric_id, metric_value, mode);
            }

            if !self.zipper.move_up() {
                break self.descend_last_leaf();
            }
        }
    }

    fn seek_index(&mut self, index: u32) -> bool {
        loop {
            self.move_to_first_sibling();
            if index < self.zipper.location().length {
                if !self.zipper.move_up() {
                    break self.zipper.metrics().length != 0;
                }
                continue;
            }

            while self.index_is_before_or_at(index) {
                if !self.zipper.move_right() {
                    break;
                }
            }

            if !self.index_is_before_or_at(index) {
                break self.descend_index(index);
            }

            if !self.zipper.move_up() {
                break self.descend_last_leaf();
            }
        }
    }

    fn descend_metric(&mut self, metric_id: MetricId, metric_value: u32, mode: SeekMode) -> bool {
        loop {
            if self.zipper.is_leaf() {
                while self.metric_is_before(metric_id, metric_value, mode) {
                    if !self.zipper.move_next() {
                        return self.zipper.metrics().length != 0;
                    }
                }

                return self.zipper.metrics().length != 0;
            }

            if !self.zipper.move_down() {
                return self.zipper.metrics().length != 0;
            }

            self.move_to_first_sibling();
            while self.metric_is_before(metric_id, metric_value, mode) {
                if !self.zipper.move_right() {
                    if self.zipper.move_next() {
                        break;
                    }

                    return self.descend_last_leaf();
                }
            }
        }
    }

    fn descend_index(&mut self, index: u32) -> bool {
        while !self.zipper.is_leaf() {
            if !self.zipper.move_down() {
                return self.zipper.metrics().length != 0;
            }

            while self.index_is_before_or_at(index) {
                if !self.zipper.move_right() {
                    return self.descend_last_leaf();
                }
            }
        }

        self.zipper.metrics().length != 0
    }

    fn move_to_first_sibling(&mut self) {
        while self.zipper.move_left() {}
    }

    fn descend_last_leaf(&mut self) -> bool {
        while self.zipper.move_down_last() {}
        self.zipper.is_leaf() && self.zipper.metrics().length != 0
    }

    fn metric_is_before(&self, metric_id: MetricId, metric_value: u32, mode: SeekMode) -> bool {
        let location = M::metric_at(&self.zipper.location().metrics, metric_id);
        let metrics = M::metric_at(&self.zipper.metrics().metrics, metric_id);
        let end = location.saturating_add(metrics);

        match mode {
            SeekMode::Before => end < metric_value,
            SeekMode::After => {
                if self.zipper.is_leaf() {
                    location < metric_value && end <= metric_value
                } else {
                    location < metric_value && end < metric_value
                }
            }
        }
    }

    fn metric_is_before_sibling_set(&self, metric_id: MetricId, metric_value: u32) -> bool {
        metric_value < M::metric_at(&self.zipper.location().metrics, metric_id)
    }

    fn index_is_before_or_at(&self, index: u32) -> bool {
        let location = self.zipper.location().length;
        let length = self.zipper.metrics().length;
        let end = location.saturating_add(length);

        end <= index
    }

    pub fn element(&self) -> &T {
        self.zipper
            .element()
            .expect("cursor is not positioned on an element")
    }

    pub fn peek_element(&self) -> Option<&T> {
        self.zipper.element()
    }

    pub fn leaf(&self) -> &[T] {
        self.zipper
            .leaf()
            .expect("cursor is not positioned on a leaf")
    }

    pub fn index_in_leaf(&self) -> usize {
        self.zipper
            .index_in_leaf()
            .expect("cursor is not positioned on a leaf")
    }

    pub fn element_metrics(&self) -> M::Metrics {
        self.zipper.metrics().metrics
    }

    pub fn position(&self) -> M::Metrics {
        self.zipper.location().metrics
    }

    pub fn index(&self) -> u32 {
        self.zipper.location().length
    }

    pub fn size(&self) -> u32 {
        self.zipper.metrics().length
    }

    pub fn advance(&mut self) -> bool {
        self.zipper.move_next()
    }

    pub fn advance_leaf(&mut self) -> bool {
        self.zipper.move_next_leaf()
    }

    pub fn retreat_leaf(&mut self) -> bool {
        self.zipper.move_prev_leaf()
    }

    pub fn delete<N>(&mut self, length: N)
    where
        N: TryInto<u32>,
        N::Error: std::fmt::Debug,
    {
        let length = length.try_into().expect("delete length exceeds u32");
        self.zipper.delete(length);
    }

    pub fn insert(&mut self, rope: Rope<T, M>) {
        self.zipper.insert(rope);
    }

    pub fn split(self) -> (Rope<T, M>, Rope<T, M>) {
        self.zipper.split()
    }

    pub fn rope(self) -> Rope<T, M> {
        self.zipper.rope()
    }

    pub fn iter(self) -> CursorIter<T, M> {
        CursorIter {
            cursor: self,
            exhausted: false,
        }
    }
}

impl<T: Clone, M: Measure<T>> Iterator for CursorIter<T, M> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted {
            None
        } else if let Some(element) = self.cursor.zipper.element().cloned() {
            self.exhausted = !self.cursor.advance();
            Some(element)
        } else {
            self.exhausted = true;
            None
        }
    }
}
