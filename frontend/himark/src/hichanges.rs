use std::collections::BTreeMap;
use std::sync::Arc;

use ::editor::{EditorCommand, EditorView};

use crate::higent::ahp_types::actions::StateAction;
use crate::higent::ahp_types::state::{ChangesetFile, ChangesetState, ChangesetStatus};
use crate::higent::{AhpServer, PollChangesetEffect, SubscribeChangesetEffect};
use crate::{
    AppCommand, Authority, ForestList, ForestNode, ForestSearcher, ModalRequest, ModalView,
    ResourceLocation, ResourceType, SpeedSearchCommand, SpeedSearchView, TreeListCommand,
};
use himark_ahp_ext_types::history as history_wire;
use imba::{
    arena::Arena,
    constraints::Constraints,
    container::container,
    effect::{AnyEffect, Effects},
    event::{Event, EventResult, Key as InputKey},
    leaf::leaf,
    store::Store,
    thunk_ext::ThunkExt,
    Thunk, UiCtx, View, Widget,
};
use skia_safe::{Paint, Rect, Size};

const NOTE_KIND: &str = "changes-note";

const PANEL_WIDTH: f32 = crate::DRAWER_WIDTH;

const PANEL_PAD: f32 = 6.0;

const REF_PREFIX: &str = "ahpref\u{1f}";

const EMPTY_AUTHORITY: &str = "changes-empty";

pub fn before_ref_location(
    origin_authority: &str,
    raw_uri: &str,
    path: Vec<String>,
) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        Authority::new(format!("{REF_PREFIX}{origin_authority}\u{1f}{raw_uri}")),
        path,
    )
}

pub fn scoped(location: &ResourceLocation) -> bool {
    location.authority().as_str().starts_with(REF_PREFIX)
}

pub fn raw_ref(location: &ResourceLocation) -> Option<(String, String)> {
    let body = location.authority().as_str().strip_prefix(REF_PREFIX)?;
    let (origin, raw) = body.split_once('\u{1f}')?;
    Some((origin.to_owned(), raw.to_owned()))
}

pub fn working_copy(location: &ResourceLocation) -> Option<ResourceLocation> {
    let (origin, _) = raw_ref(location)?;
    Some(ResourceLocation::new(
        ResourceType::document(),
        Authority::new(origin),
        location.path().to_vec(),
    ))
}

pub(crate) fn empty_side(of: &ResourceLocation) -> ResourceLocation {
    ResourceLocation::new(
        ResourceType::document(),
        Authority::new(EMPTY_AUTHORITY),
        of.path().to_vec(),
    )
}

#[derive(serde::Deserialize)]
struct WireSide {
    uri: String,
    #[serde(default)]
    content: Option<WireContent>,
}

