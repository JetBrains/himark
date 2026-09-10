use std::sync::Arc;

use crate::node::{Node, BRANCHING_FACTOR, LEAVES_CAPACITY};
use crate::{Bias, Dimension, Item, Seek, Splice, SumTree, Summary};

#[derive(Clone, Debug, PartialEq)]
struct Span {
    width: u32,
    bytes: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct SpanSummary {
    count: usize,
    bytes: u64,
    max_width: u32,
}

impl Summary for SpanSummary {
    fn empty() -> Self {
        Self {
            count: 0,
            bytes: 0,
            max_width: 0,
        }
    }

    fn add(&mut self, other: &Self) {
        self.count += other.count;
        self.bytes += other.bytes;
        self.max_width = self.max_width.max(other.max_width);
    }
}

impl Item for Span {
    type Summary = SpanSummary;

    fn summary(&self) -> SpanSummary {
        SpanSummary {
            count: 1,
            bytes: u64::from(self.bytes),
            max_width: self.width,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
struct Bytes(u64);

impl Dimension<SpanSummary> for Bytes {
    fn from_summary(summary: &SpanSummary) -> Self {
        Self(summary.bytes)
    }

    fn add(&mut self, other: Self) {
        self.0 += other.0;
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        match bound {
            0 => 0,
            _ => (self.next() % bound as u64) as usize,
        }
    }
}

fn spans(rng: &mut Rng, count: usize) -> Vec<Span> {
    (0..count)
        .map(|_| Span {
            width: rng.below(10_000) as u32,
            bytes: rng.below(200) as u32,
        })
        .collect()
}

fn naive_summary(model: &[Span]) -> SpanSummary {
    let mut out = SpanSummary::empty();
    for span in model {
        out.add(&span.summary());
    }
    out
}

fn assert_matches_model(tree: &SumTree<Span>, model: &[Span]) {
    assert_eq!(tree.len(), model.len());
    let collected: Vec<Span> = tree.iter().cloned().collect();
    assert_eq!(collected, model);
    assert_eq!(*tree.summary(), naive_summary(model));
    assert_invariants(tree);
}

fn assert_invariants(tree: &SumTree<Span>) {
    fn check(node: &Node<Span>, is_root: bool) -> (usize, SpanSummary, u8) {
        match node {
            Node::Leaf(items) => {
                assert!(is_root || !items.is_empty(), "non-root leaf is empty");
                assert!(items.len() <= LEAVES_CAPACITY, "leaf exceeds capacity");
                (items.len(), naive_summary(items), 0)
            }
            Node::Internal { height, children } => {
                assert!(children.len() > 1 || !is_root, "root chain not unwrapped");
                assert!(!children.is_empty(), "internal node is empty");
                assert!(
                    children.len() <= BRANCHING_FACTOR,
                    "internal node exceeds branching factor"
                );
                let mut len = 0;
                let mut summary = SpanSummary::empty();
                for child in children {
                    let (child_len, child_summary, child_height) = check(&child.node, false);
                    assert_eq!(child.weight.len, child_len, "stale child length");
                    assert_eq!(child.weight.summary, child_summary, "stale child summary");
                    assert_eq!(child_height + 1, *height, "uneven heights");
                    len += child_len;
                    summary.add(&child_summary);
                }
                (len, summary, *height)
            }
        }
    }
    let (len, summary, _) = check(root(tree), true);
    assert_eq!(tree.len(), len);
    assert_eq!(*tree.summary(), summary);
}

fn root(tree: &SumTree<Span>) -> &Node<Span> {
    tree.root_for_tests()
}

#[test]
fn builds_match_the_model_across_sizes() {
    let mut rng = Rng(7);
    for count in [0, 1, 2, 63, 64, 65, 129, 1_000, 4_097, 20_000] {
        let model = spans(&mut rng, count);
        let tree: SumTree<Span> = model.iter().cloned().collect();
        assert_matches_model(&tree, &model);
        for _ in 0..50 {
            let index = rng.below(count.max(1));
            assert_eq!(tree.get(index), model.get(index));
        }
        assert_eq!(tree.get(count), None);
    }
}

#[test]
fn range_summaries_match_a_naive_fold() {
    let mut rng = Rng(11);
    for count in [1, 64, 65, 1_000, 20_000] {
        let model = spans(&mut rng, count);
        let tree: SumTree<Span> = model.iter().cloned().collect();
        for _ in 0..200 {
            let a = rng.below(count + 1);
            let b = rng.below(count + 1);
            let range = a.min(b)..a.max(b);
            assert_eq!(
                tree.summary_in(range.clone()),
                naive_summary(&model[range]),
                "count {count}"
            );
        }
        assert_eq!(tree.summary_in(0..count), naive_summary(&model));
        assert_eq!(tree.summary_in(3..3), SpanSummary::empty());
        assert_eq!(tree.summary_in(0..usize::MAX), naive_summary(&model));
    }
}

#[test]
fn dimension_summaries_match_the_intersection_rule() {
    let mut rng = Rng(13);
    for count in [1, 100, 5_000] {
        let model = spans(&mut rng, count);
        let tree: SumTree<Span> = model.iter().cloned().collect();
        let total: u64 = model.iter().map(|span| u64::from(span.bytes)).sum();
        for _ in 0..300 {
            let a = (rng.next() % (total + 2)).saturating_sub(1);
            let b = rng.next() % (total + 2);
            let (a, b) = (a.min(b), a.max(b));

            let mut expected = SpanSummary::empty();
            let mut offset = 0u64;
            for span in &model {
                let end = offset + u64::from(span.bytes);
                let intersects = offset < b && end > a;
                let zero_inside = offset == end && a < offset && offset < b;
                if intersects || zero_inside {
                    expected.add(&span.summary());
                }
                offset = end;
            }

            assert_eq!(
                tree.summary_between(Bytes(a)..Bytes(b)),
                expected,
                "count {count} range {a}..{b}"
            );
        }
    }
}

#[test]
fn finds_land_on_the_biased_side_of_boundaries() {
    let spans = |bytes: &[u32]| -> SumTree<Span> {
        bytes
            .iter()
            .map(|bytes| Span {
                width: 1,
                bytes: *bytes,
            })
            .collect()
    };

    let tree = spans(&[3, 0, 2]);

    assert_eq!(
        tree.find(Bytes(0), Bias::Right),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );
    assert_eq!(
        tree.find(Bytes(0), Bias::Left),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );
    assert_eq!(
        tree.find(Bytes(1), Bias::Right),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );
    assert_eq!(
        tree.find(Bytes(1), Bias::Left),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );

    assert_eq!(
        tree.find(Bytes(3), Bias::Left),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );
    assert_eq!(
        tree.find(Bytes(3), Bias::Right),
        Seek {
            index: 2,
            start: Bytes(3)
        }
    );
    assert_eq!(
        tree.find(Bytes(4), Bias::Right),
        Seek {
            index: 2,
            start: Bytes(3)
        }
    );

    assert_eq!(
        tree.find(Bytes(5), Bias::Left),
        Seek {
            index: 2,
            start: Bytes(3)
        }
    );
    assert_eq!(
        tree.find(Bytes(5), Bias::Right),
        Seek {
            index: 3,
            start: Bytes(5)
        }
    );
    assert_eq!(
        tree.find(Bytes(9), Bias::Right),
        Seek {
            index: 3,
            start: Bytes(5)
        }
    );

    let empty = SumTree::<Span>::new();
    assert_eq!(
        empty.find(Bytes(0), Bias::Right),
        Seek {
            index: 0,
            start: Bytes(0)
        }
    );

    assert_eq!(tree.offset_of::<Bytes>(0), Bytes(0));
    assert_eq!(tree.offset_of::<Bytes>(2), Bytes(3));
    assert_eq!(tree.offset_of::<Bytes>(3), Bytes(5));
}

fn random_splices(rng: &mut Rng, len: usize, count: usize) -> Vec<Splice<Span>> {
    let mut cuts: Vec<usize> = (0..count * 2).map(|_| rng.below(len + 1)).collect();
    cuts.sort_unstable();
    let mut edits = Vec::new();
    for pair in cuts.chunks(2) {
        let range = pair[0]..pair[1];
        if let Some(last) = edits.last() {
            let last: &Splice<Span> = last;
            if range.start < last.range.end {
                continue;
            }
        }
        edits.push(Splice {
            range,
            insert: {
                let count = rng.below(40);
                spans(rng, count)
            },
        });
    }
    edits
}

fn apply_to_model(model: &[Span], edits: &[Splice<Span>]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for edit in edits {
        out.extend_from_slice(&model[cursor..edit.range.start]);
        out.extend_from_slice(&edit.insert);
        cursor = edit.range.end;
    }
    out.extend_from_slice(&model[cursor..]);
    out
}

#[test]
fn bulk_splices_match_the_model() {
    let mut rng = Rng(17);
    for count in [0, 1, 64, 1_000, 8_000] {
        for _ in 0..20 {
            let model = spans(&mut rng, count);
            let tree: SumTree<Span> = model.iter().cloned().collect();
            let splice_count = 1 + rng.below(8);
            let edits = random_splices(&mut rng, count, splice_count);
            let spliced_model = apply_to_model(&model, &edits);
            let spliced = tree.splice(edits.iter().map(|edit| Splice {
                range: edit.range.clone(),
                insert: edit.insert.clone(),
            }));
            assert_matches_model(&spliced, &spliced_model);

            assert_matches_model(&tree, &model);
        }
    }
}

#[test]
fn splice_edges_hold() {
    let mut rng = Rng(19);
    let model = spans(&mut rng, 500);
    let tree: SumTree<Span> = model.iter().cloned().collect();

    let emptied = tree.splice([Splice {
        range: 0..500,
        insert: Vec::new(),
    }]);
    assert_matches_model(&emptied, &[]);

    let seeded = emptied.splice([Splice {
        range: 0..0,
        insert: model.clone(),
    }]);
    assert_matches_model(&seeded, &model);

    let touched = tree.splice([
        Splice {
            range: 0..0,
            insert: spans(&mut rng, 3),
        },
        Splice {
            range: 0..250,
            insert: spans(&mut rng, 1),
        },
        Splice {
            range: 250..500,
            insert: spans(&mut rng, 90),
        },
        Splice {
            range: 500..500,
            insert: spans(&mut rng, 70),
        },
    ]);
    assert_eq!(touched.len(), 3 + 1 + 90 + 70);
    assert_invariants(&touched);
    assert_matches_model(&tree.splice([]), &model);
}

#[test]
fn sets_match_the_model_and_share_structure() {
    let mut rng = Rng(23);
    let mut model = spans(&mut rng, 20_000);
    let mut tree: SumTree<Span> = model.iter().cloned().collect();
    let before = tree.clone();
    let before_model = model.clone();

    for _ in 0..100 {
        let index = rng.below(model.len());
        let span = Span {
            width: rng.below(10_000) as u32,
            bytes: rng.below(200) as u32,
        };
        model[index] = span.clone();
        tree = tree.set(index, span);
    }
    assert_matches_model(&tree, &model);
    assert_matches_model(&before, &before_model);

    let one = before.set(10_000, Span { width: 1, bytes: 1 });
    let leaves = |tree: &SumTree<Span>| -> Vec<*const Node<Span>> {
        fn walk(node: &Arc<Node<Span>>, out: &mut Vec<*const Node<Span>>) {
            match node.as_ref() {
                Node::Leaf(_) => out.push(Arc::as_ptr(node)),
                Node::Internal { children, .. } => {
                    for child in children {
                        walk(&child.node, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(tree.root_arc_for_tests(), &mut out);
        out
    };
    let old = leaves(&before);
    let new = leaves(&one);
    let shared = new.iter().filter(|leaf| old.contains(leaf)).count();
    assert!(
        shared * 100 >= new.len() * 99,
        "a point set must share almost every leaf: {shared}/{}",
        new.len()
    );
}

#[test]
fn slices_and_appends_match_the_model() {
    let mut rng = Rng(29);
    let model = spans(&mut rng, 10_000);
    let tree: SumTree<Span> = model.iter().cloned().collect();
    for _ in 0..40 {
        let a = rng.below(model.len() + 1);
        let b = rng.below(model.len() + 1);
        let range = a.min(b)..a.max(b);
        let slice = tree.slice(range.clone());
        assert_matches_model(&slice, &model[range.clone()]);

        let rest = tree.slice(range.end..model.len());
        let glued = tree.slice(0..range.start).append(&slice).append(&rest);
        assert_matches_model(&glued, &model);
    }
    assert_matches_model(&tree.slice(0..0), &[]);
    assert_matches_model(&SumTree::<Span>::new().append(&tree), &model);
    assert_matches_model(&tree.append(&SumTree::new()), &model);
}

#[test]
fn heavy_churn_stays_balanced() {
    let mut rng = Rng(31);
    let mut model = spans(&mut rng, 5_000);
    let mut tree: SumTree<Span> = model.iter().cloned().collect();

    for round in 0..300 {
        let splice_count = 1 + rng.below(5);
        let edits = random_splices(&mut rng, model.len(), splice_count);
        model = apply_to_model(&model, &edits);
        tree = tree.splice(edits);
        if round % 50 == 0 {
            assert_matches_model(&tree, &model);
        }
        let height = tree.root_for_tests().height() as u32;
        let bound = usize::BITS - tree.len().max(2).leading_zeros();
        assert!(
            height <= bound.div_ceil(2) + 2,
            "height {height} degenerated for {} elements",
            tree.len()
        );
    }
    assert_matches_model(&tree, &model);
}

#[test]
fn versions_stay_independent() {
    let mut rng = Rng(37);
    let mut model = spans(&mut rng, 2_000);
    let mut tree: SumTree<Span> = model.iter().cloned().collect();
    let mut versions = vec![(tree.clone(), model.clone())];

    for _ in 0..10 {
        let edits = random_splices(&mut rng, model.len(), 3);
        model = apply_to_model(&model, &edits);
        tree = tree.splice(edits);
        versions.push((tree.clone(), model.clone()));
    }
    for (tree, model) in &versions {
        assert_matches_model(tree, model);
    }
}

#[test]
fn float_summaries_and_dimensions_work() {
    #[derive(Clone, Debug)]
    struct Line(f32);
    #[derive(Clone, Debug, PartialEq)]
    struct MaxWidth {
        width_sum: f32,
        max: f32,
    }
    impl Summary for MaxWidth {
        fn empty() -> Self {
            Self {
                width_sum: 0.0,
                max: 0.0,
            }
        }
        fn add(&mut self, other: &Self) {
            self.width_sum += other.width_sum;
            self.max = self.max.max(other.max);
        }
    }
    impl Item for Line {
        type Summary = MaxWidth;
        fn summary(&self) -> MaxWidth {
            MaxWidth {
                width_sum: self.0,
                max: self.0,
            }
        }
    }
    #[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
    struct Widths(f32);
    impl Dimension<MaxWidth> for Widths {
        fn from_summary(summary: &MaxWidth) -> Self {
            Self(summary.width_sum)
        }
        fn add(&mut self, other: Self) {
            self.0 += other.0;
        }
    }

    let tree: SumTree<Line> = (1..=1_000).map(|i| Line(i as f32)).collect();
    assert_eq!(tree.summary().max, 1_000.0);
    assert_eq!(tree.summary_in(0..500).max, 500.0);
    assert_eq!(tree.summary_in(499..500).max, 500.0);
    let seek = tree.find(Widths(1.5), Bias::Right);
    assert_eq!(seek.index, 1);
    let widest = tree.set(3, Line(9_999.0));
    assert_eq!(widest.summary().max, 9_999.0);
    assert_eq!(tree.summary().max, 1_000.0);
}

#[test]
fn range_iteration_matches_the_model() {
    let mut rng = Rng(43);
    for count in [0, 1, 64, 1_000, 20_000] {
        let model = spans(&mut rng, count);
        let tree: SumTree<Span> = model.iter().cloned().collect();
        for _ in 0..50 {
            let a = rng.below(count + 1);
            let b = rng.below(count + 1);
            let range = a.min(b)..a.max(b);
            let walked: Vec<Span> = tree.iter_in(range.clone()).cloned().collect();
            assert_eq!(walked, &model[range], "count {count}");
        }
        let clamped: Vec<Span> = tree.iter_in(0..usize::MAX).cloned().collect();
        assert_eq!(clamped, model);
    }
}

#[test]
fn trees_are_send_and_sync() {
    fn assert_send_sync<V: Send + Sync>() {}
    assert_send_sync::<SumTree<Span>>();

    let tree: SumTree<Span> = spans(&mut Rng(47), 10_000).iter().cloned().collect();
    let snapshot = tree.clone();
    let handle = std::thread::spawn(move || snapshot.summary_in(100..9_000).max_width);
    let here = tree.summary_in(100..9_000).max_width;
    assert_eq!(handle.join().expect("thread"), here);
}

#[test]
fn mixed_operation_fuzz_holds_the_line() {
    let mut rng = Rng(53);
    let mut model = spans(&mut rng, 1_000);
    let mut tree: SumTree<Span> = model.iter().cloned().collect();

    for round in 0..400 {
        match rng.below(5) {
            0 => {
                let splice_count = 1 + rng.below(6);
                let edits = random_splices(&mut rng, model.len(), splice_count);
                model = apply_to_model(&model, &edits);
                tree = tree.splice(edits);
            }
            1 if !model.is_empty() => {
                let index = rng.below(model.len());
                let span = Span {
                    width: rng.below(10_000) as u32,
                    bytes: rng.below(200) as u32,
                };
                model[index] = span.clone();
                tree = tree.set(index, span);
            }
            2 => {
                let a = rng.below(model.len() + 1);
                let b = rng.below(model.len() + 1);
                let range = a.min(b)..a.max(b);
                model = model[range.clone()].to_vec();
                tree = tree.slice(range);
            }
            3 => {
                let extra_count = rng.below(300);
                let extra = spans(&mut rng, extra_count);
                let other: SumTree<Span> = extra.iter().cloned().collect();
                if rng.below(2) == 0 {
                    model.extend_from_slice(&extra);
                    tree = tree.append(&other);
                } else {
                    let mut joined = extra.clone();
                    joined.extend_from_slice(&model);
                    model = joined;
                    tree = other.append(&tree);
                }
            }
            _ => {
                let a = rng.below(model.len() + 1);
                let b = rng.below(model.len() + 1);
                let range = a.min(b)..a.max(b);
                assert_eq!(tree.summary_in(range.clone()), naive_summary(&model[range]));
            }
        }
        if round % 40 == 0 {
            assert_matches_model(&tree, &model);
        }
    }
    assert_matches_model(&tree, &model);
}

#[test]
fn degenerate_shapes_behave() {
    let zeros: Vec<Span> = (0..1_000)
        .map(|i| Span {
            width: i as u32,
            bytes: 0,
        })
        .collect();
    let tree: SumTree<Span> = zeros.iter().cloned().collect();
    assert_invariants(&tree);
    assert_eq!(
        tree.summary_between(Bytes(0)..Bytes(1)),
        SpanSummary::empty()
    );
    assert_eq!(tree.find(Bytes(0), Bias::Right).index, 1_000);
    assert_eq!(tree.find(Bytes(0), Bias::Left).index, 0);
    assert_eq!(tree.summary_in(0..1_000).max_width, 999);

    let mut rng = Rng(59);
    let small: SumTree<Span> = spans(&mut rng, 10).iter().cloned().collect();
    let big = spans(&mut rng, 120_000);
    let grown = small.splice([Splice {
        range: 5..5,
        insert: big.clone(),
    }]);
    assert_eq!(grown.len(), 120_010);
    assert_invariants(&grown);
    assert_eq!(grown.summary_in(5..120_005), naive_summary(&big));

    let flat: SumTree<Span> = (0..10_000).map(|_| Span { width: 7, bytes: 1 }).collect();
    assert_eq!(flat.summary_in(123..9_876).max_width, 7);
    assert_eq!(flat.summary_between(Bytes(50)..Bytes(51)).max_width, 7);
}

#[test]
#[ignore = "multi-second probe; run with --ignored --release"]
fn million_element_churn_probe() {
    let mut rng = Rng(61);
    let mut model = spans(&mut rng, 1_000_000);
    let mut tree: SumTree<Span> = model.iter().cloned().collect();
    assert_eq!(*tree.summary(), naive_summary(&model));

    for round in 0..500 {
        let splice_count = 1 + rng.below(20);
        let mut edits = Vec::new();
        let mut cursor = 0;
        for _ in 0..splice_count {
            let gap = rng.below(model.len().saturating_sub(cursor) / splice_count + 1);
            let start = (cursor + gap).min(model.len());
            let deleted = rng.below(50.min(model.len() - start + 1));
            let insert_count = rng.below(50);
            let insert = spans(&mut rng, insert_count);
            edits.push(Splice {
                range: start..start + deleted,
                insert,
            });
            cursor = start + deleted;
        }
        model = apply_to_model(&model, &edits);
        tree = tree.splice(edits);

        let a = rng.below(model.len() + 1);
        let b = rng.below(model.len() + 1);
        let range = a.min(b)..a.max(b);
        assert_eq!(
            tree.summary_in(range.clone()),
            naive_summary(&model[range]),
            "round {round}"
        );
        let height = tree.root_for_tests().height() as u32;
        let bound = usize::BITS - tree.len().max(2).leading_zeros();
        assert!(
            height <= bound.div_ceil(2) + 2,
            "height {height} degenerated"
        );

        if round % 100 == 99 {
            assert_matches_model(&tree, &model);
        }
    }
    assert_matches_model(&tree, &model);
}

fn record(bench: &str, metric: &str, value_ms: f64) {
    let mut dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(_) => return,
    };
    while !dir.join(".git").exists() {
        if !dir.pop() {
            return;
        }
    }
    let path = dir.join("perf.csv");
    let header = !path.exists();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let commit = std::fs::read_to_string(dir.join(".git/HEAD"))
        .ok()
        .and_then(|head| {
            let head = head.trim().to_owned();
            match head.strip_prefix("ref: ") {
                Some(reference) => std::fs::read_to_string(dir.join(".git").join(reference))
                    .ok()
                    .map(|hash| hash.trim().to_owned()),
                None => Some(head),
            }
        })
        .map(|full| full.chars().take(9).collect::<String>())
        .unwrap_or_else(|| "unknown".to_owned());
    let mut line = String::new();
    if header {
        line.push_str("timestamp,commit,profile,bench,metric,value_ms\n");
    }
    line.push_str(&format!(
        "{timestamp},{commit},{profile},{bench},{metric},{value_ms:.4}\n"
    ));
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[test]
fn sumtree_bench() {
    use std::time::Instant;
    let mut rng = Rng(41);
    let count = 200_000;
    let items = spans(&mut rng, count);

    let start = Instant::now();
    let tree: SumTree<Span> = items.iter().cloned().collect();
    record(
        "sumtree",
        "build200k_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );

    let start = Instant::now();
    let mut checksum = 0u64;
    for _ in 0..10_000 {
        let a = rng.below(count + 1);
        let b = rng.below(count + 1);
        checksum ^= u64::from(tree.summary_in(a.min(b)..a.max(b)).max_width);
    }
    record(
        "sumtree",
        "rmq10k_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    assert!(checksum != u64::MAX);

    let start = Instant::now();
    let edits: Vec<Splice<Span>> = (0..1_000)
        .map(|i| {
            let at = i * (count / 1_000);
            Splice {
                range: at..at + 1,
                insert: vec![Span { width: 7, bytes: 7 }],
            }
        })
        .collect();
    let spliced = tree.splice(edits);
    record(
        "sumtree",
        "splice1k_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    assert_eq!(spliced.len(), count);

    let start = Instant::now();
    let mut updated = tree.clone();
    for _ in 0..10_000 {
        let index = rng.below(count);
        updated = updated.set(index, Span { width: 3, bytes: 3 });
    }
    record(
        "sumtree",
        "set10k_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    assert_eq!(updated.len(), count);

    let start = Instant::now();
    let mut total = Bytes(0);
    for _ in 0..10_000 {
        let target = Bytes(rng.next() % (tree.summary().bytes + 1));
        total.add(tree.find(target, Bias::Right).start);
    }
    record(
        "sumtree",
        "find10k_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    assert!(total.0 != u64::MAX);
}
