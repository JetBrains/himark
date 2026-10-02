// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

//! The Graphite road: Skia's next-generation GPU backend over Metal,
//! replacing the CPU softbuffer blit on macOS (`--features graphite`).
//!
//! One `CAMetalLayer` is hosted in the winit view; every frame wraps
//! the layer's next drawable as a Graphite surface, paints, snaps the
//! recording and presents through the shared command queue.

use std::error::Error;

use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGSize;
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLPixelFormat,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use skia_safe::gpu::graphite::{self, mtl as graphite_mtl, Context as GraphiteContext};
use skia_safe::{Canvas, ColorType};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

pub(crate) struct GraphiteSurface {
    layer: Retained<CAMetalLayer>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    context: GraphiteContext,
}

impl GraphiteSurface {
    pub(crate) fn new(window: &Window) -> Result<Self, Box<dyn Error>> {
        let RawWindowHandle::AppKit(handle) = window.window_handle()?.as_raw() else {
            return Err("graphite needs an AppKit window".into());
        };
        let device = MTLCreateSystemDefaultDevice().ok_or("no system Metal device")?;

        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setPresentsWithTransaction(false);
        // Skia's blend modes sample the destination — the drawable
        // cannot be framebuffer-only.
        layer.setFramebufferOnly(false);
        layer.setContentsScale(window.scale_factor());
        let size = window.inner_size();
        layer.setDrawableSize(CGSize::new(size.width as f64, size.height as f64));

        let view = handle.ns_view.as_ptr() as *mut NSView;
        let view = unsafe { view.as_ref() }.ok_or("the window has no view")?;
        view.setWantsLayer(true);
        view.setLayer(Some(&layer.clone().into_super()));

        let queue = device.newCommandQueue().ok_or("no Metal command queue")?;
        let backend = unsafe {
            graphite_mtl::BackendContext::new(
                Retained::as_ptr(&device) as graphite_mtl::Handle,
                Retained::as_ptr(&queue) as graphite_mtl::Handle,
            )
        };
        let context = graphite_mtl::context_factory::make_metal(&backend, None)
            .ok_or("graphite rejected the Metal device")?;
        Ok(Self {
            layer,
            queue,
            context,
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.layer
            .setDrawableSize(CGSize::new(width as f64, height as f64));
    }

    /// One frame: acquire the drawable, paint through `draw`, submit
    /// the recording and present. `Ok(false)` means no drawable was
    /// available — skip and keep the previous frame on screen.
    pub(crate) fn frame(&mut self, draw: impl FnOnce(&Canvas)) -> Result<bool, Box<dyn Error>> {
        autoreleasepool(|_| {
            let Some(drawable) = (unsafe { self.layer.nextDrawable() }) else {
                return Ok(false);
            };
            let size = self.layer.drawableSize();

            let mut recorder = self
                .context
                .make_recorder(None)
                .ok_or("graphite recorder")?;
            let texture = unsafe {
                graphite_mtl::backend_textures::make_metal(
                    (size.width as i32, size.height as i32),
                    Retained::as_ptr(&drawable.texture()) as graphite_mtl::Handle,
                )
            };
            let mut surface = graphite::surfaces::wrap_backend_texture(
                &mut recorder,
                &texture,
                ColorType::BGRA8888,
                None,
                None,
            )
            .ok_or("the drawable refused to wrap")?;

            draw(surface.canvas());
            drop(surface);

            let mut recording = recorder.snap().ok_or("the recording did not snap")?;
            self.context
                .insert_recording(&graphite::InsertRecordingInfo::new(&mut recording));
            self.context.submit(None);

            let command_buffer = self.queue.commandBuffer().ok_or("no command buffer")?;
            let drawable: Retained<ProtocolObject<dyn objc2_metal::MTLDrawable>> =
                (&drawable).into();
            command_buffer.presentDrawable(&drawable);
            command_buffer.commit();
            Ok(true)
        })
    }
}
