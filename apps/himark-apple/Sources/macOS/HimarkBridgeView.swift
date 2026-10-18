// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import AppKit
import Metal
import QuartzCore

/// One bridge in one NSView: the GENERIC host-side wrapper the
/// architecture is built around (docs/himark-core.md — a shell is
/// the engine plus some number of mounted views). It owns the
/// metal layer and skia surface, feeds the bridge through the
/// normalized event handles, paints it with `himark_bridge_draw`,
/// and points the text-input client at the bridge. It knows
/// nothing of windows or the workbench; `HimarkView` is the
/// workbench-window special case on top.
class HimarkBridgeView: NSView, NSTextInputClient {
    let engine: HimarkEngine
    let bridgeId: HimarkBridge
    /// Kept for the overlay layers this view spawns — each hosts
    /// a full bridge view of its own (recursion is normal).
    private let device: MTLDevice

    /// Embedded as a SCROLL VIEW's content: the view spans the whole
    /// content, its coordinates ARE content coordinates (flipped),
    /// and only the VISIBLE band gets a drawable and a paint (the
    /// stage road). The host scroller owns all physics.
    let scrollEmbedded: Bool

    /// The measured content size (points) — the embedding scroller
    /// sizes this view and its contentSize from it.
    var onContentMeasured: ((CGSize) -> Void)?

    private var surface: SkiaMetalSurface!

    private var caLink: CADisplayLink?
    private var linkRunning = false
    private var needsRedraw = true
    /// The viewport moved but nothing else did. A scrolled frame
    /// whose band still covers the viewport needs NO paint: the
    /// layer rides document coordinates, so the right pixels are
    /// already on screen — the margin is consumed, not repainted.
    private var scrolled = false

    private var idleTicks = 0

    private static let idleGraceTicks = 30

    /// THE BAND MARGIN. The tick paints in its own transaction,
    /// after the wheel's — the strip a fast frame exposes must
    /// already be painted, or it flashes background until the next
    /// tick. The band extends past the viewport by a momentum
    /// frame's travel on both ends; the painted rows ride the
    /// document, so the margin stays correct while it is consumed.
    private static let bandMargin: CGFloat = 192

    /// Repaint BEFORE the margin runs out: within this much of a
    /// band edge the next band is painted while the current one
    /// still covers — waiting for coverage to fail put the strip on
    /// screen first and the repaint (worst 17.5ms, two frames) kept
    /// it there.
    private static let bandGuard: CGFloat = 64

    // ─── the sync probe (HIMARK_TRACE_SYNC) ───
    //
    // Flicker is a TEMPORAL fault: the band's geometry and its pixels
    // reaching the screen in different frames. Two things make that
    // possible, and both are countable from in here — a present that
    // is not coupled to a CA transaction (so the pixels land whenever
    // the GPU finishes), and a present whose layer position disagrees
    // with where the scroller has already moved the clip view.
    private static let traceSync =
        ProcessInfo.processInfo.environment["HIMARK_TRACE_SYNC"] != nil
    private var presents = 0
    private var driftPresents = 0
    private var rolledBack = 0
    private var slowRenders = 0
    private var worstMs = 0.0
    private var totalMs = 0.0
    private var deferredRenders = 0
    /// WHY ticks rendered — the skip only pays if scrolled frames
    /// are not all dirty for other reasons.
    private var renderedDirty = 0
    private var renderedAnimating = 0
    private var renderedMargin = 0
    private var skippedScrolls = 0