#[derive(serde::Deserialize)]
struct WireContent {
    uri: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WireCounts {
    #[serde(default)]
    added: Option<i64>,
    #[serde(default)]
    removed: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeEntry {
    id: String,

    pub(crate) rel: Vec<String>,

    pub working: ResourceLocation,

    pub before: Option<ResourceLocation>,

    pub after: Option<ResourceLocation>,
    pub added: Option<i64>,
    pub removed: Option<i64>,
}

pub(crate) fn entry_of(
    uris: &dyn crate::higent::ResourceUriMap,
    folder: &ResourceLocation,
    file: &ChangesetFile,
) -> Option<ChangeEntry> {
    let side = |value: &Option<serde_json::Value>| -> Option<WireSide> {
        value
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    };
    let before = side(&file.edit.before);
    let after = side(&file.edit.after);
    let working = after.as_ref().or(before.as_ref()).and_then(|side| {
        uris.location_of(
            &crate::higent::ResourceUri::new(side.uri.as_str()),
            ResourceType::document(),
            folder.authority(),
        )
    })?;
    if !working.path().starts_with(folder.path()) {
        return None;
    }
    let rel: Vec<String> = working.path()[folder.path().len()..].to_vec();
    if rel.is_empty() {
        return None;
    }
    let path: Vec<String> = working.path().to_vec();
    let after_ref = after.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let before = before.and_then(|side| side.content).map(|content| {
        before_ref_location(folder.authority().as_str(), &content.uri, path.clone())
    });
    let counts: WireCounts = file
        .edit
        .diff
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    Some(ChangeEntry {
        id: file.id.clone(),
        rel,
        working,
        before,
        after: after_ref,
        added: counts.added,
        removed: counts.removed,
    })
}

#[derive(Clone, Default)]
pub struct ChangeRefs(Arc<std::sync::Mutex<std::collections::HashMap<String, ResourceLocation>>>);

impl ChangeRefs {
    pub fn lookup(&self, abs_path: &str) -> Option<ResourceLocation> {
        self.0.lock().unwrap().get(abs_path).cloned()
    }

    fn replace_folder<I: Iterator<Item = (String, ResourceLocation)>>(
        &self,
        folder_prefix: &str,
        fresh: I,
    ) {
        let mut map = self.0.lock().unwrap();
        map.retain(|abs, _| {
            !(abs.strip_prefix(folder_prefix)).is_some_and(|rest| rest.starts_with('/'))
        });
        for (abs, before) in fresh {
            map.insert(abs, before);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangesStatus {
    Computing,
    Ready,
    Error(String),
}

#[derive(Clone)]
pub struct FolderChanges {
    seat: Arc<dyn AhpServer>,

    session: String,

    channel: Option<String>,
    pub status: ChangesStatus,
    pub files: rpds::VectorSync<ChangeEntry>,
}

impl ChangesStatus {
    pub(crate) fn of_wire(status: &ChangesetStatus, error: Option<&str>) -> ChangesStatus {
        match status {
            ChangesetStatus::Computing => ChangesStatus::Computing,
            ChangesetStatus::Ready => ChangesStatus::Ready,
            ChangesetStatus::Error => {
                ChangesStatus::Error(error.unwrap_or("changeset error").to_owned())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    pub(crate) uri: String,
    pub(crate) description: Option<String>,

    pub(crate) kind: String,
}

#[derive(Clone)]
struct SessionFeed {
    uri: String,
    seat: Arc<dyn AhpServer>,
    catalog: rpds::VectorSync<CatalogEntry>,
}

fn digest_catalog(changesets: &[crate::higent::ahp_types::state::Changeset]) -> Vec<CatalogEntry> {
    changesets
        .iter()
        .filter(|entry| {
            (entry.change_kind == "uncommitted"
                || entry.change_kind == history_wire::HISTORY_CHANGE_KIND)
                && !entry.uri_template.contains('{')
        })
        .map(|entry| CatalogEntry {
            uri: entry.uri_template.clone(),
            description: entry.description.clone(),
            kind: entry.change_kind.clone(),
        })
        .collect()
}

pub(crate) fn entry_serves(folder: &ResourceLocation, entry: &CatalogEntry) -> bool {
    let abs = format!("/{}", folder.path().join("/"));
    entry.description.as_deref() == Some(abs.as_str()) || entry.uri.ends_with(&abs)
}

#[derive(Clone, Default)]
pub struct Changes {
    folders: rpds::HashTrieMapSync<ResourceLocation, FolderChanges>,

    session: Option<SessionFeed>,

    uris: Option<Arc<dyn crate::higent::ResourceUriMap>>,

    generation: u64,
}

#[derive(Clone, Default)]
pub struct ChangesInstall {
    refs: ChangeRefs,
}

impl Changes {
    pub fn install(store: &mut Store, refs: ChangeRefs) {
        store.put(ChangesInstall { refs });
    }

    pub fn installed(store: &Store) -> bool {
        store.get::<ChangesInstall>().is_some()
    }

    fn feed_for(&self, session: &str) -> Option<&SessionFeed> {
        self.session.as_ref().filter(|feed| feed.uri == session)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.session.is_none()
    }

    pub fn generation(store: &Store) -> u64 {
        store
            .get::<Changes>()
            .map(|changes| changes.generation)
            .unwrap_or(0)
    }

    pub fn folder(store: &Store, folder: &ResourceLocation) -> Option<FolderChanges> {
        store.get::<Changes>()?.folders.get(folder).cloned()
    }

    pub(crate) fn uris(store: &Store) -> Option<Arc<dyn crate::higent::ResourceUriMap>> {
        store.get::<Changes>()?.uris.clone()
    }

    pub fn ensure(
        store: &mut Store,
        window: crate::WindowId,
        workspace: crate::SessionId,
        fx: &mut crate::AppFx<'_>,
    ) {
        for folder in crate::higent::session_folders(store, &workspace) {
            Self::ensure_folder(store, window, folder, fx);
        }
    }

    pub fn ensure_folder(
        store: &mut Store,
        window: crate::WindowId,
        folder: ResourceLocation,
        fx: &mut crate::AppFx<'_>,
    ) {
        if !Self::installed(store) {
            return;
        }
        let known = store
            .get::<Changes>()
            .is_some_and(|changes| changes.folders.contains_key(&folder));
        if known {
            return;
        }
        let Some((host, seat, session)) =
            crate::higent::seat::route_seat(store, folder.authority().as_str())
        else {
            return;
        };
        let Some(uris) = crate::higent::Hosts::uris(store, host) else {
            return;
        };
        store.update::<Changes>(|changes| changes.uris = Some(uris.clone()));
        let scope = crate::SessionId {
            host,
            session: session.clone(),
        };
        store.update::<Changes>(|changes| {
            changes.folders.insert_mut(
                folder.clone(),
                FolderChanges {
                    seat: seat.clone(),
                    session: session.clone(),
                    channel: None,
                    status: ChangesStatus::Computing,
                    files: rpds::VectorSync::new_sync(),
                },
            );
            changes.generation += 1;
        });
        crate::hihistory::History::ensure_folder(store, &folder, &seat, &session);

        let directory = uris.uri_of(&folder).into_string();
        fx.push(
            AnyEffect::new(crate::higent::DispatchChatActionEffect {
                seat: seat.clone(),
                channel: session.clone(),
                action: StateAction::SessionWorkingDirectorySet(
                    crate::higent::ahp_types::actions::SessionWorkingDirectorySetAction {
                        directory,
                    },
                ),
            })
            .map(move |result| AppCommand::Dynamic(window, Arc::new(Dispatched { result }))),
        );
        let feed_known = store
            .get::<Changes>()
            .is_some_and(|changes| changes.feed_for(&session).is_some());
        if !feed_known {
            store.update::<Changes>(|changes| {
                changes.session = Some(SessionFeed {
                    uri: session.clone(),
                    seat: seat.clone(),
                    catalog: rpds::VectorSync::new_sync(),
                });
            });
            let landing = session.clone();
            fx.push(
                AnyEffect::new(crate::higent::SubscribeSessionEffect { seat, session }).map(
                    move |result| {
                        AppCommand::dynamic_in(
                            scope.clone(),
                            window,
                            Arc::new(SessionLanded {
                                session: landing.clone(),
                                result,
                            }),
                        )
                    },
                ),
            );
        }
    }

    pub fn script_summary(store: &Store) -> Option<String> {
        let changes = store.get::<Changes>()?;
        let mut lines = Vec::new();
        for (folder, entry) in changes.folders.iter() {
            for file in entry.files.iter() {
                let status = match file.before.is_some() {
                    true => "M",
                    false => "A",
                };
                let counts = match (file.added, file.removed) {
                    (Some(added), Some(removed)) => format!(" (+{added} -{removed})"),
                    _ => String::new(),
                };
                lines.push(format!(
                    "{status} {}/{}{counts}",
                    folder.name(),
                    file.rel.join("/"),
                ));
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    pub fn refetch(store: &mut Store, window: crate::WindowId, fx: &mut crate::AppFx<'_>) {
        let Some(changes) = store.get::<Changes>() else {
            return;
        };
        let riding: Vec<(ResourceLocation, Arc<dyn AhpServer>, String)> = changes
            .folders
            .iter()
            .filter_map(|(folder, entry)| {
                entry
                    .channel
                    .clone()
                    .map(|channel| (folder.clone(), entry.seat.clone(), channel))
            })
            .collect();
        if riding.is_empty() {
            return;
        }
        store.update::<Changes>(|changes| {
            for (folder, _, _) in &riding {
                if let Some(mut entry) = changes.folders.get(folder).cloned() {
                    entry.status = ChangesStatus::Computing;
                    changes.folders.insert_mut(folder.clone(), entry);
                }
            }
            changes.generation += 1;
        });
        for (folder, seat, channel) in riding {
            let landing = folder.clone();
            let scope = folder_scope(&folder);
            fx.push(
                AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                    let landed = Arc::new(SnapshotLanded {
                        folder: landing.clone(),
                        result,
                    });
                    match scope.clone() {
                        Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                        None => AppCommand::Dynamic(window, landed),
                    }
                }),
            );
        }
    }

    fn adopt_catalog(
        &mut self,
        session: &str,
        entries: Vec<CatalogEntry>,
    ) -> Vec<(ResourceLocation, Arc<dyn AhpServer>, String)> {
        let Some(mut feed) = self.feed_for(session).cloned() else {
            return Vec::new();
        };
        feed.catalog = entries.iter().cloned().collect();
        self.session = Some(feed);
        let changesets: Vec<CatalogEntry> = entries
            .iter()
            .filter(|entry| entry.kind == "uncommitted")
            .cloned()
            .collect();
        let mut fresh = Vec::new();
        let lone_folder = self
            .folders
            .iter()
            .filter(|(_, entry)| entry.session == session)
            .count()
            == 1;
        for (folder, entry) in self.folders.clone().iter() {
            if entry.session != session || entry.channel.is_some() {
                continue;
            }
            let matched = changesets
                .iter()
                .find(|candidate| entry_serves(folder, candidate))

                .or_else(|| (lone_folder && changesets.len() == 1).then(|| &changesets[0]));
            let Some(matched) = matched else {
                continue;
            };
            let mut entry = entry.clone();
            entry.channel = Some(matched.uri.clone());
            let seat = entry.seat.clone();
            let channel = matched.uri.clone();
            self.folders.insert_mut(folder.clone(), entry);
            fresh.push((folder.clone(), seat, channel));
        }
        self.generation += 1;
        fresh
    }

    fn adopt(&mut self, refs: &ChangeRefs, folder: &ResourceLocation, state: &ChangesetState) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = ChangesStatus::of_wire(
            &state.status,
            state.error.as_ref().map(|error| error.message.as_str()),
        );
        let Some(uris) = self.uris.clone() else {
            return;
        };
        entry.files = state
            .files
            .iter()
            .filter_map(|file| entry_of(&*uris, folder, file))
            .collect();
        Self::write_refs(refs, folder, &entry);
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }

    fn session_failed(&mut self, refs: &ChangeRefs, session: &str, error: &str) {
        let riding: Vec<ResourceLocation> = self
            .folders
            .iter()
            .filter(|(_, entry)| entry.session == session)
            .map(|(folder, _)| folder.clone())
            .collect();
        for folder in riding {
            self.adopt_error(refs, &folder, error.to_owned());
        }
    }

    fn adopt_error(&mut self, refs: &ChangeRefs, folder: &ResourceLocation, error: String) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        entry.status = ChangesStatus::Error(error);
        entry.files = rpds::VectorSync::new_sync();
        Self::write_refs(refs, folder, &entry);
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }

    fn fold(&mut self, refs: &ChangeRefs, folder: &ResourceLocation, actions: &[StateAction]) {
        let Some(mut entry) = self.folders.get(folder).cloned() else {
            return;
        };
        let Some(uris) = self.uris.clone() else {
            return;
        };

        let mut files: Vec<Option<ChangeEntry>> = entry.files.iter().cloned().map(Some).collect();
        let mut by_id: std::collections::HashMap<String, usize> = files
            .iter()
            .enumerate()
            .filter_map(|(at, slot)| slot.as_ref().map(|file| (file.id.clone(), at)))
            .collect();
        for action in actions {
            match action {
                StateAction::ChangesetContentChanged(content) => {
                    files = content
                        .files
                        .iter()
                        .filter_map(|file| entry_of(&*uris, folder, file))
                        .map(Some)
                        .collect();
                    by_id = files
                        .iter()
                        .enumerate()
                        .filter_map(|(at, slot)| slot.as_ref().map(|file| (file.id.clone(), at)))
                        .collect();
                }
                StateAction::ChangesetStatusChanged(status) => {
                    entry.status = ChangesStatus::of_wire(
                        &status.status,
                        status.error.as_ref().map(|error| error.message.as_str()),
                    );
                }
                StateAction::ChangesetFileSet(set) => {
                    if let Some(at) = by_id.remove(&set.file.id) {
                        files[at] = None;
                    }
                    if let Some(fresh) = entry_of(&*uris, folder, &set.file) {
                        by_id.insert(fresh.id.clone(), files.len());
                        files.push(Some(fresh));
                    }
                }
                StateAction::ChangesetFileRemoved(removed) => {
                    if let Some(at) = by_id.remove(&removed.file_id) {
                        files[at] = None;
                    }
                }
                StateAction::ChangesetCleared(_) => {
                    files.clear();
                    by_id.clear();
                }
                _ => {}
            }
        }
        entry.files = files.into_iter().flatten().collect();
        Self::write_refs(refs, folder, &entry);
        self.folders.insert_mut(folder.clone(), entry);
        self.generation += 1;
    }

    fn write_refs(refs: &ChangeRefs, folder: &ResourceLocation, entry: &FolderChanges) {
        let prefix = format!("/{}", folder.path().join("/"));
        refs.replace_folder(
            &prefix,
            entry.files.iter().filter_map(|change| {
                let before = change.before.clone()?;
                Some((format!("/{}", change.working.path().join("/")), before))
            }),
        );
    }
}

fn installed_refs(store: &Store) -> ChangeRefs {
    store
        .get::<ChangesInstall>()
        .map(|install| install.refs.clone())
        .unwrap_or_default()
}

pub(crate) struct Dispatched {
    pub(crate) result: Result<(), String>,
}

impl crate::DynamicCommand for Dispatched {
    fn id(&self) -> &'static str {
        "changes.dispatched"
    }
    fn name(&self) -> String {
        "Changes Dispatch".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        _window: crate::WindowId,
        _fx: &mut crate::AppFx<'_>,
    ) {
        if let Err(error) = &self.result {
            eprintln!("[hichanges] workingDirectorySet failed: {error}");
        }
    }
}

struct SessionLanded {
    session: String,
    result: Result<crate::higent::ahp_types::state::SessionState, String>,
}

impl crate::DynamicCommand for SessionLanded {
    fn id(&self) -> &'static str {
        "changes.session-landed"
    }
    fn name(&self) -> String {
        "Changes Catalog".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        match &self.result {
            Ok(state) => {
                let entries = digest_catalog(state.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, window, &self.session, entries, fx);
                relaunch_session_poll(store, window, &self.session, fx);
            }
            Err(error) => {
                eprintln!("[hichanges] session subscribe failed: {error}");
                let refs = installed_refs(store);
                store.update::<Changes>(|changes| {
                    changes.session_failed(&refs, &self.session, error);
                });
                crate::hihistory::History::session_failed(store, &self.session, error);
            }
        }
    }
}

struct SessionPolled {
    session: String,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for SessionPolled {
    fn id(&self) -> &'static str {
        "changes.session-polled"
    }
    fn name(&self) -> String {
        "Changes Catalog Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        for action in &self.actions {
            if let StateAction::SessionChangesetsChanged(changed) = action {
                let entries = digest_catalog(changed.changesets.as_deref().unwrap_or_default());
                subscribe_fresh(store, window, &self.session, entries, fx);
            }
        }
        relaunch_session_poll(store, window, &self.session, fx);
    }
}

fn subscribe_fresh(
    store: &mut Store,
    window: crate::WindowId,
    session: &str,
    entries: Vec<CatalogEntry>,
    fx: &mut crate::AppFx<'_>,
) {
    let mut fresh = Vec::new();
    store.update::<Changes>(|changes| {
        fresh = changes.adopt_catalog(session, entries.clone());
    });

    crate::hihistory::subscribe_fresh(store, window, session, &entries, fx);
    for (folder, seat, channel) in fresh {
        let landing = folder.clone();
        let scope = folder_scope(&folder);
        fx.push(
            AnyEffect::new(SubscribeChangesetEffect { seat, channel }).map(move |result| {
                let landed = Arc::new(SnapshotLanded {
                    folder: landing.clone(),
                    result,
                });
                match scope.clone() {
                    Some(scope) => AppCommand::dynamic_in(scope, window, landed),
                    None => AppCommand::Dynamic(window, landed),
                }
            }),
        );
    }
}

pub(crate) fn folder_scope(folder: &ResourceLocation) -> Option<crate::SessionId> {
    let (host, session) = crate::higent::seat::parse(folder.authority().as_str())?;
    Some(crate::SessionId { host, session })
}

fn relaunch_session_poll(
    store: &Store,
    window: crate::WindowId,
    session: &str,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(feed) = store
        .get::<Changes>()
        .and_then(|changes| changes.feed_for(session).cloned())
    else {
        return;
    };
    let landing = session.to_owned();

    let scope = crate::Gathered::scope(store).cloned();
    fx.push(
        AnyEffect::new(crate::higent::PollSessionEffect {
            seat: feed.seat,
            session: session.to_owned(),
        })
        .map(move |actions| {
            let polled = Arc::new(SessionPolled {
                session: landing.clone(),
                actions,
            });
            match scope.clone() {
                Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                None => AppCommand::Dynamic(window, polled),
            }
        }),
    );
}

struct SnapshotLanded {
    folder: ResourceLocation,
    result: Result<ChangesetState, String>,
}

impl crate::DynamicCommand for SnapshotLanded {
    fn id(&self) -> &'static str {
        "changes.snapshot-landed"
    }
    fn name(&self) -> String {
        "Changes Snapshot".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let refs = installed_refs(store);
        store.update::<Changes>(|changes| match &self.result {
            Ok(state) => changes.adopt(&refs, &self.folder, state),
            Err(error) => changes.adopt_error(&refs, &self.folder, error.clone()),
        });
        rearm_stripes(store, &self.folder, fx);
        if self.result.is_ok() {
            relaunch_poll(store, window, &self.folder, fx);
        }
    }
}

struct Polled {
    folder: ResourceLocation,
    actions: Vec<StateAction>,
}

impl crate::DynamicCommand for Polled {
    fn id(&self) -> &'static str {
        "changes.polled"
    }
    fn name(&self) -> String {
        "Changes Update".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let refs = installed_refs(store);
        store.update::<Changes>(|changes| changes.fold(&refs, &self.folder, &self.actions));
        rearm_stripes(store, &self.folder, fx);
        relaunch_poll(store, window, &self.folder, fx);
    }
}

fn rearm_stripes(store: &mut Store, folder: &ResourceLocation, fx: &mut crate::AppFx<'_>) {
    let authority = folder.authority().clone();
    let prefix = format!("/{}/", folder.path().join("/"));
    crate::rearm_base_asks(store, &|location| {
        location.authority() == &authority
            && format!("/{}", location.path().join("/")).starts_with(&prefix)
    });
    crate::sync_stripe_bases(store, fx);
}

fn relaunch_poll(
    store: &Store,
    window: crate::WindowId,
    folder: &ResourceLocation,
    fx: &mut crate::AppFx<'_>,
) {
    let Some(entry) = Changes::folder(store, folder) else {
        return;
    };
    let Some(channel) = entry.channel else {
        return;
    };
    let landing = folder.clone();
    let scope = folder_scope(folder);
    fx.push(
        AnyEffect::new(PollChangesetEffect {
            seat: entry.seat,
            channel,
        })
        .map(move |actions| {
            let polled = Arc::new(Polled {
                folder: landing.clone(),
                actions,
            });
            match scope.clone() {
                Some(scope) => AppCommand::dynamic_in(scope, window, polled),
                None => AppCommand::Dynamic(window, polled),
            }
        }),
    );
}

pub(crate) struct OpenDiffForPair {
    pub(crate) old: ResourceLocation,
    pub(crate) new: ResourceLocation,
}

impl crate::DynamicCommand for OpenDiffForPair {
    fn id(&self) -> &'static str {
        "changes.open-diff"
    }
    fn name(&self) -> String {
        "Open Diff".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        _store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let _ = fx.push(AnyEffect::new(crate::OpenDiffByLocationsEffect {
            window,
            old: self.old.clone(),
            new: self.new.clone(),
        }));
    }
}

#[derive(Clone)]
enum RowItem {
    Branch,

