use crate::{Bias, Op, Operation};

fn apply_operation(base: &str, operation: &Operation) -> String {
    let mut cursor = 0usize;
    let mut out = String::new();

    for op in operation {
        match op {
            Op::Retain(len) => {
                let end = cursor + len as usize;
                out.push_str(
                    base.get(cursor..end)
                        .expect("retain must stay on UTF-8 boundaries"),
                );
                cursor = end;
            }
            Op::Insert(text) => out.push_str(&text),
            Op::Delete(text) => {
                let len = text.len();
                let deleted = base
                    .get(cursor..cursor + len)
                    .expect("delete must stay on UTF-8 boundaries");
                assert_eq!(deleted, text, "delete text must match base");
                cursor += len;
            }
        }
    }

    assert_eq!(
        cursor,
        base.len(),
        "operation did not consume the full base text"
    );
    out
}

fn op_retain(len: u32) -> Op {
    Op::retain(len)
}

fn op_insert(text: &str) -> Op {
    Op::insert(text)
}

fn op_delete(text: &str) -> Op {
    Op::delete(text)
}

fn operation(ops: impl IntoIterator<Item = Op>) -> Operation {
    Operation::from_ops(ops)
}

fn assert_ops(actual: &Operation, expected: &[Op]) {
    let actual_ops: Vec<_> = actual.iter().collect();
    assert_eq!(actual_ops, expected);
}

fn assert_compose_case(
    base: &str,
    first: Operation,
    second: Operation,
    expected: &[Op],
    expected_text: &str,
) {
    let composed = first.compose(&second);
    assert_ops(&composed, expected);
    assert_eq!(apply_operation(base, &composed), expected_text);
    let once = apply_operation(base, &first);
    assert_eq!(apply_operation(&once, &second), expected_text);
}

fn assert_transform_pair(base: &str, left: Operation, right: Operation) {
    let left_prime = left.transform(&right);
    let right_prime = right.transform(&left);
    let after_left = apply_operation(base, &left);
    let after_right = apply_operation(base, &right);
    let left_then_right = apply_operation(&after_left, &right_prime);
    let right_then_left = apply_operation(&after_right, &left_prime);
    assert_eq!(left_then_right, right_then_left);
}

#[test]
fn empty_operation_is_empty() {
    let op = Operation::new();
    assert!(op.is_empty());
    assert_eq!(op.size(), 0);
    assert_eq!(op.old_len(), 0);
    assert_eq!(op.new_len(), 0);
    assert_ops(&op, &[]);
}

#[test]
fn retain_operation_preserves_lengths() {
    let op = Operation::retain(7);
    assert_eq!(op.old_len(), 7);
    assert_eq!(op.new_len(), 7);
    assert_ops(&op, &[op_retain(7)]);
}

#[test]
fn insert_operation_preserves_lengths() {
    let op = Operation::insert("abc");
    assert_eq!(op.old_len(), 0);
    assert_eq!(op.new_len(), 3);
    assert_ops(&op, &[op_insert("abc")]);
}

#[test]
fn delete_operation_preserves_lengths() {
    let op = Operation::delete("abc");
    assert_eq!(op.old_len(), 3);
    assert_eq!(op.new_len(), 0);
    assert_ops(&op, &[op_delete("abc")]);
}

#[test]
fn multibyte_text_lengths_are_bytes() {
    let text = "é漢";
    assert_eq!(text.len(), 5);
    assert_eq!(text.chars().count(), 2);

    let insert = Operation::insert(text);
    assert_eq!(insert.old_len(), 0);
    assert_eq!(insert.new_len(), 5);

    let delete = Operation::delete(text);
    assert_eq!(delete.old_len(), 5);
    assert_eq!(delete.new_len(), 0);
}

#[test]
fn builder_merges_adjacent_retains() {
    let op = operation([op_retain(2), op_retain(3)]);
    assert_ops(&op, &[op_retain(5)]);
}

#[test]
fn builder_merges_adjacent_inserts() {
    let op = operation([op_insert("ab"), op_insert("cd")]);
    assert_ops(&op, &[op_insert("abcd")]);
}

#[test]
fn builder_merges_adjacent_deletes() {
    let op = operation([op_delete("ab"), op_delete("cd")]);
    assert_ops(&op, &[op_delete("abcd")]);
}

#[test]
fn builder_drops_empty_insert() {
    let op = operation([op_insert(""), op_retain(3)]);
    assert_ops(&op, &[op_retain(3)]);
}

