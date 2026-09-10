use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rope::{Measure, MetricId, Metrics, Rope, SeekMode};

use crate::{rewrite, Safepoint};

const LEN: MetricId = MetricId(0);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Piece {
    text: String,
    safepoint: bool,
}

#[derive(Clone, Copy, Debug)]
struct PieceMeasure;

impl Measure<Piece> for PieceMeasure {
    type Metrics = Metrics<1>;

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

    fn measure(piece: &Piece) -> Self::Metrics {
        Metrics([piece.text.len() as u32])
    }
}

struct PieceSafepoints {
    cuts: Rc<RefCell<Vec<(String, u32)>>>,
}

impl PieceSafepoints {
    fn new() -> Self {
        Self {
            cuts: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl Safepoint<Piece> for PieceSafepoints {
    fn is_safepoint(&self, element: &Piece) -> bool {
        element.safepoint
    }

    fn cut(&self, element: Piece, offset: u32) -> Piece {
        self.cuts.borrow_mut().push((element.text.clone(), offset));
        Piece {
            text: element.text[offset as usize..].to_string(),
            safepoint: true,
        }
    }
}

struct CountingTokens {
    tokens: Vec<Piece>,
    index: usize,
    next_calls: Rc<Cell<usize>>,
}

impl Iterator for CountingTokens {
    type Item = Piece;

    fn next(&mut self) -> Option<Self::Item> {
        let token = self.tokens.get(self.index)?.clone();
        self.index += 1;
        self.next_calls.set(self.next_calls.get() + 1);
        Some(token)
    }
}

fn piece(text: &str, safepoint: bool) -> Piece {
    Piece {
        text: text.to_string(),
        safepoint,
    }
}

fn owned_piece(text: String, safepoint: bool) -> Piece {
    Piece { text, safepoint }
}

fn rope(pieces: impl IntoIterator<Item = Piece>) -> Rope<Piece, PieceMeasure> {
    Rope::from_iter(pieces)
}

fn text(rope: &Rope<Piece, PieceMeasure>) -> String {
    rope.iter().map(|piece| piece.text).collect()
}

fn piece_texts(rope: &Rope<Piece, PieceMeasure>) -> Vec<String> {
    rope.iter().map(|piece| piece.text).collect()
}

fn counting_tokens(tokens: Vec<Piece>) -> (CountingTokens, Rc<Cell<usize>>) {
    let next_calls = Rc::new(Cell::new(0));
    (
        CountingTokens {
            tokens,
            index: 0,
            next_calls: Rc::clone(&next_calls),
        },
        next_calls,
    )
}

fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn split_space_tokens(text: &str) -> Vec<Piece> {
    split_space_tokens_from(text, 0)
}

fn split_space_tokens_from(text: &str, offset: usize) -> Vec<Piece> {
    let bytes = text.as_bytes();
    let mut index = offset;
    let mut tokens = Vec::new();

    while index < bytes.len() {
        let space = bytes[index] == b' ';
        let mut end = index + 1;

        while end < bytes.len() && (bytes[end] == b' ') == space {
            end += 1;
        }

        tokens.push(piece(&text[index..end], true));
        index = end;
    }

    tokens
}

fn generated_space_text(words: usize) -> String {
    let mut text = String::new();

    for index in 0..words {
        if index != 0 {
            match index % 5 {
                0 => text.push_str("   "),
                1 | 2 => text.push(' '),
                _ => text.push_str("  "),
            }
        }
        text.push_str("word");
        text.push_str(&index.to_string());
    }

    text
}

fn insert_dirty_space(rope: Rope<Piece, PieceMeasure>, offset: usize) -> Rope<Piece, PieceMeasure> {
    let mut cursor = rope.cursor();
    assert!(cursor.seek(LEN, offset as u32, SeekMode::After));

    let element = cursor.element().clone();
    let element_start = cursor.position().metric_at(LEN) as usize;
    let local_offset = offset - element_start;
    let mut replacement = Vec::with_capacity(3);

    if local_offset != 0 {
        replacement.push(owned_piece(element.text[..local_offset].to_string(), false));
    }

    replacement.push(piece(" ", false));

    if local_offset != element.text.len() {
        replacement.push(owned_piece(element.text[local_offset..].to_string(), false));
    }

    cursor.delete(1);
    cursor.insert(rope_from_vec(replacement));
    cursor.rope()
}

fn rope_from_vec(pieces: Vec<Piece>) -> Rope<Piece, PieceMeasure> {
    Rope::from_iter(pieces)
}

fn insert_random_spaces(
    mut rope: Rope<Piece, PieceMeasure>,
    text: &mut String,
    edits: usize,
    seed: &mut u64,
    min_offset: usize,
) -> Rope<Piece, PieceMeasure> {
    for _ in 0..edits {
        let span = text.len() + 1 - min_offset;
        let offset = min_offset + (lcg_next(seed) as usize % span);
        text.insert(offset, ' ');
        rope = insert_dirty_space(rope, offset);
    }

    rope
}

fn rewrite_space_tokens_in_chunks(
    mut rope: Rope<Piece, PieceMeasure>,
    text: &str,
    start: usize,
) -> (Rope<Piece, PieceMeasure>, usize, usize) {
    let policy = PieceSafepoints::new();
    let mut offset = start as u32;
    let mut passes = 0usize;

    while offset < text.len() as u32 {
        let mut cursor = rope.cursor();
        assert!(cursor.seek(LEN, offset, SeekMode::After));

        let chunk = 37 + (passes % 7) * 19;
        let stop_at = offset + chunk as u32;
        let report = rewrite(
            &mut cursor,
            split_space_tokens_from(text, offset as usize),
            &policy,
            LEN,
            |location| location.metric_at(LEN) >= stop_at,
        );
        let next_offset = report.location.metric_at(LEN);

        rope = cursor.rope();
        assert_eq!(self::text(&rope), text, "text mismatch after pass {passes}");

        if report.stopped {
            assert!(
                next_offset > offset,
                "rewrite stopped without progress at {offset}",
            );
            offset = next_offset;
        } else {
            assert_eq!(next_offset, text.len() as u32);
            break;
        }

        passes += 1;
        assert!(passes < 1024, "rewrite did not converge");
    }

    let cuts = policy.cuts.borrow().len();
    (rope, passes, cuts)
}

#[test]
fn rewrite_stops_before_safepoint_when_stop_accepts_location() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("XX", true), piece("cd", true), piece("ef", true)]);
    let mut cursor = old.cursor();