    File {
        old: Option<ResourceLocation>,
        new: ResourceLocation,
    },

    Note,
}

#[derive(Default)]
pub(crate) struct DirTrie {
    dirs: BTreeMap<String, DirTrie>,
    files: Vec<ChangeEntry>,
}

impl DirTrie {
    pub(crate) fn insert(&mut self, entry: ChangeEntry) {
        let mut node = self;
        for segment in &entry.rel[..entry.rel.len() - 1] {
            node = node.dirs.entry(segment.clone()).or_default();
        }
        node.files.push(entry);
    }
}

fn folder_node(
    folder: &ResourceLocation,
    changes: Option<&FolderChanges>,
    items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
    counts: (skia_safe::Color, skia_safe::Color),
) -> ForestNode<ResourceLocation> {
    items.insert_mut(folder.clone(), RowItem::Branch);
    let note = |text: &str, items: &mut rpds::HashTrieMapSync<ResourceLocation, RowItem>| {
        let key = folder.child(ResourceType::new(NOTE_KIND), text);
        items.insert_mut(key.clone(), RowItem::Note);
        vec![ForestNode {
            key,
            label: text.to_owned(),
            pick: false,
            dim: true,
            trail: Vec::new(),
            children: Vec::new(),
        }]
    };
    let children = match changes {
        None => note("no changes source", items),
        Some(changes) => match (&changes.status, changes.files.is_empty()) {
            (ChangesStatus::Error(message), _) => note(message, items),
            (ChangesStatus::Computing, true) => note("computing…", items),
            (ChangesStatus::Ready, true) => note("no changes", items),
            _ => {
                let mut trie = DirTrie::default();
                for entry in changes.files.iter() {
                    trie.insert(entry.clone());
                }
                struct Sink<'a> {
                    items: &'a mut rpds::HashTrieMapSync<ResourceLocation, RowItem>,
                }
                impl DirSink for Sink<'_> {
                    fn branch(&mut self, key: &ResourceLocation) {
                        self.items.insert_mut(key.clone(), RowItem::Branch);
                    }

                    fn file_key(
                        &self,
                        entry: &ChangeEntry,
                        _at: &ResourceLocation,
                    ) -> ResourceLocation {
                        entry.working.clone()
                    }
                    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation) {
                        self.items.insert_mut(
                            key.clone(),
                            RowItem::File {
                                old: entry.before.clone(),
                                new: entry.working.clone(),
                            },
                        );
                    }
                }
                dir_forest(folder, trie, counts, &mut Sink { items })
            }
        },
    };
    ForestNode {
        key: folder.clone(),
        label: folder.name().to_owned(),
        pick: false,
        dim: false,
        trail: Vec::new(),
        children,
    }
}

