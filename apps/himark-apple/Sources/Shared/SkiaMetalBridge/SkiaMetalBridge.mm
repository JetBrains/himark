// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#import "SkiaMetalBridge.h"
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include "include/core/SkCanvas.h"
#include "include/core/SkColorSpace.h"
#include "include/core/SkSurface.h"
#include "include/gpu/graphite/BackendTexture.h"
#include "include/gpu/graphite/Context.h"
#include "include/gpu/graphite/ContextOptions.h"
#include "include/gpu/graphite/GraphiteTypes.h"
#include "include/gpu/graphite/Recorder.h"
#include "include/gpu/graphite/Recording.h"
#include "include/gpu/graphite/Surface.h"
#include "include/gpu/graphite/mtl/MtlBackendContext.h"
#include "include/gpu/graphite/mtl/MtlGraphiteTypes_cpp.h"

/// GRAPHITE over Metal — Skia's successor backend. The paint road is
/// unchanged (the engine sees a plain SkCanvas); what moved is the
/// seam: draws RECORD into a Recorder, a snapped Recording is
/// inserted and submitted through the shared Context.
///
/// The GPU state belongs to the DEVICE, not to a layer: one Context
/// per device, and — the Graphite twin of the one-resource-cache
/// lesson — ONE long-lived Recorder shared by every view. Graphite
/// keeps its glyph and path atlases on the RECORDER: a recorder per
/// view (or per frame) would shape and upload the same text again
/// and again.
///
/// THREAD AFFINITY: Context and Recorder may only be used from one
/// thread, and every `render()` runs on the main thread. Anything
/// that draws a bridge off-main must get its own pair.
struct SkiaMetalGpu {
    id<MTLDevice> device;
    id<MTLCommandQueue> queue;
    std::unique_ptr<skgpu::graphite::Context> context;
    std::unique_ptr<skgpu::graphite::Recorder> recorder;
};

/// One screen of retina text and its atlases, shared across views.
static constexpr size_t kGpuBudgetBytes = 128 << 20;

static SkiaMetalGpu *shared_gpu(id<MTLDevice> device) {
    static SkiaMetalGpu *shared = nullptr;
    if (shared != nullptr && shared->device == device) {
        return shared;
    }
    // A second device is not a case this app has; serve it with its
    // own context rather than silently drawing on the wrong one.
    auto *gpu = new SkiaMetalGpu();
    gpu->device = device;
    gpu->queue = [device newCommandQueue];

    skgpu::graphite::MtlBackendContext backend;
    backend.fDevice.retain((__bridge CFTypeRef)device);
    backend.fQueue.retain((__bridge CFTypeRef)gpu->queue);
    skgpu::graphite::ContextOptions options;
    // Text out of the atlas, rects and hairlines with analytic AA —
    // Graphite's DEFAULT 4x internal MSAA only buys complex-path
    // quality this UI never draws, and charges a multisampled
    // target plus a resolve on every render pass for it.
    options.fInternalMultisampleCount = skgpu::graphite::SampleCount::k1;
    gpu->context = skgpu::graphite::ContextFactory::MakeMetal(backend, options);
    if (gpu->context) {
        gpu->context->setMaxBudgetedBytes(kGpuBudgetBytes);
        skgpu::graphite::RecorderOptions recorder;
        recorder.fGpuBudgetInBytes = kGpuBudgetBytes;
        // Every recording is inserted in the order it was snapped —
        // telling Graphite so lets the text atlas carry state from
        // frame to frame instead of proving it each time.
        recorder.fRequireOrderedRecordings = true;
        gpu->recorder = gpu->context->makeRecorder(recorder);
    }
    if (shared == nullptr) {
        shared = gpu;
    }
    return gpu;
}

struct SkiaMetal {
    CAMetalLayer *layer;
    /// Borrowed: the context outlives every view that draws with it.
    SkiaMetalGpu *gpu;
    id<CAMetalDrawable> drawable;
    sk_sp<SkSurface> surface;
};

extern "C" SkiaMetal *skia_metal_create(void *ca_metal_layer, void *mtl_device) {
    auto *self = new SkiaMetal();
    self->layer = (__bridge CAMetalLayer *)ca_metal_layer;
    self->gpu = shared_gpu((__bridge id<MTLDevice>)mtl_device);
    // ALWAYS transaction-coupled. `presentDrawable:` hands the pixels
    // to the GPU to show whenever it finishes, which is a different
    // frame from the one where CoreAnimation commits the layer's new
    // position — that gap IS the flicker. Presenting inside the
    // transaction costs a main-thread wait and buys atomicity.
    self->layer.presentsWithTransaction = YES;
    return self;
}

extern "C" void skia_metal_destroy(SkiaMetal *self) {
    if (!self) return;
    // The per-view surface goes; the shared context stays — other
    // views are still drawing with it.
    self->surface.reset();
    delete self;
}

extern "C" void *skia_metal_begin(SkiaMetal *self, int32_t width, int32_t height) {
    if (!self || !self->gpu->recorder) return nullptr;
    self->drawable = [self->layer nextDrawable];
    if (!self->drawable) return nullptr;

    // Skia retains the MTLTexture inside the wrap; the drawable
    // itself stays ours until the present.
    auto texture = skgpu::graphite::BackendTextures::MakeMetal(
        SkISize::Make(width, height), (__bridge CFTypeRef)self->drawable.texture);
    self->surface = SkSurfaces::WrapBackendTexture(
        self->gpu->recorder.get(), texture, kBGRA_8888_SkColorType, nullptr, nullptr);
    return self->surface ? self->surface->getCanvas() : nullptr;
}

extern "C" void skia_metal_end(SkiaMetal *self) {
    if (!self || !self->surface) return;
    // The draws recorded against this surface become one Recording;
    // inserting and submitting plays it onto the drawable's texture.
    self->surface.reset();
    std::unique_ptr<skgpu::graphite::Recording> recording = self->gpu->recorder->snap();
    if (recording) {
        skgpu::graphite::InsertRecordingInfo info;
        info.fRecording = recording.get();
        self->gpu->context->insertRecording(info);
        self->gpu->context->submit();
    }

    // The presentsWithTransaction sequence: the submit above rides
    // the shared queue; an empty buffer behind it schedules AFTER
    // the paint, so waiting on it then presenting ON THIS THREAD
    // makes the drawable visible as part of the CA transaction the
    // caller is inside.
    id<MTLCommandBuffer> buffer = [self->gpu->queue commandBuffer];
    [buffer commit];
    [buffer waitUntilScheduled];
    [self->drawable present];
    self->drawable = nil;
}