    let report = rewrite(
        &mut cursor,
        [piece("ab", true), piece("cd", true), piece("ef", true)],
        &policy,
        LEN,
        |location| location.metric_at(LEN) >= 2,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "abcdef");
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 1);
    assert_eq!(report.location, Metrics([2]));
    assert!(report.stopped);
    assert!(policy.cuts.borrow().is_empty());
}

#[test]
fn rewrite_does_not_stop_on_non_safepoint_tokens() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("XXX", true), piece("d", true)]);
    let mut cursor = old.cursor();

    let report = rewrite(
        &mut cursor,
        [piece("ab", true), piece("c", false), piece("d", true)],
        &policy,
        LEN,
        |location| location.metric_at(LEN) >= 2,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "abcd");
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 2);
    assert_eq!(report.location, Metrics([3]));
    assert!(report.stopped);
}

#[test]
fn rewrite_ignores_advisory_stop_until_old_alignment() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("XXXX", true), piece("ef", true)]);
    let mut cursor = old.cursor();

    let report = rewrite(
        &mut cursor,
        [piece("ab", true), piece("cd", true)],
        &policy,
        LEN,
        |location| location.metric_at(LEN) >= 2,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "abcdef");
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 2);
    assert_eq!(report.location, Metrics([4]));
    assert!(!report.stopped);
    assert!(policy.cuts.borrow().is_empty());
}

