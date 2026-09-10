use operation::{Op, Operation};

use crate::{LineNumber, Text};

fn large_text() -> String {
    let mut out = String::new();
    for idx in 0..4000 {
        out.push_str("line ");
        out.push_str(&idx.to_string());
        out.push_str(" :: abcdefghijklmnopqrstuvwxyz :: ");
        out.push_str(&(idx % 13).to_string());
        out.push('\n');
    }
    out.push_str("tail");
    out
}

fn apply_operation_to_model(source: &str, operation: &Operation) -> String {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    let mut out = Vec::new();

    for op in operation.iter() {
        match op {
            Op::Retain(len) => {
                let next = cursor + len as usize;
                out.extend_from_slice(&bytes[cursor..next]);
                cursor = next;
            }
            Op::Insert(text) => {
                out.extend_from_slice(text.as_bytes());
            }
            Op::Delete(text) => {
                cursor += text.len();
            }
        }
    }

    String::from_utf8(out).expect("test operation must stay on UTF-8 boundaries")
}

#[test]
fn text_counts_bytes_and_lines() {
    let text = Text::from_string_exact("ab\ncd\n");
    assert_eq!(text.byte_count(), 6);
    assert_eq!(text.line_count(), LineNumber(3));
}

#[test]
fn empty_text_has_single_line() {
    let text = Text::from_string_exact("");
    assert_eq!(text.byte_count(), 0);
    assert_eq!(text.line_count(), LineNumber(1));
}

#[test]
fn byte_string_extracts_within_single_leaf() {
    let text = Text::from_string_exact("abcdef");
    let mut view = text.view();
    assert_eq!(view.byte_string(1, 4), "bcd");
}

#[test]
fn byte_string_extracts_across_many_leaves() {
    let source = large_text();
    let text = Text::from_string_exact(&source);
    let mut view = text.view();

    let from = source.char_indices().nth(1200).expect("offset").0;
    let to = source.char_indices().nth(4200).expect("offset").0;
    assert_eq!(view.byte_string(from, to), source[from..to]);
}

#[test]
fn line_at_handles_boundaries() {
    let text = Text::from_string_exact("ab\ncd\nef");
    let mut view = text.view();
    assert_eq!(view.line_at(0), LineNumber(0));
    assert_eq!(view.line_at(2), LineNumber(0));
    assert_eq!(view.line_at(3), LineNumber(1));
    assert_eq!(view.line_at(5), LineNumber(1));
    assert_eq!(view.line_at(6), LineNumber(2));
    assert_eq!(view.line_at(8), LineNumber(2));
}

#[test]
fn line_start_offset_handles_boundaries() {
    let text = Text::from_string_exact("ab\ncd\nef");
    let mut view = text.view();
    assert_eq!(view.line_start_offset(LineNumber(0)), 0);
    assert_eq!(view.line_start_offset(LineNumber(1)), 3);
    assert_eq!(view.line_start_offset(LineNumber(2)), 6);
}

#[test]
fn line_end_offset_is_one_past_the_newline_and_text_end_for_the_last() {
    let text = Text::from_string_exact("ab\ncd\nef");
    let mut view = text.view();
    assert_eq!(view.line_end_offset(LineNumber(0)), 3);
    assert_eq!(view.line_end_offset(LineNumber(1)), 6);
    assert_eq!(
        view.line_end_offset(LineNumber(2)),
        8,
        "the last line ends the text"
    );

    let text = Text::from_string_exact("ab\n");
    let mut view = text.view();
    assert_eq!(view.line_end_offset(LineNumber(0)), 3);
    assert_eq!(view.line_end_offset(LineNumber(1)), 3);
}

#[test]
fn text_round_trips_from_view() {
    let source = "aåβ😀z\nnext";
    let text = Text::from_string_exact(source);
    let rebuilt = text.view().text();
    assert_eq!(rebuilt.to_string(), source);
}

