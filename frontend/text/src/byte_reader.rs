use rope::Cursor;

use crate::{measure::TextMeasure, Text};

pub struct ByteReader {
    cursor: Cursor<u8, TextMeasure>,
    remaining_bytes: usize,
    index_in_leaf: usize,
}

impl Text {
    pub fn byte_reader(&self) -> ByteReader {
        self.byte_reader_at(0)
    }

    pub fn byte_reader_at(&self, byte_offset: usize) -> ByteReader {
        let byte_count = self.byte_count();
        let byte_offset = byte_offset.min(byte_count);
        let mut cursor = self.rope.cursor();

        if byte_offset < byte_count {
            cursor.seek_to_index(byte_offset as u32);
        }

        ByteReader {
            index_in_leaf: match byte_offset < byte_count {
                true => cursor.index_in_leaf(),
                false => 0,
            },
            cursor,
            remaining_bytes: byte_count - byte_offset,
        }
    }

    pub fn byte_string(&self, byte_offset: usize, byte_len: usize) -> String {
        self.byte_reader_at(byte_offset).take_string(byte_len)
    }
}

impl ByteReader {
    pub fn take_string(&mut self, byte_len: usize) -> String {
        let mut bytes = Vec::with_capacity(byte_len.min(self.remaining_bytes));
        self.take_bytes(&mut bytes, byte_len);
        String::from_utf8(bytes).expect("text rope must remain valid UTF-8")
    }

    pub fn take_bytes(&mut self, out: &mut Vec<u8>, byte_len: usize) {
        let mut remaining = byte_len.min(self.remaining_bytes);
        out.reserve(remaining);
        while remaining > 0 {
            let leaf = self.cursor.leaf();
            let take = remaining.min(leaf.len().saturating_sub(self.index_in_leaf));

            if take > 0 {
                out.extend_from_slice(&leaf[self.index_in_leaf..self.index_in_leaf + take]);
                self.index_in_leaf += take;
                self.remaining_bytes -= take;
                remaining -= take;
            }

            if remaining > 0 || self.index_in_leaf == leaf.len() {
                if self.cursor.advance_leaf() {
                    self.index_in_leaf = 0;
                } else {
                    break;
                }
            }
        }
    }

    pub fn take_bytes_into(&mut self, out: &mut impl Extend<u8>, byte_len: usize) {
        let mut remaining = byte_len.min(self.remaining_bytes);
        if remaining == 0 {
            return;
        }

        while remaining > 0 {
            let leaf = self.cursor.leaf();
            let take = remaining.min(leaf.len().saturating_sub(self.index_in_leaf));

            if take > 0 {
                out.extend(
                    leaf[self.index_in_leaf..self.index_in_leaf + take]
                        .iter()
                        .copied(),
                );
                self.index_in_leaf += take;
                self.remaining_bytes -= take;
                remaining -= take;
            }

            if remaining > 0 || self.index_in_leaf == leaf.len() {
                if self.cursor.advance_leaf() {
                    self.index_in_leaf = 0;
                } else {
                    break;
                }
            }
        }
    }
}
