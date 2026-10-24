// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! IntelliJ's `WordComparisonUtilTest` cases (the DEFAULT policy
//! ones), in its notation: `_` is a newline, and a mark line puts
//! `-` under every changed char of its text.

use operation::op::Op;

/// The changed chars of both texts under `ops`, as mark lines.
fn marks(old: &str, new: &str, ops: &[Op]) -> (String, String) {
    let mut changed_old = vec![false; old.len()];
    let mut changed_new = vec![false; new.len()];
    let (mut at1, mut at2) = (0usize, 0usize);
    for op in ops {
        match op {
            Op::Retain(len) => {
                at1 += *len as usize;
                at2 += *len as usize;
            }
            Op::Delete(text) => {
                changed_old[at1..at1 + text.len()].fill(true);
                at1 += text.len();
            }
            Op::Insert(text) => {
                changed_new[at2..at2 + text.len()].fill(true);
                at2 += text.len();
            }
        }
    }
    assert_eq!(at1, old.len(), "the ops consume the old text");
    assert_eq!(at2, new.len(), "the ops produce the new text");
    let render = |text: &str, changed: &[bool]| -> String {
        text.char_indices()
            .map(|(offset, _)| if changed[offset] { '-' } else { ' ' })
            .collect()
    };
    (render(old, &changed_old), render(new, &changed_new))
}

/// `("x z" - "y z")` then `("-  " - "-  ")`, IntelliJ's way: a
/// `lines_inner` case — the line pass, then the word pass inside
/// each replaced block.
#[track_caller]
fn check(texts: (&str, &str), expected: (&str, &str)) {
    check_with(texts, expected, |old, new| crate::diff_str(old, new));
}

/// IntelliJ's `words { }` case: the word pass over the whole text,
/// no line pass first.
#[track_caller]
fn check_words(texts: (&str, &str), expected: (&str, &str)) {
    check_with(texts, expected, |old, new| {
        let mut ops = Vec::new();
        crate::refine(&mut ops, old, new);
        ops
    });
}

#[track_caller]
fn check_with(texts: (&str, &str), expected: (&str, &str), diff: impl Fn(&str, &str) -> Vec<Op>) {
    let old = texts.0.replace('_', "\n");
    let new = texts.1.replace('_', "\n");
    let (got_old, got_new) = marks(&old, &new, &diff(&old, &new));
    let want_old = expected.0.replace('_', " ");
    let want_new = expected.1.replace('_', " ");
    assert_eq!(
        (got_old.as_str(), got_new.as_str()),
        (want_old.as_str(), want_new.as_str()),
        "\n{:?} -> {:?}\n",
        texts.0,
        texts.1
    );
}

#[test]
fn applying_the_ops_yields_the_new_text() {
    for (old, new) in [
        ("x z", "y z"),
        ("abc def, 123", "ab def, 12"),
        ("a\nb\nc\n", "a\nx\nc\n"),
        ("汉语漢語", "語汉语漢"),
        ("", "new"),
        ("old", ""),
    ] {
        let ops = crate::diff_str(old, new);
        let mut out = String::new();
        let mut at = 0;
        for op in &ops {
            match op {
                Op::Retain(len) => {
                    out.push_str(&old[at..at + *len as usize]);
                    at += *len as usize;
                }
                Op::Delete(text) => {
                    assert_eq!(&old[at..at + text.len()], text);
                    at += text.len();
                }
                Op::Insert(text) => out.push_str(text),
            }
        }
        assert_eq!(out, new, "{old:?} -> {new:?}");
    }
}

#[test]
fn simple_cases() {
    check(("x z", "y z"), ("-  ", "-  "));
    check((" x z", "y z"), ("--  ", "-  "));
    check(("x z ", "y z"), ("-  -", "-  "));
    check(("x z", " y z "), ("-  ", "--  -"));
    check(("x y", "x z "), ("  -", "  --"));
    check(("x,y", "x"), (" --", " "));
    check(("x,y", "y"), ("-- ", " "));
    check((".x=", ".!="), (" - ", " - "));
    check(("X xyz1 Z", "X xyz2 Z"), ("  ----  ", "  ----  "));
}

#[test]
fn punctuation() {
    check((" x.z.x ", "x..x"), ("-  -  -", "    "));
    check(("x..x", " x.z.x "), ("    ", "-  -  -"));
    check(("x ... z", "y ... z"), ("-      ", "-      "));
    check(("x ... z", "x ... y"), ("      -", "      -"));
    check(("x ,... z", "x ... y"), ("  -    -", "      -"));
    check(("x . , .. z", "x ... y"), ("   ---   -", "      -"));
    check(("x==y==z", "x====z"), ("   -   ", "      "));
    check(("x====z", "x==t==z"), ("      ", "   -   "));
    check(
        ("X Y ) {_ A B C", "X Y Z ) {_ y B C ) {"),
        ("         -    ", "   --      -    ----"),
    );
}

#[test]
fn old_diff_bug() {
    check(("x'y'>", "x'>"), ("  -- ", "   "));
    check(("x'>", "x'y'>"), ("   ", "  -- "));
}

#[test]
fn whitespace_only_changes() {
    check(("x  =z", "x=  z"), (" --  ", "  -- "));
    check(("x  =", "x=  z"), (" -- ", "  ---"));
}

