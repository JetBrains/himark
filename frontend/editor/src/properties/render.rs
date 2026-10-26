// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Painting is compared PIXEL FOR PIXEL between two live paints —
//! never against a stored image. The shaper and the CPU raster are
//! deterministic within one process, so two paints of the same thing
//! are the same bytes, and any difference is incremental state (the
//! layout, the shape cache, the viewport memo) that drifted from
//! what a fresh editor paints.

use std::ops::Range;

use proptest::prelude::*;
use skia_safe::{surfaces, Color, EncodedImageFormat, Rect};

use super::{
    gen::{self, Block, EditSpec, Inline, Spec},
    layout::settle,
};
use crate::{
    document::{Document, EditorBuild},
    editor::EditorId,
    test_document::test_ui,
};

fn theme() -> crate::theme::Theme {
    crate::theme::embedded()
}

const CLEAR: Color = Color::from_argb(255, 9, 8, 7);

struct Raster {
    width: i32,
    bytes: Vec<u8>,
    png: Option<skia_safe::Data>,
}

/// Paint the band `top..bottom` of the editor into a surface exactly
/// that tall, the canvas translated so the band's top is the
/// surface's first row — the way a scrolled pane paints.
fn paint(
    document: &Document,
    editor: EditorId,
    width: i32,
    band: Range<f32>,
    store: &imba::store::Store,
    ui: &imba::ui::UiCtx,
) -> Raster {
    let height = ((band.end - band.start).ceil() as i32).max(1);
    let mut surface = surfaces::raster_n32_premul((width, height)).expect("a raster surface");
    surface.canvas().clear(CLEAR);
    surface.canvas().translate((0.0, -band.start));
    document.paint(
        editor,
        surface.canvas(),
        Rect::from_xywh(0.0, band.start, width as f32, band.end - band.start),
        false,
        store,
        ui,
        &theme(),
    );
    let image = surface.image_snapshot();
    let bytes = image
        .peek_pixels()
        .expect("raster pixels")
        .bytes()
        .expect("pixel bytes")
        .to_vec();
    let png = std::env::var_os("HIMARK_PROPERTY_DUMP")
        .and_then(|_| image.encode(None, EncodedImageFormat::PNG, 100));
    Raster { width, bytes, png }
}

/// On a mismatch, `HIMARK_PROPERTY_DUMP=<dir>` writes both paints as
/// PNGs there for eyes.
fn dump(name: &str, raster: &Raster) {
    let Some(dir) = std::env::var_os("HIMARK_PROPERTY_DUMP") else {
        return;
    };
    if let Some(png) = &raster.png {
        let path = std::path::PathBuf::from(dir).join(format!("{name}.png"));
        let _ = std::fs::write(&path, png.as_bytes());
        eprintln!("wrote {}", path.display());
    }
}

fn assert_same_pixels(
    a: &Raster,
    b: &Raster,
    rows: Range<usize>,
    context: &str,
) -> Result<(), TestCaseError> {
    let row = (a.width * 4) as usize;
    let a_bytes = &a.bytes[rows.start * row..rows.end * row];
    let b_bytes = &b.bytes;
    // Skia's analytic anti-aliasing of a rounded corner comes out a
    // unit or two different at a different device offset (a band
    // paints through a translated canvas). That is numerical noise,
    // not a different picture; a glyph or a fill out of place is
    // tens of units.
    const NOISE: u8 = 2;
    let same = |x: &[u8], y: &[u8]| x.iter().zip(y).all(|(x, y)| x.abs_diff(*y) <= NOISE);
    if a_bytes.len() == b_bytes.len()
        && a_bytes
            .chunks(4)
            .zip(b_bytes.chunks(4))
            .all(|(x, y)| same(x, y))
    {
        return Ok(());
    }
    dump(&format!("{}-a", context.replace(' ', "_")), a);
    dump(&format!("{}-b", context.replace(' ', "_")), b);
    let first = a_bytes
        .iter()
        .zip(b_bytes)
        .position(|(x, y)| x != y)
        .unwrap_or(a_bytes.len().min(b_bytes.len()));
    let differing: Vec<_> = a_bytes
        .chunks(4)
        .zip(b_bytes.chunks(4))
        .enumerate()
        .filter(|(_, (x, y))| !same(x, y))
        .collect();
    let sample: Vec<String> = differing
        .iter()
        .take(12)
        .map(|(index, (x, y))| {
            format!(
                "(row {} col {}: {:?} vs {:?})",
                rows.start + (index * 4) / row,
                (index * 4 % row) / 4,
                x,
                y
            )
        })
        .collect();
    prop_assert!(
        false,
        "{context}: {} pixels differ (lengths {} vs {}), first at row {} column {}: {}",
        differing.len(),
        a_bytes.len(),
        b_bytes.len(),
        rows.start + first / row,
        (first % row) / 4,
        sample.join(" "),
    );
    Ok(())
}

