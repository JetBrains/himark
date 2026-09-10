use crate::{Measure, MetricId, Metrics, Rope, SeekMode};

struct TextMeasure;

const LARGE_TEXT_FIXTURE: &str = include_str!("../testdata/large_fixture.txt");
const GROWTH_TEXT_FIXTURE: &str = include_str!("../testdata/growth_fixture.txt");

impl Measure<char> for TextMeasure {
    type Metrics = Metrics<2>;

    fn zero() -> Self::Metrics {
        Metrics::zero()
    }

    fn metric_at(metrics: &Self::Metrics, id: MetricId) -> u32 {
        metrics.metric_at(id)
    }

    fn add_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.add_assign(other);
    }

    fn sub_assign(metrics: &mut Self::Metrics, other: Self::Metrics) {
        metrics.sub_assign(other);
    }

    fn measure(t: &char) -> Metrics<2> {
        Metrics([1, u32::from(*t == '\n')])
    }
}

fn rope_to_string(rope: &Rope<char, TextMeasure>) -> String {
    rope.assert_well_balanced();

    if rope.is_empty() {
        return String::new();
    }

    let mut cursor = rope.cursor();
    let mut out = String::new();
    out.push(*cursor.element());
    while cursor.advance() {
        out.push(*cursor.element());
    }
    out
}

fn assert_cursor_well_balanced(cursor: &crate::Cursor<char, TextMeasure>) {
    cursor.clone().rope().assert_well_balanced();
}

fn empty_rope() -> Rope<char, TextMeasure> {
    let rope = Rope::<char, TextMeasure>::new();
    rope.assert_well_balanced();
    rope
}

fn rope_from_iter(elements: impl IntoIterator<Item = char>) -> Rope<char, TextMeasure> {
    let rope = Rope::<char, TextMeasure>::from_iter(elements);
    rope.assert_well_balanced();
    rope
}

fn rope_from_iter_with_branching(
    elements: impl IntoIterator<Item = char>,
    leaf_capacity: usize,
    branching_factor: usize,
) -> Rope<char, TextMeasure> {
    let rope = Rope::<char, TextMeasure>::from_iter_with_branching(
        elements,
        leaf_capacity,
        branching_factor,
    );
    rope.assert_well_balanced();
    rope
}

fn rope_from_cursor(cursor: crate::Cursor<char, TextMeasure>) -> Rope<char, TextMeasure> {
    let rope = cursor.rope();
    rope.assert_well_balanced();
    rope
}

fn replace_at_cursor<I>(cursor: &mut crate::Cursor<char, TextMeasure>, n: usize, replacement: I)
where
    I: IntoIterator<Item = char>,
{
    cursor.delete(n);
    let rope = rope_from_iter(replacement);
    if !rope.is_empty() {
        cursor.insert(rope);
    }
    assert_cursor_well_balanced(cursor);
}

fn apply_delete_insert_model<I>(model: &mut Vec<char>, index: usize, n: usize, replacement: I)
where
    I: IntoIterator<Item = char>,
{
    let removed = n.min(model.len().saturating_sub(index));
    model.drain(index..index + removed);

    model.splice(index.min(model.len())..index.min(model.len()), replacement);
}

fn model_seek(chars: &[char], metric_id: MetricId, value: u32) -> Option<(usize, Metrics<2>)> {
    if chars.is_empty() {
        return None;
    }

    let mut pos = Metrics([0, 0]);
    let last_idx = chars.len() - 1;
    for (idx, ch) in chars.iter().enumerate() {
        if value <= pos.metric_at(metric_id) {
            return Some((idx, pos));
        }

        let width = TextMeasure::measure(ch).metric_at(metric_id);
        if value < pos.metric_at(metric_id).saturating_add(width) || idx == last_idx {
            return Some((idx, pos));
        }

        pos.add_assign(TextMeasure::measure(ch));
    }

    None
}

fn model_seek_before(
    chars: &[char],
    metric_id: MetricId,
    value: u32,
) -> Option<(usize, Metrics<2>)> {
    if chars.is_empty() {
        return None;
    }

    let mut pos = Metrics([0, 0]);
    let last_idx = chars.len() - 1;
    for (idx, ch) in chars.iter().enumerate() {
        let width = TextMeasure::measure(ch).metric_at(metric_id);
        if value <= pos.metric_at(metric_id).saturating_add(width) || idx == last_idx {
            return Some((idx, pos));
        }

        pos.add_assign(TextMeasure::measure(ch));
    }

    None
}

