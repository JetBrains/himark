use text::Text;

pub fn byte_count(text: &Text) -> u32 {
    text.byte_count().min(u32::MAX as usize) as u32
}

pub fn previous_char_before(text: &Text, byte_offset: u32) -> Option<(u32, String)> {
    let end = byte_offset.min(byte_count(text)) as usize;
    if end == 0 {
        return None;
    }

    let start_floor = end.saturating_sub(4);
    let mut view = text.view();
    let mut bytes = Vec::with_capacity(4);

    for start in (start_floor..end).rev() {
        bytes.clear();
        view.byte_range_into(start, end, &mut bytes);
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if text.chars().count() == 1 {
            return Some((start.min(u32::MAX as usize) as u32, text.to_owned()));
        }
    }

    None
}

pub fn next_char_after(text: &Text, byte_offset: u32) -> Option<(u32, String)> {
    let start = byte_offset.min(byte_count(text)) as usize;
    if start >= text.byte_count() {
        return None;
    }

    let end_ceiling = (start + 4).min(text.byte_count());
    let mut view = text.view();
    let mut bytes = Vec::with_capacity(4);

    for end in start + 1..=end_ceiling {
        bytes.clear();
        view.byte_range_into(start, end, &mut bytes);
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if text.chars().count() == 1 {
            return Some((end.min(u32::MAX as usize) as u32, text.to_owned()));
        }
    }

    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Punctuation,
}

fn class_of(ch: char) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

pub fn previous_word_start(text: &Text, byte: u32) -> Option<u32> {
    let mut offset = byte;
    let (start, first) = previous_char_before(text, offset)?;
    offset = start;
    let mut class = class_of(first.chars().next()?);

    while class == CharClass::Whitespace {
        match previous_char_before(text, offset) {
            Some((start, ch)) => {
                let ch_class = class_of(ch.chars().next()?);
                if ch_class == CharClass::Whitespace {
                    offset = start;
                } else {
                    class = ch_class;
                    offset = start;
                }
            }
            None => return Some(offset),
        }
    }
    while let Some((start, ch)) = previous_char_before(text, offset) {
        if class_of(ch.chars().next().expect("one char")) != class {
            break;
        }
        offset = start;
    }
    Some(offset)
}

pub fn next_word_end(text: &Text, byte: u32) -> Option<u32> {
    let mut offset = byte;
    let (end, first) = next_char_after(text, offset)?;
    offset = end;
    let mut class = class_of(first.chars().next()?);
    while class == CharClass::Whitespace {
        match next_char_after(text, offset) {
            Some((end, ch)) => {
                let ch_class = class_of(ch.chars().next()?);
                offset = end;
                if ch_class != CharClass::Whitespace {
                    class = ch_class;
                }
            }
            None => return Some(offset),
        }
    }
    while let Some((end, ch)) = next_char_after(text, offset) {
        if class_of(ch.chars().next().expect("one char")) != class {
            break;
        }
        offset = end;
    }
    Some(offset)
}

pub fn word_around(text: &Text, byte: u32) -> Option<std::ops::Range<u32>> {
    let after_is_word = next_char_after(text, byte)
        .and_then(|(_, ch)| ch.chars().next())
        .is_some_and(|ch| class_of(ch) == CharClass::Word);
    let before_is_word = previous_char_before(text, byte)
        .and_then(|(_, ch)| ch.chars().next())
        .is_some_and(|ch| class_of(ch) == CharClass::Word);
    if !after_is_word && !before_is_word {
        return None;
    }

    let mut start = byte;
    while let Some((previous, ch)) = previous_char_before(text, start) {
        if class_of(ch.chars().next().expect("one char")) != CharClass::Word {
            break;
        }
        start = previous;
    }
    let mut end = byte;
    while let Some((next, ch)) = next_char_after(text, end) {
        if class_of(ch.chars().next().expect("one char")) != CharClass::Word {
            break;
        }
        end = next;
    }
    (start < end).then_some(start..end)
}

pub fn hard_line_range(text: &Text, byte: u32) -> std::ops::Range<u32> {
    let count = byte_count(text);
    let byte = byte.min(count);
    let mut view = text.view();
    let line = view.line_at(byte as usize);
    let start = view.line_start_offset(line) as u32;
    let end = view.line_end_offset(line) as u32;
    start..end
}
