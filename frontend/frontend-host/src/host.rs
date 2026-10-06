// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use editor::location::ResourceLocation;
use editor::location::ResourceType;
use himark::app::AppCommand;
use himark::app::AppFx;
use himark::commands::WindowedCommand;
use himark::workspace::open_locations;
use imba::effect::{AnyEffect, Effect, EffectHandler};
use imba::store::Store;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkHostCallbacks {
    pub ctx: *mut std::ffi::c_void,

    pub pick_files:
        Option<unsafe extern "C" fn(ctx: *mut std::ffi::c_void, request: u64, window: u64)>,

    pub pick_save: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            request: u64,
            suggested: *const std::ffi::c_char,
            suggested_len: usize,
        ),
    >,

    pub fetch_document: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            request: u64,
            location: *const HimarkLocation,
        ),
    >,

    pub store_document: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            request: u64,
            location: *const HimarkLocation,
            text: *const std::ffi::c_char,
            text_len: usize,
        ),
    >,

    pub list_directory: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            request: u64,
            location: *const HimarkLocation,
        ),
    >,

    pub subscribe: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            request: u64,
            location: *const HimarkLocation,
        ),
    >,

    pub unsubscribe: Option<unsafe extern "C" fn(ctx: *mut std::ffi::c_void, subscription: u64)>,

    pub set_clipboard: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            text: *const std::ffi::c_char,
            text_len: usize,
        ),
    >,
}

impl HimarkHostCallbacks {
    pub fn agent_host_filesystem() -> Self {
        Self {
            ctx: std::ptr::null_mut(),
            pick_files: None,
            pick_save: None,
            fetch_document: Some(agent_host_fetch_capability),
            store_document: Some(agent_host_store_capability),
            list_directory: Some(agent_host_list_capability),
            subscribe: None,
            unsubscribe: None,
            set_clipboard: None,
        }
    }
}

unsafe extern "C" fn agent_host_fetch_capability(
    _ctx: *mut std::ffi::c_void,
    _request: u64,
    _location: *const HimarkLocation,
) {
}

unsafe extern "C" fn agent_host_store_capability(
    _ctx: *mut std::ffi::c_void,
    _request: u64,
    _location: *const HimarkLocation,
    _text: *const std::ffi::c_char,
    _text_len: usize,
) {
}

unsafe extern "C" fn agent_host_list_capability(
    _ctx: *mut std::ffi::c_void,
    _request: u64,
    _location: *const HimarkLocation,
) {
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkStr {
    pub ptr: *const std::ffi::c_char,
    pub len: usize,
}

impl HimarkStr {
    pub(crate) unsafe fn to_str<'a>(self) -> Option<&'a str> {
        if self.ptr.is_null() {
            return None;
        }
        std::str::from_utf8(std::slice::from_raw_parts(self.ptr.cast(), self.len)).ok()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HimarkLocation {
    pub kind: HimarkStr,
    pub authority: HimarkStr,
    pub segments: *const HimarkStr,
    pub segment_count: usize,
}

pub(crate) unsafe fn location_from_abi(abi: &HimarkLocation) -> Option<ResourceLocation> {
    let kind = abi.kind.to_str()?;
    let authority = abi.authority.to_str()?;
    let raw = match abi.segment_count {
        0 => &[],
        _ => std::slice::from_raw_parts(abi.segments, abi.segment_count),
    };
    let mut path = Vec::with_capacity(raw.len());
    for segment in raw {
        path.push(segment.to_str()?.to_owned());
    }
    Some(ResourceLocation::new(
        ResourceType::new(kind),
        editor::location::Authority::new(authority),
        path,
    ))
}

#[derive(Default)]
pub(crate) struct HostRequests {
    next: AtomicU64,
    pending: Mutex<std::collections::HashMap<u64, Answer>>,
}

#[derive(Default)]
struct Answer {
    value: Option<Box<dyn std::any::Any + Send + Sync>>,
    waker: Option<std::task::Waker>,
}

impl HostRequests {
    fn begin(&self) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.pending
            .lock()
            .expect("host requests")
            .insert(id, Answer::default());
        id
    }

    pub(crate) fn fulfill(&self, id: u64, value: Box<dyn std::any::Any + Send + Sync>) -> bool {
        let mut pending = self.pending.lock().expect("host requests");
        let Some(answer) = pending.get_mut(&id) else {
            return false;
        };
        answer.value = Some(value);
        if let Some(waker) = answer.waker.take() {
            waker.wake();
        }
        true
    }

    async fn response<T: 'static>(&self, id: u64) -> T {
        struct ResponseFuture<'a> {
            requests: &'a HostRequests,
            id: u64,
        }
        impl Future for ResponseFuture<'_> {
            type Output = Box<dyn std::any::Any + Send + Sync>;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let mut pending = self.requests.pending.lock().expect("host requests");
                let answer = pending
                    .get_mut(&self.id)
                    .expect("request outlives its await");
                match answer.value.take() {
                    Some(value) => std::task::Poll::Ready(value),
                    None => {
                        answer.waker = Some(cx.waker().clone());
                        std::task::Poll::Pending
                    }
                }
            }
        }
        impl Drop for ResponseFuture<'_> {
            fn drop(&mut self) {
                let mut pending = self.requests.pending.lock().expect("host requests");
                pending.remove(&self.id);
            }
        }
        let value = ResponseFuture { requests: self, id }.await;
        *value
            .downcast::<T>()
            .expect("answered with the request's type")
    }
}