pub(crate) trait DirSink {
    fn branch(&mut self, key: &ResourceLocation);
    fn file_key(&self, entry: &ChangeEntry, at: &ResourceLocation) -> ResourceLocation;
    fn file(&mut self, entry: &ChangeEntry, key: &ResourceLocation);
}

pub(crate) fn dir_forest(
    at: &ResourceLocation,
    trie: DirTrie,
    counts: (skia_safe::Color, skia_safe::Color),
    sink: &mut dyn DirSink,
) -> Vec<ForestNode<ResourceLocation>> {
    let mut children = Vec::new();
    for (name, sub) in trie.dirs {
        let mut label = name.clone();
        let mut location = at.child(ResourceType::directory(), &name);
        let mut sub = sub;
        while sub.files.is_empty() && sub.dirs.len() == 1 {
            let (name, inner) = sub.dirs.into_iter().next().expect("the single child");
            label.push('/');
            label.push_str(&name);
            location = location.child(ResourceType::directory(), &name);
            sub = inner;
        }
        sink.branch(&location);
        let nested = dir_forest(&location, sub, counts, sink);
        children.push(ForestNode {
            key: location,
            label,
            pick: false,
            dim: false,
            trail: Vec::new(),
            children: nested,
        });
    }
    let mut files = trie.files;
    files.sort_by(|a, b| a.working.name().cmp(b.working.name()));
    for entry in files {
        let label = entry.working.name().to_owned();

        let mut trail = Vec::new();
        if entry.added.is_some() || entry.removed.is_some() {
            trail.push((format!("+{}", entry.added.unwrap_or(0)), counts.0));
            trail.push((format!("−{}", entry.removed.unwrap_or(0)), counts.1));
        }
        let key = sink.file_key(&entry, at);
        sink.file(&entry, &key);
        children.push(ForestNode {
            key,
            label,
            pick: true,
            dim: false,
            trail,
            children: Vec::new(),
        });
    }
    children
}