fn model_forward_seek(
    chars: &[char],
    current_idx: usize,
    metric_id: MetricId,
    value: u32,
) -> Option<(usize, Metrics<2>)> {
    if chars.is_empty() {
        return None;
    }

    let mut pos = Metrics([0, 0]);
    for ch in chars.iter().take(current_idx) {
        pos.add_assign(TextMeasure::measure(ch));
    }
    if value <= pos.metric_at(metric_id) {
        return Some((current_idx, pos));
    }

    for (idx, ch) in chars.iter().enumerate().skip(current_idx) {
        let width = TextMeasure::measure(ch).metric_at(metric_id);
        if value <= pos.metric_at(metric_id)
            || value < pos.metric_at(metric_id).saturating_add(width)
        {
            return Some((idx, pos));
        }
        pos.add_assign(TextMeasure::measure(ch));
    }

    let mut final_pos = Metrics::zero();
    for ch in chars.iter().take(chars.len().saturating_sub(1)) {
        final_pos.add_assign(TextMeasure::measure(ch));
    }
    Some((chars.len() - 1, final_pos))
}

fn lcg_next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state
}

fn assert_root_seek_case(source: &str, metric_id: MetricId, target: u32) {
    let chars: Vec<char> = source.chars().collect();
    let rope = rope_from_iter(chars.iter().copied());
    let expected = model_seek(&chars, metric_id, target).unwrap();

    let mut cursor = rope.cursor();
    assert!(cursor.seek(metric_id, target, SeekMode::After));
    assert_eq!(*cursor.element(), chars[expected.0]);
    assert_eq!(cursor.position(), expected.1);
}

fn assert_forward_seek_case(source: &str, start_target: u32, target: u32) {
    let chars: Vec<char> = source.chars().collect();
    let rope = rope_from_iter(chars.iter().copied());
    let start = model_seek(&chars, MetricId(0), start_target).unwrap();
    let expected = model_forward_seek(&chars, start.0, MetricId(0), target).unwrap();

    let mut cursor = rope.cursor();
    assert!(cursor.seek(MetricId(0), start_target, SeekMode::After));
    assert_eq!(*cursor.element(), chars[start.0]);
    assert_eq!(cursor.position(), start.1);

    assert!(cursor.seek(MetricId(0), target, SeekMode::After));
    assert_eq!(*cursor.element(), chars[expected.0]);
    assert_eq!(cursor.position(), expected.1);
}