#[test]
fn builder_drops_empty_delete() {
    let op = operation([op_delete(""), op_retain(3)]);
    assert_ops(&op, &[op_retain(3)]);
}

#[test]
fn builder_drops_empty_retain() {
    let op = operation([op_retain(0), op_insert("x")]);
    assert_ops(&op, &[op_insert("x")]);
}

#[test]
fn apply_helper_handles_insert_delete_and_retain() {
    let op = operation([op_retain(1), op_delete("b"), op_insert("XY"), op_retain(1)]);
    assert_eq!(apply_operation("abc", &op), "aXYc");
}

#[test]
fn compose_retain_then_retain() {
    assert_compose_case(
        "abcd",
        operation([op_retain(4)]),
        operation([op_retain(4)]),
        &[op_retain(4)],
        "abcd",
    );
}

#[test]
fn compose_insert_then_retain() {
    assert_compose_case(
        "",
        operation([op_insert("abc")]),
        operation([op_retain(3)]),
        &[op_insert("abc")],
        "abc",
    );
}

#[test]
fn compose_insert_then_delete_same_text() {
    assert_compose_case(
        "",
        operation([op_insert("abc")]),
        operation([op_delete("abc")]),
        &[],
        "",
    );
}

#[test]
fn compose_delete_then_retain() {
    assert_compose_case(
        "abcd",
        operation([op_delete("ab"), op_retain(2)]),
        operation([op_retain(2)]),
        &[op_delete("ab"), op_retain(2)],
        "cd",
    );
}

#[test]
fn compose_retain_then_delete() {
    assert_compose_case(
        "abcd",
        operation([op_retain(4)]),
        operation([op_delete("ab"), op_retain(2)]),
        &[op_delete("ab"), op_retain(2)],
        "cd",
    );
}

#[test]
fn compose_insert_before_retain_position() {
    assert_compose_case(
        "ab",
        operation([op_insert("X"), op_retain(2)]),
        operation([op_insert("Y"), op_retain(3)]),
        &[op_insert("YX"), op_retain(2)],
        "YXab",
    );
}

#[test]
fn compose_insert_after_retain_position() {
    assert_compose_case(
        "ab",
        operation([op_retain(2), op_insert("X")]),
        operation([op_retain(2), op_insert("Y"), op_retain(1)]),
        &[op_retain(2), op_insert("YX")],
        "abYX",
    );
}

#[test]
fn compose_delete_and_insert_same_spot() {
    assert_compose_case(
        "abcd",
        operation([op_delete("ab"), op_retain(2)]),
        operation([op_insert("XY"), op_retain(2)]),
        &[op_insert("XY"), op_delete("ab"), op_retain(2)],
        "XYcd",
    );
}

#[test]
fn compose_replace_middle() {
    assert_compose_case(
        "abcd",
        operation([op_retain(1), op_delete("bc"), op_insert("XY"), op_retain(1)]),
        operation([op_retain(2), op_insert("!"), op_retain(2)]),
        &[
            op_retain(1),
            op_delete("bc"),
            op_insert("X!Y"),
            op_retain(1),
        ],
        "aX!Yd",
    );
}

#[test]
fn compose_split_insert_consumed_by_delete() {
    assert_compose_case(
        "ab",
        operation([op_retain(1), op_insert("XYZ"), op_retain(1)]),
        operation([op_retain(2), op_delete("YZ"), op_retain(1)]),
        &[op_retain(1), op_insert("X"), op_retain(1)],
        "aXb",
    );
}

#[test]
fn compose_delete_prefix_then_insert_prefix() {
    assert_compose_case(
        "abcdef",
        operation([op_delete("ab"), op_retain(4)]),
        operation([op_insert("!"), op_retain(4)]),
        &[op_insert("!"), op_delete("ab"), op_retain(4)],
        "!cdef",
    );
}

#[test]
fn compose_delete_suffix_then_insert_suffix() {
    assert_compose_case(
        "abcdef",
        operation([op_retain(4), op_delete("ef")]),
        operation([op_retain(4), op_insert("!")]),
        &[op_retain(4), op_insert("!"), op_delete("ef")],
        "abcd!",
    );
}

#[test]
fn compose_insert_then_delete_split_in_two_ops() {
    assert_compose_case(
        "ab",
        operation([op_retain(1), op_insert("XYZ"), op_retain(1)]),
        operation([op_retain(1), op_delete("X"), op_delete("YZ"), op_retain(1)]),
        &[op_retain(2)],
        "ab",
    );
}

