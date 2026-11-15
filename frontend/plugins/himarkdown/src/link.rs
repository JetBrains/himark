// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! What a markdown link points at: the `editor::link` resolver for
//! written links. A destination with a scheme is the browser's; a
//! path names a document — resolved against the linking file when
//! the document has one, handed on as a bare path when it does not
//! (a chat cell).

use std::ops::Range;

use editor::document::Document;
use editor::link::{Link, LinkKind, LinkResolution, LinkTarget};
use editor::location::ResourceLocation;
use editor::location::ResourceType;
use imba::store::Store;

/// How far past a link's text the destination is read — a
/// destination longer than this is not followed.
const MAX_DESTINATION_BYTES: usize = 4096;

pub struct MarkdownLinks;

impl editor::link::LinkResolver for MarkdownLinks {
    fn claims(
        &self,
        document: &Document,
        link: &Link,
        location: Option<&ResourceLocation>,
    ) -> bool {
        link.kind == LinkKind::Written
            && destination_after(document, link.range.clone())
                .is_some_and(|destination| link_target(location, &destination).is_some())
    }

    fn resolve(
        &self,
        _store: &Store,
        document: &Document,
        link: &Link,
        location: Option<&ResourceLocation>,
    ) -> Option<LinkResolution> {
        let destination = destination_after(document, link.range.clone())?;
        link_target(location, &destination).map(LinkResolution::Ready)
    }
}

/// The destination of the link whose TEXT spans `text` — the
/// `(…)` that follows its closing bracket, read the way the inline
/// pass hid it (to the first `)`). `None` for a bracket pair with no
/// destination.
fn destination_after(document: &Document, text: Range<u32>) -> Option<String> {
    let mut view = document.text().view();
    let from = text.end as usize;
    let mut to = (from + MAX_DESTINATION_BYTES).min(view.byte_count());
    while to > from && !view.is_char_boundary(to) {
        to -= 1;
    }
    let tail = view.byte_string(from, to);
    let inner = tail.strip_prefix("](")?;
    let destination = crate::image::target(&inner[..inner.find(')')?]);
    (!destination.is_empty()).then_some(destination)
}

/// Classify a destination: a scheme makes it external; anything else
/// is a path — `#L10` names the line to land on. With a `base` (the
/// linking file) the path resolves to a location: `/`-rooted from
/// the base's root, otherwise the way a fence embed's does. Without
/// one it stays a path. A bare `#fragment` names nothing to open.
pub fn link_target(base: Option<&ResourceLocation>, destination: &str) -> Option<LinkTarget> {
    // A `file://` destination is a rooted path, not the browser's.
    let destination = destination.strip_prefix("file://").unwrap_or(destination);
    if has_scheme(destination) {
        return Some(LinkTarget::Url(destination.to_owned()));
    }
    let (path, lines) = crate::fence_embed::split_fragment(destination);
    if path.is_empty() {
        return None;
    }
    let line = lines.map(|(first, _)| first.saturating_sub(1));
    let Some(base) = base else {
        return Some(LinkTarget::Path {
            path: path.to_owned(),
            line,
        });
    };
    let location = match path.strip_prefix('/') {
        Some(rooted) => {
            let segments: Vec<String> = rooted
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect();
            if segments.is_empty() {
                return None;
            }
            ResourceLocation::new(ResourceType::document(), base.authority().clone(), segments)
        }
        None => crate::fence_embed::resolve(base, path)?,
    };
    Some(LinkTarget::Location {
        location,
        range: line.map(|line| {
            let at = editor::linecol::LineCol { line, col: 0 };
            at..at
        }),
    })
}

/// `scheme:` per RFC 3986 — two letters at least, so a drive letter
/// is not one.
fn has_scheme(destination: &str) -> bool {
    let Some((scheme, _)) = destination.split_once(':') else {
        return false;
    };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[cfg(test)]
mod tests;