fn assert_replace_case(source: &str, target: u32, replacement: &str) {
    let mut model: Vec<char> = source.chars().collect();
    let rope = rope_from_iter(model.iter().copied());
    let expected = model_seek(&model, MetricId(0), target).unwrap();

    let mut cursor = rope.cursor();
    assert!(cursor.seek(MetricId(0), target, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, replacement.chars());
    apply_delete_insert_model(&mut model, expected.0, 1, replacement.chars());

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(rope_to_string(&rope), expected_text);
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
}

#[test]
fn root_seek_char_cases() {
    let source = "ab\ncd\nef\nghij\nklmno\npq";
    for target in 0..=24 {
        assert_root_seek_case(source, MetricId(0), target);
    }
}

#[test]
fn root_seek_line_cases() {
    let source = "ab\ncd\nef\nghij\nklmno\npq";
    for target in 0..=14 {
        assert_root_seek_case(source, MetricId(1), target);
    }
}

#[test]
fn forward_seek_cases() {
    let source = "ab\ncd\nef\nghij\nklmno\npq";
    let cases = [
        (0, 0),
        (0, 1),
        (0, 3),
        (0, 7),
        (0, 20),
        (2, 2),
        (2, 4),
        (2, 8),
        (5, 5),
        (5, 9),
        (5, 18),
        (8, 8),
        (8, 12),
        (8, 21),
        (10, 10),
        (10, 15),
        (12, 17),
        (15, 20),
        (18, 21),
        (21, 30),
    ];

    for (start, target) in cases {
        assert_forward_seek_case(source, start, target);
    }
}

#[test]
fn replace_cases() {
    let cases = [
        ("abcdef", 0, "X"),
        ("abcdef", 1, ""),
        ("abcdef", 2, "YZ"),
        ("abcdef", 3, "\n"),
        ("abcdef", 5, "!"),
        ("abcdef", 9, "QQ"),
        ("ab\ncd\nef", 0, ""),
        ("ab\ncd\nef", 2, "XYZ"),
        ("ab\ncd\nef", 3, "\n\n"),
        ("ab\ncd\nef", 5, "RST"),
        ("ab\ncd\nef", 7, ""),
        ("ab\ncd\nef", 20, "K"),
        ("0123456789", 4, "abc"),
        ("0123456789", 4, ""),
        ("0123456789", 9, "\nZ"),
        ("hello\nworld", 5, "_"),
        ("hello\nworld", 6, "++"),
        ("hello\nworld", 10, ""),
        ("a\nb\nc\nd", 1, "BB"),
        ("a\nb\nc\nd", 6, "tail"),
    ];

    for (source, target, replacement) in cases {
        assert_replace_case(source, target, replacement);
    }
}

#[test]
fn cursor_walks_elements() {
    let rope = rope_from_iter("abc".chars());
    let mut cursor = rope.cursor();

    assert_eq!(*cursor.element(), 'a');
    assert_eq!(cursor.position(), Metrics([0, 0]));
    assert!(cursor.advance());
    assert_eq!(*cursor.element(), 'b');
    assert_eq!(cursor.position(), Metrics([1, 0]));
    assert!(cursor.advance());
    assert_eq!(*cursor.element(), 'c');
    assert_eq!(cursor.position(), Metrics([2, 0]));
    assert!(!cursor.advance());
}

#[test]
fn cursor_creates_iterator() {
    let rope = rope_from_iter("abc".chars());
    let collected: String = rope.iter().collect();
    assert_eq!(collected, "abc");
}

#[test]
fn seek_uses_requested_metric() {
    let rope = rope_from_iter("ab\ncd\nef".chars());
    let mut by_chars = rope.cursor();
    let mut by_lines = rope.cursor();

    assert!(by_chars.seek(MetricId(0), 4, SeekMode::After));
    assert_eq!(*by_chars.element(), 'd');
    assert_eq!(by_chars.position(), Metrics([4, 1]));

    assert!(by_lines.seek(MetricId(1), 1, SeekMode::After));
    assert_eq!(*by_lines.element(), 'c');
    assert_eq!(by_lines.position(), Metrics([3, 1]));
}

#[test]
fn seek_before_stops_on_left_side_of_metric_boundary() {
    let rope = rope_from_iter("ab\ncd".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 2, SeekMode::After));
    assert_eq!(*cursor.element(), '\n');
    assert_eq!(cursor.position(), Metrics([2, 0]));

    assert!(cursor.seek(MetricId(0), 2, SeekMode::Before));
    assert_eq!(*cursor.element(), 'b');
    assert_eq!(cursor.position(), Metrics([1, 0]));

    assert!(cursor.seek(MetricId(1), 1, SeekMode::After));
    assert_eq!(*cursor.element(), 'c');
    assert_eq!(cursor.position(), Metrics([3, 1]));

    assert!(cursor.seek(MetricId(1), 1, SeekMode::Before));
    assert_eq!(*cursor.element(), '\n');
    assert_eq!(cursor.position(), Metrics([2, 0]));
}

#[test]
fn seek_before_at_zero_and_past_end_clamps_to_existing_elements() {
    let rope = rope_from_iter("abc".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 0, SeekMode::Before));
    assert_eq!(*cursor.element(), 'a');
    assert_eq!(cursor.position(), Metrics([0, 0]));

    assert!(cursor.seek(MetricId(0), 999, SeekMode::Before));
    assert_eq!(*cursor.element(), 'c');
    assert_eq!(cursor.position(), Metrics([2, 0]));
}

#[test]
fn cursor_replace_rebuilds_metrics() {
    let rope = rope_from_iter("abc".chars());
    let mut cursor = rope.cursor();
    assert!(cursor.advance());

    replace_at_cursor(&mut cursor, 1, "XYZ".chars());
    let rope = rope_from_cursor(cursor);
    assert_eq!(rope.len(), 5);
    assert_eq!(rope.metrics(), Metrics([5, 0]));
    assert_eq!(rope_to_string(&rope), "aXYZc");
}

#[test]
fn clone_is_persistent_across_mutation() {
    let left = rope_from_iter("abcdef".chars());
    let right = left.clone();
    right.assert_well_balanced();

    let mut cursor = left.cursor();
    assert!(cursor.advance());
    replace_at_cursor(&mut cursor, 1, "ZZ".chars());
    let left = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&left), "aZZcdef");
    assert_eq!(rope_to_string(&right), "abcdef");
}