#[test]
fn rewrite_cuts_old_tail_when_tokens_are_exhausted() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("XXXX", true), piece("tail", true)]);
    let mut cursor = old.cursor();

    let report = rewrite(
        &mut cursor,
        [piece("ab", true), piece("c", false)],
        &policy,
        LEN,
        |_| false,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "abcXtail");
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 3);
    assert_eq!(report.location, Metrics([3]));
    assert!(!report.stopped);
    assert_eq!(policy.cuts.borrow().as_slice(), &[("XXXX".to_string(), 3)]);
}

#[test]
fn rewrite_consumes_no_tokens_after_stop_safepoint() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("X", true), piece("XXX", true)]);
    let mut cursor = old.cursor();
    let (tokens, next_calls) = counting_tokens(vec![
        piece("a", true),
        piece("b", true),
        piece("c", true),
        piece("d", true),
    ]);

    let report = rewrite(&mut cursor, tokens, &policy, LEN, |location| {
        location.metric_at(LEN) >= 1
    });
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "aXXX");
    assert_eq!(next_calls.get(), 2);
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 1);
    assert_eq!(report.location, Metrics([1]));
    assert!(report.stopped);
    assert!(policy.cuts.borrow().is_empty());
}

#[test]
fn rewrite_can_start_from_middle_of_rope() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("aa", true), piece("XX", true), piece("tail", true)]);
    let mut cursor = old.cursor();
    assert!(cursor.seek(LEN, 2, SeekMode::After));

    let report = rewrite(
        &mut cursor,
        [piece("bb", true), piece("tail", true)],
        &policy,
        LEN,
        |location| location.metric_at(LEN) >= 4,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "aabbtail");
    assert_eq!(report.old_elements_replaced, 1);
    assert_eq!(report.replacement_elements, 1);
    assert_eq!(report.location, Metrics([4]));
}

#[test]
fn rewrite_can_stop_without_changing_rope() {
    let policy = PieceSafepoints::new();
    let old = rope([piece("ab", true), piece("cd", true)]);
    let mut cursor = old.cursor();

    let report = rewrite(
        &mut cursor,
        [piece("ab", true), piece("cd", true)],
        &policy,
        LEN,
        |location| location.metric_at(LEN) == 0,
    );
    let rewritten = cursor.rope();

    assert_eq!(text(&rewritten), "abcd");
    assert_eq!(report.old_elements_replaced, 0);
    assert_eq!(report.replacement_elements, 0);
    assert_eq!(report.location, Metrics([0]));
    assert!(report.stopped);
}

#[test]
fn rewrite_repairs_random_space_insertions_in_chunks() {
    let mut text = generated_space_text(180);
    let original = rope(split_space_tokens(&text));
    let mut seed = 0x71d0_5eed_u64;
    let dirty = insert_random_spaces(original, &mut text, 240, &mut seed, 0);

    assert_eq!(self::text(&dirty), text);

    let (rewritten, passes, _) = rewrite_space_tokens_in_chunks(dirty, &text, 0);

    assert!(passes > 8);
    assert_eq!(self::text(&rewritten), text);
    assert_eq!(
        piece_texts(&rewritten),
        split_space_tokens(&text)
            .into_iter()
            .map(|piece| piece.text)
            .collect::<Vec<_>>(),
    );
    assert!(rewritten.iter().all(|piece| piece.safepoint));
}

#[test]
fn rewrite_repairs_random_space_insertions_from_middle_restart() {
    let mut text = generated_space_text(220);
    let restart = text.find("word90").expect("generated word exists");
    let original = rope(split_space_tokens(&text));
    let mut seed = 0x5eed_9123_u64;
    let dirty = insert_random_spaces(original, &mut text, 160, &mut seed, restart);

    assert_eq!(self::text(&dirty), text);

    let (rewritten, passes, _) = rewrite_space_tokens_in_chunks(dirty, &text, restart);

    assert!(passes > 4);
    assert_eq!(self::text(&rewritten), text);
    assert_eq!(
        piece_texts(&rewritten),
        split_space_tokens(&text)
            .into_iter()
            .map(|piece| piece.text)
            .collect::<Vec<_>>(),
    );
    assert!(rewritten.iter().all(|piece| piece.safepoint));
}