#[test]
fn edit_insert_at_beginning() {
    let text = Text::from_string_exact("world");
    let edited = text.edit(&Operation::from_ops([
        Op::Insert("hello ".into()),
        Op::Retain(5),
    ]));
    assert_eq!(edited.to_string(), "hello world");
}

#[test]
fn edit_insert_at_end() {
    let text = Text::from_string_exact("hello");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(5),
        Op::Insert(" world".into()),
    ]));
    assert_eq!(edited.to_string(), "hello world");
}

#[test]
fn edit_delete_middle() {
    let text = Text::from_string_exact("hello cruel world");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(6),
        Op::Delete("cruel ".into()),
        Op::Retain(5),
    ]));
    assert_eq!(edited.to_string(), "hello world");
}

#[test]
fn edit_replace_middle() {
    let text = Text::from_string_exact("abcXYZdef");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(3),
        Op::Delete("XYZ".into()),
        Op::Insert("123".into()),
        Op::Retain(3),
    ]));
    assert_eq!(edited.to_string(), "abc123def");
}

#[test]
fn edit_handles_unicode_insert() {
    let text = Text::from_string_exact("ab");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(1),
        Op::Insert("😀β".into()),
        Op::Retain(1),
    ]));
    assert_eq!(edited.to_string(), "a😀βb");
}

#[test]
fn edit_handles_unicode_delete() {
    let text = Text::from_string_exact("a😀βb");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(1),
        Op::Delete("😀β".into()),
        Op::Retain(1),
    ]));
    assert_eq!(edited.to_string(), "ab");
}

#[test]
fn edit_preserves_line_metrics() {
    let text = Text::from_string_exact("a\nb\nc");
    let edited = text.edit(&Operation::from_ops([
        Op::Retain(2),
        Op::Insert("x\ny\n".into()),
        Op::Retain(3),
    ]));
    assert_eq!(edited.to_string(), "a\nx\ny\nb\nc");
    assert_eq!(edited.line_count(), LineNumber(5));
}

#[test]
fn edit_empty_operation_keeps_text() {
    let text = Text::from_string_exact("abc\n😀\nxyz");
    let edited = text.edit(&Operation::new());
    assert_eq!(edited, text);
}

#[test]
fn edit_deletes_entire_text() {
    let source = "abc😀\ndef";
    let text = Text::from_string_exact(source);
    let edited = text.edit(&Operation::from_ops([Op::Delete(source.into())]));
    assert_eq!(edited.to_string(), "");
    assert_eq!(edited.byte_count(), 0);
    assert_eq!(edited.line_count(), LineNumber(1));
}

#[test]
fn edit_replaces_entire_text() {
    let source = "abc\ndef";
    let replacement = "😀\nHELLO\nβ";
    let text = Text::from_string_exact(source);
    let edited = text.edit(&Operation::from_ops([
        Op::Delete(source.into()),
        Op::Insert(replacement.into()),
    ]));
    assert_eq!(edited.to_string(), replacement);
    assert_eq!(edited.byte_count(), replacement.len());
    assert_eq!(
        edited.line_count(),
        LineNumber(replacement.chars().filter(|&ch| ch == '\n').count() + 1),
    );
}

#[test]
fn edit_sequential_operations_match_model() {
    let operations = [
        Operation::from_ops([Op::Retain(5), Op::Insert(" brave".into()), Op::Retain(7)]),
        Operation::from_ops([
            Op::Delete("hello".into()),
            Op::Insert("hi".into()),
            Op::Retain(13),
        ]),
        Operation::from_ops([
            Op::Retain(2),
            Op::Delete(" brave".into()),
            Op::Insert("😀β".into()),
            Op::Retain(7),
        ]),
        Operation::from_ops([Op::Retain(15), Op::Insert("\nEND".into())]),
    ];
    let mut expected = String::from("hello world!");
    let mut text = Text::from_string_exact(&expected);

    for operation in operations {
        expected = apply_operation_to_model(&expected, &operation);
        text = text.edit(&operation);
        assert_eq!(text.to_string(), expected);
    }
}