fn complete_editor(
    document: &mut Document,
    width: f32,
    store: &imba::store::Store,
    ui: &imba::ui::UiCtx,
) -> EditorId {
    let editor = document.add_editor(
        width,
        None,
        EditorBuild::Complete,
        &[],
        store,
        ui,
        &theme(),
        fx!(),
    );
    settle(document);
    editor
}

fn surface_width() -> impl Strategy<Value = i32> {
    200i32..=800
}

/// After edits land and the repair worker settles, the editor paints
/// exactly what a fresh editor over the same document paints.
fn live_matches_fresh(spec: &Spec, width: i32, edits: &[EditSpec]) -> Result<(), TestCaseError> {
    let store = imba::store::Store::new();
    let ui = test_ui();
    let mut document = gen::build(spec);
    let editor = complete_editor(&mut document, width as f32, &store, ui);
    for edit in edits {
        if let Some(operation) = gen::resolve(document.text(), edit) {
            document.edit(&operation, &store, ui, &theme(), fx!());
        }
    }
    settle(&mut document);
    let height = document.content_height(editor).ceil().clamp(1.0, 800.0);
    let live = paint(&document, editor, width, 0.0..height, &store, ui);

    let mut fresh_document = document.substance();
    let fresh_editor = complete_editor(&mut fresh_document, width as f32, &store, ui);
    let fresh = paint(
        &fresh_document,
        fresh_editor,
        width,
        0.0..height,
        &store,
        ui,
    );
    assert_same_pixels(&live, &fresh, 0..height as usize, "live vs fresh")
}

