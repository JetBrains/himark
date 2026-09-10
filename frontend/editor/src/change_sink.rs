use std::sync::Arc;

use imba::store::Store;
use text::Text;

use crate::document::Document;
use crate::editor::EditorEffects;

pub trait ChangeSink: Send + Sync {
    fn changed(
        &self,
        store: &Store,
        document: &Document,
        location: &crate::ResourceLocation,
        base_revision: u64,
        text_before: &Text,
        fx: &mut EditorEffects<'_>,
    );
}

#[derive(Clone, Default)]
pub struct InstalledChangeSink(pub rpds::VectorSync<Arc<dyn ChangeSink>>);

impl InstalledChangeSink {
    pub fn install(store: &mut Store, sink: Arc<dyn ChangeSink>) {
        let mut installed = store
            .get::<InstalledChangeSink>()
            .map(|sinks| sinks.0.clone())
            .unwrap_or_else(rpds::VectorSync::new_sync);
        installed.push_back_mut(sink);
        store.put(InstalledChangeSink(installed));
    }

    pub fn of(store: &Store) -> Vec<Arc<dyn ChangeSink>> {
        store
            .get::<InstalledChangeSink>()
            .map(|sinks| sinks.0.iter().map(Arc::clone).collect())
            .unwrap_or_default()
    }
}