#[test]
fn large_replace_keeps_all_content() {
    let source: String = (0..200)
        .map(|idx| char::from(b'a' + (idx % 26) as u8))
        .collect();
    let rope = rope_from_iter(source.chars());
    let replacement: String = (0..300)
        .map(|idx| char::from(b'A' + (idx % 26) as u8))
        .collect();

    let mut cursor = rope.cursor();
    for _ in 0..100 {
        assert!(cursor.advance());
    }
    replace_at_cursor(&mut cursor, 1, replacement.chars());
    let rope = rope_from_cursor(cursor);

    let mut expected = String::new();
    expected.push_str(&source[..100]);
    expected.push_str(&replacement);
    expected.push_str(&source[101..]);
    assert_eq!(rope_to_string(&rope), expected);
    assert_eq!(rope.metrics(), Metrics([499, 0]));
}

#[test]
fn seek_scans_forward_from_current_position() {
    let rope = rope_from_iter("abcdef".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 3, SeekMode::After));
    assert_eq!(*cursor.element(), 'd');

    assert!(cursor.seek(MetricId(0), 4, SeekMode::After));
    assert_eq!(*cursor.element(), 'e');

    assert!(cursor.seek(MetricId(0), 2, SeekMode::After));
    assert_eq!(*cursor.element(), 'c');
}

#[test]
fn empty_rope_has_no_positioned_cursor() {
    let rope = empty_rope();
    let mut cursor = rope.cursor();
    assert!(!cursor.advance());
    assert!(!cursor.seek(MetricId(0), 0, SeekMode::After));
}

#[test]
fn seek_past_end_clamps_to_last_element() {
    let rope = rope_from_iter("abc".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 999, SeekMode::After));
    assert_eq!(*cursor.element(), 'c');
    assert_eq!(cursor.position(), Metrics([2, 0]));
}

#[test]
fn replace_with_empty_removes_current_element() {
    let rope = rope_from_iter("abcd".chars());
    let mut cursor = rope.cursor();
    assert!(cursor.advance());
    assert!(cursor.advance());

    cursor.delete(1);
    assert_cursor_well_balanced(&cursor);
    let rope = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&rope), "abd");
    assert_eq!(rope.metrics(), Metrics([3, 0]));
}

#[test]
fn replace_only_element_can_empty_the_rope() {
    let rope = rope_from_iter("x".chars());
    let mut cursor = rope.cursor();

    cursor.delete(1);
    assert_cursor_well_balanced(&cursor);
    let rope = rope_from_cursor(cursor);

    assert!(rope.is_empty());
    assert_eq!(rope.len(), 0);
    assert_eq!(rope.metrics(), Metrics([0, 0]));
}

#[test]
fn repeated_replacements_keep_structure_consistent() {
    let rope = rope_from_iter("abcdef".chars());

    let mut cursor = rope.cursor();
    assert!(cursor.advance());
    replace_at_cursor(&mut cursor, 1, "XY".chars());
    let rope = rope_from_cursor(cursor);
    assert_eq!(rope_to_string(&rope), "aXYcdef");

    let mut cursor = rope.cursor();
    for _ in 0..4 {
        assert!(cursor.advance());
    }
    replace_at_cursor(&mut cursor, 1, "!".chars());
    let rope = rope_from_cursor(cursor);
    assert_eq!(rope_to_string(&rope), "aXYc!ef");
    assert_eq!(rope.metrics(), Metrics([7, 0]));
}

#[test]
fn replacement_can_expand_across_existing_leaf_boundaries() {
    let source: String = (0..180)
        .map(|idx| char::from(b'a' + (idx % 26) as u8))
        .collect();
    let replacement: String = (0..120)
        .map(|idx| char::from(b'A' + (idx % 26) as u8))
        .collect();
    let rope = rope_from_iter(source.chars());

    let mut cursor = rope.cursor();
    for _ in 0..70 {
        assert!(cursor.advance());
    }
    replace_at_cursor(&mut cursor, 1, replacement.chars());
    let rope = rope_from_cursor(cursor);

    let mut expected = source;
    expected.replace_range(70..71, &replacement);
    assert_eq!(rope_to_string(&rope), expected);
    assert_eq!(rope.metrics(), Metrics([299, 0]));
}

#[test]
fn replace_forward_span_with_shorter_sequence() {
    let rope = rope_from_iter("abcdefghij".chars());
    let mut cursor = rope.cursor();

    for _ in 0..2 {
        assert!(cursor.advance());
    }

    replace_at_cursor(&mut cursor, 4, "XY".chars());
    let rope = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&rope), "abXYghij");
    assert_eq!(rope.metrics(), Metrics([8, 0]));
}

