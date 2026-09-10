use imba::effect::{AnyEffect, Effect};
use imba::store::Store;

fn probe() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("HIMARK_WATCH_PROBE").is_some())
}
use crate::{FetchDocumentEffect, OpenDocuments};
use editor::ResourceLocation;
use imba::effect::Effects;

#[derive(Clone, Default)]
pub struct Watching;

impl Watching {
    pub fn install(store: &mut Store) {
        store.put(Watching);
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<Watching>().is_some()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Subscription(pub u64);

pub struct SubscribeEffect {
    pub location: ResourceLocation,
}

impl Effect for SubscribeEffect {
    type Result = Option<Subscription>;
}

pub struct UnsubscribeEffect {
    pub subscription: Subscription,
}

impl Effect for UnsubscribeEffect {
    type Result = ();
}

pub struct FilesChanged(pub std::sync::Arc<std::collections::HashSet<Subscription>>);

pub struct FileChanged {
    pub subscription: Subscription,
}

pub struct RefetchDiffEffect {
    pub baseline: editor::Text,

    pub current: editor::Text,

    pub fetched: String,
}

pub struct RefetchRebase {
    pub operation: operation::Operation,

    pub fetched: editor::Text,

    pub clean: bool,
}

impl Effect for RefetchDiffEffect {
    type Result = RefetchRebase;
}

pub struct RefetchDiffHandler;

impl imba::effect::EffectHandler<RefetchDiffEffect> for RefetchDiffHandler {
    async fn handle(&self, effect: RefetchDiffEffect) -> RefetchRebase {
        let fetched = editor::Text::from_string_exact(&effect.fetched);
        let theirs = ::editor::diff::diff(&effect.baseline, &fetched);
        let ours = ::editor::diff::diff(&effect.baseline, &effect.current);
        let clean = ours
            .iter()
            .all(|op| matches!(op, operation::Op::Retain(_)));
        let operation = match clean {
            true => theirs,
            false => theirs.transform(&ours),
        };
        RefetchRebase {
            operation,
            fetched,
            clean,
        }
    }
}

pub fn sync_document_watches<R: 'static>(
    store: &mut Store,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, Option<Subscription>) -> R + Send + Clone + 'static,
) {
    if !Watching::installed(store) {
        return;
    }
    for (document, entity) in OpenDocuments::list(store) {
        if entity.watch.is_some() || entity.watch_requested {
            continue;
        }
        let Some(location) = entity.location.clone() else {
            continue;
        };

        if crate::is_synthetic(&location) {
            continue;
        }
        OpenDocuments::set_watch_requested(store, document);
        if probe() {
            eprintln!("[watch] sweep: subscribing /{}", location.path().join("/"));
        }
        let _ = fx.push(AnyEffect::new(SubscribeEffect { location }).map({
            let wrap = wrap.clone();
            move |subscription| wrap(document, subscription)
        }));
    }
}

pub fn refetch_watched<R: 'static>(
    store: &mut Store,
    subscription: Subscription,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, u64, Option<String>) -> R + Send + Clone + 'static,
) {
    let riders = store
        .get::<OpenDocuments>()
        .map(|documents| documents.watch_riders(subscription))
        .unwrap_or_default();
    if probe() {
        eprintln!(
            "[watch] FileChanged #{} -> {} open document(s) re-fetch",
            subscription.0,
            riders.len()
        );
    }
    for document in riders {
        let Some(location) =
            OpenDocuments::entity(store, document).and_then(|entity| entity.location)
        else {
            continue;
        };

        let serial = OpenDocuments::stamp_refetch(store, document);
        let _ = fx.push(AnyEffect::new(FetchDocumentEffect { location }).map({
            let wrap = wrap.clone();
            move |text| wrap(document, serial, text)
        }));
    }
}

pub fn apply_refetched<R: 'static>(
    store: &mut Store,
    document_id: crate::DocumentId,
    serial: u64,
    text: Option<String>,
    fx: &mut Effects<'_, R>,
    wrap: impl Fn(crate::DocumentId, u64, u64, RefetchRebase) -> R + Send + 'static,
) {
    let Some(text) = text else {
        if probe() {
            eprintln!("[watch] refetch landed: gone/unreadable — keeping ours");
        }
        return;
    };
    let Some(entity) = OpenDocuments::entity(store, document_id) else {
        return;
    };
    if entity.refetch_serial != serial {
        if probe() {
            eprintln!(
                "[watch] refetch landed: superseded (serial {serial} vs {}) — dropped",
                entity.refetch_serial
            );
        }
        return;
    }
    let Some(document) = OpenDocuments::document_ref(store, document_id) else {
        return;
    };
    if probe() && document.revision() != entity.saved_revision {
        eprintln!(
            "[watch] refetch landed: dirty (revision {} vs saved {}) — merging over the baseline",
            document.revision(),
            entity.saved_revision
        );
    }
    let base_revision = document.revision();
    let _ = fx.push(
        AnyEffect::new(RefetchDiffEffect {
            baseline: entity.baseline.clone(),
            current: document.text().clone(),
            fetched: text,
        })
        .map(move |rebase| wrap(document_id, base_revision, serial, rebase)),
    );
}