#[test]
fn compose_unicode_insert_and_delete() {
    assert_compose_case(
        "",
        operation([op_insert("åßç")]),
        operation([op_delete("å"), op_retain("ßç".len() as u32)]),
        &[op_insert("ßç")],
        "ßç",
    );
}

#[test]
fn compose_identity_left() {
    let op = operation([op_retain(3), op_insert("x")]);
    assert_eq!(Operation::new().compose(&op), op);
}

#[test]
fn compose_identity_right() {
    let op = operation([op_retain(3), op_insert("x")]);
    assert_eq!(op.compose(&Operation::new()), op);
}

#[test]
fn compose_large_insert_keeps_result_lengths() {
    let insert = "abc".repeat(200);
    let op = operation([op_insert(&insert)]);
    let composed = op.compose(&operation([op_retain(insert.len() as u32)]));
    assert_eq!(composed.new_len(), insert.len() as u32);
    assert_eq!(apply_operation("", &composed), insert);
}

#[test]
fn compose_mixed_long_case_one() {
    assert_compose_case(
        "abcdefghi",
        operation([
            op_retain(2),
            op_delete("cd"),
            op_insert("WXYZ"),
            op_retain(5),
        ]),
        operation([op_retain(4), op_delete("Y"), op_insert("!"), op_retain(6)]),
        &[
            op_retain(2),
            op_delete("cd"),
            op_insert("WX!Z"),
            op_retain(5),
        ],
        "abWX!Zefghi",
    );
}

#[test]
fn compose_mixed_long_case_two() {
    assert_compose_case(
        "abcdef",
        operation([op_insert(">"), op_retain(6), op_insert("<")]),
        operation([
            op_retain(1),
            op_delete("ab"),
            op_retain(4),
            op_insert("!"),
            op_retain(1),
        ]),
        &[
            op_insert(">"),
            op_delete("ab"),
            op_retain(4),
            op_insert("!<"),
        ],
        ">cdef!<",
    );
}

#[test]
fn transform_retain_against_retain() {
    assert_transform_pair("abcd", operation([op_retain(4)]), operation([op_retain(4)]));
}

#[test]
fn transform_insert_against_retain() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(2), op_insert("X"), op_retain(2)]),
        operation([op_retain(4)]),
    );
}

#[test]
fn transform_retain_against_insert() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(4)]),
        operation([op_retain(2), op_insert("X"), op_retain(2)]),
    );
}

#[test]
fn transform_insert_against_insert_same_spot() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(2), op_insert("L"), op_retain(2)]),
        operation([op_retain(2), op_insert("R"), op_retain(2)]),
    );
}

#[test]
fn transform_insert_against_insert_start() {
    assert_transform_pair(
        "abcd",
        operation([op_insert("L"), op_retain(4)]),
        operation([op_insert("R"), op_retain(4)]),
    );
}

#[test]
fn transform_insert_against_insert_end() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(4), op_insert("L")]),
        operation([op_retain(4), op_insert("R")]),
    );
}

#[test]
fn transform_delete_against_retain() {
    assert_transform_pair(
        "abcd",
        operation([op_delete("ab"), op_retain(2)]),
        operation([op_retain(4)]),
    );
}

#[test]
fn transform_retain_against_delete() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(4)]),
        operation([op_delete("ab"), op_retain(2)]),
    );
}

#[test]
fn transform_delete_against_delete_same_range() {
    assert_transform_pair(
        "abcd",
        operation([op_delete("ab"), op_retain(2)]),
        operation([op_delete("ab"), op_retain(2)]),
    );
}

#[test]
fn transform_delete_against_delete_overlap_left() {
    assert_transform_pair(
        "abcdef",
        operation([op_delete("abc"), op_retain(3)]),
        operation([op_retain(1), op_delete("bcd"), op_retain(2)]),
    );
}

#[test]
fn transform_delete_against_delete_overlap_right() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(1), op_delete("bcd"), op_retain(2)]),
        operation([op_delete("abc"), op_retain(3)]),
    );
}

#[test]
fn transform_delete_against_insert() {
    assert_transform_pair(
        "abcd",
        operation([op_delete("ab"), op_retain(2)]),
        operation([op_retain(2), op_insert("X"), op_retain(2)]),
    );
}

#[test]
fn transform_insert_against_delete() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(2), op_insert("X"), op_retain(2)]),
        operation([op_delete("ab"), op_retain(2)]),
    );
}