    init(
        frame: NSRect,
        engine: HimarkEngine,
        device: MTLDevice,
        bridge: HimarkBridge,
        scrollEmbedded: Bool = false
    ) {
        self.engine = engine
        self.bridgeId = bridge
        self.device = device
        self.scrollEmbedded = scrollEmbedded
        super.init(frame: frame)
        wantsLayer = true
        metal.device = device
        metal.pixelFormat = .bgra8Unorm
        metal.framebufferOnly = false
        metal.contentsScale = NSScreen.main?.backingScaleFactor ?? 2.0
        // A resize moves the layer before we can paint the new size.
        // The default gravity (`resize`) SCALES the last drawable to
        // fit — that is the stretched text during a window drag.
        // Anchored at the top-left, an unpainted frame shows correct
        // pixels in the right place with a gap, which the next paint
        // fills.
        metal.contentsGravity = .topLeft
        if scrollEmbedded {
            // A SUBLAYER sized to the visible band, not the root
            // layer: the content-sized view must not back its whole
            // (possibly huge) bounds with a drawable. TRANSPARENT:
            // the host's glass shows through the stage paint.
            metal.isOpaque = false
            layer?.addSublayer(metal)
        } else {
            layer = metal
        }

        surface = SkiaMetalSurface(layer: metal, device: device)
        // The engine clock's fan-out: a tick that moved ANY state
        // owes every view a frame, not just the view whose link
        // happened to fire first.
        engine.frameClock.adopt(self) { [weak self] in self?.request() }
        startDisplayLink()
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit {
        engine.frameClock.orphan(self)
        caLink?.invalidate()
    }

    private let metal = CAMetalLayer()
    var metalLayer: CAMetalLayer { metal }

    /// Content coordinates read top-down inside a scroller, exactly
    /// as the engine lays out.
    override var isFlipped: Bool { scrollEmbedded }

    // ─── the event road: mint, dispatch, release ───

    /// Dispatch one freshly minted event handle to this bridge and
    /// release it. `shapes` applies the answered pointer shape —
    /// the move/modifier events' half of the contract.
    @discardableResult
    func send(_ event: HimarkEvent, shapes: Bool = false) -> Bool {
        let pointed = engine.bridgeEvent(bridge: bridgeId, event: event)
        himark_event_release(event)
        if pointed.changed { request() }
        if shapes { applyPointerShape(pointed.shape) }
        engine.drainReveal()
        return pointed.changed
    }

    @discardableResult
    func sendKey(_ code: UInt32, mods: UInt32 = 0) -> Bool {
        send(himark_event_key_down(code, mods))
    }

    // ─── the frame loop ───

    private func startDisplayLink() {
        let link = displayLink(target: self, selector: #selector(caTick(_:)))
        link.add(to: .main, forMode: .common)
        link.isPaused = true
        caLink = link
        // A fresh link is born paused. A request that arrived while
        // the view stood windowless left `linkRunning` set with no
        // link behind it — resume's guard would then keep this new
        // link paused forever, a frozen view.
        linkRunning = false
        resumeDisplayLink()
    }

    @objc private func caTick(_ link: CADisplayLink) {
        // The driver's own estimate of when THIS frame reaches
        // glass — animations advance to where they will be SEEN,
        // not to where the tick happened to run.
        tick(nowMs: link.targetTimestamp * 1000.0)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        if newWindow == nil, let caLink {
            caLink.invalidate()
            self.caLink = nil
            linkRunning = false
            closeAllPopovers()
        }
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil, caLink == nil {
            startDisplayLink()
        }
    }

    private func resumeDisplayLink() {
        guard !linkRunning else { return }
        linkRunning = true
        idleTicks = 0
        caLink?.isPaused = false
    }

    private func pauseDisplayLink() {
        guard linkRunning else { return }
        linkRunning = false
        caLink?.isPaused = true
    }

    private func tick(nowMs: Double = CACurrentMediaTime() * 1000.0) {
        // ONE engine tick per display frame, whoever's link fires
        // first — the clock caches the verdict for same-frame
        // followers and requests every other view when it moved.
        let animating = engine.frameClock.tick(nowMs: nowMs, from: self)
        engine.drainReveal()
        reconcileOverlays()
        if warmupTicks > 0 {
            warmupTicks -= 1
            needsRedraw = true
        }
        if needsRedraw || animating {
            if Self.traceSync {
                if needsRedraw { renderedDirty += 1 } else { renderedAnimating += 1 }
            }
            needsRedraw = false
            scrolled = false
            render()
            idleTicks = 0
        } else if scrolled {
            scrolled = false
            // Paint when the margin runs LOW — not out.
            if bandNeedsRepaint() {
                if Self.traceSync { renderedMargin += 1 }
                render()
            } else {
                if Self.traceSync { skippedScrolls += 1 }
            }
            idleTicks = 0
        } else {
            idleTicks += 1
            if idleTicks >= Self.idleGraceTicks {
                pauseDisplayLink()
            }
        }
    }

    /// Render is NEVER re-entered: applying a measure resizes the
    /// document view, AppKit calls setFrameSize synchronously, and
    /// an inner render would re-measure a still-growing canvas into
    /// unbounded recursion (seen five frames deep in the wild). The
    /// inner ask coalesces into the next tick instead.
    private var rendering = false

    func render() {
        if rendering {
            needsRedraw = true
            if Self.traceSync { deferredRenders += 1 }
            return
        }
        rendering = true
        defer { rendering = false }
        guard surface != nil else { return }
        // SwiftUI's platform host can hand a NaN/infinite frame
        // mid-layout — converting that to a drawable size traps.
        guard bounds.width.isFinite, bounds.height.isFinite else { return }
        let scale = window?.backingScaleFactor ?? metalLayer.contentsScale
        metalLayer.contentsScale = scale

        if scrollEmbedded {
            // POSITION FIRST, PIXELS SECOND — the band law's twin.
            // A parked reveal (the chat's standing tail ask) moves
            // the scroller NOW, so the band realized below is the
            // one that will rest on screen. Painting before the
            // move realized a band at the stale position each time
            // the content grew — on a chat boot that walk woke
            // every sleeping row it crossed, loading their edits
            // and crawling the viewport visibly down the transcript.
            engine.drainReveal()
            // visibleRect is NOT clamped to bounds outside a clip
            // view — a bare embedded view (the composer) reports
            // the whole WINDOW in its coordinates.
            let visible = visibleRect.intersection(bounds)
            guard visible.width > 0.5, visible.height > 0.5,
                  visible.width.isFinite, visible.height.isFinite,
                  visible.minY.isFinite
            else { return }
            // The painted BAND: the viewport plus the margin, inside
            // the document.
            let top = max(0, visible.minY - Self.bandMargin)
            let bottom = min(bounds.maxY, visible.maxY + Self.bandMargin)
            let band = CGRect(
                x: visible.minX, y: top, width: visible.width, height: bottom - top)
            // GEOMETRY AND PIXELS MOVE TOGETHER OR NOT AT ALL. One
            // transaction holds the band's new position AND the
            // present that fills it, so CoreAnimation commits both in
            // the same frame. Driven from the scroller's
            // `boundsChanged`, that transaction is also AppKit's own
            // scroll update — which is what makes the band travel
            // with the clip view instead of chasing it.
            let wasAt = metalLayer.frame
            let w = Int32(band.width * scale)
            let h = Int32(band.height * scale)
            // The band MOVED or RESIZED: its new position and its
            // pixels must land in one commit (the flicker law) —
            // transaction-coupled present. A still band repainting
            // in place rides the async submit road.
            let coupled = wasAt != band
                || metalLayer.drawableSize != CGSize(width: CGFloat(w), height: CGFloat(h))
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            metalLayer.frame = band
            metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))
            let started = Self.traceSync ? CACurrentMediaTime() : 0
            let painted = renderBand(
                band, scale: scale, pixelWidth: w, pixelHeight: h, coupled: coupled)
            if Self.traceSync { noteDuration(CACurrentMediaTime() - started) }
            if !painted {
                // No drawable, nothing painted: a moved band showing
                // its old contents is the very artifact this exists
                // to prevent. Put it back and ask for another pass.
                metalLayer.frame = wasAt
                if Self.traceSync { rolledBack += 1 }
                request()
            }
            CATransaction.commit()
            // A reveal can be BORN at the paint (a canvas revealing
            // its row right after populate) — drain it this pass,
            // or the scroll waits for the next event.
            engine.drainReveal()
            return
        }

        if let origin = overlayOrigin {
            renderOverlaySlice(origin, scale: scale)
            return
        }

        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        // Same rule as the band: the drawable's new SIZE and the
        // pixels that fill it belong to one transaction, or a resize
        // commits a layer whose contents are a frame behind. A
        // STEADY size has nothing to commit with — the frame rides
        // the async submit road and this thread skips the GPU wait.
        let size = CGSize(width: CGFloat(w), height: CGFloat(h))
        // A LIVE RESIZE PRESENTS ON THIS THREAD. The async road
        // (a0c6635e) lets a still frame hand its drawable to the
        // submit queue and return — but during a drag the next
        // frame opens its CA transaction and asks the layer for a
        // drawable while that present is still pending, and nothing
        // can release it until we commit the transaction we are
        // standing in. CoreAnimation breaks the knot with its
        // one-second timeout, over and over: 240 resize steps took
        // 51.9s on the async road and 4.6s coupled.
        let resized = metalLayer.drawableSize != size
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        metalLayer.drawableSize = size
        let started = Self.traceSync ? CACurrentMediaTime() : 0
        switch engine.renderBridge(bridge: bridgeId, into: surface,
                                   pixelWidth: w, pixelHeight: h, scale: Float(scale),
                                   coupled: resized) {
        case .reconciling, .starved:
            // Starved: no drawable, nothing painted — the band road's
            // rollback law, minus the rollback (the layer never
            // moved). Dropping it here left stale pixels standing
            // until the next event.
            request()
        case .painted:
            break
        }
        CATransaction.commit()
        if Self.traceSync { noteDuration(CACurrentMediaTime() - started) }
        engine.drainReveal()
    }

