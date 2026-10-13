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

    private var idleTicks = 0

    private static let idleGraceTicks = 30

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

    init(
        frame: NSRect,
        engine: HimarkEngine,
        device: MTLDevice,
        bridge: HimarkBridge,
        scrollEmbedded: Bool = false
    ) {
        self.engine = engine
        self.bridgeId = bridge
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
        startDisplayLink()
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit { caLink?.invalidate() }

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
        let link = displayLink(target: self, selector: #selector(caTick))
        link.add(to: .main, forMode: .common)
        link.isPaused = true
        caLink = link
        resumeDisplayLink()
    }

    @objc private func caTick() {
        tick()
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        if newWindow == nil, let caLink {
            caLink.invalidate()
            self.caLink = nil
            linkRunning = false
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

    private func tick() {
        let animating = engine.tick(nowMs: CACurrentMediaTime() * 1000.0)
        engine.drainReveal()
        if needsRedraw || animating {
            needsRedraw = false
            render()
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
            // visibleRect is NOT clamped to bounds outside a clip
            // view — a bare embedded view (the composer) reports
            // the whole WINDOW in its coordinates.
            let visible = visibleRect.intersection(bounds)
            guard visible.width > 0.5, visible.height > 0.5,
                  visible.width.isFinite, visible.height.isFinite,
                  visible.minY.isFinite
            else { return }
            // GEOMETRY AND PIXELS MOVE TOGETHER OR NOT AT ALL. One
            // transaction holds the band's new position AND the
            // present that fills it, so CoreAnimation commits both in
            // the same frame. Driven from the scroller's
            // `boundsChanged`, that transaction is also AppKit's own
            // scroll update — which is what makes the band travel
            // with the clip view instead of chasing it.
            let wasAt = metalLayer.frame
            let w = Int32(visible.width * scale)
            let h = Int32(visible.height * scale)
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            metalLayer.frame = visible
            metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))
            let started = Self.traceSync ? CACurrentMediaTime() : 0
            let painted = renderBand(visible, scale: scale, pixelWidth: w, pixelHeight: h)
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

        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        // Same rule as the band: the drawable's new SIZE and the
        // pixels that fill it belong to one transaction, or a resize
        // commits a layer whose contents are a frame behind.
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))
        let started = Self.traceSync ? CACurrentMediaTime() : 0
        if engine.renderBridge(bridge: bridgeId, into: surface,
                               pixelWidth: w, pixelHeight: h, scale: Float(scale)) {
            request()
        }
        CATransaction.commit()
        if Self.traceSync { noteDuration(CACurrentMediaTime() - started) }
        engine.drainReveal()
    }

    /// The stage road: layout at (width, ∞), content size off the
    /// sized stage, realize + paint the visible band. The stages
    /// are ONE-PASS values — a paint that changed state means the
    /// engine quiesced them, and the next tick re-derives.
    @discardableResult
    private func renderBand(
        _ visible: NSRect, scale: CGFloat, pixelWidth w: Int32, pixelHeight h: Int32
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
        guard let canvas = surface.begin(pixelWidth: w, pixelHeight: h) else { return false }
        let changed = himark_frame_paint(
            raw, frame, canvas, 0, offsetY,
            Float(visible.width * scale), Float(visible.height * scale))
        surface.end()
        if Self.traceSync { notePresent(layerY: visible.minY) }
        if changed {
            request()
        } else {
            himark_frame_release(raw, frame)
        }
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

    /// One present, weighed against what the scroller already shows.
    private func notePresent(layerY: CGFloat) {
        presents += 1
        if let clip = enclosingScrollView?.contentView.bounds.origin.y,
           abs(clip - layerY) > 0.5 {
            driftPresents += 1
        }
        if presents % 60 == 0 {
            NSLog(
                "[sync] bridge \(bridgeId.raw): \(presents) presents, "
                    + "\(driftPresents) drifted, \(deferredRenders) deferred, "
                    + "\(rolledBack) rolled back, "
                    + String(format: "avg %.1fms worst %.1fms %d over 8ms",
                             totalMs / Double(presents), worstMs, slowRenders))
        }
    }

    /// The embedding scroller moved the viewport: repaint the band
    /// NOW, in the same runloop turn — waiting for the display link
    /// leaves one frame of the old band riding the scroll.
    func renderNow() { render() }

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
        return (Float(p.x * s), Float(y * s))
    }

    override var mouseDownCanMoveWindow: Bool { false }

    /// The down's dispatch, factored so subclasses can act on the
    /// MISS (the workbench window's titlebar drag rides it).
    @discardableResult
    func routeMouseDown(_ event: NSEvent) -> Bool {
        let (x, y) = devicePoint(event)
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
        render()
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