#[test]
fn transform_replace_against_replace_same_range() {
    assert_transform_pair(
        "abcd",
        operation([op_retain(1), op_delete("bc"), op_insert("XY"), op_retain(1)]),
        operation([op_retain(1), op_delete("bc"), op_insert("12"), op_retain(1)]),
    );
}

#[test]
fn transform_replace_against_replace_disjoint() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(1), op_delete("b"), op_insert("X"), op_retain(4)]),
        operation([op_retain(4), op_delete("e"), op_insert("Y"), op_retain(1)]),
    );
}

#[test]
fn transform_unicode_insert_against_delete() {
    assert_transform_pair(
        "åßç∂",
        operation([
            op_retain("å".len() as u32),
            op_insert("λ"),
            op_retain("ßç∂".len() as u32),
        ]),
        operation([
            op_retain("åß".len() as u32),
            op_delete("ç"),
            op_retain("∂".len() as u32),
        ]),
    );
}

#[test]
fn transform_unicode_delete_against_insert() {
    assert_transform_pair(
        "åßç∂",
        operation([
            op_retain("åß".len() as u32),
            op_delete("ç"),
            op_retain("∂".len() as u32),
        ]),
        operation([
            op_retain("å".len() as u32),
            op_insert("λ"),
            op_retain("ßç∂".len() as u32),
        ]),
    );
}

#[test]
fn transform_prefix_delete_against_suffix_insert() {
    assert_transform_pair(
        "abcdef",
        operation([op_delete("ab"), op_retain(4)]),
        operation([op_retain(6), op_insert("!")]),
    );
}

#[test]
fn transform_suffix_delete_against_prefix_insert() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(4), op_delete("ef")]),
        operation([op_insert("!"), op_retain(6)]),
    );
}

#[test]
fn transform_middle_insert_against_middle_insert() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(3), op_insert("L"), op_retain(3)]),
        operation([op_retain(3), op_insert("R"), op_retain(3)]),
    );
}

#[test]
fn transform_middle_delete_against_middle_delete() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(2), op_delete("cd"), op_retain(2)]),
        operation([op_retain(2), op_delete("cd"), op_retain(2)]),
    );
}

#[test]
fn transform_insert_inside_deleted_range() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(3), op_insert("!"), op_retain(3)]),
        operation([op_retain(2), op_delete("cd"), op_retain(2)]),
    );
}

#[test]
fn transform_delete_around_insert_point() {
    assert_transform_pair(
        "abcdef",
        operation([op_retain(1), op_delete("bcde"), op_retain(1)]),
        operation([op_retain(3), op_insert("!"), op_retain(3)]),
    );
}

#[test]
fn transform_large_insertions_commute_via_primes() {
    let left_text = "L".repeat(120);
    let right_text = "R".repeat(80);
    assert_transform_pair(
        "abcdef",
        operation([op_retain(2), op_insert(&left_text), op_retain(4)]),
        operation([op_retain(2), op_insert(&right_text), op_retain(4)]),
    );
}

#[test]
fn transform_large_deletions_commute_via_primes() {
    let base = "a".repeat(200);
    let left = operation([op_retain(20), op_delete(&"a".repeat(100)), op_retain(80)]);
    let right = operation([op_retain(40), op_delete(&"a".repeat(80)), op_retain(80)]);
    assert_transform_pair(&base, left, right);
}

#[test]
fn transform_identity_left() {
    let op = operation([op_retain(3), op_insert("x")]);
    assert_eq!(Operation::new().transform(&op), Operation::new());
}

#[test]
fn transform_identity_right() {
    let op = operation([op_retain(3), op_insert("x")]);
    assert_eq!(op.transform(&Operation::new()), op);
}

#[test]
fn transform_result_lengths_match_other_output() {
    let left = operation([op_retain(2), op_insert("xy"), op_retain(2)]);
    let right = operation([op_retain(1), op_delete("b"), op_insert("!"), op_retain(2)]);
    let transformed = left.transform(&right);
    assert_eq!(transformed.old_len(), right.new_len());
}

#[test]
fn transform_result_stays_normalized() {
    let transformed = operation([op_insert("a")]).transform(&operation([op_insert("b")]));
    assert_ops(&transformed, &[op_insert("a"), op_retain(1)]);
}

#[test]
fn ops_iteration_preserves_order() {
    let op = operation([op_retain(1), op_insert("x"), op_delete("y"), op_retain(2)]);
    let ops: Vec<_> = op.iter().collect();
    assert_eq!(
        ops,
        vec![op_retain(1), op_insert("x"), op_delete("y"), op_retain(2)]
    );
}