pub enum ChangesCommand {
    Rows(SpeedSearchCommand<TreeListCommand>),

    Message(EditorCommand),

    FocusMessage(bool),

    Commit,

    Select(isize),

    Fold(bool),

    Pick,

    Refresh,

    Refetch,

    Dismiss,
}

pub struct ChangesView {
    list: SpeedSearchView<ForestList<ResourceLocation>, ForestSearcher<ResourceLocation>>,
    items: rpds::HashTrieMapSync<ResourceLocation, RowItem>,

    message: EditorView,

    message_focused: bool,

    workspace: crate::SessionId,

    window: crate::WindowId,

    seen: u64,
    request: Option<ModalRequest>,
}

impl Clone for ChangesView {
    fn clone(&self) -> Self {
        Self {
            list: self.list.clone(),
            items: self.items.clone(),
            message: self.message.clone(),
            message_focused: self.message_focused,
            workspace: self.workspace.clone(),
            window: self.window,
            seen: self.seen,

            request: None,
        }
    }
}

impl ChangesView {
    pub fn open(store: &Store, window: crate::WindowId, workspace: crate::SessionId) -> Self {
        let mut panel = Self {
            list: SpeedSearchView::new(
                ForestList::new(store),
                ForestSearcher::default(),
                crate::env::Fonts::of(store),
            ),
            items: rpds::HashTrieMapSync::new_sync(),
            message: EditorView::input(600.0, crate::fonts::source()),
            message_focused: false,
            workspace,
            window,
            seen: 0,
            request: None,
        };
        panel.refresh(store);
        panel
    }

