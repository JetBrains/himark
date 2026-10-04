// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use imba::store::Store;

#[derive(Clone, Default)]
pub struct RecentLocations(Vec<editor::location::ResourceLocation>);

/// Recents belong to the session you are working in; its session row
/// hands the id to whoever has that context (docs/entities.md law 3) —
/// this module never sees a `SessionId`.
impl RecentLocations {
    const CAP: usize = 100;

    pub fn touch(
        store: &mut Store,
        recents: imba::store::Id<Self>,
        location: &editor::location::ResourceLocation,
    ) {
        store.update_entity(recents, |recents| {
            let recents = &mut recents.0;
            recents.retain(|listed| listed != location);
            recents.insert(0, location.clone());
            recents.truncate(Self::CAP);
        });
    }

    pub fn replace(
        store: &mut Store,
        recents: imba::store::Id<Self>,
        old: &editor::location::ResourceLocation,
        new: &editor::location::ResourceLocation,
    ) {
        store.update_entity(recents, |recents| {
            let recents = &mut recents.0;
            recents.retain(|listed| listed != old && listed != new);
            recents.insert(0, new.clone());
            recents.truncate(Self::CAP);
        });
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn list(store: &Store, recents: imba::store::Id<Self>) -> Vec<editor::location::ResourceLocation> {
        store
            .entity(recents)
            .map(|recents| recents.0.clone())
            .unwrap_or_default()
    }
}