#[test]
fn compose_with_many_small_ops_normalizes_output() {
    let first = operation([op_retain(1), op_insert("a"), op_insert("b"), op_retain(1)]);
    let second = operation([op_retain(1), op_retain(2), op_insert("!"), op_retain(1)]);
    let composed = first.compose(&second);
    assert_ops(&composed, &[op_retain(1), op_insert("ab!"), op_retain(1)]);
}

#[test]
fn transform_with_many_small_ops_normalizes_output() {
    let transformed = operation([op_retain(1), op_insert("a"), op_insert("b"), op_retain(1)])
        .transform(&operation([op_retain(1), op_insert("!"), op_retain(1)]));
    assert_ops(&transformed, &[op_retain(2), op_insert("ab"), op_retain(1)]);
}

#[test]
fn compose_and_transform_handle_newlines() {
    let base = "a\nb\nc\n";
    let left = operation([op_retain(2), op_insert("X\n"), op_retain(4)]);
    let right = operation([op_retain(4), op_delete("c"), op_insert("Y"), op_retain(1)]);
    assert_transform_pair(base, left.clone(), right.clone());
    let mid = apply_operation(base, &left);
    let end = apply_operation(&mid, &right.transform(&left));
    let composed = left.compose(&right.transform(&left));
    assert_eq!(apply_operation(base, &composed), end);
}

fn scramble(mut seed: u64) -> impl FnMut() -> u64 {
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    }
}

fn arbitrary_operation(rand: &mut impl FnMut() -> u64) -> Operation {
    let mut ops = Vec::new();
    let words = ["alpha", "béta—", "x", "😀y", "\n", "line\n"];
    for _ in 0..(rand() % 12 + 1) {
        match rand() % 3 {
            0 => ops.push(Op::Retain((rand() % 9 + 1) as u32)),
            1 => ops.push(Op::Insert(words[(rand() % 6) as usize].to_owned())),
            _ => ops.push(Op::Delete(words[(rand() % 6) as usize].to_owned())),
        }
    }
    Operation::from_ops(ops)
}

#[test]
fn invert_swaps_inserts_and_deletes_and_round_trips() {
    let operation = Operation::from_ops([
        Op::Retain(3),
        Op::Delete("old—".to_owned()),
        Op::Insert("new".to_owned()),
        Op::Retain(2),
    ]);
    let inverted = operation.invert();
    let ops: Vec<_> = inverted.iter().collect();
    assert_eq!(
        ops,
        vec![
            Op::Retain(3),
            Op::Insert("old—".to_owned()),
            Op::Delete("new".to_owned()),
            Op::Retain(2),
        ]
    );

    let twice: Vec<_> = inverted.invert().iter().collect();
    assert_eq!(twice, operation.iter().collect::<Vec<_>>());
}

#[test]
fn transform_offset_back_mirrors_the_inverted_forward_mapping() {
    let mut rand = scramble(0x2545f4914f6cdd1d);
    for _ in 0..300 {
        let operation = arbitrary_operation(&mut rand);
        let inverted = operation.invert();
        let new_len: u32 = operation.iter().map(|op| op.new_len()).sum();
        for probe in 0..=new_len.min(64) {
            for bias in [Bias::Left, Bias::Right] {
                assert_eq!(
                    operation.transform_offset_back(probe, bias),

                    inverted.transform_offset(probe, bias),
                    "op {:?} probe {probe} bias {bias:?}",
                    operation.iter().collect::<Vec<_>>(),
                );
            }
        }
    }
}

fn linear_transform_offset(operation: &Operation, offset: u32, bias: Bias) -> u32 {
    let mut old_offset = 0u32;
    let mut transformed = offset;
    for op in operation.iter() {
        match op {
            Op::Retain(len) => old_offset = old_offset.saturating_add(len),
            Op::Insert(text) => {
                let shifts = match bias {
                    Bias::Left => old_offset < offset,
                    Bias::Right => old_offset <= offset,
                };
                if shifts {
                    transformed = transformed.saturating_add(text.len() as u32);
                }
            }
            Op::Delete(text) => {
                let len = text.len() as u32;
                if old_offset.saturating_add(len) <= offset {
                    transformed = transformed.saturating_sub(len);
                } else if old_offset < offset {
                    transformed = transformed.saturating_sub(offset - old_offset);
                }
                old_offset = old_offset.saturating_add(len);
            }
        }
        let past = match bias {
            Bias::Left => old_offset >= offset,
            Bias::Right => old_offset > offset,
        };
        if past {
            break;
        }
    }
    transformed
}