    #[doc(hidden)]
    pub fn message_text(&self) -> String {
        let text = self.message.document.text();
        let end = text.byte_count().min(u32::MAX as usize) as u32;
        text.view().substring(0..end)
    }

    #[doc(hidden)]
    pub fn message_focused(&self) -> bool {
        self.message_focused
    }

    pub fn row_count(&self) -> usize {
        self.list.inner().list().len()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn rows(&self) -> Vec<(u8, String, bool)> {
        self.list.inner().forest.rows_trailed()
    }

    fn refresh(&mut self, store: &Store) {
        self.seen = Changes::generation(store);
        let mut items = rpds::HashTrieMapSync::new_sync();
        let chat = crate::env::Themes::of(store).ui().chat.clone();
        let counts = (chat.added_color.0, chat.removed_color.0);
        let nodes: Vec<ForestNode<ResourceLocation>> =
            crate::higent::session_folders(store, &self.workspace)
                .iter()
                .map(|folder| {
                    folder_node(
                        folder,
                        Changes::folder(store, folder).as_ref(),
                        &mut items,
                        counts,
                    )
                })
                .collect();
        self.items = items;
        self.list.inner_mut().set(&nodes);
    }

    pub fn activate(&mut self, index: usize) {
        let Some(key) = self.list.inner().list().key_at(index).cloned() else {
            return;
        };
        self.activate_key(&key);
    }

    fn activate_key(&mut self, key: &ResourceLocation) {
        match self.items.get(key).cloned() {
            Some(RowItem::Branch) => self.list.inner_mut().toggle(key),
            Some(RowItem::File { old, new }) => {
                self.list.inner_mut().list_mut().select_only(key.clone());

                let old = old.unwrap_or_else(|| empty_side(&new));
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(OpenDiffForPair { old, new }),
                )));
            }
            Some(RowItem::Note) | None => {}
        }
    }
}

impl View for ChangesView {
    type Command = ChangesCommand;

    fn destroy(&mut self, _store: &mut Store, fx: &mut Effects<'_, Self::Command>) {
        fx.scope(ChangesCommand::Rows, |fx| self.list.clear(fx));
    }