    /// The OVERLAY road: the overlay frame shares the parent's
    /// coordinates; this paints exactly the content slice at
    /// `origin`, sized to this view.
    private func renderOverlaySlice(_ origin: CGPoint, scale: CGFloat) {
        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        guard w > 0, h > 0 else { return }
        let raw = engine.raw
        // The overlay layer RESIZES — from the seed to the content
        // slice, and with the menu's filter.
        // A resize is geometry: its pixels must land in the same
        // commit (the flicker law), or the commit shows the grown
        // layer empty and the async present has nothing to join.
        let size = CGSize(width: CGFloat(w), height: CGFloat(h))
        let resized = metalLayer.drawableSize != size
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        metalLayer.drawableSize = size
        let layout = himark_bridge_layout(
            raw, bridgeId, Float(bounds.width * scale), Float(bounds.height * scale))
        if layout.raw != 0 {
            let ox = Float(origin.x * scale)
            let oy = Float(origin.y * scale)
            let frame = himark_layout_realize(
                raw, layout, ox, oy, Float(bounds.width * scale), Float(bounds.height * scale))
            if frame.raw != 0 {
                if let canvas = surface.begin(pixelWidth: w, pixelHeight: h, coupled: resized) {
                    let changed = himark_frame_paint(
                        raw, frame, canvas, ox, oy,
                        Float(bounds.width * scale), Float(bounds.height * scale))
                    surface.end()
                    if Self.traceOverlay {
                        NSLog("[overlay] slice bridge \(bridgeId.raw): \(w)x\(h) at (\(ox),\(oy)) resized \(resized) changed \(changed)")
                    }
                    if changed {
                        request()
                    }
                    himark_frame_release(raw, frame)
                } else {
                    // Starved — nothing painted, retry owed.
                    himark_frame_release(raw, frame)
                    request()
                }
            }
        }
        CATransaction.commit()
        engine.drainReveal()
    }

    /// The stage road: layout at (width, ∞), content size off the
    /// sized stage, realize + paint the visible band. The stages
    /// are ONE-PASS values — a paint that changed state means the
    /// engine quiesced them, and the next tick re-derives.
    @discardableResult
    private func renderBand(
        _ visible: NSRect, scale: CGFloat, pixelWidth w: Int32, pixelHeight h: Int32,
        coupled: Bool = true
    ) -> Bool {
        let raw = engine.raw
        let layout = himark_bridge_layout(raw, bridgeId, Float(bounds.width * scale), .infinity)
        guard layout.raw != 0 else { return false }
        var cw: Float = 0
        var ch: Float = 0
        _ = himark_layout_size(raw, layout, &cw, &ch)

        if ProcessInfo.processInfo.environment["HIMARK_TRACE_BAND"] != nil {
            NSLog("band bridge \(bridgeId.raw) bounds \(bounds.size) visible \(visible) content \(ch / Float(scale))")
        }
        let offsetY = Float(max(visible.minY, 0) * scale)
        let frame = himark_layout_realize(
            raw, layout, 0, offsetY, Float(visible.width * scale), Float(visible.height * scale))
        guard frame.raw != 0 else { return false }
        guard let canvas = surface.begin(pixelWidth: w, pixelHeight: h, coupled: coupled)
        else {
            himark_frame_release(raw, frame)
            return false
        }
        let changed = himark_frame_paint(
            raw, frame, canvas, 0, offsetY,
            Float(visible.width * scale), Float(visible.height * scale))
        surface.end()
        if Self.traceSync { notePresent(layerY: visible.minY) }
        if changed {
            request()
        }
        // Released on EVERY road: the changed road kept the handle —
        // and the frame's whole arena with it — until unmount.
        himark_frame_release(raw, frame)
        // The measure lands AFTER the pass: applying it resizes the
        // document view, which re-enters render synchronously — an
        // outstanding layout would die refused and the band would
        // paint twice per streamed change.
        if ch.isFinite {
            onContentMeasured?(CGSize(width: bounds.width, height: CGFloat(ch) / scale))
        }
        return true
    }

    /// How long a pass took. A spring runs at the display's cadence,
    /// so anything past ~8 ms cannot keep up with it however well the
    /// present is synchronised — that is a COST fault, not a timing
    /// one, and it is the number that tells the two apart.
    private func noteDuration(_ seconds: Double) {
        let ms = seconds * 1000.0
        totalMs += ms
        worstMs = max(worstMs, ms)
        if ms > 8.0 { slowRenders += 1 }
    }

    /// One present, weighed against what the scroller already
    /// shows: drift is a band that fails to COVER the viewport —
    /// the margin spent faster than the tick repaints.
    private func notePresent(layerY: CGFloat) {
        presents += 1
        if let clip = enclosingScrollView?.contentView.bounds,
           clip.minY < metalLayer.frame.minY - 0.5
               || clip.maxY > metalLayer.frame.maxY + 0.5 {
            driftPresents += 1
        }
        if presents % 60 == 0 {
            NSLog(
                "[sync] bridge \(bridgeId.raw): \(presents) presents, "
                    + "\(driftPresents) drifted, \(deferredRenders) deferred, "
                    + "\(rolledBack) rolled back, "
                    + String(format: "avg %.1fms worst %.1fms %d over 8ms | ",
                             totalMs / Double(presents), worstMs, slowRenders)
                    + "why: \(renderedDirty) dirty \(renderedAnimating) anim "
                    + "\(renderedMargin) margin \(skippedScrolls) skipped")
        }
    }