#[test]
fn seek_based_transform_offset_matches_the_linear_reference() {
    let mut rand = scramble(0x853c49e6748fea9b);
    for _ in 0..300 {
        let operation = arbitrary_operation(&mut rand);
        let old_len: u32 = operation.iter().map(|op| op.old_len()).sum();
        let _ = old_len;
        let inverted = operation.invert();
        let new_len: u32 = operation.iter().map(|op| op.new_len()).sum();
        for probe in 0..=new_len.min(64) {
            for bias in [Bias::Left, Bias::Right] {
                assert_eq!(
                    operation.transform_offset_back(probe, bias),
                    linear_transform_offset(&inverted, probe, bias),
                    "op {:?} probe {probe} bias {bias:?}",
                    operation.iter().collect::<Vec<_>>(),
                );
            }
        }
    }
}

fn arbitrary_operation_over(source: &str, rand: &mut impl FnMut() -> u64) -> Operation {
    let words = ["alpha", "béta—", "x", "😀y", "\n"];
    let boundaries: Vec<usize> = source
        .char_indices()
        .map(|(index, _)| index)
        .chain([source.len()])
        .collect();
    let mut ops = Vec::new();
    let mut at = 0usize;
    while at < boundaries.len() - 1 {
        let step = ((rand() % 3) + 1) as usize;
        let next = (at + step).min(boundaries.len() - 1);
        let (from, to) = (boundaries[at], boundaries[next]);
        match rand() % 4 {
            0 | 1 => ops.push(Op::Retain((to - from) as u32)),
            2 => ops.push(Op::Delete(source[from..to].to_owned())),
            _ => {
                ops.push(Op::Insert(words[(rand() % 5) as usize].to_owned()));
                ops.push(Op::Retain((to - from) as u32));
            }
        }
        at = next;
    }
    if rand() % 3 == 0 {
        ops.push(Op::Insert(words[(rand() % 5) as usize].to_owned()));
    }
    Operation::from_ops(ops)
}

fn arbitrary_local_edit(source: &str, rand: &mut impl FnMut() -> u64) -> Operation {
    let words = ["tail", "μμ", "z😀", ""];
    let boundaries: Vec<usize> = source
        .char_indices()
        .map(|(index, _)| index)
        .chain([source.len()])
        .collect();
    let a = (rand() as usize) % boundaries.len();
    let b = (rand() as usize) % boundaries.len();
    let (from, to) = (boundaries[a.min(b)], boundaries[a.max(b)]);
    let mut ops = Vec::new();
    if from > 0 {
        ops.push(Op::Retain(from as u32));
    }
    if to > from {
        ops.push(Op::Delete(source[from..to].to_owned()));
    }
    let word = words[(rand() % 4) as usize];
    if !word.is_empty() {
        ops.push(Op::Insert(word.to_owned()));
    }
    if source.len() > to {
        ops.push(Op::Retain((source.len() - to) as u32));
    }
    Operation::from_ops(ops)
}

#[test]
fn splice_compose_matches_the_full_compose() {
    let mut rand = scramble(0x51ce_c0de_d00d_5eed);
    let sources = [
        "",
        "alpha béta gamma\nline two 😀 tail\nmore μονή text\n",
        "x",
        "😀😀😀\n",
    ];
    for round in 0..4000 {
        let source = sources[(rand() % 4) as usize];
        let base = arbitrary_operation_over(source, &mut rand);
        let mid = apply_operation(source, &base);
        let small = arbitrary_local_edit(&mid, &mut rand);
        let full = base.compose(&small);
        let spliced = base.splice_compose(&small);
        assert_eq!(spliced.old_len(), full.old_len(), "round {round}");
        assert_eq!(spliced.new_len(), full.new_len(), "round {round}");
        assert_eq!(
            apply_operation(source, &spliced),
            apply_operation(source, &full),
            "round {round}"
        );

        let small = arbitrary_local_edit(source, &mut rand);
        let mid = apply_operation(source, &small);
        let big = arbitrary_operation_over(&mid, &mut rand);
        let full = small.compose(&big);
        let spliced = small.splice_compose_into(&big);
        assert_eq!(spliced.old_len(), full.old_len(), "round {round}");
        assert_eq!(spliced.new_len(), full.new_len(), "round {round}");
        assert_eq!(
            apply_operation(source, &spliced),
            apply_operation(source, &full),
            "round {round}"
        );
    }
}
