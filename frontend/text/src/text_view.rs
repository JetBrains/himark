use rope::{Cursor, SeekMode};

use crate::line_number::LineNumber;
use crate::measure::{count_newlines, TextMeasure, NEWLINES, UTF16};
use crate::text::Text;

pub struct TextView {
    cursor: Cursor<u8, TextMeasure>,
    byte_count: usize,
    line_count: LineNumber,
    cached_from_byte: usize,
    cached_to_byte: usize,
    cached_from_line: usize,
    has_cached_leaf: bool,
}

impl TextView {
    pub(crate) fn new(rope: rope::Rope<u8, TextMeasure>) -> Self {
        let byte_count = rope.len();
        let line_count = LineNumber(rope.metrics().metric_at(NEWLINES) as usize + 1);
        let mut cursor = rope.cursor();
        let (cached_from_byte, cached_to_byte, cached_from_line, has_cached_leaf) =
            match byte_count == 0 {
                true => (0, 0, 0, false),
                false => Self::cache_from_cursor(&mut cursor),
            };

        Self {
            cursor,
            byte_count,
            line_count,
            cached_from_byte,
            cached_to_byte,
            cached_from_line,
            has_cached_leaf,
        }
    }

    pub fn byte_count(&self) -> usize {
        self.byte_count
    }

    pub fn line_count(&self) -> LineNumber {
        self.line_count
    }

    pub fn text(&self) -> Text {
        Text::from_rope(self.cursor.clone().rope())
    }

    pub fn byte_string(&mut self, from: usize, to: usize) -> String {
        assert!(from <= to, "invalid range");
        assert!(to <= self.byte_count, "range end out of bounds");

        let mut bytes = Vec::with_capacity(to - from);
        self.byte_range_into(from, to, &mut bytes);
        String::from_utf8(bytes).unwrap_or_else(|error| {
            panic!("text rope must remain valid UTF-8 in [{from}, {to}): {error:?}")
        })
    }

    pub fn substring(&mut self, range: std::ops::Range<u32>) -> String {
        let start = (range.start as usize).min(self.byte_count);
        let end = (range.end as usize).min(self.byte_count).max(start);
        self.byte_string(start, end)
    }

    pub fn utf16_to_byte(&mut self, utf16: u32) -> u32 {
        if utf16 == 0 || self.byte_count == 0 {
            return 0;
        }
        let rope = self.cursor.clone().rope();
        if utf16 >= rope.metrics().metric_at(UTF16) {
            return self.byte_count.min(u32::MAX as usize) as u32;
        }
        let mut cursor = rope.cursor();
        assert!(
            cursor.seek(UTF16, utf16, SeekMode::After),
            "the UTF-16 offset exists"
        );
        let mut index = cursor.index() as usize;

        if Cursor::position(&cursor).metric_at(UTF16) < utf16 {
            index += 1;
        }

        while index < self.byte_count && !self.is_char_boundary(index) {
            index += 1;
        }
        index.min(u32::MAX as usize) as u32
    }

    pub fn byte_to_utf16(&mut self, byte: u32) -> u32 {
        let byte = (byte as usize).min(self.byte_count);
        if byte == 0 {
            return 0;
        }
        let rope = self.cursor.clone().rope();
        if byte >= self.byte_count {
            return rope.metrics().metric_at(UTF16);
        }
        let mut cursor = rope.cursor();
        assert!(cursor.seek_to_index(byte as u32), "target byte must exist");
        Cursor::position(&cursor).metric_at(UTF16)
    }

    pub fn is_char_boundary(&mut self, byte: usize) -> bool {
        if byte == 0 || byte >= self.byte_count {
            return byte <= self.byte_count;
        }
        let mut probe: Vec<u8> = Vec::with_capacity(1);
        self.byte_range_into(byte, byte + 1, &mut probe);

        probe.first().is_some_and(|byte| byte & 0xC0 != 0x80)
    }

