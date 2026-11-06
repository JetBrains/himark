// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The tailnet C surface over the hitailnet plugin's store state.

use himark_ffi::copy_optional_utf8;

// ---- The C surface (hand-declared in himark's cbindgen trailer) --
// Reads go through the engine's store between batches — the thread
// the shell already drains on.

/// 0 off · 1 starting · 2 needs login · 3 waiting approval ·
/// 4 running. Append-only.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_state(engine: *mut crate::HimarkEngine) -> u32 {
    read_state(engine).code()
}

/// UTF-8, no NUL; returns the byte length, copies when it fits.
/// Non-empty only in the needs-login state.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_auth_url(
    engine: *mut crate::HimarkEngine,
    out: *mut std::os::raw::c_char,
    cap: usize,
) -> usize {
    let url = match read_state(engine) {
        hitailnet::TailnetState::NeedsLogin { auth_url } => Some(auth_url),
        _ => None,
    };
    himark_ffi::copy_optional_utf8(url, out, cap)
}

/// The shell's "Connect Tailscale" action.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_login(engine: *mut crate::HimarkEngine) {
    let Some(engine) = engine.as_ref() else {
        return;
    };
    if let Some(flows) = engine.app.store().get::<hitailnet::TailnetFlows>() {
        (flows.connect)();
    }
}

/// The shell came back to the foreground: kick the embedded node (a
/// suspension can wedge its engine) and retry the failed hosts.
#[no_mangle]
pub unsafe extern "C" fn himark_tailnet_resume(engine: *mut crate::HimarkEngine) {
    let Some(engine) = engine.as_ref() else {
        return;
    };
    if let Some(flows) = engine.app.store().get::<hitailnet::TailnetFlows>() {
        (flows.resume)();
    }
}

unsafe fn read_state(engine: *mut crate::HimarkEngine) -> hitailnet::TailnetState {
    let Some(engine) = engine.as_ref() else {
        return hitailnet::TailnetState::Off;
    };
    engine
        .app
        .store()
        .get::<hitailnet::TailnetFaceState>()
        .map(|held| held.state.clone())
        .unwrap_or_default()
}

