// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The host's capability callbacks, answered by gpui — what the Swift
//! shell's `HostBridge` is to AppKit. The engine calls these from its
//! effect thread; each one only hands the ask to the gpui foreground
//! task (`serve`), which runs the portal dialog / clipboard / browser
//! and answers the request through the engine.
//!
//! Without them the engine never seats the capability-gated chrome:
//! no file or folder picker (Add Folder does nothing), no Share
//! commands, and the effects those commands fire have no handler.

use std::path::{Path, PathBuf};

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt as _;

use editor::location::{Authority, ResourceLocation, ResourceType};
use himark_ffi::host::HimarkRequest;
use himark_ffi::{HimarkEngineExt as _, HimarkHostCallbacks};
use himark_gpui::gpui::{AsyncApp, ClipboardItem, PathPromptOptions};
use himark_gpui::Engine;

/// One ask from the engine, carried to the foreground.
pub enum HostAsk {
    PickFiles(HimarkRequest),
    PickSave(HimarkRequest, String),
    Clipboard(String),
    OpenUrl(String),
}

pub struct HostAsks(UnboundedReceiver<HostAsk>);

/// The callbacks for `install_host_callbacks`, and the queue they
/// fill. The agent host serves the filesystem (as in the C path's
/// `agent_host_filesystem`); the pickers, clipboard and browser are
/// this shell's.
pub fn callbacks() -> (HimarkHostCallbacks, HostAsks) {
    let (tx, rx) = unbounded();
    // Lives as long as the engine, which lives as long as the process.
    let ctx = Box::into_raw(Box::new(tx)).cast();
    let callbacks = HimarkHostCallbacks {
        ctx,
        pick_files: Some(pick_files),
        pick_save: Some(pick_save),
        set_clipboard: Some(set_clipboard),
        open_url: Some(open_url),
        ..HimarkHostCallbacks::agent_host_filesystem()
    };
    (callbacks, HostAsks(rx))
}

unsafe fn send(ctx: *mut std::ffi::c_void, ask: HostAsk) {
    if let Some(tx) = ctx.cast::<UnboundedSender<HostAsk>>().as_ref() {
        let _ = tx.unbounded_send(ask);
    }
}

unsafe fn text(ptr: *const std::ffi::c_char, len: usize) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), len);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

unsafe extern "C" fn pick_files(
    ctx: *mut std::ffi::c_void,
    request: HimarkRequest,
    _origin: himark_ffi::bridges::HimarkBridge,
) {
    send(ctx, HostAsk::PickFiles(request));
}

unsafe extern "C" fn pick_save(
    ctx: *mut std::ffi::c_void,
    request: HimarkRequest,
    suggested: *const std::ffi::c_char,
    suggested_len: usize,
) {
    let name = text(suggested, suggested_len).unwrap_or_else(|| "document.md".to_owned());
    send(ctx, HostAsk::PickSave(request, name));
}

unsafe extern "C" fn set_clipboard(
    ctx: *mut std::ffi::c_void,
    ptr: *const std::ffi::c_char,
    len: usize,
) {
    if let Some(copied) = text(ptr, len) {
        send(ctx, HostAsk::Clipboard(copied));
    }
}

unsafe extern "C" fn open_url(
    ctx: *mut std::ffi::c_void,
    ptr: *const std::ffi::c_char,
    len: usize,
) {
    if let Some(url) = text(ptr, len) {
        send(ctx, HostAsk::OpenUrl(url));
    }
}

/// A local path as the engine's location — the Swift shell's
/// `locationSpec(of:kind:)`.
fn location(path: &Path, kind: ResourceType) -> ResourceLocation {
    let segments: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    ResourceLocation::new(kind, Authority::new("local"), segments)
}

fn picked_locations(paths: Vec<PathBuf>) -> Vec<ResourceLocation> {
    paths
        .into_iter()
        .filter_map(|path| match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => Some(location(&path, ResourceType::directory())),
            Ok(_) => Some(location(&path, ResourceType::document())),
            Err(_) => None,
        })
        .collect()
}

/// The foreground half: answers every ask, one at a time (a dialog
/// is modal anyway).
pub async fn serve(asks: HostAsks, engine: Engine, cx: &mut AsyncApp) {
    let mut asks = asks.0;
    while let Some(ask) = asks.next().await {
        match ask {
            HostAsk::PickFiles(request) => {
                let prompt = cx.update(|cx| {
                    cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: true,
                        multiple: true,
                        prompt: None,
                    })
                });
                let paths = match prompt {
                    Ok(prompt) => match prompt.await {
                        Ok(Ok(Some(paths))) => paths,
                        Ok(Err(error)) => {
                            eprintln!("[host] file picker failed: {error:#}");
                            Vec::new()
                        }
                        _ => Vec::new(),
                    },
                    Err(_) => return,
                };
                let locations = picked_locations(paths);
                engine.with_app(|app| app.host_picked(request, locations));
            }
            HostAsk::PickSave(request, suggested) => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default();
                let prompt = cx.update(|cx| cx.prompt_for_new_path(&home, Some(&suggested)));
                let path = match prompt {
                    Ok(prompt) => match prompt.await {
                        Ok(Ok(path)) => path,
                        Ok(Err(error)) => {
                            eprintln!("[host] save picker failed: {error:#}");
                            None
                        }
                        Err(_) => None,
                    },
                    Err(_) => return,
                };
                let picked = path.map(|path| location(&path, ResourceType::document()));
                engine.with_app(|app| app.host_picked_folder(request, picked));
            }
            HostAsk::Clipboard(copied) => {
                let _ = cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(copied)));
            }
            HostAsk::OpenUrl(url) => {
                let _ = cx.update(|cx| cx.open_url(&url));
            }
        }
    }
}
