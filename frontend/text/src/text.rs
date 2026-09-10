use std::fmt;

use operation::{Op, Operation};
use rope::{from_leaves_with_branching, Cursor, Rope};

use crate::line_number::LineNumber;
use crate::measure::{split_utf8_bytes, TextMeasure, BRANCH_FACTOR, LEAF_CAPACITY, NEWLINES};
use crate::TextView;

#[derive(Clone, Debug)]
pub struct Text {
    pub(crate) rope: Rope<u8, TextMeasure>,
}

impl Text {
    pub fn from_string_exact(text: impl AsRef<str>) -> Self {
        let bytes = text.as_ref().as_bytes();
        Self {
            rope: from_leaves_with_branching(
                split_utf8_bytes(bytes, LEAF_CAPACITY),
                LEAF_CAPACITY,
                BRANCH_FACTOR,
            ),
        }
    }

    pub fn from_string(text: impl AsRef<str>) -> Self {
        let normalized = text.as_ref().replace("\r\n", "\n").replace('\r', "\n");
        Self::from_string_exact(normalized)
    }

    pub fn page_at(&self, offset: usize, end: usize, max: usize) -> Vec<u8> {
        let end = end.min(self.byte_count());
        if offset >= end {
            return Vec::new();
        }
        let mut page = Vec::new();
        self.byte_reader_at(offset)
            .take_bytes(&mut page, (end - offset).min(max));
        page
    }

    pub fn byte_count(&self) -> usize {
        self.rope.len()
    }

    pub fn line_count(&self) -> LineNumber {
        LineNumber(self.rope.metrics().metric_at(NEWLINES) as usize + 1)
    }

    pub fn view(&self) -> TextView {
        TextView::new(self.rope.clone())
    }

    pub fn edit(&self, operation: &Operation) -> Self {
        let mut cursor = self.rope.cursor();

        let mut offset = 0usize;
        let mut byte_count = self.byte_count();

        for op in operation.iter() {
            match op {
                Op::Retain(len) => {
                    offset = offset.saturating_add(len as usize).min(byte_count);
                }
                Op::Delete(text) => {
                    let delete_bytes = text.len();
                    if delete_bytes != 0 && byte_count != 0 {
                        cursor =
                            Self::delete_text_at_byte(cursor, offset, byte_count, delete_bytes);
                        byte_count = byte_count.saturating_sub(delete_bytes);
                    }
                }
                Op::Insert(text) => {
                    cursor = Self::insert_text_at_byte(cursor, offset, byte_count, &text);
                    offset = offset.saturating_add(text.len());
                    byte_count = byte_count.saturating_add(text.len());
                }
            }
        }

        Self::from_rope(cursor.rope())
    }

    pub(crate) fn from_rope(rope: Rope<u8, TextMeasure>) -> Self {
        Self { rope }
    }

    fn as_string(&self) -> String {
        let bytes: Vec<u8> = self.rope.iter().collect();
        String::from_utf8(bytes).expect("text rope must remain valid UTF-8")
    }

    fn delete_text_at_byte(
        mut cursor: Cursor<u8, TextMeasure>,
        byte_offset: usize,
        byte_count: usize,
        byte_len: usize,
    ) -> Cursor<u8, TextMeasure> {
        if byte_len == 0 || byte_offset >= byte_count {
            return cursor;
        }

        Self::seek_to_byte(&mut cursor, byte_offset, byte_count);
        cursor.delete(byte_len);
        cursor
    }

    fn insert_text_at_byte(
        mut cursor: Cursor<u8, TextMeasure>,
        byte_offset: usize,
        byte_count: usize,
        text: &str,
    ) -> Cursor<u8, TextMeasure> {
        if text.is_empty() {
            return cursor;
        }

        let inserted = Text::from_string_exact(text).rope;
        if byte_count == 0 {
            return inserted.cursor();
        }

        if byte_offset == byte_count {
            let mut inserted_cursor = inserted.cursor();
            inserted_cursor.insert(cursor.rope());
            return inserted_cursor;
        }

        Self::seek_to_byte(&mut cursor, byte_offset, byte_count);
        cursor.insert(inserted);
        cursor
    }

    fn seek_to_byte(cursor: &mut Cursor<u8, TextMeasure>, byte_offset: usize, byte_count: usize) {
        assert!(byte_offset < byte_count, "byte offset out of bounds");
        assert!(
            cursor.seek_to_index(byte_offset as u32),
            "target byte must exist"
        );
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::from_string_exact("")
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.byte_count() == other.byte_count() && self.rope.iter().eq(other.rope.iter())
    }
}

impl Eq for Text {}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_string())
    }
}
