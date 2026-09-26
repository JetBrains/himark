// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! STAGE TIMINGS for the normalize lane, run by hand:
//!
//!   cargo test -p structdiff --release --test stages -- --ignored --nocapture
//!
//! Splits one normalize into its stages — tree-sitter parse per side,
//! the difftastic engine, the Myers fallback on the same input, and
//! the hunk-markup derivation — so "is it the structural diff or the
//! parsing" is answered with numbers, per input size and edit shape.

use std::time::Instant;

use text::Text;

fn synth_rust(functions: usize) -> String {
    let mut source = String::from("//! synthetic module\n\nuse std::collections::HashMap;\n\n");
    for n in 0..functions {
        if n % 10 == 0 {
            source.push_str(&format!(
                "#[derive(Clone, Debug)]\npub struct Record{n} {{\n    pub key: String,\n    pub value: u64,\n    pub tags: Vec<String>,\n}}\n\n",
            ));
        }
        source.push_str(&format!(
            "/// Handles case {n} of the synthetic workload.\n\
             pub fn handle_{n}(input: &str, table: &mut HashMap<String, u64>) -> Option<u64> {{\n\
             \x20   let mut value = 0u64;\n\
             \x20   for (index, token) in input.split_whitespace().enumerate() {{\n\
             \x20       match token.len() % 3 {{\n\
             \x20           0 => value += index as u64,\n\
             \x20           1 => value = value.wrapping_mul(31).wrapping_add(token.len() as u64),\n\
             \x20           _ => {{\n\
             \x20               table.insert(token.to_owned(), value);\n\
             \x20           }}\n\
             \x20       }}\n\
             \x20   }}\n\
             \x20   // The accumulated value is only meaningful when non-zero.\n\
             \x20   (value > 0).then_some(value)\n\
             }}\n\n",
        ));
    }
    source
}

fn time<R>(label: &str, mut run: impl FnMut() -> R) -> R {
    let mut best = f64::MAX;
    let mut result = None;
    for _ in 0..5 {
        let start = Instant::now();
        result = Some(run());
        best = best.min(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!("    {label:<44} {best:9.2} ms");
    result.unwrap()
}

fn stage_report(name: &str, base_src: &str, target_src: &str) {
    println!(
        "  {name}: {:.0}KB -> {:.0}KB",
        base_src.len() as f64 / 1024.0,
        target_src.len() as f64 / 1024.0
    );
    let mut languages = editor::SyntaxLanguages::new();
    hirust::register(&mut languages);
    let rust = languages.ensure("rust").expect("rust grammar");

    let base = Text::from_string_exact(base_src.to_owned());
    let target = Text::from_string_exact(target_src.to_owned());

    // The catch-up path: the STALE tree is edit-adjusted (the edit
    // door's discipline) and tree-sitter reuses everything untouched.
    {
        let myers = myersdiff::diff(&base, &target);
        let mut stale = rust
            .parse(&base, 0..base.view().byte_count() as u32, None)
            .expect("stale base parse");
        stale.edit(&myers, &mut target.view(), 0);
        time("catch-up parse (stale old tree)", || {
            rust.parse(
                &target,
                0..target.view().byte_count() as u32,
                Some(stale.as_ref()),
            )
            .expect("catch-up parse")
        });
    }

    let base_tree = time("tree-sitter parse (base)", || {
        rust.parse(&base, 0..base.view().byte_count() as u32, None)
            .expect("base parse")
    });
    let target_tree = time("tree-sitter parse (target)", || {
        rust.parse(&target, 0..target.view().byte_count() as u32, None)
            .expect("target parse")
    });

    let input = structdiff::SyntaxInput {
        left_tree: hisitter::TsTree::of(base_tree.as_ref()).expect("ts base"),
        right_tree: hisitter::TsTree::of(target_tree.as_ref()).expect("ts target"),
        language: Some("rust"),
    };
    let operation = time("difftastic engine (trees given)", || {
        structdiff::diff(&base, &target, Some(&input))
    });
    let myers = time("myers fallback (same input)", || {
        myersdiff::diff(&base, &target)
    });
    time("hunk_markup (structural op)", || {
        editor::diff::hunk_markup(&operation, &target)
    });
    let hunks = |op: &operation::Operation| {
        op.iter()
            .filter(|op| !matches!(op, operation::Op::Retain(_)))
            .count()
    };
    println!(
        "    structural pieces {} / myers pieces {}",
        hunks(&operation),
        hunks(&myers),
    );
}

#[test]
#[ignore = "manual timing run; prints stage costs"]
fn normalize_stage_timings() {
    for functions in [200usize, 800] {
        let base = synth_rust(functions);

        // One local edit — the keystroke/normalize steady state.
        let one = base.replacen("value += index as u64", "value += 2 * index as u64", 1);

        // A scattered touch — an agent edit brushing every ~40th fn.
        let mut scatter = base.clone();
        for n in (0..functions).step_by(40) {
            let from = format!("Handles case {n} of the synthetic workload.");
            let to = format!("Handles case {n} of the RESHAPED workload.");
            scatter = scatter.replacen(&from, &to, 1);
        }

        // A global identifier rename — the structural stressor.
        let rename = base.replace("value", "val");

        println!("== {functions} functions ==");
        stage_report("one-line edit", &base, &one);
        stage_report("scattered edits", &base, &scatter);
        stage_report("global rename", &base, &rename);
    }
}