#[test]
fn replace_forward_span_with_insertion_only() {
    let rope = rope_from_iter("abcd".chars());
    let mut cursor = rope.cursor();
    assert!(cursor.advance());

    cursor.insert(rope_from_iter("XYZ".chars()));
    assert_cursor_well_balanced(&cursor);
    let rope = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&rope), "aXYZbcd");
    assert_eq!(rope.metrics(), Metrics([7, 0]));
}

#[test]
fn replace_forward_span_crossing_multiple_leaf_nodes() {
    let rope = rope_from_iter_with_branching("abcdefghijklmnopqrstuvwxyz".chars(), 4, 4);
    let mut cursor = rope.cursor();

    for _ in 0..5 {
        assert!(cursor.advance());
    }

    replace_at_cursor(&mut cursor, 11, "RST".chars());
    let rope = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&rope), "abcdeRSTqrstuvwxyz");
    assert_eq!(rope.metrics(), Metrics([18, 0]));
}

#[test]
fn replace_forward_span_clamps_at_end() {
    let rope = rope_from_iter("abcdef".chars());
    let mut cursor = rope.cursor();

    for _ in 0..4 {
        assert!(cursor.advance());
    }

    replace_at_cursor(&mut cursor, 99, "XY".chars());
    let rope = rope_from_cursor(cursor);

    assert_eq!(rope_to_string(&rope), "abcdXY");
    assert_eq!(rope.metrics(), Metrics([6, 0]));
}

#[test]
fn seek_by_lines_across_multiple_newlines() {
    let rope = rope_from_iter("a\nb\nc\nd".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(1), 2, SeekMode::After));
    assert_eq!(*cursor.element(), 'c');
    assert_eq!(cursor.position(), Metrics([4, 2]));
}

#[test]
fn forward_seek_across_many_leaves() {
    let rope = rope_from_iter("abcdefghijklmnopqrstuvwxyz".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 10, SeekMode::After));
    assert_eq!(*cursor.element(), 'k');

    assert!(cursor.seek(MetricId(0), 20, SeekMode::After));
    assert_eq!(*cursor.element(), 'u');

    assert!(cursor.seek(MetricId(0), 25, SeekMode::After));
    assert_eq!(*cursor.element(), 'z');
}

#[test]
fn randomized_replacements_match_string_model() {
    let mut seed = 0x5eed_u64;
    let mut model: Vec<char> = (0..300)
        .map(|idx| {
            if idx % 17 == 0 {
                '\n'
            } else {
                char::from(b'a' + (idx % 26) as u8)
            }
        })
        .collect();
    let mut rope = rope_from_iter(model.iter().copied());

    for _step in 0..200 {
        if model.is_empty() {
            let refill_len = (lcg_next(&mut seed) % 8 + 1) as usize;
            let refill: Vec<char> = (0..refill_len)
                .map(|i| {
                    if i % 5 == 0 {
                        '\n'
                    } else {
                        char::from(b'A' + (lcg_next(&mut seed) % 26) as u8)
                    }
                })
                .collect();
            model.extend(refill.iter().copied());
            rope = rope_from_iter(model.iter().copied());
        }

        let index = (lcg_next(&mut seed) as usize) % model.len();
        let replacement_len = (lcg_next(&mut seed) % 7) as usize;
        let replacement: Vec<char> = (0..replacement_len)
            .map(|i| {
                if (lcg_next(&mut seed) + i as u64).is_multiple_of(9) {
                    '\n'
                } else {
                    char::from(b'k' + (lcg_next(&mut seed) % 10) as u8)
                }
            })
            .collect();

        let mut cursor = rope.cursor();
        assert!(cursor.seek(MetricId(0), index as u32, SeekMode::After));
        replace_at_cursor(&mut cursor, 1, replacement.iter().copied());
        rope = rope_from_cursor(cursor);

        apply_delete_insert_model(&mut model, index, 1, replacement.iter().copied());

        let rope_text = rope_to_string(&rope);
        let model_text: String = model.iter().collect();
        assert_eq!(rope_text, model_text);
        assert_eq!(rope.len(), model.len());
        assert_eq!(
            rope.metrics(),
            Metrics([
                model.len() as u32,
                model.iter().filter(|&&ch| ch == '\n').count() as u32,
            ]),
        );
    }
}

