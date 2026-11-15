// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! Following links — the editor's own feature. A link is a span the
//! parse marked ([`Document::link_at`]); a cmd-click on one sets the
//! caret and follows it, in EVERY editor view. What a link points at
//! is a registered [`LinkResolver`]'s to say; where the target opens
//! is the seated [`LinkDoor`]'s. The editor knows neither a language
//! nor a shell.

use std::ops::Range;
use std::sync::Arc;

use imba::effect::AnyEffect;
use imba::store::Store;

use crate::document::Document;
use crate::editor::EditorEffects;
use crate::editor_view::EditorCommand;
use crate::linecol::LineCol;
use crate::location::ResourceLocation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// An identifier the syntax pass marked
    /// ([`crate::theme::StyleId::Reference`]).
    Reference,
    /// A written link's text ([`crate::theme::StyleId::Link`]).
    Written,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub range: Range<u32>,
    pub kind: LinkKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// The browser's.
    Url(String),
    /// A document, optionally at a range.
    Location {
        location: ResourceLocation,
        range: Option<Range<LineCol>>,
    },
    /// A path its document could not place — the document has no
    /// location of its own (a chat cell). The door places it against
    /// where the gesture happened. `line` is 0-based.
    Path { path: String, line: Option<u32> },
}

pub enum LinkResolution {
    Ready(LinkTarget),
    /// An ask (a language server's definition); `None` lands as
    /// nothing.
    Pending(AnyEffect<Option<LinkTarget>>),
}

/// What a link points at. `location` is the document's own, when it
/// has one.
pub trait LinkResolver: Send + Sync + 'static {
    /// Would [`Self::resolve`] answer here — cheap, no effect built:
    /// the hover asks it on every command-held hit test.
    fn claims(&self, document: &Document, link: &Link, location: Option<&ResourceLocation>)
        -> bool;

    fn resolve(
        &self,
        store: &Store,
        document: &Document,
        link: &Link,
        location: Option<&ResourceLocation>,
    ) -> Option<LinkResolution>;
}

#[derive(Clone, Default)]
pub struct LinkResolvers(Vec<Arc<dyn LinkResolver>>);

impl LinkResolvers {
    pub fn of(store: &Store) -> LinkResolvers {
        store.get::<LinkResolvers>().cloned().unwrap_or_default()
    }

    pub fn register(store: &mut Store, resolver: Arc<dyn LinkResolver>) {
        store.update::<LinkResolvers>(|resolvers| resolvers.0.push(resolver));
    }

    /// The first resolver claiming the link.
    pub fn claiming(
        &self,
        document: &Document,
        link: &Link,
        location: Option<&ResourceLocation>,
    ) -> Option<&Arc<dyn LinkResolver>> {
        self.0
            .iter()
            .find(|resolver| resolver.claims(document, link, location))
    }
}

/// Where a resolved target opens — seated by the shell. Absent (a
/// bare editor, a test), a follow resolves and goes nowhere.
#[derive(Clone)]
pub struct LinkDoor(pub Arc<dyn Fn(&mut Store, LinkTarget) + Send + Sync>);

impl LinkDoor {
    pub fn open(store: &mut Store, target: LinkTarget) {
        if let Some(door) = store.get::<LinkDoor>().cloned() {
            (door.0)(store, target);
        }
    }
}

/// The claimed link under `byte`, if any resolver would follow it.
pub fn claimed_at(
    store: &Store,
    document: &Document,
    byte: u32,
    location: Option<&ResourceLocation>,
) -> Option<Link> {
    let link = document.link_at(byte)?;
    LinkResolvers::of(store)
        .claiming(document, &link, location)
        .is_some()
        .then_some(link)
}

/// Follow the link under `byte`: resolve it, and hand a ready target
/// to the door — a pending one lands as
/// [`EditorCommand::FollowLink`]. Nothing under the byte, or nobody
/// claiming it, is a no-op.
pub fn follow_at(
    store: &mut Store,
    document: &Document,
    byte: u32,
    location: Option<&ResourceLocation>,
    fx: &mut EditorEffects<'_>,
) {
    let Some(link) = document.link_at(byte) else {
        return;
    };
    let resolvers = LinkResolvers::of(store);
    let Some(resolution) = resolvers
        .claiming(document, &link, location)
        .and_then(|resolver| resolver.resolve(store, document, &link, location))
    else {
        return;
    };
    follow(store, resolution, fx);
}

/// Hand a resolution on: a ready target to the door, a pending one
/// to land as [`EditorCommand::FollowLink`].
pub fn follow(store: &mut Store, resolution: LinkResolution, fx: &mut EditorEffects<'_>) {
    match resolution {
        LinkResolution::Ready(target) => LinkDoor::open(store, target),
        LinkResolution::Pending(ask) => {
            let _ = fx.push(ask.map(EditorCommand::FollowLink));
        }
    }
}