pub(crate) struct HostBridge {
    callbacks: HimarkHostCallbacks,
    pub(crate) requests: HostRequests,
}

unsafe impl Send for HostBridge {}
unsafe impl Sync for HostBridge {}

impl HostBridge {
    pub(crate) fn new(callbacks: HimarkHostCallbacks) -> Arc<Self> {
        Arc::new(Self {
            callbacks,
            requests: HostRequests::default(),
        })
    }

    pub(crate) fn set_clipboard(&self, text: &str) {
        if let Some(set) = self.callbacks.set_clipboard {
            unsafe { set(self.callbacks.ctx, text.as_ptr().cast(), text.len()) };
        }
    }
}

pub struct FilePickerEffect {
    window: workbench::window::WindowId,
}

impl std::fmt::Display for FilePickerEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("pick files")
    }
}

impl Effect for FilePickerEffect {
    type Result = Vec<ResourceLocation>;
}

pub(crate) struct FilePickerHandler(pub(crate) Arc<HostBridge>);

pub(crate) struct ShareHostHandler(pub(crate) Arc<HostBridge>);

impl EffectHandler<ahp_wire::effects::ShareHostEffect> for ShareHostHandler {
    async fn handle(&self, effect: ahp_wire::effects::ShareHostEffect) -> Result<String, String> {
        let url = effect.client.http_serve().await?;
        self.0.set_clipboard(&url);
        Ok(url)
    }
}

pub(crate) struct PickFoldersHandler(pub(crate) Arc<HostBridge>);

impl EffectHandler<himark::new_session::PickFoldersEffect> for PickFoldersHandler {
    async fn handle(
        &self,
        effect: himark::new_session::PickFoldersEffect,
    ) -> Vec<ResourceLocation> {
        let Some(pick) = self.0.callbacks.pick_files else {
            return Vec::new();
        };
        let request = self.0.requests.begin();
        unsafe { pick(self.0.callbacks.ctx, request, effect.window.raw()) };
        self.0.requests.response(request).await
    }
}

impl EffectHandler<FilePickerEffect> for FilePickerHandler {
    async fn handle(&self, effect: FilePickerEffect) -> Vec<ResourceLocation> {
        let Some(pick) = self.0.callbacks.pick_files else {
            return Vec::new();
        };
        let request = self.0.requests.begin();
        unsafe { pick(self.0.callbacks.ctx, request, effect.window.raw()) };
        self.0.requests.response(request).await
    }
}

pub(crate) struct OpenFilePicker;

impl WindowedCommand for OpenFilePicker {
    fn id(&self) -> &'static str {
        "file.open"
    }
    fn name(&self) -> String {
        "Open…".to_owned()
    }
    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let _ = fx.push(
            AnyEffect::new(FilePickerEffect { window }).map(move |locations| {
                AppCommand::Windowed(window, Arc::new(OpenPicked { locations }))
            }),
        );
    }
}

struct OpenPicked {
    locations: Vec<ResourceLocation>,
}