/// A scrolled pane paints a band of the document; the band must be
/// the matching rows of a paint of the whole document, so nothing
/// appears or vanishes as the band moves. `top` is a fraction of the
/// document's height, `band` the band's height in rows.
fn band_matches_whole(spec: &Spec, width: i32, top: f32, band: i32) -> Result<(), TestCaseError> {
    let store = imba::store::Store::new();
    let ui = test_ui();
    let mut document = gen::build(spec);
    let editor = complete_editor(&mut document, width as f32, &store, ui);
    let total = document.content_height(editor).ceil().clamp(1.0, 1200.0) as i32;
    let whole = paint(&document, editor, width, 0.0..total as f32, &store, ui);
    let top = (((total - 1) as f32) * top).floor() as i32;
    let bottom = (top + band).min(total);
    prop_assume!(bottom > top);
    let slice = paint(
        &document,
        editor,
        width,
        top as f32..bottom as f32,
        &store,
        ui,
    );
    assert_same_pixels(
        &whole,
        &slice,
        top as usize..bottom as usize,
        &format!("rows {top}..{bottom} of {total}"),
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn a_settled_editor_paints_the_pixels_of_a_fresh_editor(
        spec in gen::document_spec(),
        width in surface_width(),
        edits in proptest::collection::vec(gen::edit_spec(), 1..6),
    ) {
        live_matches_fresh(&spec, width, &edits)?;
    }

    /// Painting the same unchanged editor twice gives the same bytes:
    /// the second paint runs on the viewport memo and the shape cache,
    /// the first filled them.
    #[test]
    fn painting_twice_gives_the_same_pixels(
        spec in gen::document_spec(),
        width in surface_width(),
    ) {
        let store = imba::store::Store::new();
        let ui = test_ui();
        let mut document = gen::build(&spec);
        let editor = complete_editor(&mut document, width as f32, &store, ui);
        let height = document.content_height(editor).ceil().clamp(1.0, 800.0);
        let first = paint(&document, editor, width, 0.0..height, &store, ui);
        let second = paint(&document, editor, width, 0.0..height, &store, ui);
        assert_same_pixels(&first, &second, 0..height as usize, "first vs second paint")?;
    }

    #[test]
    fn a_band_paints_the_matching_rows_of_the_whole_paint(
        spec in gen::document_spec(),
        width in surface_width(),
        top in 0.0f32..1.0,
        band in 16i32..=240,
    ) {
        band_matches_whole(&spec, width, top, band)?;
    }
}

/// The properties' shrunk findings, kept as plain tests: each input
/// below failed before its fix on 2026-10-26.
mod shrunk {
    use super::*;

    fn spec(source: &str, blocks: Vec<Block>, spans: Vec<(usize, f32, f32, Inline)>) -> Spec {
        Spec {
            source: source.to_owned(),
            blocks,
            spans,
        }
    }

    fn edits(list: &[(f32, f32, &str)]) -> Vec<EditSpec> {
        list.iter()
            .map(|(at, delete, insert)| EditSpec {
                at: *at,
                delete: *delete,
                insert: (*insert).to_owned(),
            })
            .collect()
    }

    /// Raw edits put newlines inside the caret's hard line; the
    /// editor's unhide overlay, refreshed only by caret commands,
    /// kept revealing the whole old line, so hidden text on the new
    /// neighbouring lines painted. Fix: an edit refreshes the unhide
    /// range of every editor.
    #[test]
    fn hidden_text_stays_hidden_on_lines_an_edit_split_off_the_carets() {
        let result = live_matches_fresh(
            &spec(
                "I  héllo I",
                vec![Block::Plain],
                vec![(0, 0.0, 0.9078731, Inline::Hidden)],
            ),
            326,
            &edits(&[
                (0.18251264, 0.0013235457, "-"),
                (0.7533604, 0.8934729, "\n\n"),
                (0.17117095, 0.067829534, "\n"),
                (0.11016279, 0.7215984, "\n\n"),
                (0.13075428, 0.0, "\n\n"),
            ]),
        );
        if let Err(failure) = result {
            panic!("{failure}");
        }
    }

    /// A code block runs past the band's last row; the band painted
    /// its box with a rounded bottom corner at the band's edge where
    /// the whole paint has a straight edge. Fix: a block cut by the
    /// band's edge bleeds its box past the band.
    #[test]
    fn a_block_cut_by_the_band_edge_paints_a_straight_edge() {
        let result = band_matches_whole(
            &spec(
                " xcraqogttnhamennbzrbxaejabjwfamkdehxzggsvncks 1. tab\tbed mvsbgqtqgibpobpubhdwfwbmtbtbjolhpostfrvccvowrfojmzx lfbjwwgjoxzkqqsfkxfjwimpzvlviqqkyngkqqdsuuanrrvswmbpkxseg pvvnaeyqlvidzlyjeijnmyreqkmxbzmjmmx héllo lorem\na sit  delta héllo blfdquyttswpjwyupwgabduatddhyzlzmoefablg\nnaïve amet ipsum ipsum héllo cucmgjqhctbueehesdnc sit tab\tbed 😀 \nalpha webssfrsqbtfunkykdanlkpdbhbbueejgr a  # gamma x😀y naïve # alpha\nemezpadzxakmnibmgeintbipknrkpxcdtwoqwrpenadszsdytei 1. a gamma",
                vec![
                    Block::CodeBlock,
                    Block::Plain,
                    Block::Plain,
                    Block::Header(2),
                    Block::Plain,
                ],
                vec![
                    (4, 0.10313814, 0.51982445, Inline::Emphasis),
                    (0, 0.22728512, 0.42378184, Inline::Link),
                    (2, 0.83468735, 0.51008755, Inline::Emphasis),
                    (4, 0.23670337, 0.6955425, Inline::Hidden),
                ],
            ),
            644,
            0.0035071315,
            236,
        );
        if let Err(failure) = result {
            panic!("{failure}");
        }
    }

    /// A code block starting just above the band: the band's corner
    /// is drawn at a different device offset and its anti-aliasing
    /// comes out a unit different. Noise, tolerated as such; with the
    /// bleed the corner is out of the band anyway.
    #[test]
    fn a_block_starting_just_above_the_band_paints_the_same() {
        let result = band_matches_whole(
            &spec(
                "ipsum héllo a amet\n# dolor\nqiapastjfzubloihfvvsdrahxcxqcrlnrtbglzdrxsvfqloaauohfifolhv oihqtisvrytegodhthqsiybhdyhcvobbznvfzbuaxmgedejcnpz vrcqwzqnfthkzlxvwsywgqsjeknnbdobneoriayobj amet naïve I\n\ntab\tbed lorem ubuogahqcumkoreyukflprtowixtwsfdqbrqhovmnd amet tlsjyvcvnharsebribcuewdmjclqkaaxwdgtsrwcybdwf\nx😀y x😀y 1. I lorem tab\tbed",
                vec![
                    Block::Centered,
                    Block::Plain,
                    Block::ListItem,
                    Block::CodeBlock,
                    Block::ListItem,
                    Block::Header(1),
                ],
                vec![],
            ),
            295,
            0.30660635,
            26,
        );
        if let Err(failure) = result {
            panic!("{failure}");
        }
    }
}