#[test]
fn edit_large_insert_at_front_matches_model() {
    let source = large_text();
    let prefix = "HEADER\n".repeat(200);
    let operation = Operation::from_ops([
        Op::Insert(prefix.clone()),
        Op::Retain(source.chars().count() as u32),
    ]);
    let edited = Text::from_string_exact(&source).edit(&operation);
    let expected = apply_operation_to_model(&source, &operation);
    assert_eq!(edited.to_string(), expected);
}

#[test]
fn edit_large_insert_at_end_matches_model() {
    let source = large_text();
    let suffix = "\nTRAILER😀".repeat(150);
    let operation = Operation::from_ops([
        Op::Retain(source.chars().count() as u32),
        Op::Insert(suffix.clone()),
    ]);
    let edited = Text::from_string_exact(&source).edit(&operation);
    let expected = apply_operation_to_model(&source, &operation);
    assert_eq!(edited.to_string(), expected);
}

#[test]
fn edit_large_delete_span_matches_model() {
    let source = large_text();
    let chars: Vec<char> = source.chars().collect();
    let delete = chars[2000..7000].iter().collect::<String>();
    let operation = Operation::from_ops([
        Op::Retain(2000),
        Op::Delete(delete.clone()),
        Op::Retain((chars.len() - 7000) as u32),
    ]);
    let edited = Text::from_string_exact(&source).edit(&operation);
    let expected = apply_operation_to_model(&source, &operation);
    assert_eq!(edited.to_string(), expected);
}

#[test]
fn edit_large_mixed_operation_matches_model() {
    let source = large_text();
    let chars: Vec<char> = source.chars().collect();
    let delete_one = chars[1500..1533].iter().collect::<String>();
    let delete_two = chars[9000..9055].iter().collect::<String>();
    let operation = Operation::from_ops([
        Op::Retain(1500),
        Op::Delete(delete_one.clone()),
        Op::Insert("<<<A\nB\nC>>>".into()),
        Op::Retain(7467),
        Op::Delete(delete_two.clone()),
        Op::Insert("ΩΩΩ".into()),
        Op::Retain((chars.len() - 9055) as u32),
    ]);
    let edited = Text::from_string_exact(&source).edit(&operation);
    let expected = apply_operation_to_model(&source, &operation);
    assert_eq!(edited.to_string(), expected);
    assert_eq!(edited.byte_count(), expected.len());
    assert_eq!(
        edited.line_count(),
        LineNumber(expected.chars().filter(|&ch| ch == '\n').count() + 1),
    );
}

#[test]
fn repeated_local_reads_use_same_view_state() {
    let source = large_text();
    let text = Text::from_string_exact(&source);
    let mut view = text.view();

    for (at, ch) in source.char_indices().take(1100).skip(1000) {
        let read = view.byte_string(at, at + ch.len_utf8());
        assert_eq!(read.chars().next(), Some(ch));
    }
}

#[test]
fn repeated_line_queries_use_same_view_state() {
    let source = large_text();
    let text = Text::from_string_exact(&source);
    let mut view = text.view();

    for line in 100..160 {
        let start = view.line_start_offset(LineNumber(line));
        let actual_line = view.line_at(start);
        assert_eq!(actual_line, LineNumber(line));
    }
}

#[test]
fn large_text_counts_match_model() {
    let source = large_text();
    let text = Text::from_string_exact(&source);
    assert_eq!(text.byte_count(), source.len());
    assert_eq!(
        text.line_count(),
        LineNumber(source.chars().filter(|&ch| ch == '\n').count() + 1),
    );
}

#[test]
fn large_text_randomish_slices_match_model() {
    let source = large_text();
    let text = Text::from_string_exact(&source);
    let mut view = text.view();
    let boundary = |chars: usize| {
        source
            .char_indices()
            .nth(chars)
            .map_or(source.len(), |(at, _)| at)
    };

    for (from, to) in [(0, 10), (17, 300), (511, 1700), (2048, 4096), (8000, 12000)] {
        let (from, to) = (boundary(from), boundary(to));
        assert_eq!(view.byte_string(from, to), source[from..to]);
    }
}

