// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

#import "SkiaMetalBridge.h"
#import <Metal/Metal.h>
#import <QuartzCore/CAMetalLayer.h>

#include <atomic>
#include <unordered_map>

#include "include/core/SkCanvas.h"
#include "include/core/SkColorSpace.h"
#include "include/core/SkSurface.h"
#include "include/gpu/graphite/BackendTexture.h"
#include "include/gpu/graphite/Context.h"
#include "include/gpu/graphite/Image.h"
#include "include/gpu/graphite/ImageProvider.h"
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
    /// The Context's home: Graphite demands external sync for the
    /// Context, and a SERIAL queue is exactly that. Recording stays
    /// on the UI thread (the Recorder's affinity); playback, submit
    /// and the still-geometry presents live here — the UI thread
    /// never waits on the GPU for a frame whose layer didn't move.
    dispatch_queue_t submit;
};

/// One screen of retina text and its atlases, shared across views.
static constexpr size_t kGpuBudgetBytes = 128 << 20;

static bool trace_images(void);

/// THE IMAGE DOOR. Graphite draws nothing at all for a CPU-backed
/// image — its default provider "doesn't perform any conversion"
/// (Skia's own words in Recorder.cpp), so a raster image is silently
/// DROPPED from the frame. Every picture the engine paints arrives
/// that way: mermaid diagrams rasterized by resvg, markdown images
/// decoded by skia itself. They all went blank the day these shells
/// moved to Graphite; nothing in the engine was wrong.
///
/// It CACHES because `findOrCreate` runs per draw, per frame: a
/// 798x1246 diagram re-uploaded every tick costs more than the frame
/// it serves. The key is the image's unique id — a re-rasterized SVG
/// mints a new one, so a stale texture falls out with its key — and
/// past the budget the whole map drops, the visible few re-uploading
/// on the next frame (the law the engine's own caches keep).
///
/// THREAD AFFINITY: one Recorder, driven from the render thread
/// alone; the map needs no lock for the same reason the Recorder
/// needs none.
class UploadingImageProvider final : public skgpu::graphite::ImageProvider {
public:
    static sk_sp<UploadingImageProvider> Make() {
        return sk_sp<UploadingImageProvider>(new UploadingImageProvider);
    }

    sk_sp<SkImage> findOrCreate(skgpu::graphite::Recorder *recorder,
                                const SkImage *image,
                                SkImage::RequiredProperties properties) override {
        const uint32_t id = image->uniqueID();
        auto found = fUploaded.find(id);
        if (found != fUploaded.end() && found->second.properties == properties) {
            return found->second.image;
        }
        sk_sp<SkImage> uploaded = SkImages::TextureFromImage(recorder, image, properties);
        if (trace_images()) {
            NSLog(@"[image] upload %dx%d -> %s (cached %zu)", image->width(), image->height(),
                  uploaded ? "ok" : "FAILED", fUploaded.size());
        }
        if (!uploaded) {
            return nullptr;
        }
        // A screen's worth of pictures; past it the map drops whole
        // rather than growing for the life of the process.
        if (fUploaded.size() >= 64) {
            fUploaded.clear();
        }
        fUploaded.insert_or_assign(id, Entry{uploaded, properties});
        return uploaded;
    }

private:
    struct Entry {
        sk_sp<SkImage> image;
        SkImage::RequiredProperties properties;
    };

    UploadingImageProvider() {}

    std::unordered_map<uint32_t, Entry> fUploaded;
};

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
    gpu->submit = dispatch_queue_create("himark.graphite.submit", DISPATCH_QUEUE_SERIAL);

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
        // Without this, every CPU-backed image draws as nothing.
        recorder.fImageProvider = UploadingImageProvider::Make();
        gpu->recorder = gpu->context->makeRecorder(recorder);
    }
    if (shared == nullptr) {
        shared = gpu;
    }
    return gpu;
}

/// `HIMARK_TRACE_FRAME=1` times the three waits a frame can sit in:
/// vending a drawable, handing the recording to the Context, and
/// the present itself. A resize that hangs off-CPU is waiting in
/// exactly one of them.
static bool trace_frame(void) {
    static bool on = getenv("HIMARK_TRACE_FRAME") != nullptr;
    return on;
}

static double now_ms(void) {
    return CACurrentMediaTime() * 1000.0;
}