#[test]
fn randomized_root_seek_matches_model() {
    let mut seed = 0x1234_5678_u64;
    let chars: Vec<char> = (0..512)
        .map(|idx| {
            let r = lcg_next(&mut seed);
            if idx % 11 == 0 || r.is_multiple_of(13) {
                '\n'
            } else {
                char::from(b'a' + (r % 26) as u8)
            }
        })
        .collect();
    let rope = rope_from_iter(chars.iter().copied());

    for target in 0..600_u32 {
        let mut cursor = rope.cursor();
        let expected = model_seek(&chars, MetricId(0), target).unwrap();
        assert!(cursor.seek(MetricId(0), target, SeekMode::After));
        assert_eq!(*cursor.element(), chars[expected.0]);
        assert_eq!(cursor.position(), expected.1);

        let mut cursor = rope.cursor();
        let expected = model_seek_before(&chars, MetricId(0), target).unwrap();
        assert!(cursor.seek(MetricId(0), target, SeekMode::Before));
        assert_eq!(*cursor.element(), chars[expected.0]);
        assert_eq!(cursor.position(), expected.1);
    }

    for target in 0..100_u32 {
        let mut cursor = rope.cursor();
        let expected = model_seek(&chars, MetricId(1), target).unwrap();
        assert!(cursor.seek(MetricId(1), target, SeekMode::After));
        assert_eq!(
            *cursor.element(),
            chars[expected.0],
            "line target {} expected index {} pos {:?} got pos {:?}",
            target,
            expected.0,
            expected.1,
            cursor.position(),
        );
        assert_eq!(cursor.position(), expected.1, "line target {}", target);

        let mut cursor = rope.cursor();
        let expected = model_seek_before(&chars, MetricId(1), target).unwrap();
        assert!(cursor.seek(MetricId(1), target, SeekMode::Before));
        assert_eq!(
            *cursor.element(),
            chars[expected.0],
            "line target {} expected index {} pos {:?} got pos {:?}",
            target,
            expected.0,
            expected.1,
            cursor.position(),
        );
        assert_eq!(cursor.position(), expected.1, "line target {}", target);
    }
}

#[test]
fn randomized_forward_seek_matches_model() {
    let mut seed = 0xabcd_9876_u64;
    let chars: Vec<char> = (0..400)
        .map(|idx| {
            let r = lcg_next(&mut seed);
            if idx % 19 == 0 || r.is_multiple_of(7) {
                '\n'
            } else {
                char::from(b'm' + (r % 10) as u8)
            }
        })
        .collect();
    let rope = rope_from_iter(chars.iter().copied());

    let mut cursor = rope.cursor();
    let mut current_idx = 0usize;
    for _ in 0..150 {
        let jump = (lcg_next(&mut seed) % 5) as u32;
        let current_pos = model_seek(&chars, MetricId(0), current_idx as u32)
            .unwrap()
            .1
            .metric_at(MetricId(0));
        let target = current_pos + jump;
        let expected = model_forward_seek(&chars, current_idx, MetricId(0), target).unwrap();

        assert!(cursor.seek(MetricId(0), target, SeekMode::After));
        assert_eq!(*cursor.element(), chars[expected.0]);
        assert_eq!(cursor.position(), expected.1);
        current_idx = expected.0;
    }
}

#[test]
fn seek_backward_after_replace_before_rope_restarts_from_updated_root() {
    let rope = rope_from_iter("abc\ndef\nghi".chars());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 5, SeekMode::After));
    assert_eq!(*cursor.element(), 'e');

    replace_at_cursor(&mut cursor, 1, "XYZ\n".chars());

    assert!(cursor.seek(MetricId(0), 1, SeekMode::After));
    assert_eq!(*cursor.element(), 'b');
    assert_eq!(cursor.position(), Metrics([1, 0]));

    assert!(cursor.seek(MetricId(1), 1, SeekMode::After));
    assert_eq!(*cursor.element(), 'd');
    assert_eq!(cursor.position(), Metrics([4, 1]));

    let rope = rope_from_cursor(cursor);
    assert_eq!(rope_to_string(&rope), "abc\ndXYZ\nf\nghi");
    assert_eq!(rope.metrics(), Metrics([14, 3]));
}

