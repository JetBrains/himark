// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The SESSION C SURFACE — the AHP half of the app's C ABI, over
//! himark-ffi's public doors (the hitailnet precedent: a plugin
//! owns its whole face). Generates `himark-ahp.h`; himark-ffi and
//! its `himark.h` stay AHP-free.

use std::ffi::c_char;
use std::sync::{Arc, LazyLock, Mutex};

use editor::location::ResourceLocation;
use imba::effect::EffectHandler;

use himark_ffi::bridges::{refuse, HimarkPlace, NavigateExport, NavigateExporter, PlaceMintRider};
use himark_ffi::host::HostBridge;
use himark_ffi::{copy_optional_utf8, str_from, HimarkEngine};

use crate::{Catalog, SessionRow};

/// The held catalog snapshots — the same handle discipline as
/// himark-ffi's own tables, in this crate because the type is.
struct CatalogTable {
    next: u64,
    held: std::collections::HashMap<u64, Catalog>,
}

impl CatalogTable {
    fn mint(&mut self, value: Catalog) -> u64 {
        self.next += 1;
        self.held.insert(self.next, value);
        self.next
    }
    fn get(&self, raw: u64) -> Option<&Catalog> {
        self.held.get(&raw)
    }
    fn take(&mut self, raw: u64) -> Option<Catalog> {
        self.held.remove(&raw)
    }
}

fn catalogs() -> &'static Mutex<CatalogTable> {
    static CATALOGS: LazyLock<Mutex<CatalogTable>> = LazyLock::new(|| {
        Mutex::new(CatalogTable {
            next: 0,
            held: std::collections::HashMap::new(),
        })
    });
    &CATALOGS
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkCatalog {
    pub raw: u64,
}

/// Snapshot the catalog — an owned, immutable copy behind a
/// handle; strings read out of it stay valid until its release.
#[no_mangle]
pub unsafe extern "C" fn himark_sessions_copy(engine: *mut HimarkEngine) -> HimarkCatalog {
    let Some(engine) = engine.as_mut() else {
        return HimarkCatalog { raw: 0 };
    };
    let snapshot = crate::engine::catalog(engine);
    HimarkCatalog {
        raw: catalogs().lock().expect("the catalog table").mint(snapshot),
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_release(engine: *mut HimarkEngine, catalog: HimarkCatalog) {
    let Some(engine) = engine.as_mut() else {
        return;
    };
    let _ = engine;
    if catalogs().lock().expect("the catalog table").take(catalog.raw).is_none() {
        refuse("catalog", catalog.raw);
    }
}

unsafe fn with_catalog<R>(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    missing: R,
    read: impl FnOnce(&crate::Catalog) -> R,
) -> R {
    if engine.is_null() {
        return missing;
    }
    let _ = engine;
    match catalogs().lock().expect("the catalog table").get(catalog.raw) {
        Some(snapshot) => read(snapshot),
        None => {
            refuse("catalog", catalog.raw);
            missing
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_hosts(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| snapshot.hosts.len())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_id(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.hosts.get(index).map_or(0, |host| host.host.raw())
    })
}

/// 0 idle · 1 connecting · 2 connected · 3 failed.
#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_status(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u32 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.hosts.get(index).map_or(0, |host| host.status)
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_name(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| {
        copy_optional_utf8(
            snapshot.hosts.get(index).map(|host| host.name.clone()),
            out,
            cap,
        )
    })
}

/// The connect error under a FAILED host row (UTF-8, no NUL;
/// returns the byte length, copies when it fits). Empty otherwise.
#[no_mangle]
pub unsafe extern "C" fn himark_catalog_host_error(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| {
        copy_optional_utf8(
            snapshot
                .hosts
                .get(index)
                .and_then(|host| host.error.clone()),
            out,
            cap,
        )
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_sessions(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| snapshot.sessions.len())
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_host(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> u64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.sessions.get(index).map_or(0, |row| row.host.raw())
    })
}

#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_status(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
) -> i64 {
    with_catalog(engine, catalog, 0, |snapshot| {
        snapshot.sessions.get(index).map_or(0, |row| row.status)
    })
}

unsafe fn catalog_session_string(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    out: *mut c_char,
    cap: usize,
    read: impl Fn(&SessionRow) -> String,
) -> usize {
    with_catalog(engine, catalog, 0, |snapshot| {
        copy_optional_utf8(snapshot.sessions.get(index).map(read), out, cap)
    })
}

/// Which string `himark_catalog_session_string` reads.
pub const HIMARK_SESSION_URI: u32 = 0;
pub const HIMARK_SESSION_TITLE: u32 = 1;
pub const HIMARK_SESSION_PROVIDER: u32 = 2;
pub const HIMARK_SESSION_MODIFIED: u32 = 3;

/// ONE sized-string door for the catalog row's text fields.
#[no_mangle]
pub unsafe extern "C" fn himark_catalog_session_string(
    engine: *mut HimarkEngine,
    catalog: HimarkCatalog,
    index: usize,
    field: u32,
    out: *mut c_char,
    cap: usize,
) -> usize {
    catalog_session_string(engine, catalog, index, out, cap, move |row| match field {
        HIMARK_SESSION_URI => row.uri.clone(),
        HIMARK_SESSION_TITLE => row.title.clone(),
        HIMARK_SESSION_PROVIDER => row.provider.clone(),
        HIMARK_SESSION_MODIFIED => row.modified_at.clone(),
        _ => String::new(),
    })
}

/// Enter a session headlessly: subscribe, adopt, wire, poll — the
/// native shell's selection. The session's views are mintable
/// after the entry lands; the catalog carries the visible state.
#[no_mangle]
pub unsafe extern "C" fn himark_session_enter(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some(uri) = str_from(uri, len) else {
        return false;
    };
    crate::engine::enter_session(engine, 
        ahp_wire::client::HostId::from_raw(host),
        ahp_wire::client::SessionUri::new(uri.to_owned()),
    )
}

/// Create a session headlessly on the host (0 = the local
/// default); the landing enters it.
#[no_mangle]
pub unsafe extern "C" fn himark_session_new(engine: *mut HimarkEngine, host: u64) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let host = match host {
        0 => None,
        raw => Some(ahp_wire::client::HostId::from_raw(raw)),
    };
    crate::engine::create_session(engine, host)
}