    fn perform(
        &mut self,
        store: &mut Store,
        ui: &UiCtx,
        command: Self::Command,
        fx: &mut Effects<'_, Self::Command>,
    ) {
        match command {
            ChangesCommand::Rows(command) => {
                if let SpeedSearchCommand::Inner(inner) = &command {
                    if let Some((index, _)) = crate::tree_interaction(inner) {
                        self.message_focused = false;
                        return self.activate(index);
                    }
                }
                fx.scope(ChangesCommand::Rows, |fx| {
                    self.list.perform(store, ui, command, fx)
                });
            }

            ChangesCommand::Message(command) => {
                if matches!(command, EditorCommand::Click { .. }) && !self.message_focused {
                    self.message_focused = true;
                    self.message.focus_text();
                }
                fx.scope(ChangesCommand::Message, |fx| {
                    self.message.perform(store, ui, command, fx)
                });
            }
            ChangesCommand::FocusMessage(focused) => {
                self.message_focused = focused;
                match focused {
                    true => self.message.focus_text(),
                    false => self.message.blur(),
                }
            }
            ChangesCommand::Commit => {
                let message = self.message_text();
                if message.trim().is_empty() {
                    return;
                }

                let folder = crate::higent::session_folders(store, &self.workspace)
                    .into_iter()
                    .find(|folder| {
                        crate::hihistory::History::folder(store, folder)
                            .is_some_and(|entry| entry.channel_named())
                    });
                let Some(folder) = folder else {
                    return;
                };
                self.message = EditorView::input(600.0, crate::fonts::source());
                self.message_focused = false;
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(crate::hihistory::CommitHistory { folder, message }),
                )));
            }
            ChangesCommand::Select(delta) => self.list.inner_mut().list_mut().cursor_step(delta),
            ChangesCommand::Fold(expand) => self.list.inner_mut().fold_cursor(expand),
            ChangesCommand::Pick => {
                if let Some(key) = self.list.inner().list().cursor().cloned() {
                    self.activate_key(&key);
                }
            }
            ChangesCommand::Refresh => self.refresh(store),
            ChangesCommand::Refetch => {
                self.request = Some(ModalRequest::Perform(AppCommand::Dynamic(
                    self.window,
                    Arc::new(RefetchChanges),
                )));
            }
            ChangesCommand::Dismiss => {
                self.request = Some(ModalRequest::Close);
            }
        }
    }

    fn layout<'a>(
        &'a self,
        arena: &'a Arena,
        store: &'a Store,
        ui: &'a UiCtx,
        constraints: Constraints,
    ) -> impl Thunk<'a, Self::Command> + 'a {
        let size = constraints.max;
        let mut overlay = container(arena, size);

        let chrome = crate::env::Themes::of(store).ui().peeker.clone();
        let search = crate::env::Themes::of(store).ui().search.clone();
        let chip_height = chrome.hint_size * 2.0;

        let well_height = search.input_height;
        let box_band = well_height + PANEL_PAD;
        let band = chrome.margin + chip_height + PANEL_PAD + box_band;
        let rows = self
            .list
            .layout(
                arena,
                store,
                ui,
                Constraints::tight(Size::new(size.width, size.height - band)),
            )
            .map(ChangesCommand::Rows);
        overlay.place(0.0, band, rows);

        let well_y = chrome.margin + chip_height + PANEL_PAD;
        let well_pad = chrome.margin;
        let well_width = (size.width - well_pad * 2.0).max(1.0);
        let input_fill = search.input_fill;
        let message_focused = self.message_focused;
        let message_empty = self.message.document.text().byte_count() == 0;
        let placeholder_font = crate::fonts::ui_text_font(ui, chrome.hint_size);
        let placeholder_dim = chrome.dim_text.0;
        let well = leaf::<ChangesCommand>(well_width, well_height)
            .paint_instead(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(input_fill.0);
                canvas.draw_round_rect(rect, 6.0, 6.0, &paint);
                if message_empty && !message_focused {
                    paint.set_color(placeholder_dim);
                    canvas.draw_str(
                        "Message (⌘⏎ to commit)",
                        (
                            rect.left + 10.0,
                            rect.top + rect.height() * 0.5 + chrome.hint_size * 0.35,
                        ),
                        &placeholder_font,
                        &paint,
                    );
                }
            })
            .event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::Command(ChangesCommand::FocusMessage(true)),
                _ => EventResult::Ignored,
            });
        overlay.place(well_pad, well_y, well);
        let inner_height = (well_height - search.input_pad_y * 2.0).max(1.0);
        overlay.place(
            well_pad + search.input_pad_x,
            well_y + search.input_pad_y,
            self.message
                .layout(
                    arena,
                    store,
                    ui,
                    Constraints {
                        min: Size::new(0.0, inner_height),
                        max: Size::new(
                            (well_width - search.input_pad_x * 2.0).max(1.0),
                            inner_height,
                        ),
                    },
                )
                .map(ChangesCommand::Message)
                .focus_scope(self.message_focused),
        );

        let chip_font = crate::fonts::ui_font(ui, chrome.hint_size);
        let advance = chip_font.measure_str("REFRESH", None).0;
        let chip_width = advance + chrome.hint_size * 2.0;
        let chip_radius = chrome.well_radius;
        let rule = chrome.rule.0;
        let dim = chrome.dim_text.0;
        let chip = leaf::<ChangesCommand>(chip_width, chip_height)
            .paint_instead(move |_arena, canvas, rect| {
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_stroke(true);
                paint.set_stroke_width(1.0);
                paint.set_color(rule);
                canvas.draw_round_rect(
                    rect.with_inset((0.5, 0.5)),
                    chip_radius,
                    chip_radius,
                    &paint,
                );
                paint.set_stroke(false);
                paint.set_color(dim);
                canvas.draw_str(
                    "REFRESH",
                    (
                        rect.left + (rect.width() - advance) * 0.5,
                        rect.top + rect.height() * 0.5 + 6.0,
                    ),
                    &chip_font,
                    &paint,
                );
            })
            .event(|_arena, event, _size| match event {
                Event::MouseDown { .. } => EventResult::Command(ChangesCommand::Refetch),
                _ => EventResult::Ignored,
            });

        let searching = self.list.searching();
        let message_focused = self.message_focused;
        let keymap =
            leaf::<ChangesCommand>(size.width, size.height).event(move |_arena, event, _size| {
                match event {
                    Event::KeyDown {
                        key: InputKey::Enter,
                        mods,
                    } if mods.command => EventResult::Command(ChangesCommand::Commit),
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if message_focused => {
                        EventResult::Command(ChangesCommand::FocusMessage(false))
                    }
                    _ if message_focused => EventResult::Ignored,
                    Event::KeyDown {
                        key: InputKey::Escape,
                        ..
                    } if !searching => EventResult::Command(ChangesCommand::Dismiss),
                    Event::KeyDown {
                        key: InputKey::Up, ..
                    } if !searching => EventResult::Command(ChangesCommand::Select(-1)),
                    Event::KeyDown {
                        key: InputKey::Down,
                        ..
                    } if !searching => EventResult::Command(ChangesCommand::Select(1)),
                    Event::KeyDown {
                        key: InputKey::Left,
                        ..
                    } if !searching => EventResult::Command(ChangesCommand::Fold(false)),
                    Event::KeyDown {
                        key: InputKey::Right,
                        ..
                    } if !searching => EventResult::Command(ChangesCommand::Fold(true)),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } if searching => EventResult::Commands(vec![
                        ChangesCommand::Pick,
                        ChangesCommand::Rows(SpeedSearchCommand::Clear),
                    ]),
                    Event::KeyDown {
                        key: InputKey::Enter,
                        ..
                    } => EventResult::Command(ChangesCommand::Pick),
                    _ => EventResult::Ignored,
                }
            });
        overlay.place(0.0, 0.0, keymap);
        overlay.place(
            (size.width - chrome.margin - chip_width).max(0.0),
            chrome.margin,
            chip,
        );

        let stale = Changes::generation(store) != self.seen;
        overlay.wrap(move |inner| ReconcileShell { inner, stale })
    }
}