#[test]
fn multiple_replacements_and_seeks_on_single_cursor_match_model() {
    let mut model: Vec<char> = "abcdefghij".chars().collect();
    let rope = rope_from_iter(model.iter().copied());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(0), 2, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, "XY".chars());
    apply_delete_insert_model(&mut model, 2, 1, "XY".chars());

    assert!(cursor.seek(MetricId(0), 0, SeekMode::After));
    assert_eq!(*cursor.element(), model[0]);

    assert!(cursor.seek(MetricId(0), 6, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, std::iter::once('\n'));
    apply_delete_insert_model(&mut model, 6, 1, std::iter::once('\n'));

    assert!(cursor.seek(MetricId(1), 0, SeekMode::After));
    let expected = model_seek(&model, MetricId(1), 0).unwrap();
    assert_eq!(*cursor.element(), model[expected.0]);
    assert_eq!(cursor.position(), expected.1);

    assert!(cursor.seek(MetricId(1), 1, SeekMode::After));
    let expected = model_seek(&model, MetricId(1), 1).unwrap();
    assert_eq!(*cursor.element(), model[expected.0]);
    assert_eq!(cursor.position(), expected.1);

    assert!(cursor.seek(MetricId(0), 8, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, "!".chars());
    apply_delete_insert_model(&mut model, 8, 1, "!".chars());

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(rope_to_string(&rope), expected_text);
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
}

#[test]
fn divergent_cursors_from_same_base_produce_independent_versions() {
    let base = rope_from_iter("lorem\nipsum\ndolor".chars());

    let mut left_cursor = base.cursor();
    assert!(left_cursor.seek(MetricId(0), 1, SeekMode::After));
    replace_at_cursor(&mut left_cursor, 1, "A".chars());
    assert!(left_cursor.seek(MetricId(1), 1, SeekMode::After));
    replace_at_cursor(&mut left_cursor, 1, "B".chars());
    let left = rope_from_cursor(left_cursor);

    let mut right_cursor = base.cursor();
    assert!(right_cursor.seek(MetricId(0), 10, SeekMode::After));
    replace_at_cursor(&mut right_cursor, 1, "XYZ".chars());
    let right = rope_from_cursor(right_cursor);

    assert_eq!(rope_to_string(&base), "lorem\nipsum\ndolor");
    assert_eq!(rope_to_string(&left), "lArem\nBpsum\ndolor");
    assert_eq!(rope_to_string(&right), "lorem\nipsuXYZ\ndolor");
}

#[test]
fn forward_and_backward_line_seeks_after_many_edits_match_model() {
    let mut model: Vec<char> = "a\nb\nc\nd\ne\nf".chars().collect();
    let rope = rope_from_iter(model.iter().copied());
    let mut cursor = rope.cursor();

    assert!(cursor.seek(MetricId(1), 2, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, "CC\n".chars());
    apply_delete_insert_model(&mut model, 4, 1, "CC\n".chars());

    assert!(cursor.seek(MetricId(1), 0, SeekMode::After));
    assert_eq!(
        cursor.position(),
        model_seek(&model, MetricId(1), 0).unwrap().1
    );

    assert!(cursor.seek(MetricId(1), 3, SeekMode::After));
    let expected = model_seek(&model, MetricId(1), 3).unwrap();
    assert_eq!(*cursor.element(), model[expected.0]);
    assert_eq!(cursor.position(), expected.1);

    assert!(cursor.seek(MetricId(0), 2, SeekMode::After));
    cursor.delete(1);
    assert_cursor_well_balanced(&cursor);
    apply_delete_insert_model(&mut model, 2, 1, std::iter::empty());

    assert!(cursor.seek(MetricId(1), 2, SeekMode::After));
    let expected = model_seek(&model, MetricId(1), 2).unwrap();
    assert_eq!(*cursor.element(), model[expected.0]);
    assert_eq!(cursor.position(), expected.1);

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(rope_to_string(&rope), expected_text);
}

#[test]
fn large_real_text_fixture_scans_and_edits_correctly() {
    let mut model: Vec<char> = LARGE_TEXT_FIXTURE.chars().collect();
    let rope = rope_from_iter(model.iter().copied());

    assert!(model.len() > 500_000);

    let char_targets = [0_u32, 1, 17, 128, 4_096, 8_192, 16_384, 32_768];
    for target in char_targets {
        let expected = model_seek(&model, MetricId(0), target).unwrap();
        let mut cursor = rope.cursor();
        assert!(cursor.seek(MetricId(0), target, SeekMode::After));
        assert_eq!(
            *cursor.element(),
            model[expected.0],
            "char target {}",
            target
        );
        assert_eq!(cursor.position(), expected.1, "char target {}", target);
    }

    let edit_target = 8_192_u32.min(model.len().saturating_sub(1) as u32);
    let expected = model_seek(&model, MetricId(0), edit_target).unwrap();
    let replacement = "\n// rope stress edit\nfn rope_fixture_probe() {}\n";

    let mut cursor = rope.cursor();
    assert!(cursor.seek(MetricId(0), edit_target, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, replacement.chars());
    apply_delete_insert_model(&mut model, expected.0, 1, replacement.chars());

    let rope = rope_from_cursor(cursor);
    assert_eq!(rope.len(), model.len());
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
}

#[test]
fn evenly_distributed_growth_rebalances_large_persistent_edit_series() {
    let source: Vec<char> = GROWTH_TEXT_FIXTURE.chars().collect();
    let mut model = source.clone();
    let rope = rope_from_iter(source.iter().copied());
    let mut cursor = rope.cursor();

    for idx in (0..source.len()).rev() {
        let fill = char::from(b'0' + (idx % 10) as u8);
        let mut replacement = String::with_capacity(100);
        replacement.push(source[idx]);
        replacement.extend(std::iter::repeat_n(fill, 99));

        assert!(cursor.seek(MetricId(0), idx as u32, SeekMode::After));
        replace_at_cursor(&mut cursor, 1, replacement.chars());

        apply_delete_insert_model(&mut model, idx, 1, replacement.chars());
    }

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(rope.len(), source.len() * 100);
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
    assert_eq!(rope_to_string(&rope), expected_text);
}

#[test]
fn evenly_distributed_shrink_rebalances_large_persistent_edit_series() {
    let source: Vec<char> = LARGE_TEXT_FIXTURE.chars().take(20_000).collect();
    let mut model = source.clone();
    let rope = rope_from_iter(source.iter().copied());
    let mut cursor = rope.cursor();

    for idx in (0..source.len()).rev() {
        if idx % 10 != 0 {
            assert!(cursor.seek(MetricId(0), idx as u32, SeekMode::After));
            cursor.delete(1);
            assert_cursor_well_balanced(&cursor);
            apply_delete_insert_model(&mut model, idx, 1, std::iter::empty());
        }
    }

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(
        rope.len(),
        source.len() / 10 + usize::from(!source.len().is_multiple_of(10))
    );
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
    assert_eq!(rope_to_string(&rope), expected_text);
}

fn assert_large_splice_into_small_case(small: &str, target: u32, large: &str) {
    let mut model: Vec<char> = small.chars().collect();
    let large_chars: Vec<char> = large.chars().collect();
    let rope = rope_from_iter(model.iter().copied());
    let expected = model_seek(&model, MetricId(0), target).unwrap();

    let mut cursor = rope.cursor();
    assert!(cursor.seek(MetricId(0), target, SeekMode::After));
    replace_at_cursor(&mut cursor, 1, large_chars.iter().copied());
    apply_delete_insert_model(&mut model, expected.0, 1, large_chars.iter().copied());

    let rope = rope_from_cursor(cursor);
    let expected_text: String = model.iter().collect();
    assert_eq!(rope_to_string(&rope), expected_text);
    assert_eq!(
        rope.metrics(),
        Metrics([
            model.len() as u32,
            model.iter().filter(|&&ch| ch == '\n').count() as u32,
        ]),
    );
}

#[test]
fn splice_large_rope_into_small_one_at_beginning() {
    assert_large_splice_into_small_case("small", 0, LARGE_TEXT_FIXTURE);
}

#[test]
fn splice_large_rope_into_small_one_in_middle() {
    assert_large_splice_into_small_case("small", 2, LARGE_TEXT_FIXTURE);
}

#[test]
fn splice_large_rope_into_small_one_at_end() {
    assert_large_splice_into_small_case("small", 10, LARGE_TEXT_FIXTURE);
}

#[test]
fn insert_taller_rope_into_small_one() {
    let base = rope_from_iter("xy".chars());
    let inserted_text: String = (0..512)
        .map(|idx| {
            if idx % 17 == 0 {
                '\n'
            } else {
                char::from(b'a' + (idx % 26) as u8)
            }
        })
        .collect();
    let inserted = rope_from_iter_with_branching(inserted_text.chars(), 4, 4);

    assert!(inserted.root.as_ref().depth() > base.root.as_ref().depth());

    let mut cursor = base.cursor();
    assert!(cursor.advance());
    cursor.insert(inserted);
    assert_cursor_well_balanced(&cursor);

    let rope = rope_from_cursor(cursor);
    let expected = format!("x{}y", inserted_text);
    assert_eq!(rope_to_string(&rope), expected);
    assert_eq!(
        rope.metrics(),
        Metrics([
            expected.chars().count() as u32,
            expected.chars().filter(|&ch| ch == '\n').count() as u32,
        ]),
    );
}