/// The place-kind vocabulary the callback speaks. `DOCUMENT` and
/// `DIFF` are READY places — minting is synchronous; the session
/// kinds are the tab places the host already knows how to mint.
pub const HIMARK_PLACE_CHAT: u32 = 1;
pub const HIMARK_PLACE_CHANGES: u32 = 2;
pub const HIMARK_PLACE_FILES: u32 = 3;
pub const HIMARK_PLACE_DOCUMENT: u32 = 4;
pub const HIMARK_PLACE_DIFF: u32 = 5;
pub const HIMARK_PLACE_CANVAS: u32 = 6;
/// Not a view: the ORIGIN's hosting face asks to close.
pub const HIMARK_PLACE_DISMISS: u32 = 7;
// The remaining session-view kinds — `himark_place_session_view`'s
// half of the vocabulary; the callback never speaks them.
pub const HIMARK_PLACE_CHAT_TRANSCRIPT: u32 = 8;
pub const HIMARK_PLACE_CHAT_COMPOSER: u32 = 9;
pub const HIMARK_PLACE_CHANGES_CANVAS: u32 = 10;
pub const HIMARK_PLACE_HISTORY: u32 = 11;
pub const HIMARK_PLACE_SEARCH: u32 = 12;
pub const HIMARK_PLACE_COMMENTS: u32 = 13;
pub const HIMARK_PLACE_PEEKER: u32 = 14;
pub const HIMARK_PLACE_PALETTE: u32 = 15;
pub const HIMARK_PLACE_SESSIONS: u32 = 16;
/// The session host's folder picker — BOTH halves speak it: a rust
/// road navigates it (the add-folder ask on a remote host) and the
/// native shell may mint it directly.
pub const HIMARK_PLACE_FOLDER_PICKER: u32 = 17;