fn open_folder_session(
    store: &mut Store,
    window: workbench::window::WindowId,
    folders: &[ResourceLocation],
    fx: &mut AppFx<'_>,
) -> bool {
    let Some((host, client, _)) = ahp_wire::client::route_client(store, "local") else {
        return false;
    };
    let Some(uris) = ahp_session::session::state::Hosts::uris(store, host) else {
        return false;
    };
    let dirs: Vec<String> = folders
        .iter()
        .map(|folder| uris.uri_of(folder).into_string())
        .collect();
    let existing = ahp_session::session::agents::Agents::record(store, host).and_then(|record| {
        record
            .sessions
            .iter()
            .find(|summary| {
                summary.working_directories.as_ref().is_some_and(|held| {
                    held.len() == dirs.len() && dirs.iter().all(|dir| held.contains(dir))
                })
            })
            .map(|summary| ahp_wire::client::SessionUri::new(summary.resource.clone()))
    });
    match existing {
        Some(session) => {
            himark::higent::open_session::open_session(store, window, host, session, false, fx)
        }
        None => {
            let _ = fx.push(
                AnyEffect::new(ahp_wire::effects::CreateSessionEffect {
                    options: ahp_wire::client::SessionOptions::default(),
                    client: client.session.clone(),
                    working_directories: dirs,
                })
                .map(move |result| {
                    AppCommand::Windowed(
                        window,
                        Arc::new(himark::higent::open_session::OpenCreatedSession {
                            initial_prompt: None,
                            server: host,
                            open_chat: false,
                            result,
                        }),
                    )
                }),
            );
        }
    }
    true
}

impl WindowedCommand for OpenPicked {
    fn id(&self) -> &'static str {
        "file.open-picked"
    }
    fn name(&self) -> String {
        "Open Picked".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let ui = ui;
        let (folders, documents): (Vec<_>, Vec<_>) = self
            .locations
            .iter()
            .cloned()
            .partition(|location| location.kind().is_directory());
        if !folders.is_empty() {
            let workspace = workbench::window::Windows::window_ref(store, window)
                .map(|entity| himark::workspace::entity_session(&entity))
                .unwrap_or_else(|| ahp_wire::SessionId::local_default(store));
            let state = himark::workspace::session_state(store, window);
            if let (true, Some(state)) = (workspace.names_session(), state) {
                let (wire, comments_wire) = (state.changes_wire(), state.comments_wire());
                for folder in folders {
                    let spelled = editor::location::ResourceLocation::new(
                        folder.kind().clone(),
                        editor::location::Authority::new(ahp_wire::client::authority(
                            workspace.host,
                            &workspace.session,
                        )),
                        folder.path().to_vec(),
                    );
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_changes::changes::ensure_folder(store, wire, spelled.clone(), fx)
                    });
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_comments::ensure(store, comments_wire, &spelled, fx)
                    });
                }
            } else if !open_folder_session(store, window, &folders, fx) {
                eprintln!("[host] folder pick dropped: no local agent host to session it");
            }
        }
        if !documents.is_empty() {
            open_locations(store, ui, window, &documents, fx);
        }
    }
}

pub(crate) struct PickSaveHandler(pub(crate) Arc<HostBridge>);

impl EffectHandler<documents::PickSaveEffect> for PickSaveHandler {
    async fn handle(&self, effect: documents::PickSaveEffect) -> Option<ResourceLocation> {
        let pick = self.0.callbacks.pick_save?;
        let request = self.0.requests.begin();
        let suggested = effect.suggested;
        unsafe {
            pick(
                self.0.callbacks.ctx,
                request,
                suggested.as_ptr() as *const std::ffi::c_char,
                suggested.len(),
            )
        };
        self.0.requests.response(request).await
    }
}

pub(crate) struct OpenWorkingCopy;

impl documents::dynamic::DocumentCommand for OpenWorkingCopy {
    fn id(&self) -> &'static str {
        "workbench.open-in-full"
    }
    fn name(&self) -> String {
        "Open Working Copy".to_owned()
    }

    fn offers_at(&self, location: &ResourceLocation) -> bool {
        changesview::hichanges::scoped(location)
    }
    #[allow(clippy::too_many_arguments)]
    fn perform(
        &self,
        store: &mut Store,
        _ui: &imba::ui::UiCtx,
        documents: imba::store::Id<documents::OpenDocuments>,
        _document_id: documents::DocumentId,
        document: &mut editor::document::Document,
        editor: editor::editor::EditorId,
        location: &editor::location::ResourceLocation,
        _payload: Option<Box<dyn std::any::Any + Send + Sync>>,
        _fx: &mut imba::effect::Effects<'_, editor::editor_view::EditorCommand>,
    ) {
        let working =
            changesview::hichanges::working_copy(location).unwrap_or_else(|| location.clone());
        let byte = document.caret_byte(editor);
        let mut view = document.text().view();
        let at = documents::text_ext::line_col_at(&mut view, byte as usize);
        himark::commands::AppRequests::push(
            store,
            Arc::new(ShowWorkingCopy {
                documents,
                location: working,
                target: at..at,
            }),
        );
    }
}