#[test]
fn newlines() {
    check((" x _ y _ z ", "x z"), ("- ------  -", "   "));
    check(("x z", " x _ y _ z "), ("   ", "- ------  -"));
    check_words(("_i", "i_"), ("- ", " -"));
    check_words(("A_B_", "X_"), ("--- ", "- "));
}

#[test]
fn fixed_bugs() {
    check((".! ", ".  y!"), ("  -", " --- "));
    check((" x n", " y_  x m"), ("   -", "----   -"));
    check(("x_", "x!  "), (" -", " ---"));
}

#[test]
fn inner_whitespaces() {
    check(("<< x >>", "<.<>.>"), ("  ---  ", " -  - "));
    check(("<< x >>", "y<<x>>y"), ("  - -  ", "-     -"));
    check(("x .. z", "x y .. z"), ("      ", " --     "));
    check(("  x..z", "x..y  "), ("--   -", "   ---"));
    check((" x y x _ x z x ", "x x_x x"), ("- --  - - --  -", "       "));
}

#[test]
fn algorithm_specific() {
    // prefer words over punctuation
    check(("...x", "x..."), ("--- ", " ---"));
    // prefer longer word sequences
    check(("x x y", "x y"), ("--   ", "   "));
    check(("y x x", "y x"), ("   --", "   "));
    check(("A X A B", "A B"), ("----   ", "   "));
    // prefer less modified 'sentences'
    check(("A.X A.Z", "A.X A.Y A.Z"), ("       ", "   ----    "));
    check(("X.A Z.A", "X.A Y.A Z.A"), ("       ", "   ----    "));
    // prefer punctuation over whitespaces
    check((".   ", "   ."), (" ---", "--- "));
    // IntelliJ's Myers ties this toward the newline and leaves a C
    // changed on each side; ours matches all four words, which is
    // the better answer, so the expectation is ours.
    check(("A B_C D", "A_B C_D"), (" - - - ", " - - - "));
    check(("B_C_D_", "X_Y_Z_"), ("- - - ", "- - - "));
    check_words(("!x_!_z", "!_!_y z"), (" -    ", "    -- "));
}

#[test]
fn trailing_punctuation() {
    check(("X = { };", "X = { _ };"), ("        ", "     --   "));
}

#[test]
fn legacy_cases_from_by_word_test() {
    check(("abc def, 123", "ab def, 12"), ("---      ---", "--      --"));
    check((" a[xy]+1", ",a[]+1"), ("-  --   ", "-     "));
    check(("0987_  a.g();_", "yyyy_"), ("------------- ", "---- "));
    check(("  abc_2222_", "    x = abc_zzzz_"), ("      ---- ", " ------     ---- "));
    check(
        (
            "   if (eventMerger!=null && !dataSelection.getValueIsAdjusting()) {",
            "   if (eventMerger!=null && (dataSelection==null || !dataSelection.getValueIsAdjusting())) {",
        ),
        (
            "                                                                   ",
            "                           ------------------------                                      -  ",
        ),
    );
    check(("f(a, b);", "f(a,_  b);"), ("        ", "    --    "));
    check((" o.f(a)", "o. f( b)"), ("-    - ", "  -  -- "));
}

#[test]
fn empty_range_positions() {
    check(("x? y", "x y"), (" -  ", "   "));
    check(("x ?y", "x y"), ("  - ", "   "));
}

#[test]
fn continuous_script() {
    check_words(("ABCD", "DABC"), ("----", "----"));
    check_words(("汉语漢語", "語汉语漢"), ("   -", "-   "));
    check_words(("AB漢CD", "DA漢CD"), ("--   ", "--   "));
    check_words(("AB漢CD", "DA语CD"), ("---  ", "---  "));
    check_words(("!_?", "!_+"), ("  -", "  -"));
}

#[test]
fn astral_codepoints_are_chunks_of_their_own() {
    // IntelliJ's surrogate-pair cases, one mark per codepoint here.
    check_words(("😀🔫👨😶", "🔫👨😶😀"), ("-   ", "   -"));
    check_words(("😀", "🔫"), ("-", "-"));
    check_words(("😀 ", " 🔫"), ("--", "--"));
}

#[test]
fn a_letter_change_marks_the_whole_word() {
    check(("the color of it", "the colour of it"), ("    -----      ", "    ------      "));
    check(("getValue(x)", "getVale(x)"), ("--------   ", "-------   "));
}

#[test]
fn hunks_land_on_empty_lines() {
    // The inserted function goes AFTER the blank line, not before it:
    // the boundary slides so the hunk starts with the blank line's
    // twin and ends before the next blank line.
    let old = "fn a() {}\n\nfn c() {}\n";
    let new = "fn a() {}\n\nfn b() {}\n\nfn c() {}\n";
    let ops = crate::diff_str(old, new);
    assert_eq!(
        ops,
        vec![
            Op::Retain("fn a() {}\n\n".len() as u32),
            Op::Insert("fn b() {}\n\n".to_owned()),
            Op::Retain("fn c() {}\n".len() as u32),
        ]
    );
}

#[test]
fn a_lone_brace_does_not_anchor_a_match() {
    // Either line could be the one match; the important line wins,
    // so the brace moves and `foo` stays.
    let old = "}\nfoo();\n";
    let new = "foo();\n}\n";
    let ops = crate::diff_str(old, new);
    assert_eq!(
        ops,
        vec![
            Op::Delete("}\n".to_owned()),
            Op::Retain("foo();\n".len() as u32),
            Op::Insert("}\n".to_owned()),
        ]
    );
}