#[test]
fn large_edit_in_middle_matches_model() {
    let source = large_text();
    let mut chars: Vec<char> = source.chars().collect();
    let text = Text::from_string_exact(&source);
    let insert = "<<<\nEDIT\n>>>";
    let delete = chars[5000..5010].iter().collect::<String>();

    let edited = text.edit(&Operation::from_ops([
        Op::Retain(5000),
        Op::Delete(delete.clone()),
        Op::Insert(insert.into()),
        Op::Retain((chars.len() - 5010) as u32),
    ]));

    chars.splice(5000..5010, insert.chars());
    let expected: String = chars.iter().collect();
    assert_eq!(edited.to_string(), expected);
}

#[test]
fn line_lookups_match_a_naive_scan() {
    let mut source = String::new();
    for i in 0..64 {
        match i % 5 {
            0 => source.push_str(&"wörd ".repeat(i * 7 + 1)),
            1 => source.push_str("short"),
            2 => {}
            3 => source.push_str(&"x".repeat(i * 31)),
            _ => source.push_str("наши строки"),
        }
        source.push('\n');
    }
    source.push_str("tail without newline");

    let text = crate::text::Text::from_string_exact(&source);
    let mut view = text.view();
    let naive = |offset: usize| -> (usize, usize) {
        let prefix = &source.as_bytes()[..offset];
        let row = prefix.iter().filter(|byte| **byte == b'\n').count();
        let line_start = prefix
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        (row, line_start)
    };

    let offsets = source.char_indices().map(|(at, _)| at);
    for offset in offsets.chain([source.len()]) {
        let (row, line_start) = naive(offset);
        assert_eq!(view.line_at(offset), LineNumber(row), "row at {offset}");
        assert_eq!(
            view.line_start_offset(LineNumber(row)),
            line_start,
            "line start at {offset}"
        );
    }

    let empty = crate::text::Text::from_string_exact("");
    assert_eq!(empty.view().line_at(0), LineNumber(0));
}

#[test]
fn utf16_bridges_match_the_reference_walk() {
    let sample = "ab é中😀\ncd😀é\n".repeat(200);
    let text = crate::text::Text::from_string_exact(&sample);
    let mut view = text.view();

    let reference_byte = |utf16: u32| -> u32 {
        let mut units = 0u32;
        for (byte, ch) in sample.char_indices() {
            if units >= utf16 {
                return byte as u32;
            }
            units += ch.len_utf16() as u32;
        }
        sample.len() as u32
    };
    let reference_utf16 = |byte: u32| -> u32 {
        sample[..byte as usize]
            .chars()
            .map(|ch| ch.len_utf16() as u32)
            .sum()
    };

    let total_units: u32 = sample.chars().map(|ch| ch.len_utf16() as u32).sum();

    for utf16 in 0..=total_units + 2 {
        assert_eq!(
            view.utf16_to_byte(utf16),
            reference_byte(utf16),
            "utf16_to_byte({utf16})"
        );
    }

    for (byte, _) in sample.char_indices() {
        assert_eq!(
            view.byte_to_utf16(byte as u32),
            reference_utf16(byte as u32),
            "byte_to_utf16({byte})"
        );
    }
    assert_eq!(view.byte_to_utf16(sample.len() as u32), total_units);
    assert_eq!(
        view.byte_to_utf16(u32::MAX.min(sample.len() as u32 + 9)),
        total_units
    );

    let empty = crate::text::Text::from_string_exact("");
    assert_eq!(empty.view().utf16_to_byte(0), 0);
    assert_eq!(empty.view().utf16_to_byte(5), 0);
    assert_eq!(empty.view().byte_to_utf16(0), 0);
}