struct ReconcileShell<Inner> {
    inner: Inner,
    stale: bool,
}

impl<'a, Inner: Widget<'a, ChangesCommand>> Widget<'a, ChangesCommand> for ReconcileShell<Inner> {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn overlays(&mut self) -> Vec<imba::overlay::Overlay<'a, ChangesCommand>> {
        self.inner.overlays()
    }

    fn handle_event(
        &self,
        arena: &Arena,
        event: &Event<'_>,
        viewport: Rect,
    ) -> EventResult<ChangesCommand> {
        let result = self.inner.handle_event(arena, event, viewport);
        if matches!(event, Event::Paint { .. }) && self.stale {
            return result.merge(EventResult::Command(ChangesCommand::Refresh));
        }
        result
    }

    fn focus_data<'w>(&'w mut self) -> imba::focus::FocusData<'w, ChangesCommand>
    where
        'a: 'w,
    {
        self.inner.focus_data()
    }
}

impl ModalView for ChangesView {
    fn clone_modal(&self) -> Box<dyn ModalView> {
        Box::new(self.clone())
    }

    fn take_request(&mut self) -> Option<ModalRequest> {
        self.request.take()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct ToggleChangesView;

impl crate::DynamicCommand for ToggleChangesView {
    fn id(&self) -> &'static str {
        "changes.view"
    }
    fn name(&self) -> String {
        "Changes".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        let mut entity = crate::Windows::window(store, window).expect("the window entity");
        if entity.dock_owner() == Some(self.id()) {
            entity.roll_away_dock();
            crate::Windows::put(store, window, entity);
            return;
        }
        let workspace = entity.current_session();
        Changes::ensure(store, window, workspace.clone(), fx);

        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.dismiss_modal(store, fx),
        );
        let panel = ChangesView::open(store, window, workspace);
        let owner = self.id();
        fx.scope(
            move |command| crate::AppCommand::Content(window, command),
            |fx| entity.show_dock(store, Box::new(panel), owner, fx),
        );
        crate::Windows::put(store, window, entity);
    }
}

pub struct RefetchChanges;

impl crate::DynamicCommand for RefetchChanges {
    fn id(&self) -> &'static str {
        "changes.refetch"
    }
    fn name(&self) -> String {
        "Refresh Changes".to_owned()
    }
    fn perform(
        &self,
        _app: &mut crate::Application,
        store: &mut Store,
        window: crate::WindowId,
        fx: &mut crate::AppFx<'_>,
    ) {
        Changes::refetch(store, window, fx);
    }
}

pub fn toolbar_button() -> crate::ToolbarButton {
    crate::ToolbarButton {
        command: "changes.view",
        order: 1.0,
        side: crate::ToolbarSide::Right,
        glyph: Arc::new(|canvas, rect, color| {
            let mut paint = skia_safe::Paint::default();
            paint.set_anti_alias(true);
            paint.set_color(color);
            paint.set_style(skia_safe::paint::Style::Stroke);
            paint.set_stroke_width((rect.width() * 0.09).max(1.0));
            paint.set_stroke_cap(skia_safe::paint::Cap::Round);
            let (l, t, w, h) = (rect.left, rect.top, rect.width(), rect.height());
            let mut path = skia_safe::PathBuilder::new();

            let (px, py, arm) = (l + w * 0.32, t + h * 0.32, w * 0.17);
            path.move_to((px - arm, py));
            path.line_to((px + arm, py));
            path.move_to((px, py - arm));
            path.line_to((px, py + arm));

            let (mx, my) = (l + w * 0.68, t + h * 0.74);
            path.move_to((mx - arm, my));
            path.line_to((mx + arm, my));
            canvas.draw_path(&path.detach(), &paint);
        }),
    }
}

#[cfg(test)]
mod tests;