    /// Seat this bridge's reveal handler — the engine routes the
    /// parked rect (device pixels) here; hand it on in points.
    func seatRevealHandler(_ onReveal: @escaping (CGRect, HimarkEngine.RevealPlacement) -> Void) {
        engine.onReveal(bridge: bridgeId) { [weak self] pixels, placement in
            guard let self else { return }
            let s = self.metalLayer.contentsScale
            onReveal(CGRect(x: pixels.minX / s, y: pixels.minY / s,
                            width: pixels.width / s, height: pixels.height / s),
                     placement)
        }
    }

    /// The embedding scroller moved: keep the clock alive; the tick
    /// decides whether the band needs repainting at all.
    func scrolledTick() {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { self.scrolledTick() }
            return
        }
        scrolled = true
        resumeDisplayLink()
    }

    /// Whether the band's remaining runway on the travelled side
    /// has fallen under the guard. Clamped document edges need no
    /// runway beyond themselves.
    private func bandNeedsRepaint() -> Bool {
        guard scrollEmbedded else { return true }
        let visible = visibleRect.intersection(bounds)
        guard visible.width > 0.5, visible.height > 0.5 else { return false }
        let band = metalLayer.frame
        guard band.width > 0.5 else { return true }
        let topRoom = visible.minY - band.minY
        let bottomRoom = band.maxY - visible.maxY
        let needTop = visible.minY > 0.5
        let needBottom = visible.maxY < bounds.height - 0.5
        return (needTop && topRoom < Self.bandGuard)
            || (needBottom && bottomRoom < Self.bandGuard)
    }

    // ─── overlay promotion: in-window layers, reconciled per frame ───
    //
    // The engine emits overlays immediate-mode; the shell retains
    // one view per overlay. Per tick: one epoch read — on change,
    // diff the emitted set against the open overlays by index. New
    // opens, absent closes, moved repositions. Each overlay's
    // content is ANOTHER bridge (the engine re-derives the overlay
    // alone), hosted as a plain subview of the window's content
    // view, above everything, at the content slice — composited in
    // the window exactly as the shell composites its own. An
    // overlay's own overlays become nested layers for free. No
    // NSPopover: a popover is a second window, and a second window
    // means its own appearance clock, dropped first presents, a
    // content that stays on screen after its overlay left, and
    // wheel events that never reach the pane beneath.
    private var overlayEpoch: UInt64 = 0
    private var popovers: [Int: (view: HimarkBridgeView, bridge: HimarkBridge, anchor: CGRect)] = [:]

    /// The host's redraw fan-out, handed down at minting so overlay
    /// content views (and THEIR overlays) hear store dirt too — an
    /// unadopted view paints once and then freezes.
    var adoptIntoRedraws: ((HimarkBridgeView) -> Void)?
    var orphanFromRedraws: ((HimarkBridgeView) -> Void)?

    /// The shell's wheel routing, when it has one (the desktop's
    /// router hands wheels to the topmost REGISTERED host under the
    /// pointer, ahead of the responder chain): an overlay layer
    /// registers so a wheel over its list is its own, not the
    /// scroller's beneath. Unset, the responder chain already
    /// delivers to the hit view.
    static var claimWheels: ((NSView, @escaping (NSEvent) -> Void) -> Void)?
    static var releaseWheels: ((NSView) -> Void)?

    /// WHERE the overlay layers stand: the shell's topmost view in
    /// the window. Unset, the window's content view — right for a
    /// window the bridge view fills (the workbench shell); wrong
    /// under SwiftUI, whose hosting view composites its own content
    /// above stray subviews (the desktop names its overlay canvas).
    static var overlayHost: ((NSWindow) -> NSView?)?

    private var overlayHostView: NSView? {
        guard let window else { return nil }
        return Self.overlayHost?(window) ?? window.contentView
    }

    /// Set on an OVERLAY's content view: the content slice's origin
    /// in the PARENT'S coordinates (points). The overlay frame
    /// shares the parent's coordinate space — events offset by
    /// this on the way in, the paint realizes this viewport.
    var overlayOrigin: CGPoint?

    /// A fresh layer's first presents can land before its layer is
    /// composited; a short warmup repaints across the appearance.
    var warmupTicks = 0

    private static let traceOverlay =
        ProcessInfo.processInfo.environment["HIMARK_TRACE_OVERLAY"] != nil

    /// An engine rect (content coordinates, top-down) in THIS
    /// view's coordinates — flipped views read it as is.
    private func viewRect(_ rect: CGRect) -> NSRect {
        isFlipped
            ? rect
            : NSRect(x: rect.minX, y: bounds.height - rect.maxY,
                     width: rect.width, height: rect.height)
    }

    private func reconcileOverlays() {
        let (epoch, pixelAnchors) = engine.overlays(bridge: bridgeId)
        if popovers.isEmpty && epoch == overlayEpoch { return }
        if Self.traceOverlay, epoch != overlayEpoch {
            let rects = pixelAnchors.map {
                "(\(Int($0.minX)),\(Int($0.minY)) \(Int($0.width))x\(Int($0.height)))"
            }.joined(separator: " ")
            NSLog("[overlay] bridge \(bridgeId.raw): epoch \(epoch) (seen \(overlayEpoch)) anchors [\(rects)] layers \(popovers.count)")
        }
        let s = metalLayer.contentsScale
        let anchors = pixelAnchors.map {
            CGRect(x: $0.minX / s, y: $0.minY / s, width: $0.width / s, height: $0.height / s)
        }
        // PRESENCE moves with the epoch: absent closes, new opens.
        if epoch != overlayEpoch {
            for (index, entry) in popovers where index >= anchors.count {
                if Self.traceOverlay {
                    NSLog("[overlay] close #\(index) (absent)")
                }
                closeOverlay(entry)
                popovers[index] = nil
            }
            // A layer needs the shell's host view to stand in;
            // without a window the epoch stays unseen and the next
            // tick retries.
            guard let host = overlayHostView else { return }
            overlayEpoch = epoch
            for (index, anchor) in anchors.enumerated() where popovers[index] == nil {
                let minted = engine.mintOverlay(bridge: bridgeId, index: index)
                if Self.traceOverlay {
                    NSLog("[overlay] open #\(index) anchor \(anchor) -> bridge \(minted.raw)")
                }
                guard minted.raw != 0 else { continue }
                let content = HimarkBridgeView(
                    frame: NSRect(origin: .zero, size: CGSize(width: 40, height: 20)),
                    engine: engine, device: device, bridge: minted)
                // Whatever the content leaves unpainted shows the
                // window beneath — an opaque layer would show it
                // black.
                content.metalLayer.isOpaque = false
                content.adoptIntoRedraws = adoptIntoRedraws
                content.orphanFromRedraws = orphanFromRedraws
                adoptIntoRedraws?(content)
                // Hidden until its first geometry lands: a seed-sized
                // layer at the window's origin is a flash.
                content.isHidden = true
                host.addSubview(content, positioned: .above, relativeTo: nil)
                Self.claimWheels?(content) { [weak content] event in
                    content?.scrollWheel(with: event)
                }
                content.warmupTicks = 3
                popovers[index] = (content, minted, anchor)
            }
        }
        // GEOMETRY is reconciled EVERY pass while anything is open:
        // the anchor moves with the parent's layout and scroll, the
        // size with the overlay's own content (a completion list
        // filters without touching its anchor). Measure is a
        // one-pass stage; overlays are few and short-lived.
        guard let host = overlayHostView else { return }
        for (index, entry) in popovers {
            guard index < anchors.count else { continue }
            let anchor = anchors[index]
            // Refresh the slice: lay (stashes the content rect),
            // release, read. The rect is the layer's size; its
            // origin is the content view's offset into the shared
            // coordinate space.
            let layout = himark_bridge_layout(
                engine.raw, entry.bridge, Float(bounds.width * s), Float(bounds.height * s))
            if layout.raw != 0 {
                himark_layout_release(engine.raw, layout)
            }
            var cx: Float = 0
            var cy: Float = 0
            var cw: Float = 0
            var ch: Float = 0
            let haveRect = himark_overlay_content_rect(engine.raw, entry.bridge, &cx, &cy, &cw, &ch)
            if Self.traceOverlay {
                NSLog("[overlay] geometry #\(index) bridge \(entry.bridge.raw): rect \(haveRect ? "(\(cx),\(cy) \(cw)x\(ch))" : "NONE")")
            }
            guard haveRect else {
                // The overlay stands empty this frame (a list with
                // no rows yet) — nothing to show, nothing to flash.
                entry.view.isHidden = true
                continue
            }
            let rect = CGRect(
                x: CGFloat(cx) / s, y: CGFloat(cy) / s,
                width: CGFloat(cw) / s, height: CGFloat(ch) / s)
            entry.view.overlayOrigin = rect.origin
            if entry.anchor != anchor {
                popovers[index]?.anchor = anchor
            }
            // WHERE the layer stands is the window's call, as it was
            // the popover's: the engine fitted the slice inside its
            // own pane (a 100pt composer cannot hold a 160pt list —
            // the fit clamps to its top), and the slice's origin
            // only names which pixels it holds. Hang the layer off
            // the ANCHOR, in window coordinates: on the side the
            // engine preferred, flipped when the window has no room
            // there, clamped inside the window.
            // In WINDOW coordinates (y up, whatever the host's flip
            // — SwiftUI's hosting view is flipped, a plain content
            // view is not), then back into the host for the frame.
            let anchorInWindow = convert(viewRect(anchor), to: nil)
            let room = host.convert(host.bounds, to: nil)
            let preferBelow = rect.midY > anchor.midY
            let belowY = anchorInWindow.minY - rect.height
            let aboveY = anchorInWindow.maxY
            let fitsBelow = belowY >= room.minY
            let fitsAbove = aboveY + rect.height <= room.maxY
            let y: CGFloat = switch (preferBelow, fitsBelow, fitsAbove) {
            case (true, true, _), (false, true, false): belowY
            case (false, _, true), (true, false, true): aboveY
            default: max(room.minY, min(belowY, room.maxY - rect.height))
            }
            let x = max(room.minX, min(anchorInWindow.minX, room.maxX - rect.width))
            let frame = host.convert(
                NSRect(x: x, y: y, width: rect.width, height: rect.height), from: nil)
            if Self.traceOverlay {
                NSLog("[overlay] layer #\(index) at \(frame) (anchor \(anchorInWindow), window \(room))")
            }
            if entry.view.superview !== host {
                host.addSubview(entry.view, positioned: .above, relativeTo: nil)
            }
            if entry.view.frame != frame {
                entry.view.frame = frame
                entry.view.request()
            }
            if entry.view.isHidden {
                entry.view.isHidden = false
                entry.view.request()
            }
        }
        reconcileOutsideClickWatch()
    }

    // ─── the outside-click watch ───
    //
    // The scrim covers only THIS pane; shell-app's covers the whole
    // window. While overlays stand, a local monitor plays the rest
    // of the window: any mouse-down outside the overlay layers and
    // this pane sends Escape into the pane's focus chain — the
    // combo or completion closes itself, the epoch moves, reconcile
    // closes the layers — and the click continues to whatever it
    // hit. A click INSIDE any overlay layer (this pane's, or a
    // nested one's) is that layer's own.
    private var outsideClickMonitor: Any?

    private func reconcileOutsideClickWatch() {
        if popovers.isEmpty {
            if let monitor = outsideClickMonitor {
                NSEvent.removeMonitor(monitor)
                outsideClickMonitor = nil
            }
            return
        }
        guard outsideClickMonitor == nil else { return }
        outsideClickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown]) {
            [weak self] event in
            guard let self, !self.popovers.isEmpty else { return event }
            guard event.window === self.window else { return event }
            if let content = event.window?.contentView,
               let frame = content.superview {
                let hit = content.hitTest(frame.convert(event.locationInWindow, from: nil))
                var walk: NSView? = hit
                while let view = walk {
                    if let bridge = view as? HimarkBridgeView,
                       bridge === self || bridge.overlayOrigin != nil {
                        return event
                    }
                    walk = view.superview
                }
            }
            if Self.traceOverlay {
                NSLog("[overlay] outside click -> escape to bridge \(self.bridgeId.raw)")
            }
            _ = self.sendKey(UInt32(HIMARK_KEY_ESCAPE))
            return event
        }
    }

    private func closeAllPopovers() {
        for (_, entry) in popovers {
            closeOverlay(entry)
        }
        popovers.removeAll()
        reconcileOutsideClickWatch()
    }

    /// Take a closed overlay's layer out of the window, unmount its
    /// bridge and orphan its view from the redraw fan-out.
    private func closeOverlay(_ entry: (view: HimarkBridgeView, bridge: HimarkBridge, anchor: CGRect)) {
        Self.releaseWheels?(entry.view)
        entry.view.removeFromSuperview()
        orphanFromRedraws?(entry.view)
        engine.unmount(bridge: entry.bridge)
    }

    func request() {
        guard Thread.isMainThread else {
            DispatchQueue.main.async { self.request() }
            return
        }
        needsRedraw = true
        resumeDisplayLink()
    }

    func devicePoint(_ event: NSEvent) -> (Float, Float) {
        let p = convert(event.locationInWindow, from: nil)
        let s = metalLayer.contentsScale
        let y = isFlipped ? p.y : (bounds.height - p.y)
        let o = overlayOrigin ?? .zero
        return (Float((p.x + o.x) * s), Float((y + o.y) * s))
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override var mouseDownCanMoveWindow: Bool { false }

    /// The down's dispatch, factored so subclasses can act on the
    /// MISS (the workbench window's titlebar drag rides it).
    @discardableResult
    func routeMouseDown(_ event: NSEvent) -> Bool {
        let (x, y) = devicePoint(event)
        // While overlays stand, the ENGINE'S SCRIM owns this pane's
        // clicks — shell-app's composited road, forwarded. The
        // overlay frame shares this pane's coordinates, so the
        // point travels untranslated; the scrim turns it into the
        // dismissal the widget wired, the epoch moves, reconcile
        // closes the layer.
        if !popovers.isEmpty {
            var consumed = false
            for (_, entry) in popovers {
                let down = himark_event_mouse_down(x, y, himarkMods(event), 1)
                consumed = engine.bridgeEvent(bridge: entry.bridge, event: down).changed || consumed
                himark_event_release(down)
                let up = himark_event_mouse_up(x, y)
                _ = engine.bridgeEvent(bridge: entry.bridge, event: up)
                himark_event_release(up)
            }
            engine.drainReveal()
            request()
            // A SCRIM took it: the click was the dismissal, spent.
            // Nothing took it (a completion popup carries no scrim
            // — the widget closes on its own interactions): the
            // click is the PANE'S, fall through.
            if consumed { return true }
        }
        let handled = send(himark_event_mouse_down(
            x, y, himarkMods(event), UInt32(clamping: event.clickCount)))
        window?.makeFirstResponder(self)
        return handled
    }

    override func mouseDown(with event: NSEvent) {
        routeMouseDown(event)
    }

    override func rightMouseDown(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        send(himark_event_secondary_down(x, y, himarkMods(event)))
        window?.makeFirstResponder(self)
    }

    override func mouseDragged(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        send(himark_event_mouse_drag(x, y, himarkMods(event)))
    }

    override func mouseUp(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        send(himark_event_mouse_up(x, y))
    }

    override func mouseMoved(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        send(himark_event_mouse_move(x, y, himarkMods(event)), shapes: true)
    }

    // Modifier-gated hover looks (the cmd-hover link) follow the keys
    // without pointer motion: re-hit-test under the standing pointer.
    override func flagsChanged(with event: NSEvent) {
        if let window {
            let location = window.mouseLocationOutsideOfEventStream
            let p = convert(location, from: nil)
            if bounds.contains(p) {
                let s = metalLayer.contentsScale
                let (x, y) = (Float(p.x * s), Float((bounds.height - p.y) * s))
                send(himark_event_modifiers_changed(x, y, himarkMods(event)), shapes: true)
            }
        }
        super.flagsChanged(with: event)
    }

    func applyPointerShape(_ shape: UInt32) {
        switch shape {
        case 1: NSCursor.pointingHand.set()
        default: NSCursor.arrow.set()
        }
    }

    override func mouseExited(with event: NSEvent) {
        send(himark_event_mouse_left())
        NSCursor.arrow.set()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        ))
    }

    func himarkMods(_ event: NSEvent) -> UInt32 {
        var mods: UInt32 = 0
        if event.modifierFlags.contains(.shift) { mods |= UInt32(HIMARK_MOD_SHIFT) }
        if event.modifierFlags.contains(.control) { mods |= UInt32(HIMARK_MOD_CONTROL) }
        if event.modifierFlags.contains(.option) { mods |= UInt32(HIMARK_MOD_ALT) }
        if event.modifierFlags.contains(.command) { mods |= UInt32(HIMARK_MOD_COMMAND) }
        return mods
    }

    override func scrollWheel(with event: NSEvent) {
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil {
            NSLog("wheel reached bridge \(bridgeId.raw) dy \(event.scrollingDeltaY) embedded \(scrollEmbedded)")
        }
        if scrollEmbedded {
            // The stage road has no scroller — the enclosing
            // NSScrollView owns the physics. But the overlays that
            // kept theirs (a completion popup's list, a horizontal
            // pan in unwrapped code) get the wheel OFFERED first.
            if offerWheel(event) { return }
            super.scrollWheel(with: event)
            return
        }
        let (x, y) = devicePoint(event)
        let s = Float(metalLayer.contentsScale)
        let perLine: Float = event.hasPreciseScrollingDeltas ? 1.0 : Self.lineScroll
        send(himark_event_scroll(x, y,
                                 Float(-event.scrollingDeltaX) * perLine * s,
                                 Float(-event.scrollingDeltaY) * perLine * s,
                                 himarkScrollPhase(event)))
    }

    /// Offer a wheel to the stage road synchronously: true when an
    /// overlay (completion popup, horizontal pan) consumed it — the
    /// embedding scroller keeps the event otherwise. Realizes the
    /// current band's frame for the one call, like a paint pass.
    func offerWheel(_ event: NSEvent) -> Bool {
        let point = convert(event.locationInWindow, from: nil)
        let perLine: Float = event.hasPreciseScrollingDeltas ? 1.0 : Self.lineScroll
        return offer(
            at: point,
            dx: Float(-event.scrollingDeltaX) * perLine,
            dy: Float(-event.scrollingDeltaY) * perLine,
            phase: himarkScrollPhase(event))
    }

    /// The offer core, NSEvent-free — the debug wheel harness drives
    /// it with crafted deltas at a content point.
    func offer(at point: NSPoint, dx: Float, dy: Float, phase: UInt32) -> Bool {
        let raw = engine.raw
        let scale = metalLayer.contentsScale
        let visible = visibleRect.intersection(bounds)
        guard !visible.isEmpty else { return false }
        let layout = himark_bridge_layout(raw, bridgeId, Float(bounds.width * scale), .infinity)
        guard layout.raw != 0 else { return false }
        let offsetY = Float(max(visible.minY, 0) * scale)
        let frame = himark_layout_realize(
            raw, layout, 0, offsetY, Float(visible.width * scale), Float(visible.height * scale))
        guard frame.raw != 0 else { return false }
        let s = Float(scale)
        let consumed = himark_frame_offer_scroll(
            raw, frame,
            Float(point.x) * s, Float(point.y) * s,
            dx * s,
            dy * s,
            phase,
            0, offsetY, Float(visible.width * scale), Float(visible.height * scale))
        himark_frame_release(raw, frame)
        if consumed { request() }
        return consumed
    }

    private func himarkScrollPhase(_ event: NSEvent) -> UInt32 {
        let momentum = event.momentumPhase
        if momentum.contains(.began) { return UInt32(HIMARK_SCROLL_PHASE_MOMENTUM_BEGAN) }
        if momentum.contains(.changed) { return UInt32(HIMARK_SCROLL_PHASE_MOMENTUM_CHANGED) }
        if !momentum.isEmpty { return UInt32(HIMARK_SCROLL_PHASE_MOMENTUM_ENDED) }
        let phase = event.phase
        if phase.contains(.mayBegin) { return UInt32(HIMARK_SCROLL_PHASE_MAY_BEGIN) }
        if phase.contains(.began) { return UInt32(HIMARK_SCROLL_PHASE_BEGAN) }
        if phase.contains(.changed) { return UInt32(HIMARK_SCROLL_PHASE_CHANGED) }
        if !phase.isEmpty { return UInt32(HIMARK_SCROLL_PHASE_ENDED) }
        return UInt32(HIMARK_SCROLL_PHASE_NONE)
    }

    private static let lineScroll: Float = 48.0

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        // A LIVE RESIZE MARKS; THE TICK PAINTS — the same law the
        // scroll path keeps. AppKit calls this many times per
        // display interval, and since the async road stopped
        // waiting for its own present (a0c6635e), each inline paint
        // asks the layer for a drawable the previous paint has not
        // released: `nextDrawable` then blocks a whole vsync, two
        // or three times per step, and the drag crawls.
        request()
    }

    // The backing scale can flip (wake from hibernation, moving to a
    // differently-scaled display) while rendering is paused; without a
    // repaint the last frame stays composited at the old scale.
    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        request()
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        render()
    }

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        if hasMarkedText() {
            interpretKeyEvents([event])
            return
        }

        if let code = himarkKey(event), sendKey(code, mods: himarkMods(event)) {
            return
        }
        interpretKeyEvents([event])
    }

    private func himarkKey(_ event: NSEvent) -> UInt32? {
        guard let scalar = event.charactersIgnoringModifiers?.unicodeScalars.first else { return nil }
        return switch scalar.value {
        case 0xF700: UInt32(HIMARK_KEY_UP)
        case 0xF701: UInt32(HIMARK_KEY_DOWN)
        case 0xF702: UInt32(HIMARK_KEY_LEFT)
        case 0xF703: UInt32(HIMARK_KEY_RIGHT)
        case 0xF704...0xF70F: UInt32(HIMARK_KEY_F1) + (scalar.value - 0xF704)
        case 0xF728: UInt32(HIMARK_KEY_DELETE)
        case 0xF729: UInt32(HIMARK_KEY_HOME)
        case 0xF72B: UInt32(HIMARK_KEY_END)
        case 0xF72C: UInt32(HIMARK_KEY_PAGE_UP)
        case 0xF72D: UInt32(HIMARK_KEY_PAGE_DOWN)
        case 0xF700...0xF8FF: nil
        case 0x0D, 0x03: UInt32(HIMARK_KEY_ENTER)
        case 0x09, 0x19: UInt32(HIMARK_KEY_TAB)
        case 0x1B: UInt32(HIMARK_KEY_ESCAPE)
        case 0x7F: UInt32(HIMARK_KEY_BACKSPACE)
        case ..<0x20: nil
        default: scalar.value
        }
    }

    // ─── text input: the IME trio rides the bridge ───

    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = plain(string)

        if replacementRange.location != NSNotFound {
            let range = HimarkRange(start: UInt32(max(0, replacementRange.location)),
                                    length: UInt32(max(0, replacementRange.length)))
            if engine.text(bridge: bridgeId, text, replacing: range) { request() }
        } else if !text.isEmpty {
            text.withCString {
                send(himark_event_text($0, UInt(strlen($0))))
            }
        }
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let selected = HimarkRange(start: UInt32(max(0, selectedRange.location)),
                                   length: UInt32(max(0, selectedRange.length)))
        let replacement = replacementRange.location == NSNotFound ? nil
            : HimarkRange(start: UInt32(max(0, replacementRange.location)),
                          length: UInt32(max(0, replacementRange.length)))
        if engine.setMarkedText(bridge: bridgeId, plain(string),
                                selected: selected, replacement: replacement) {
            request()
        }
    }

    func unmarkText() { if engine.unmarkText(bridge: bridgeId) { request() } }
    func hasMarkedText() -> Bool { engine.hasMarkedText(bridge: bridgeId) }

    func markedRange() -> NSRange { nsRange(engine.markedRange(bridge: bridgeId)) }
    func selectedRange() -> NSRange { nsRange(engine.selectedRange(bridge: bridgeId)) }

    func attributedSubstring(forProposedRange range: NSRange,
                             actualRange: NSRangePointer?) -> NSAttributedString? {
        guard let himark = himarkRange(range),
              let s = engine.substring(bridge: bridgeId, himark) else { return nil }
        actualRange?.pointee = range
        return NSAttributedString(string: s)
    }

    func firstRect(forCharacterRange range: NSRange,
                   actualRange: NSRangePointer?) -> NSRect {
        guard let himark = himarkRange(range),
              let rect = engine.firstRect(bridge: bridgeId, himark) else { return .zero }
        let s = metalLayer.contentsScale
        let top = isFlipped
            ? CGFloat(rect.y) / s
            : bounds.height - CGFloat(rect.y + rect.height) / s
        let viewRect = NSRect(x: CGFloat(rect.x) / s,
                              y: top,
                              width: CGFloat(rect.width) / s,
                              height: CGFloat(rect.height) / s)
        let windowRect = convert(viewRect, to: nil)
        return window?.convertToScreen(windowRect) ?? windowRect
    }

    func characterIndex(for point: NSPoint) -> Int {
        guard let window else { return NSNotFound }
        let viewPoint = convert(window.convertPoint(fromScreen: point), from: nil)
        let s = Float(metalLayer.contentsScale)
        let y = isFlipped ? viewPoint.y : (bounds.height - viewPoint.y)
        return engine.charIndex(bridge: bridgeId, x: Float(viewPoint.x) * s,
                                y: Float(y) * s) ?? NSNotFound
    }

    func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    override func doCommand(by selector: Selector) {
        let shift = UInt32(HIMARK_MOD_SHIFT)
        let alt = UInt32(HIMARK_MOD_ALT)
        let command = UInt32(HIMARK_MOD_COMMAND)
        let (key, mods): (Int32?, UInt32) = switch selector {
        case #selector(deleteBackward(_:)):  (HIMARK_KEY_BACKSPACE, 0)
        case #selector(deleteForward(_:)):   (HIMARK_KEY_DELETE, 0)
        case #selector(insertNewline(_:)):   (HIMARK_KEY_ENTER, 0)

        case #selector(insertLineBreak(_:)): (HIMARK_KEY_ENTER, shift)
        case #selector(moveLeft(_:)):        (HIMARK_KEY_LEFT, 0)
        case #selector(moveRight(_:)):       (HIMARK_KEY_RIGHT, 0)
        case #selector(moveUp(_:)):          (HIMARK_KEY_UP, 0)
        case #selector(moveDown(_:)):        (HIMARK_KEY_DOWN, 0)
        case #selector(moveLeftAndModifySelection(_:)):  (HIMARK_KEY_LEFT, shift)
        case #selector(moveRightAndModifySelection(_:)): (HIMARK_KEY_RIGHT, shift)
        case #selector(moveUpAndModifySelection(_:)):    (HIMARK_KEY_UP, shift)
        case #selector(moveDownAndModifySelection(_:)):  (HIMARK_KEY_DOWN, shift)
        case #selector(moveWordLeft(_:)):    (HIMARK_KEY_LEFT, alt)
        case #selector(moveWordRight(_:)):   (HIMARK_KEY_RIGHT, alt)
        case #selector(moveWordLeftAndModifySelection(_:)):  (HIMARK_KEY_LEFT, alt | shift)
        case #selector(moveWordRightAndModifySelection(_:)): (HIMARK_KEY_RIGHT, alt | shift)
        case #selector(moveToLeftEndOfLine(_:)),
             #selector(moveToBeginningOfLine(_:)),
             #selector(moveToBeginningOfParagraph(_:)): (HIMARK_KEY_HOME, 0)
        case #selector(moveToRightEndOfLine(_:)),
             #selector(moveToEndOfLine(_:)),
             #selector(moveToEndOfParagraph(_:)): (HIMARK_KEY_END, 0)
        case #selector(moveToLeftEndOfLineAndModifySelection(_:)),
             #selector(moveToBeginningOfLineAndModifySelection(_:)),
             #selector(moveToBeginningOfParagraphAndModifySelection(_:)): (HIMARK_KEY_HOME, shift)
        case #selector(moveToRightEndOfLineAndModifySelection(_:)),
             #selector(moveToEndOfLineAndModifySelection(_:)),
             #selector(moveToEndOfParagraphAndModifySelection(_:)): (HIMARK_KEY_END, shift)
        case #selector(moveToBeginningOfDocument(_:)): (HIMARK_KEY_UP, command)
        case #selector(moveToEndOfDocument(_:)):       (HIMARK_KEY_DOWN, command)
        case #selector(moveToBeginningOfDocumentAndModifySelection(_:)): (HIMARK_KEY_UP, command | shift)
        case #selector(moveToEndOfDocumentAndModifySelection(_:)):       (HIMARK_KEY_DOWN, command | shift)
        case #selector(cancelOperation(_:)): (HIMARK_KEY_ESCAPE, 0)
        case #selector(insertTab(_:)):       (HIMARK_KEY_TAB, 0)

        case #selector(insertBacktab(_:)):   (HIMARK_KEY_TAB, shift)
        case #selector(pageUp(_:)):          (HIMARK_KEY_PAGE_UP, 0)
        case #selector(pageDown(_:)):        (HIMARK_KEY_PAGE_DOWN, 0)
        default: (nil, 0)
        }
        if let key { sendKey(UInt32(key), mods: mods) }
    }

    override func selectAll(_ sender: Any?) {
        sendKey(UInt32(UnicodeScalar("a").value), mods: UInt32(HIMARK_MOD_COMMAND))
    }

    @objc func undoEdit(_ sender: Any?) {
        sendKey(UInt32(UnicodeScalar("z").value), mods: UInt32(HIMARK_MOD_COMMAND))
    }

    @objc func redoEdit(_ sender: Any?) {
        sendKey(UInt32(UnicodeScalar("z").value),
                mods: UInt32(HIMARK_MOD_COMMAND | HIMARK_MOD_SHIFT))
    }

    @objc func copy(_ sender: Any?) {
        guard let text = engine.clipboardCopy(bridge: bridgeId) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @objc func cut(_ sender: Any?) {
        guard let text = engine.clipboardCut(bridge: bridgeId) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        request()
    }

    @objc func paste(_ sender: Any?) {
        guard let text = NSPasteboard.general.string(forType: .string) else { return }
        if engine.clipboardPaste(bridge: bridgeId, text) { request() }
    }

    private func plain(_ string: Any) -> String {
        (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
    }
    private func nsRange(_ r: HimarkRange?) -> NSRange {
        guard let r else { return NSRange(location: NSNotFound, length: 0) }
        return NSRange(location: Int(r.start), length: Int(r.length))
    }
    private func himarkRange(_ r: NSRange) -> HimarkRange? {
        guard r.location != NSNotFound,
              let start = UInt32(exactly: r.location),
              let length = UInt32(exactly: r.length),
              start <= UInt32.max - length
        else { return nil }
        return HimarkRange(start: start, length: length)
    }
}