struct ShowWorkingCopy {
    /// The collection the working copy opens into — closed over at the
    /// gesture, in the pane that fired it.
    documents: imba::store::Id<documents::OpenDocuments>,
    location: editor::location::ResourceLocation,
    target: std::ops::Range<documents::text_ext::LineCol>,
}

impl himark::commands::WindowedCommand for ShowWorkingCopy {
    fn id(&self) -> &'static str {
        "vcs.apply-open-working-copy"
    }
    fn name(&self) -> String {
        "Open Working Copy".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let ui = ui;
        let documents = self.documents;
        match documents::OpenDocuments::by_location(store, documents, &self.location) {
            Some(document_id) => {
                let Some(mut entity) = workbench::window::Windows::window(store, window) else {
                    return;
                };
                fx.scope(himark::app::AppCommand::Verb, |fx| {
                    entity.show_document(
                        store,
                        ui,
                        window,
                        document_id,
                        Some(self.target.clone()),
                        false,
                        fx,
                    )
                });
                workbench::window::Windows::put(store, window, entity);
            }
            None => {
                fx.push(himark::workspace::open_by_location_effect(
                    window,
                    documents,
                    self.location.clone(),
                    true,
                    false,
                    Some(self.target.clone()),
                ));
            }
        }
    }
}

pub struct NewTerminalEffect {
    pub(crate) client: Arc<dyn ahp_wire::client::TerminalClient>,
    /// The session the terminal is spawned IN — the wire's address.
    pub(crate) home: ahp_wire::SessionId,
    pub(crate) cwd: Option<String>,
    pub(crate) window: workbench::window::WindowId,
}

impl std::fmt::Display for NewTerminalEffect {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "new terminal {}", self.home.session.as_str())
    }
}

impl Effect for NewTerminalEffect {
    type Result = Option<Arc<terminals::Session>>;
}

pub(crate) struct SessionTerminalHandler {
    pub(crate) refresh:
        Arc<dyn Fn(workbench::window::WindowId, Arc<std::sync::atomic::AtomicBool>) + Send + Sync>,
}

struct AhpBackend {
    client: Arc<dyn ahp_wire::client::TerminalClient>,
    channel: ahp_wire::client::ChannelUri,
}

impl terminals::TerminalBackend for AhpBackend {
    fn write(&self, bytes: &[u8]) {
        self.client
            .terminal_input(&self.channel, String::from_utf8_lossy(bytes).into_owned());
    }
    fn resize(&self, cols: u16, rows: u16, _px_width: f32, _px_height: f32) {
        self.client.terminal_resize(&self.channel, cols, rows);
    }
    fn hangup(&self) {
        self.client.terminal_dispose(&self.channel);
    }
}

impl EffectHandler<NewTerminalEffect> for SessionTerminalHandler {
    async fn handle(&self, effect: NewTerminalEffect) -> Option<Arc<terminals::Session>> {
        let channel =
            ahp_wire::client::ChannelUri::new(format!("ahp-terminal:/{}", ahp_wire::uuid_v4()));
        let session = terminals::Session::new(Box::new(AhpBackend {
            client: Arc::clone(&effect.client),
            channel: channel.clone(),
        }));
        let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let events = {
            let session = Arc::clone(&session);
            let refresh = Arc::clone(&self.refresh);
            let window = effect.window;
            let pending = Arc::clone(&pending);
            Arc::new(move |event: ahp_wire::client::TerminalEvent| {
                match event {
                    ahp_wire::client::TerminalEvent::Data(text) => {
                        let _ = session.output(text.as_bytes());
                    }
                    ahp_wire::client::TerminalEvent::Exited(code) => {
                        let _ = session.exited(code.unwrap_or(-1));
                    }
                    // A reconnect replays the host's snapshot from
                    // byte one — wipe the grid first.
                    ahp_wire::client::TerminalEvent::Reset => {
                        let _ = session.reset();
                    }
                }

                if !pending.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    refresh(window, Arc::clone(&pending));
                }
            }) as Arc<dyn Fn(ahp_wire::client::TerminalEvent) + Send + Sync>
        };
        effect
            .client
            .terminal_open(
                effect.home.session.clone(),
                channel,
                effect.cwd.clone(),
                80,
                24,
                events,
            )
            .await?;
        Some(session)
    }
}

pub(crate) struct RefreshTerminal(pub(crate) Arc<std::sync::atomic::AtomicBool>);

impl WindowedCommand for RefreshTerminal {
    fn id(&self) -> &'static str {
        "terminal.refresh"
    }
    fn name(&self) -> String {
        String::new()
    }
    fn perform(
        &self,
        _store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        _window: ::workbench::window::WindowId,
        _fx: &mut himark::app::AppFx<'_>,
    ) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) struct OpenTerminal;

