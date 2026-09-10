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

struct SkiaMetal {
    CAMetalLayer *layer;
    id<MTLDevice> device;
    id<MTLCommandQueue> queue;
    sk_sp<GrDirectContext> context;
    id<CAMetalDrawable> drawable;
    sk_sp<SkSurface> surface;
    bool sync;
};

extern "C" SkiaMetal *skia_metal_create(void *ca_metal_layer, void *mtl_device) {
    auto *self = new SkiaMetal();
    self->layer = (__bridge CAMetalLayer *)ca_metal_layer;
    self->device = (__bridge id<MTLDevice>)mtl_device;
    self->queue = [self->device newCommandQueue];

    GrMtlBackendContext backend;
    backend.fDevice.retain((__bridge void *)self->device);
    backend.fQueue.retain((__bridge void *)self->queue);
    self->context = GrDirectContexts::MakeMetal(backend);
    return self;
}

extern "C" void skia_metal_destroy(SkiaMetal *self) {
    if (!self) return;
    self->surface.reset();
    self->context.reset();
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
        self->context.get(), target, kTopLeft_GrSurfaceOrigin,
        kBGRA_8888_SkColorType, nullptr, nullptr);
    return self->surface ? self->surface->getCanvas() : nullptr;
}

extern "C" void skia_metal_set_sync(SkiaMetal *self, bool sync) {
    if (!self) return;
    self->sync = sync;
    self->layer.presentsWithTransaction = sync ? YES : NO;
}

extern "C" void skia_metal_end(SkiaMetal *self) {
    if (!self || !self->surface) return;
    self->context->flushAndSubmit();
    self->surface.reset();

    id<MTLCommandBuffer> buffer = [self->queue commandBuffer];
    if (self->sync) {
        [buffer commit];
        [buffer waitUntilScheduled];
        [self->drawable present];
    } else {
        [buffer presentDrawable:self->drawable];
        [buffer commit];
    }
    self->drawable = nil;
}
