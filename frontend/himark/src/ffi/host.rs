// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use crate::app::AppCommand;
use crate::app::AppFx;
use crate::commands::HimarkCommand;
use crate::workspace::open_locations;
use editor::location::ResourceLocation;
use editor::location::ResourceType;
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

pub unsafe fn location_from_abi(abi: &HimarkLocation) -> Option<ResourceLocation> {
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
pub struct HostRequests {
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

    pub fn fulfill(&self, id: u64, value: Box<dyn std::any::Any + Send + Sync>) -> bool {
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

impl EffectHandler<crate::higent::folder_grant::PickFoldersEffect> for PickFoldersHandler {
    async fn handle(
        &self,
        effect: crate::higent::folder_grant::PickFoldersEffect,
    ) -> Vec<ResourceLocation> {
        let Some(pick) = self.0.callbacks.pick_files else {
            return Vec::new();
        };
        let request = self.0.requests.begin();
        unsafe { pick(self.0.callbacks.ctx, request, effect.window) };
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

impl HimarkCommand for OpenFilePicker {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let _ = fx.push(
            AnyEffect::new(FilePickerEffect { window }).map(move |locations| {
                crate::app::windowed(window, Arc::new(OpenPicked { locations }))
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
            crate::higent::open_session::open_session(store, window, host, session, false, fx)
        }
        None => {
            let _ = fx.push(
                AnyEffect::new(ahp_wire::effects::CreateSessionEffect {
                    options: ahp_wire::client::SessionOptions::default(),
                    client: client.session.clone(),
                    working_directories: dirs,
                })
                .map(move |result| {
                    crate::app::windowed(
                        window,
                        Arc::new(crate::higent::open_session::OpenCreatedSession {
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

impl HimarkCommand for OpenPicked {
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
        bound: &imba::focus::Bindings,
        fx: &mut crate::app::AppFx<'_>,
    ) {
        let Some(window) = crate::commands::bound_window(self.id(), bound) else {
            return;
        };
        let ui = ui;
        let (folders, documents): (Vec<_>, Vec<_>) = self
            .locations
            .iter()
            .cloned()
            .partition(|location| location.kind().is_directory());
        if !folders.is_empty() {
            let workspace = workbench::window::Windows::window_ref(store, window)
                .map(|entity| crate::workspace::entity_session(&entity))
                .unwrap_or_else(|| ahp_wire::SessionId::local_default(store));
            let state = crate::workspace::session_state(store, window);
            if let (true, Some(state)) = (workspace.names_session(), state) {
                let (wire, comments_wire) = (state.changes_wire(), state.comments_wire());
                let diagnostics_wire = state.diagnostics_wire();
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
                    fx.scope(AppCommand::Verb, |fx| {
                        ahp_lsp::diagnostics::ensure(store, diagnostics_wire, &spelled, fx)
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
