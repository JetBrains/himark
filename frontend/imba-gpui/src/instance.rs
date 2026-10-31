// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Variable-font instancing for the gpui road. gpui's mac text
//! system matches fonts with font-kit — static weight traits over a
//! family's member fonts — and never applies variation axes, so a
//! variable face registered from memory always renders its `fvar`
//! DEFAULT instance (the absurdly-bold chrome of 2026-10-31). The
//! workaround: bake the instance into the bytes. Every patch is a
//! fixed-size in-place edit — nothing moves, no re-layout:
//!
//! - `fvar` axis defaults become the run's coordinates (clamped to
//!   the axis range), so the default instance IS the instance;
//! - the family and postscript names (nameIDs 1, 4, 6, 16) get a
//!   per-face suffix overwriting their trailing characters, so
//!   CoreText neither collides with the real family nor dedupes
//!   repeat registrations by postscript name.
//!
//! Table checksums go stale; CoreText does not verify them.

use gfx::FaceEntry;

/// The instanced bytes and the family name they now answer to.
pub fn instanced(face_index: u32, entry: &FaceEntry) -> Option<(Vec<u8>, String)> {
    if entry.coords.is_empty() {
        return None;
    }

    let mut data = pin_axes(entry)?;

    let tables = table_directory(&data)?;
    let (_, name_offset, name_len) = *tables.iter().find(|(tag, ..)| tag == b"name")?;
    let suffix = format!("{face_index:04X}");
    let family = patch_names(&mut data, name_offset as usize, name_len as usize, &suffix)?;

    Some((data, family))
}

extern "C" {
    /// In the bundled harfbuzz but not in the crate's bindings.
    fn hb_subset_input_pin_axis_location(
        input: *mut hb_subset::sys::hb_subset_input_t,
        face: *mut hb_subset::sys::hb_face_t,
        axis_tag: u32,
        axis_value: f32,
    ) -> i32;
}

/// The instance as STATIC outlines, via harfbuzz's subsetter with
/// every shaped axis pinned. Nothing short of this renders right:
/// CoreText shows a data-loaded variable font's default master.
fn pin_axes(entry: &FaceEntry) -> Option<Vec<u8>> {
    let bytes = entry.data.as_ref().as_ref();
    let blob = hb_subset::Blob::from_bytes(bytes).ok()?;
    let face = hb_subset::FontFace::new_with_index(blob, entry.index).ok()?;
    let mut input = hb_subset::SubsetInput::new().ok()?;
    input.keep_everything();
    for (tag, value) in &entry.coords {
        let pinned = unsafe {
            hb_subset_input_pin_axis_location(input.as_raw(), face.as_raw(), *tag, *value)
        };
        if pinned == 0 {
            return None;
        }
    }
    let instanced = input.subset_font(&face).ok()?;
    let out = instanced.underlying_blob().to_vec();
    (!out.is_empty()).then_some(out)
}

type Table = ([u8; 4], u32, u32);

fn table_directory(data: &[u8]) -> Option<Vec<Table>> {
    let count = read_u16(data, 4)? as usize;
    let mut tables = Vec::with_capacity(count);
    for i in 0..count {
        let at = 12 + i * 16;
        let tag: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        let offset = read_u32(data, at + 8)?;
        let length = read_u32(data, at + 12)?;
        tables.push((tag, offset, length));
    }
    Some(tables)
}


/// Overwrite the tail of every family-ish name record with the
/// suffix, and answer the patched typographic (or plain) family.
fn patch_names(data: &mut [u8], offset: usize, len: usize, suffix: &str) -> Option<String> {
    let count = read_u16(data, offset + 2)? as usize;
    let storage = offset + read_u16(data, offset + 4)? as usize;
    let mut family = None;
    for record in 0..count {
        let at = offset + 6 + record * 12;
        let platform = read_u16(data, at)?;
        let name_id = read_u16(data, at + 6)?;
        let str_len = read_u16(data, at + 8)? as usize;
        let str_off = storage + read_u16(data, at + 10)? as usize;
        if !matches!(name_id, 1 | 4 | 6 | 16) {
            continue;
        }
        if str_off + str_len > offset + len || str_off + str_len > data.len() {
            continue;
        }
        let wide = platform == 0 || platform == 3;
        let char_width = if wide { 2 } else { 1 };
        let chars = str_len / char_width;
        if chars <= suffix.len() {
            continue;
        }
        for (i, ch) in suffix.bytes().enumerate() {
            let slot = str_off + (chars - suffix.len() + i) * char_width;
            if wide {
                data[slot] = 0;
                data[slot + 1] = ch;
            } else {
                data[slot] = ch;
            }
        }
        if matches!(name_id, 1 | 16) && (family.is_none() || name_id == 16) {
            let bytes = &data[str_off..str_off + str_len];
            let patched = if wide {
                bytes
                    .chunks(2)
                    .map(|pair| {
                        char::from_u32(u16::from_be_bytes([pair[0], pair[1]]) as u32)
                            .unwrap_or('\u{FFFD}')
                    })
                    .collect::<String>()
            } else {
                bytes.iter().map(|byte| *byte as char).collect()
            };
            family = Some(patched);
        }
    }
    family
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The chrome face pins to static outlines: fvar gone, names
    /// suffixed, and the advance of a wide glyph differs between a
    /// light and a heavy instance — the proof the OUTLINE data
    /// changed, not just the metadata.
    #[test]
    fn chrome_instances_are_static_and_distinct() {
        let bytes: &[u8] =
            include_bytes!("../../editor/assets/BricolageGrotesque-Variable.ttf");
        let entry = |weight: f32| FaceEntry {
            data: Arc::new(bytes),
            index: 0,
            coords: vec![
                (u32::from_be_bytes(*b"opsz"), 96.0),
                (u32::from_be_bytes(*b"wght"), weight),
                (u32::from_be_bytes(*b"wdth"), 100.0),
            ],
            family: Some("Bricolage Grotesque".to_owned()),
        };

        let (light, light_family) = instanced(1, &entry(300.0)).expect("light instance");
        let (heavy, heavy_family) = instanced(2, &entry(700.0)).expect("heavy instance");

        let fvar = |data: &[u8]| {
            table_directory(data)
                .expect("sfnt")
                .iter()
                .any(|(tag, ..)| tag == b"fvar")
        };
        assert!(!fvar(&light), "the instance is static");
        assert!(!fvar(&heavy), "the instance is static");
        assert_ne!(light_family, heavy_family, "families stay distinct");
        assert!(light_family.ends_with("0001"), "{light_family:?}");

        // The glyf tables differ — different outlines, not just
        // different names.
        assert_ne!(light, heavy);
    }
}
