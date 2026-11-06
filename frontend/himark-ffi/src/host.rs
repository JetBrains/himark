// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::future::Future;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use himark::commands::HimarkCommand;
use editor::location::ResourceLocation;
use editor::location::ResourceType;
use imba::effect::{Effect, EffectHandler};

/// The CAPABILITY-GATED chrome seats: what the composition layer
/// registers when the host's callbacks carry a capability (the
/// workbench's Open… command for a file picker, its share commands
/// for a clipboard). Seated before `set_host` runs.
#[derive(Clone, Default)]
pub struct HostChrome {
    pub on_pick_files: Option<std::sync::Arc<dyn Fn(&mut himark::app::Himark) + Send + Sync>>,
    pub on_clipboard: Option<std::sync::Arc<dyn Fn(&mut himark::app::Himark) + Send + Sync>>,
}

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

    /// Open a URL in the system browser (UTF-8, no NUL).
    pub open_url: Option<
        unsafe extern "C" fn(
            ctx: *mut std::ffi::c_void,
            url: *const std::ffi::c_char,
            url_len: usize,
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
            open_url: None,
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

    pub(crate) fn open_url(&self, url: &str) -> bool {
        let Some(open) = self.callbacks.open_url else {
            return false;
        };
        unsafe { open(self.callbacks.ctx, url.as_ptr() as *const _, url.len()) };
        true
    }

    pub(crate) fn set_clipboard(&self, text: &str) {
        if let Some(set) = self.callbacks.set_clipboard {
            unsafe { set(self.callbacks.ctx, text.as_ptr().cast(), text.len()) };
        }
    }
}

pub struct FilePickerEffect {
    pub origin: himark::bridge::ViewBridgeId,
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

pub(crate) struct ShareTailnetHandler(pub(crate) Arc<HostBridge>);

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

pub(crate) struct PickFoldersHandler(pub(crate) Arc<HostBridge>);

impl EffectHandler<hiahp::higent::folder_grant::PickFoldersEffect> for PickFoldersHandler {
    async fn handle(
        &self,
        effect: hiahp::higent::folder_grant::PickFoldersEffect,
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
        unsafe { pick(self.0.callbacks.ctx, request, effect.origin.raw()) };
        self.0.requests.response(request).await
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
    use crate::host::*;

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