/// `HIMARK_TRACE_IMAGES=1` reports every upload through the door —
/// the probe for a picture that silently refuses to appear.
static bool trace_images(void) {
    static bool on = getenv("HIMARK_TRACE_IMAGES") != nullptr;
    return on;
}

static bool trace_present(void) {
    static bool on = getenv("HIMARK_TRACE_PRESENT") != nullptr;
    return on;
}

static void watch_present(id<CAMetalDrawable> drawable, const char *road) {
    if (!trace_present()) return;
    [drawable addPresentedHandler:^(id<MTLDrawable> presented) {
        // presentedTime 0 = the system DROPPED this present — the
        // pixels never reached glass.
        NSLog(@"[present] %s drawable %lu t=%f", road,
              (unsigned long)presented.drawableID, presented.presentedTime);
    }];
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

extern "C" void *skia_metal_begin(SkiaMetal *self, int32_t width, int32_t height, bool coupled) {
    if (!self || !self->gpu->recorder) return nullptr;
    // THE LATCH LIVES HERE: a drawable adopts the layer's
    // presentsWithTransaction at VEND time, not at present time.
    // Deciding the road at end() handed out transaction-coupled
    // drawables to the async road — presented on the submit
    // thread, they waited for a commit that never comes (a
    // popover empty until its first re-dirty).
    // ALWAYS transaction-coupled, and latched here because a
    // drawable adopts the mode at VEND time. Flipping it per frame
    // (the async road, a0c6635e) leaves `nextDrawable` waiting out
    // CoreAnimation's one-second timeout whenever a present from
    // the other mode is still pending — a drag spent 51.9s on 240
    // steps that way, and 4.6s this way.
    (void)coupled;
    self->layer.presentsWithTransaction = YES;
    const double asked = trace_frame() ? now_ms() : 0;
    static std::atomic<int> vended{0};
    vended.fetch_add(1);
    self->drawable = [self->layer nextDrawable];
    if (trace_frame()) {
        const double waited = now_ms() - asked;
        if (waited > 4.0) {
            NSLog(@"[frame] nextDrawable WAITED %.1fms (%dx%d vended=%d)%@", waited, width,
                  height, vended.load(), self->drawable ? @"" : @" -> NIL");
        }
    }
    if (!self->drawable) return nullptr;

    // Skia retains the MTLTexture inside the wrap; the drawable
    // itself stays ours until the present.
    auto texture = skgpu::graphite::BackendTextures::MakeMetal(
        SkISize::Make(width, height), (__bridge CFTypeRef)self->drawable.texture);
    self->surface = SkSurfaces::WrapBackendTexture(
        self->gpu->recorder.get(), texture, kBGRA_8888_SkColorType, nullptr, nullptr);
    return self->surface ? self->surface->getCanvas() : nullptr;
}

/// MOVED-GEOMETRY frames: the layer's position changed in this CA
/// transaction, so pixels must land in the same commit (the flicker
/// law). Playback runs on the Context's queue — synchronously, the
/// caller still inside the transaction — then this thread waits for
/// scheduling and presents.
extern "C" void skia_metal_end(SkiaMetal *self) {
    if (!self || !self->surface) return;
    self->surface.reset();
    std::unique_ptr<skgpu::graphite::Recording> recording = self->gpu->recorder->snap();
    const double handed = trace_frame() ? now_ms() : 0;
    if (recording) {
        skgpu::graphite::Recording *played = recording.release();
        SkiaMetalGpu *gpu = self->gpu;
        dispatch_sync(gpu->submit, ^{
            skgpu::graphite::InsertRecordingInfo info;
            info.fRecording = played;
            gpu->context->insertRecording(info);
            gpu->context->submit();
            delete played;
        });
    }

    const double played_ms = trace_frame() ? now_ms() - handed : 0;
    const double presenting = trace_frame() ? now_ms() : 0;
    id<MTLCommandBuffer> buffer = [self->gpu->queue commandBuffer];
    [buffer commit];
    [buffer waitUntilScheduled];
    watch_present(self->drawable, "coupled");
    [self->drawable present];
    self->drawable = nil;
    if (trace_frame()) {
        const double present_ms = now_ms() - presenting;
        if (played_ms + present_ms > 4.0) {
            NSLog(@"[frame] present: submit-queue %.1fms present %.1fms", played_ms,
                  present_ms);
        }
    }
}