impl WindowedCommand for OpenTerminal {
    fn id(&self) -> &'static str {
        "terminal.open"
    }
    fn name(&self) -> String {
        "New Terminal".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        _ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let Some(entity) = workbench::window::Windows::window(store, window) else {
            return;
        };
        let workspace = himark::workspace::entity_session(&entity);
        workbench::window::Windows::put(store, window, entity);
        let resolved = ahp_session::session::agents::Agents::live_session(store, &workspace)
            .and_then(|key| {
                let client = ahp_wire::client::Servers::client(store, key.host)?;
                let cwd = ahp_session::session::agents::Agents::record(store, key.host).and_then(
                    |record| {
                        record.summary(&key.session).and_then(|summary| {
                            summary.working_directories.as_ref()?.first().cloned()
                        })
                    },
                );
                Some((client, key.clone(), cwd))
            })
            .or_else(|| {
                let server = store
                    .get::<ahp_wire::client::LocalHost>()
                    .copied()
                    .unwrap_or_default()
                    .0?;
                let client = ahp_wire::client::Servers::client(store, server)?;
                let cwd = ahp_session::session::folders::session_folders(store, &workspace)
                    .first()
                    .map(|folder| {
                        ahp_wire::client::ResourceUriMap::uri_of(&crate::uris::FileUris, folder)
                            .into_string()
                    });
                Some((
                    client,
                    ahp_wire::SessionId {
                        host: server,
                        session: ahp_wire::client::SessionUri::new(
                            host_discovery::LOCAL_FS_SESSION,
                        ),
                    },
                    cwd,
                ))
            });
        let Some((client, home, cwd)) = resolved else {
            return;
        };
        // The terminal files into the WINDOW's session — the collection
        // the pane is minted against, carried from here to the landing.
        let Some(terminals) =
            himark::workspace::session_state(store, window).map(|state| state.terminals())
        else {
            return;
        };
        let _ = fx.push(
            AnyEffect::new(NewTerminalEffect {
                client: client.terminals.clone(),
                home,
                cwd,
                window,
            })
            .map(move |session| {
                AppCommand::Windowed(window, Arc::new(ShowTerminal { session, terminals }))
            }),
        );
    }
}

struct ShowTerminal {
    session: Option<Arc<terminals::Session>>,
    terminals: imba::store::Id<terminals::Terminals>,
}

impl WindowedCommand for ShowTerminal {
    fn id(&self) -> &'static str {
        "terminal.show"
    }
    fn name(&self) -> String {
        "Show Terminal".to_owned()
    }
    fn perform(
        &self,
        store: &mut imba::store::Store,
        ui: &imba::ui::UiCtx,
        window: ::workbench::window::WindowId,
        fx: &mut himark::app::AppFx<'_>,
    ) {
        let ui = ui;
        let Some(session) = &self.session else {
            return;
        };
        let mut entity =
            workbench::window::Windows::window(store, window).expect("the window entity");

        let terminals = self.terminals;
        let id = terminals::TerminalId::mint();
        terminals::Terminals::put(store, terminals, id, session.clone());
        if !entity.open_panel(
            store,
            ui,
            Box::new(terminals::pane::TerminalView::new(terminals, id)),
            fx,
        ) {
            session.hangup();
            terminals::Terminals::remove(store, terminals, id);
        }
        workbench::window::Windows::put(store, window, entity);
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn a_dropped_response_future_cleans_its_slot_and_fulfill_answers_false() {
        let requests = HostRequests::default();
        let id = requests.begin();
        {
            let mut future = Box::pin(requests.response::<u32>(id));
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(
                future.as_mut().poll(&mut cx).is_pending(),
                "no answer yet: the future parks"
            );
        }

        assert!(
            !requests.fulfill(id, Box::new(7u32)),
            "a dropped request's fulfill finds nobody"
        );
    }

    #[test]
    fn a_fulfilled_request_answers_its_awaiter_once() {
        let requests = HostRequests::default();
        let id = requests.begin();
        assert!(requests.fulfill(id, Box::new(7u32)));
        let mut future = Box::pin(requests.response::<u32>(id));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => assert_eq!(value, 7),
            std::task::Poll::Pending => panic!("the filled slot answers immediately"),
        }
        drop(future);
        assert!(
            !requests.fulfill(id, Box::new(8u32)),
            "the consumed slot is gone — one request, one answer"
        );
    }
}
