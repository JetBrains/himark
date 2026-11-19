// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The apple paint doors over gpui's Metal renderer. The shell owns
//! a `CAMetalLayer` (its geometry, its drawable size); a surface is
//! a renderer over that layer plus the scene a frame is collected
//! into; the surfaces of one shell share a render context. No canvas crosses the C ABI: the engine paints into the
//! scene here and the renderer presents it.

use std::os::raw::c_void;
use std::rc::Rc;
use std::sync::Arc;

use gfx::Canvas;
use gfx_gpui::gpui::Scene;
use gfx_gpui::{ScenePainter, SceneText};
use gpui_apple::metal_renderer::{Context, MetalRenderer};
use gpui_apple::AppleTextSystem;
use objc2_quartz_core::CAMetalLayer;

use crate::bridges::{bridge_draw, frame_paint, HimarkBridge};
use crate::HimarkEngine;

/// What every surface of one shell shares: the text system (fonts
/// register once and resolve to the same ids everywhere) and the
/// pool its renderers draw instance buffers from. The shell creates
/// one and makes its surfaces from it; nothing here is global.
pub struct HimarkRenderContext {
    text: SceneText,
    buffers: Context,
}

pub struct HimarkSurface {
    context: Rc<HimarkRenderContext>,
    renderer: MetalRenderer,
    scene: Scene,
}

#[no_mangle]
pub extern "C" fn himark_render_context_create() -> *const HimarkRenderContext {
    Rc::into_raw(Rc::new(HimarkRenderContext {
        text: SceneText::new(Arc::new(AppleTextSystem::new())),
        buffers: Context::default(),
    }))
}

/// Releases the shell's hold; surfaces made from the context keep
/// it alive until they are destroyed themselves.
#[no_mangle]
pub unsafe extern "C" fn himark_render_context_destroy(context: *const HimarkRenderContext) {
    if !context.is_null() {
        drop(Rc::from_raw(context));
    }
}

/// A surface over the shell's `CAMetalLayer`; the renderer retains
/// the layer. Null when Metal refuses.
#[no_mangle]
pub unsafe extern "C" fn himark_surface_create(
    context: *const HimarkRenderContext,
    ca_metal_layer: *mut c_void,
) -> *mut HimarkSurface {
    let Some(layer) = (ca_metal_layer as *const CAMetalLayer).as_ref() else {
        return std::ptr::null_mut();
    };
    if context.is_null() {
        return std::ptr::null_mut();
    }
    // The surface's own hold on the context, beside the shell's.
    Rc::increment_strong_count(context);
    let context = Rc::from_raw(context);
    // Transparent: a bridge composites over the shell's own ground.
    match MetalRenderer::from_layer(context.buffers.clone(), layer, true) {
        Ok(mut renderer) => {
            // Geometry and pixels commit in one CoreAnimation
            // transaction — the shell moves the layer and presents
            // inside the same one.
            renderer.set_presents_with_transaction(true);
            Box::into_raw(Box::new(HimarkSurface {
                context,
                renderer,
                scene: Scene::default(),
            }))
        }
        Err(error) => {
            eprintln!("[himark] surface: {error:#}");
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn himark_surface_destroy(surface: *mut HimarkSurface) {
    if !surface.is_null() {
        drop(Box::from_raw(surface));
    }
}

/// One frame: collect the paint into the scene, then present it.
fn frame<R>(
    surface: &mut HimarkSurface,
    width: f32,
    height: f32,
    scale: f32,
    paint: impl FnOnce(&Canvas<'_>) -> R,
) -> R {
    surface.scene.clear();
    let atlas = surface.renderer.sprite_atlas().clone();
    let painter = ScenePainter::new(
        &mut surface.scene,
        &*atlas,
        &surface.context.text,
        gfx::Size::new(width, height),
        scale,
    );
    let result = paint(&painter);
    drop(painter);
    surface.scene.finish();
    surface.renderer.draw(&surface.scene);
    result
}

/// Paint the window bridge and present it. `width`/`height` are the
/// layer's drawable size in pixels.
#[no_mangle]
pub unsafe extern "C" fn himark_surface_bridge_draw(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    surface: *mut HimarkSurface,
    width: f32,
    height: f32,
    scale: f32,
) -> bool {
    let (Some(engine), Some(surface)) = (engine.as_mut(), surface.as_mut()) else {
        return false;
    };
    frame(surface, width, height, scale, |canvas| {
        bridge_draw(engine, bridge, canvas, width, height)
    })
}

/// Paint the realized slice and present it.
#[no_mangle]
pub unsafe extern "C" fn himark_surface_frame_paint(
    engine: *mut HimarkEngine,
    bridge: HimarkBridge,
    surface: *mut HimarkSurface,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    scale: f32,
) -> bool {
    let (Some(engine), Some(surface)) = (engine.as_mut(), surface.as_mut()) else {
        return false;
    };
    frame(surface, width, height, scale, |canvas| {
        frame_paint(engine, bridge, canvas, x, y, width, height)
    })
}