    pub fn byte_range_into(&mut self, from: usize, to: usize, out: &mut impl Extend<u8>) {
        assert!(from <= to, "invalid range");
        assert!(to <= self.byte_count, "range end out of bounds");
        if from == to {
            return;
        }

        self.move_to_byte(from);
        let mut leaf_from_byte = self.cached_from_byte;
        let mut leaf_to_byte = self.cached_to_byte;
        let mut remaining = to - from;
        let mut local_byte_offset = from - leaf_from_byte;

        loop {
            let leaf = self.cursor.leaf();
            let leaf_bytes = leaf_to_byte - leaf_from_byte;
            let take_bytes = remaining.min(leaf_bytes - local_byte_offset);
            out.extend(
                leaf[local_byte_offset..local_byte_offset + take_bytes]
                    .iter()
                    .copied(),
            );
            remaining -= take_bytes;

            if remaining == 0 {
                break;
            }

            assert!(
                self.cursor.advance_leaf(),
                "next leaf must exist while extracting byte range"
            );
            let (next_from_byte, next_to_byte, next_from_line, _) =
                Self::cache_from_cursor(&mut self.cursor);
            self.cached_from_byte = next_from_byte;
            self.cached_to_byte = next_to_byte;
            self.cached_from_line = next_from_line;
            self.has_cached_leaf = true;
            leaf_from_byte = next_from_byte;
            leaf_to_byte = next_to_byte;
            local_byte_offset = 0;
        }
    }

    pub fn line_at(&mut self, offset: usize) -> LineNumber {
        assert!(offset <= self.byte_count, "offset out of bounds");
        if self.byte_count == 0 {
            return LineNumber(0);
        }
        self.move_to_byte(offset.min(self.byte_count - 1));
        let leaf = self.cursor.leaf();
        let local = (offset - self.cached_from_byte).min(leaf.len());
        LineNumber(self.cached_from_line + count_newlines(&leaf[..local]))
    }

    pub fn line_start_offset(&mut self, line: LineNumber) -> usize {
        assert!(line.0 < self.line_count.0, "line out of bounds");
        if line.0 == 0 {
            return 0;
        }

        let mut cursor = self.cursor.clone().rope().cursor();
        assert!(
            cursor.seek(NEWLINES, line.0 as u32, SeekMode::After),
            "the line start exists"
        );

        match (Cursor::position(&cursor).metric_at(NEWLINES) as usize) < line.0 {
            true => self.byte_count,
            false => cursor.index() as usize,
        }
    }

    pub fn line_end_offset(&mut self, line: LineNumber) -> usize {
        let next = LineNumber(line.0 + 1);
        match next.0 < self.line_count.0 {
            true => self.line_start_offset(next),
            false => self.byte_count,
        }
    }

    fn move_to_byte(&mut self, offset: usize) {
        assert!(offset < self.byte_count, "offset out of bounds");
        let should_reuse =
            self.has_cached_leaf && self.cached_from_byte <= offset && offset < self.cached_to_byte;

        if !should_reuse {
            assert!(
                self.cursor.seek_to_index(offset as u32),
                "target byte must exist"
            );
            let (from_byte, to_byte, from_line, has_cached_leaf) =
                Self::cache_from_cursor(&mut self.cursor);
            self.cached_from_byte = from_byte;
            self.cached_to_byte = to_byte;
            self.cached_from_line = from_line;
            self.has_cached_leaf = has_cached_leaf;
        }
    }

    fn cache_from_cursor(cursor: &mut Cursor<u8, TextMeasure>) -> (usize, usize, usize, bool) {
        let leaf = cursor.leaf();
        let index_in_leaf = cursor.index_in_leaf();
        let from_byte = cursor.index() as usize - index_in_leaf;
        let to_byte = from_byte + leaf.len();
        let lines_before = count_newlines(&leaf[..index_in_leaf]);
        let from_line = Cursor::position(&*cursor).metric_at(NEWLINES) as usize;
        let leaf_from_line = from_line.saturating_sub(lines_before);
        (from_byte, to_byte, leaf_from_line, true)
    }
}