unsafe fn mint_session_place(
    engine: *mut HimarkEngine,
    host: u64,
    uri: *const c_char,
    len: usize,
    place: impl Fn(ahp_wire::SessionId) -> hikit::navigation::NavigationLocation,
) -> HimarkPlace {
    let Some(engine) = engine.as_mut() else {
        return HimarkPlace { raw: 0 };
    };
    let Some(uri) = str_from(uri, len) else {
        return HimarkPlace { raw: 0 };
    };
    let session = ahp_wire::SessionId {
        host: ahp_wire::client::HostId::from_raw(host),
        session: ahp_wire::client::SessionUri::new(uri.to_owned()),
    };
    let _ = engine;
    himark_ffi::bridges::mint_place_handle(
        place(session.clone()),
        Some(std::sync::Arc::new(session) as himark_ffi::bridges::PlacePayload),
    )
}

/// ONE minting door for every session view: `kind` speaks the
/// `HIMARK_PLACE_*` vocabulary (the session-view kinds — the READY
/// kinds `DOCUMENT`/`DIFF`/`CANVAS` and `DISMISS` are the navigate
/// callback's and refuse here).
#[no_mangle]
pub unsafe extern "C" fn himark_place_session_view(
    engine: *mut HimarkEngine,
    kind: u32,
    host: u64,
    uri: *const c_char,
    len: usize,
) -> HimarkPlace {
    use crate::places::*;
    
    use hikit::navigation::NavigationLocation;
    let make: fn(ahp_wire::SessionId) -> NavigationLocation = match kind {
        HIMARK_PLACE_CHAT => |s| NavigationLocation::new(ChatPlace(s)),
        HIMARK_PLACE_CHAT_TRANSCRIPT => |s| NavigationLocation::new(ChatTranscriptPlace(s)),
        HIMARK_PLACE_CHAT_COMPOSER => |s| NavigationLocation::new(ChatComposerPlace(s)),
        HIMARK_PLACE_CHANGES => |s| NavigationLocation::new(ChangesPlace(s)),
        HIMARK_PLACE_CHANGES_CANVAS => |s| NavigationLocation::new(ChangesCanvasPlace(s)),
        HIMARK_PLACE_HISTORY => |s| NavigationLocation::new(HistoryPlace(s)),
        HIMARK_PLACE_SEARCH => |s| NavigationLocation::new(SearchPlace(s)),
        HIMARK_PLACE_COMMENTS => |s| NavigationLocation::new(CommentsPlace(s)),
        HIMARK_PLACE_PEEKER => |s| NavigationLocation::new(PeekerPlace(s)),
        HIMARK_PLACE_PALETTE => |s| NavigationLocation::new(PalettePlace(s)),
        HIMARK_PLACE_SESSIONS => |s| NavigationLocation::new(SessionsPlace(s)),
        HIMARK_PLACE_FILES => |s| NavigationLocation::new(FilesPlace(s)),
        HIMARK_PLACE_FOLDER_PICKER => |s| NavigationLocation::new(FolderPickerPlace(s)),
        _ => {
            refuse("session view kind", kind as u64);
            return HimarkPlace { raw: 0 };
        }
    };
    mint_session_place(engine, host, uri, len, make)
}

