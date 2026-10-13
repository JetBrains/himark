// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#import "SkiaMetalBridge.h"
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include "include/core/SkCanvas.h"
#include "include/core/SkColorSpace.h"
#include "include/core/SkSurface.h"
#include "include/gpu/ganesh/GrDirectContext.h"
#include "include/gpu/ganesh/GrBackendSurface.h"
#include "include/gpu/ganesh/SkSurfaceGanesh.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendContext.h"
#include "include/gpu/ganesh/mtl/GrMtlDirectContext.h"
#include "include/gpu/ganesh/mtl/GrMtlBackendSurface.h"

/// The GPU context belongs to the DEVICE, not to a layer: it can wrap
/// any layer's drawable, and every view in the app draws on the one
/// system device. One context per view meant one RESOURCE CACHE per
/// view — four of them, each at Skia's default budget, each with its
/// own glyph atlas and pipeline state. Measured on a live session
/// that was 652 MB of textures across 399 render targets.
///
/// THREAD AFFINITY: a `GrDirectContext` may only be used from one
/// thread, and every `render()` runs on the main thread. Anything
/// that draws a bridge off-main must get its own context.
struct SkiaMetalGpu {
    id<MTLDevice> device;
    id<MTLCommandQueue> queue;
    sk_sp<GrDirectContext> context;
};

/// One screen of retina text and its atlases, shared now that there
/// is a single cache rather than one per view.
static constexpr size_t kResourceCacheBytes = 128 << 20;

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

    GrMtlBackendContext backend;
    backend.fDevice.retain((__bridge void *)device);
    backend.fQueue.retain((__bridge void *)gpu->queue);
    gpu->context = GrDirectContexts::MakeMetal(backend);
    if (gpu->context) {
        gpu->context->setResourceCacheLimit(kResourceCacheBytes);
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
    if (!self) return nullptr;
    self->drawable = [self->layer nextDrawable];
    if (!self->drawable) return nullptr;

    GrMtlTextureInfo info;
    info.fTexture.retain((__bridge void *)self->drawable.texture);
    GrBackendRenderTarget target =
        GrBackendRenderTargets::MakeMtl(width, height, info);

    self->surface = SkSurfaces::WrapBackendRenderTarget(
        self->gpu->context.get(), target, kTopLeft_GrSurfaceOrigin,
        kBGRA_8888_SkColorType, nullptr, nullptr);
    return self->surface ? self->surface->getCanvas() : nullptr;
}

extern "C" void skia_metal_end(SkiaMetal *self) {
    if (!self || !self->surface) return;
    self->gpu->context->flushAndSubmit();
    self->surface.reset();

    // The presentsWithTransaction sequence: schedule the work, wait
    // for it, then present ON THIS THREAD so the drawable becomes
    // visible as part of the CA transaction the caller is inside.
    id<MTLCommandBuffer> buffer = [self->gpu->queue commandBuffer];
    [buffer commit];
    [buffer waitUntilScheduled];
    [self->drawable present];
    self->drawable = nil;
}