/// WHICH SESSION a place names. A shell that claims a place has to
/// re-point its views at that session — the sessions drawer picking
/// a row is the case — and the handle is the only thing it holds.
#[no_mangle]
pub unsafe extern "C" fn himark_place_session_host(
    engine: *mut HimarkEngine,
    place: HimarkPlace,
) -> u64 {
    let Some(_engine) = engine.as_mut() else {
        return 0;
    };
    himark_ffi::bridges::place_entry(place)
        .and_then(|(_, payload)| payload)
        .and_then(|payload| payload.downcast_ref::<ahp_wire::SessionId>().map(|s| s.host.raw()))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn himark_place_session_uri(
    engine: *mut HimarkEngine,
    place: HimarkPlace,
    out: *mut c_char,
    cap: usize,
) -> usize {
    let Some(_engine) = engine.as_mut() else {
        return 0;
    };
    let uri = himark_ffi::bridges::place_entry(place)
        .and_then(|(_, payload)| payload)
        .and_then(|payload| {
            payload
                .downcast_ref::<ahp_wire::SessionId>()
                .map(|s| s.session.to_string())
        });
    copy_optional_utf8(uri, out, cap)
}

/// The EXPLICIT tree-follow: the host landed a navigation (the
/// navigate callback handed it this very place) and commands the
/// session's files tree to stand on the landed location — the same
/// `TreeCommand::Follow` the workbench pushes from its focus walk.
/// False when the place carries no location or no tree is mounted.
#[no_mangle]
pub unsafe extern "C" fn himark_tree_follow(engine: *mut HimarkEngine, place: HimarkPlace) -> bool {
    let Some(engine) = engine.as_mut() else {
        return false;
    };
    let Some((place, Some(payload))) = himark_ffi::bridges::place_entry(place) else {
        return false;
    };
    let Some(session) = payload.downcast_ref::<ahp_wire::SessionId>().cloned() else {
        return false;
    };
    let location = if let Some(ready) = place.place::<himark::places::OpenDocumentPlace>() {
        ahp_session::session::state::Hosts::states(engine.store())
            .into_iter()
            .map(|state| state.documents())
            .collect::<Vec<_>>()
            .into_iter()
            .find_map(|collection| {
                documents::OpenDocuments::location(engine.store(), collection, ready.document)
            })
    } else if let Some(ready) = place.place::<himark::places::DiffViewPlace>() {
        documents::OpenDocuments::diff_view_ref(engine.store(), ready.documents, ready.view)
            .map(|held| held.right.document())
            .and_then(|right| {
                documents::OpenDocuments::location(engine.store(), ready.documents, right)
            })
    } else if let Some(ready) = place.place::<canvas::diff_canvas::CanvasPlace>() {
        ready.reveal.clone()
    } else {
        None
    };
    let Some(location) = location else {
        return false;
    };
    crate::sessions::follow_session_tree(engine, &session, location)
}

/// Seat the AHP half over an assembled engine: the navigate
/// EXPORTER (which places the native callback is offered), the
/// place MINT RIDER (the session remembered on a minted bridge),
/// and the host-capability chrome (folder grant, share roads, the
/// agent-host filesystem routes). The composition root calls this
/// once, before callbacks can arrive.
pub fn seat_ahp(
    app: &mut himark::app::Himark,
    languages: Arc<editor::reparse::SyntaxLanguages>,
    clients: Arc<ahp_wire::fs::ClientDirectory>,
    resource_uris: Arc<dyn ahp_wire::client::ResourceUriMap>,
) {
    app.with_store(|store| {
        store.put(PlaceMintRider(Arc::new(|app, id, payload| {
            if let Some(session) = payload.downcast_ref::<ahp_wire::SessionId>() {
                let session = session.clone();
                app.with_store(move |store| crate::sessions::remember_session(store, id, session));
            }
        })));
        store.put(NavigateExporter(Arc::new(classify_navigate)));
    });
    let chrome = app
        .store()
        .get::<himark_ffi::host::HostChrome>()
        .cloned()
        .unwrap_or_default();
    let fs = (languages, clients, resource_uris);
    app.with_store(move |store| {
        store.put(himark_ffi::host::HostChrome {
            on_pick_folders: Some(Arc::new(|app, bridge| {
                app.register_handler::<crate::higent::folder_grant::PickFoldersEffect>(
                    PickFoldersHandler(bridge),
                );
            })),
            on_share: Some(Arc::new(|app, bridge| {
                app.register_handler::<ahp_wire::effects::ShareHostEffect>(ShareHostHandler(
                    Arc::clone(&bridge),
                ));
                app.register_handler::<ahp_wire::effects::ShareTailnetEffect>(
                    ShareTailnetHandler(bridge),
                );
            })),
            on_agent_filesystem: Some(Arc::new(move |app, capabilities| {
                crate::agent_fs::install_routes(
                    app,
                    capabilities.fetch_document,
                    capabilities.store_document,
                    capabilities.list_directory,
                    &fs.0,
                    &fs.1,
                    &fs.2,
                );
            })),
            ..chrome.clone()
        });
    });
}

/// The classifier the exporter seat answers with — the whole
/// place-kind vocabulary, session riders included.
fn classify_navigate(
    place: &hikit::navigation::NavigationLocation,
    origin: himark::bridge::ViewBridgeId,
    store: &imba::store::Store,
) -> Option<NavigateExport> {
    let mut focus = false;
    let classified = if let Some(ready) = place.place::<himark::places::OpenDocumentPlace>()
            {
                focus = ready.focus;
                // The document's name: it is registered by the time
                // this place is navigated (the open landing's law).
                let title = ahp_session::session::state::Hosts::states(store)
                    .into_iter()
                    .map(|state| state.documents())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .find_map(|collection| {
                        documents::OpenDocuments::name(store, collection, ready.document)
                    })
                    .unwrap_or_default();
                Some((HIMARK_PLACE_DOCUMENT, title))
            } else if let Some(ready) = place.place::<himark::places::DiffViewPlace>() {
                Some((HIMARK_PLACE_DIFF, ready.title.clone()))
            } else if let Some(ready) = place.place::<canvas::diff_canvas::CanvasPlace>() {
                Some((
                    HIMARK_PLACE_CANVAS,
                    ready.source.title(store, ready.changes),
                ))
            } else if place.place::<himark::places::DismissPlace>().is_some() {
                Some((HIMARK_PLACE_DISMISS, String::new()))
            } else if place.place::<crate::places::ChatPlace>().is_some() {
                // "Show this session" — the sessions drawer's pick.
                Some((HIMARK_PLACE_CHAT, String::new()))
            } else if place.place::<crate::places::FolderPickerPlace>().is_some() {
                // The add-folder ask on a remote host — the native
                // shell presents the picker (iOS: a sheet).
                Some((HIMARK_PLACE_FOLDER_PICKER, "Add Folder".to_owned()))
            } else {
                None
            };
    let (kind, title) = classified?;
    let session = place
        .place::<crate::places::ChatPlace>()
        .map(|chat| chat.0.clone())
        .or_else(|| {
            place
                .place::<crate::places::FolderPickerPlace>()
                .map(|picker| picker.0.clone())
        })
        .or_else(|| crate::sessions::session_of(store, origin));
    Some(NavigateExport {
        kind,
        title,
        focus,
        payload: session
            .map(|session| Arc::new(session) as himark_ffi::bridges::PlacePayload),
    })
}

struct ShareHostHandler(Arc<HostBridge>);

impl EffectHandler<ahp_wire::effects::ShareHostEffect> for ShareHostHandler {
    async fn handle(&self, effect: ahp_wire::effects::ShareHostEffect) -> Result<String, String> {
        let url = effect.client.http_serve().await?;
        self.0.set_clipboard(&url);
        Ok(url)
    }
}

struct ShareTailnetHandler(Arc<HostBridge>);

impl EffectHandler<ahp_wire::effects::ShareTailnetEffect> for ShareTailnetHandler {
    async fn handle(
        &self,
        effect: ahp_wire::effects::ShareTailnetEffect,
    ) -> Result<ahp_wire::client::TailnetShare, String> {
        // The first ask enables; the enrollment then moves on its
        // own clock. A few re-asks catch the login URL (or a prior
        // enrollment going straight to running) without the user
        // rerunning the command.
        let mut share = effect.client.tailnet_serve().await?;
        for _ in 0..4 {
            if share.auth_url.is_some() || share.url.is_some() || share.error.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            share = effect.client.tailnet_serve().await?;
        }
        if let Some(copy) = share.url.as_deref().or(share.auth_url.as_deref()) {
            self.0.set_clipboard(copy);
        }
        // The login is a browser trip — take the user straight
        // there when the shell has a door.
        if let Some(auth_url) = share.auth_url.as_deref() {
            self.0.open_url(auth_url);
        }
        Ok(share)
    }
}

struct PickFoldersHandler(Arc<HostBridge>);

impl EffectHandler<crate::higent::folder_grant::PickFoldersEffect> for PickFoldersHandler {
    async fn handle(
        &self,
        effect: crate::higent::folder_grant::PickFoldersEffect,
    ) -> Vec<ResourceLocation> {
        self.0.pick_locations(effect.origin).await
    }
}

