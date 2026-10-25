// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit
import Metal
import QuartzCore

/// One bridge in one UIView: the GENERIC host-side wrapper
/// (docs/himark-core.md — a shell is the engine plus some number
/// of mounted views). It owns the metal layer and skia surface,
/// feeds the bridge through the normalized event handles, paints
/// it with the bridge draw, and points UIKit's text input at the
/// bridge. It creates NOTHING: the engine and the bridge are
/// handed in.
class HimarkBridgeView: UIView {
    let engine: HimarkEngine
    let bridgeId: HimarkBridge
    /// Kept for the overlay layers this view spawns — each hosts
    /// a full bridge view of its own (recursion is normal).
    private let device: MTLDevice

    /// Embedded as a SCROLL VIEW's content: the view spans the whole
    /// content, its coordinates ARE content coordinates, and only the
    /// VISIBLE band gets a drawable and a paint (the stage road). The
    /// host scroller owns all physics — pans and the glide retire.
    let scrollEmbedded: Bool

    /// The measured content size (points) — the embedding scroller
    /// sizes this view and its contentSize from it.
    var onContentMeasured: ((CGSize) -> Void)?

    /// The view asked for a rect (points, content coordinates) to
    /// be brought on screen — a caret-follow while typing, a
    /// selection jump. The embedding scroller moves.
    var onReveal: ((CGRect, HimarkEngine.RevealPlacement) -> Void)?

    weak var inputDelegate: UITextInputDelegate?
    var markedTextStyle: [NSAttributedString.Key: Any]?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)

    var pixelScale: CGFloat { metalLayer.contentsScale }
    private var surface: SkiaMetalSurface!
    private var link: CADisplayLink?
    private var needsRedraw = true

    /// A SUBLAYER sized to the visible band, not the root layer: the
    /// content-sized view must not back its whole (possibly huge)
    /// bounds with a drawable.
    private let metalLayer = CAMetalLayer()

    init(
        frame: CGRect,
        engine: HimarkEngine,
        bridge: HimarkBridge,
        device: MTLDevice,
        scrollEmbedded: Bool = false
    ) {
        self.engine = engine
        self.bridgeId = bridge
        self.device = device
        self.scrollEmbedded = scrollEmbedded
        super.init(frame: frame)
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = false
        if scrollEmbedded { metalLayer.isOpaque = false }
        metalLayer.contentsScale = UIScreen.main.scale
        layer.addSublayer(metalLayer)

        surface = SkiaMetalSurface(layer: metalLayer, device: device)

        // The engine clock's fan-out: a tick that moved ANY state
        // owes every view a frame, not just the view whose link
        // happened to fire first.
        engine.frameClock.adopt(self) { [weak self] in self?.request() }
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.add(to: .main, forMode: .common)
        self.link = link

        if !scrollEmbedded {
            let pan = UIPanGestureRecognizer(target: self, action: #selector(onPan))
            addGestureRecognizer(pan)
        }

        let tap = UITapGestureRecognizer(target: self, action: #selector(onTap))
        tap.delegate = self
        addGestureRecognizer(tap)

        let text = UITextInteraction(for: .editable)
        text.textInput = self
        text.delegate = self
        addInteraction(text)

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(resume),
                           name: UIApplication.didBecomeActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(pause),
                           name: UIApplication.willResignActiveNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardChanged),
                           name: UIResponder.keyboardDidShowNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardChanged),
                           name: UIResponder.keyboardDidChangeFrameNotification, object: nil)
    }

    @objc private func resume() { link?.isPaused = false }
    @objc private func pause() { link?.isPaused = true }

    @objc private func keyboardChanged() {
        if engine.revealSelection(bridge: bridgeId) { request() }
        drainReveal()
    }

    func seatRevealHandler() {
        engine.onReveal(bridge: bridgeId) { [weak self] pixels, placement in
            guard let self, let onReveal = self.onReveal else { return }
            let s = self.pixelScale
            onReveal(CGRect(x: pixels.minX / s, y: pixels.minY / s,
                            width: pixels.width / s, height: pixels.height / s),
                     placement)
        }
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit {
        engine.frameClock.orphan(self)
        link?.invalidate()
    }

    /// Leaving the window folds every promoted layer with the view.
    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil {
            overlaysHost?.closeAll(for: self)
        }
    }

    // ─── the event road: mint, dispatch, release ───

    @discardableResult
    func send(_ event: HimarkEvent) -> Bool {
        let pointed = engine.bridgeEvent(bridge: bridgeId, event: event)
        himark_event_release(event)
        if pointed.changed { request() }
        drainReveal()
        return pointed.changed
    }

    /// A mounted road parks its unconsumed Reveal in the engine —
    /// the view owns no scroller. Drain after every dispatch; the
    /// engine routes to the OWNING view's handler, which lands in
    /// `onReveal` below in points.
    private func drainReveal() { engine.drainReveal() }

    @discardableResult
    func sendKey(_ code: UInt32, mods: UInt32 = 0) -> Bool {
        send(himark_event_key_down(code, mods))
    }

    @discardableResult
    func sendText(_ text: String) -> Bool {
        guard !text.isEmpty else { return false }
        return text.withCString { send(himark_event_text($0, UInt(strlen($0)))) }
    }

    // ─── the frame loop ───

    @objc private func tick(_ link: CADisplayLink) {
        // ONE engine tick per display frame, whoever's link fires
        // first — the clock caches the verdict for same-frame
        // followers and requests every other view when it moved.
        // The driver's own estimate of when THIS frame reaches
        // glass: animations advance to where they will be SEEN.
        let animating = engine.frameClock.tick(
            nowMs: link.targetTimestamp * 1000.0, from: self)
        drainReveal()
        if warmupTicks > 0 {
            warmupTicks -= 1
            needsRedraw = true
        }

        // The hand-rolled momentum: imba's scroller owns the physics
        // for now, so the flick decays here. This retires when the
        // content bridges move into real UIScrollViews (the stage
        // road — docs/ui/imba-ffi.md, touch section).
        if glide != .zero, !scrollEmbedded {
            let now = CACurrentMediaTime()
            let dt = min(now - lastTick, 1.0 / 30.0)
            lastTick = now
            let s = Float(metalLayer.contentsScale)
            let (x, y) = devicePoint(glidePoint)
            let dy = Float(glide.y * dt) * s
            send(himark_event_scroll(x, y, 0, -dy, UInt32(HIMARK_SCROLL_PHASE_NONE)))
            let decay = CGFloat(pow(0.998, dt * 1000.0))
            glide.x *= decay
            glide.y *= decay
            if abs(glide.y) < 12, abs(glide.x) < 12 { glide = .zero }
        }
        if needsRedraw || animating {
            needsRedraw = false
            render()
        }
        // AFTER the render: the exported anchors now speak THIS
        // tick's viewport, so the layers reconcile against the
        // truth, not last frame's.
        reconcileOverlays()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        render()
    }

    /// The visible band: the part of this view its superview shows —
    /// the whole bounds when full-bleed, the scrolled window when
    /// this view IS a scroller's content. The overlay host reads it
    /// too, to re-anchor a sticky band to the live viewport.
    func visibleContentRect() -> CGRect {
        guard let sv = superview else { return bounds }
        return convert(sv.bounds, from: sv).intersection(bounds)
    }

    private func render() {
        guard surface != nil, UIApplication.shared.applicationState == .active else { return }
        // SwiftUI's platform host can hand a NaN/infinite frame
        // mid-layout — converting that to a drawable size traps.
        guard bounds.width.isFinite, bounds.height.isFinite else { return }
        let scale = window?.screen.scale ?? UIScreen.main.scale
        metalLayer.contentsScale = scale

        if scrollEmbedded {
            // POSITION FIRST, PIXELS SECOND — a parked reveal moves
            // the scroller NOW, so the band realized below is the
            // one that will rest on screen (the macOS band law).
            engine.drainReveal()
        }
        let visible = visibleContentRect()
        guard visible.width > 0.5, visible.height > 0.5,
              visible.minY.isFinite
        else { return }
        let w = Int32(visible.width * scale)
        let h = Int32(visible.height * scale)
        // Metal's texture ceiling is 16384 — a layer this size is a
        // geometry bug upstream, and asking for the drawable ABORTS
        // the app. Refuse loudly instead.
        guard w > 0, h > 0, w <= 16384, h <= 16384 else {
            NSLog("[render] bridge \(bridgeId.raw) REFUSED: \(w)x\(h) exceeds the texture ceiling")
            return
        }

        // GEOMETRY AND PIXELS MOVE TOGETHER OR NOT AT ALL: one
        // transaction holds the layer's new position AND the present
        // that fills it, so CoreAnimation commits both in the same
        // frame (the flicker law).
        let wasAt = metalLayer.frame
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        metalLayer.frame = visible
        metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))

        if scrollEmbedded {
            if !renderBand(visible, scale: scale, pixelWidth: w, pixelHeight: h) {
                // No drawable, nothing painted: a moved band showing
                // its old contents is the very artifact this exists
                // to prevent. Put it back and ask for another pass.
                metalLayer.frame = wasAt
                needsRedraw = true
            }
        } else if let origin = overlayOrigin {
            renderOverlaySlice(origin, scale: scale, pixelWidth: w, pixelHeight: h)
        } else {
            switch engine.renderBridge(bridge: bridgeId, into: surface,
                                       pixelWidth: w, pixelHeight: h, scale: Float(scale)) {
            case .reconciling, .starved:
                // Starved: no drawable, nothing painted — retry owed,
                // or the view shows stale pixels until the next event.
                needsRedraw = true
            case .painted:
                break
            }
        }
        CATransaction.commit()
        if scrollEmbedded {
            // A reveal can be BORN at the paint (a canvas revealing
            // its row right after populate) — drain it this pass,
            // or the scroll waits for the next event.
            engine.drainReveal()
        }
    }

    /// The OVERLAY road: the overlay frame shares the parent's
    /// coordinates; this paints exactly the content slice at
    /// `origin`, sized to this view.
    private func renderOverlaySlice(
        _ origin: CGPoint, scale: CGFloat, pixelWidth w: Int32, pixelHeight h: Int32
    ) {
        let raw = engine.raw
        let layout = himark_bridge_layout(
            raw, bridgeId, Float(bounds.width * scale), Float(bounds.height * scale))
        guard layout.raw != 0 else { return }
        let ox = Float(origin.x * scale)
        let oy = Float(origin.y * scale)
        let frame = himark_layout_realize(
            raw, layout, ox, oy, Float(bounds.width * scale), Float(bounds.height * scale))
        guard frame.raw != 0 else { return }
        if let canvas = surface.begin(pixelWidth: w, pixelHeight: h) {
            let changed = himark_frame_paint(
                raw, frame, canvas, ox, oy,
                Float(bounds.width * scale), Float(bounds.height * scale))
            surface.end()
            if OverlayHostView.traceOverlay {
                NSLog("[overlay] slice bridge \(bridgeId.raw): \(w)x\(h) at (\(ox),\(oy)) changed \(changed)")
            }
            if changed { needsRedraw = true }
            himark_frame_release(raw, frame)
        } else {
            // Starved — nothing painted, retry owed.
            himark_frame_release(raw, frame)
            needsRedraw = true
        }
    }

    /// The stage road: layout at (width, ∞), content size off the
    /// sized stage, realize + paint the visible band. The stages
    /// are ONE-PASS values — a paint that changed state means the
    /// engine quiesced them, and the next tick re-derives.
    @discardableResult
    private func renderBand(
        _ visible: CGRect, scale: CGFloat, pixelWidth w: Int32, pixelHeight h: Int32
    ) -> Bool {
        let raw = engine.raw
        let layout = himark_bridge_layout(raw, bridgeId, Float(bounds.width * scale), .infinity)
        guard layout.raw != 0 else { return false }
        var cw: Float = 0
        var ch: Float = 0
        _ = himark_layout_size(raw, layout, &cw, &ch)

        let offsetY = Float(max(visible.minY, 0) * scale)
        let frame = himark_layout_realize(
            raw, layout, 0, offsetY, Float(visible.width * scale), Float(visible.height * scale))
        guard frame.raw != 0 else { return false }
        guard let canvas = surface.begin(pixelWidth: w, pixelHeight: h) else {
            himark_frame_release(raw, frame)
            return false
        }
        let changed = himark_frame_paint(
            raw, frame, canvas, 0, offsetY,
            Float(visible.width * scale), Float(visible.height * scale))
        surface.end()
        if changed {
            needsRedraw = true
        }
        // Released on EVERY road: the changed road kept the handle —
        // and the frame's whole arena with it — until unmount.
        himark_frame_release(raw, frame)
        // The measure lands AFTER the pass: applying it resizes the
        // scroller's content, which re-enters render synchronously —
        // an outstanding layout would die refused and the band would
        // paint twice per streamed change.
        if ch.isFinite {
            onContentMeasured?(CGSize(width: bounds.width, height: CGFloat(ch) / scale))
        }
        return true
    }

    func request() { needsRedraw = true }

    // ─── overlay promotion: in-window layers, reconciled per frame ───
    //
    // The RECONCILE logic lives in `OverlayHostView` — the one view
    // the layers hang in, standing at any level: the nearest
    // wrapping host up the superview chain, else one installed over
    // the window once. This view keeps only its side of the
    // contract: the per-tick delegation, the content minting, and
    // the state an overlay CONTENT view carries.

    /// The host's redraw fan-out, handed down at minting so overlay
    /// content views (and THEIR overlays) hear store dirt too — an
    /// unadopted view paints once and then freezes.
    var adoptIntoRedraws: ((HimarkBridgeView) -> Void)?
    var orphanFromRedraws: ((HimarkBridgeView) -> Void)?

    /// Set on an OVERLAY's content view: the content slice's origin
    /// in the PARENT'S coordinates (points). The overlay frame
    /// shares the parent's coordinate space — events offset by
    /// this on the way in, the paint realizes this viewport.
    var overlayOrigin: CGPoint?

    /// A fresh layer's first presents can land before its layer is
    /// composited; a short warmup repaints across the appearance.
    var warmupTicks = 0

    /// The standing host, resolved on first use: the nearest
    /// WRAPPING host up the superview chain wins, else a host
    /// installed over the window once — layers float over the whole
    /// app there, above the SwiftUI chrome.
    private weak var overlaysHost: OverlayHostView?

    private func resolveOverlaysHost() -> OverlayHostView? {
        if let host = overlaysHost, host.window === window { return host }
        var resolved: OverlayHostView?
        var walk = superview
        while let view = walk {
            if let host = view as? OverlayHostView {
                resolved = host
                break
            }
            walk = view.superview
        }
        if resolved == nil, let window {
            resolved = OverlayHostView.find(in: window) ?? {
                let host = OverlayHostView(frame: window.bounds)
                host.autoresizingMask = [.flexibleWidth, .flexibleHeight]
                window.addSubview(host)
                return host
            }()
        }
        overlaysHost = resolved
        return resolved
    }

    private func reconcileOverlays() {
        resolveOverlaysHost()?.reconcile(for: self)
    }

    /// Mint one overlay's CONTENT view for the host — the device
    /// and the redraw fan-out are this view's to hand down.
    func mintOverlayContent(_ bridge: HimarkBridge) -> HimarkBridgeView {
        let content = HimarkBridgeView(
            frame: CGRect(origin: .zero, size: CGSize(width: 40, height: 20)),
            engine: engine, bridge: bridge, device: device)
        // Whatever the content leaves unpainted shows what lies
        // beneath — an opaque layer would show it black.
        content.metalLayer.isOpaque = false
        content.adoptIntoRedraws = adoptIntoRedraws
        content.orphanFromRedraws = orphanFromRedraws
        adoptIntoRedraws?(content)
        // Hidden until its first geometry lands: a seed-sized
        // layer at the window's origin is a flash.
        content.isHidden = true
        content.warmupTicks = 3
        return content
    }

    /// The embedding scroller moved the viewport: repaint the band
    /// NOW, in the same runloop turn — waiting for the display link
    /// leaves one frame of the old band riding the scroll.
    func renderNow() { render() }

    func performCommand(_ id: String) {
        if engine.perform(bridge: bridgeId, command: id) { request() }
        drainReveal()
    }

    func syncFocus() { syncFirstResponder() }

    func documentText() -> String? { engine.documentText(bridge: bridgeId) }

    private func devicePoint(_ p: CGPoint) -> (Float, Float) {
        let s = Float(metalLayer.contentsScale)
        let o = overlayOrigin ?? .zero
        return (Float(p.x + o.x) * s, Float(p.y + o.y) * s)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        glide = .zero
    }

    @objc private func onTap(_ gesture: UITapGestureRecognizer) {
        let point = gesture.location(in: self)
        let (x, y) = devicePoint(point)
        // While overlays stand, the ENGINE'S SCRIM owns this pane's
        // taps — the overlay frame shares this pane's coordinates,
        // so the point travels untranslated; the scrim turns it into
        // the dismissal the widget wired, the epoch moves, reconcile
        // closes the layer (the macOS routeMouseDown road).
        let scrims = overlaysHost?.popoverBridges(for: self) ?? []
        if !scrims.isEmpty {
            var consumed = false
            for bridge in scrims {
                let down = himark_event_mouse_down(x, y, 0, 1)
                consumed = engine.bridgeEvent(bridge: bridge, event: down).changed || consumed
                himark_event_release(down)
                let up = himark_event_mouse_up(x, y)
                _ = engine.bridgeEvent(bridge: bridge, event: up)
                himark_event_release(up)
            }
            drainReveal()
            request()
            // A SCRIM took it: the tap was the dismissal, spent.
            // Nothing took it (a completion popup carries no scrim):
            // the tap is the PANE'S, fall through.
            if consumed { return }
        }
        inputDelegate?.selectionWillChange(self)
        send(himark_event_mouse_down(x, y, 0, 1))
        // A tap is a CLICK: rows, buttons and tree chevrons complete
        // on the up, exactly as the mac shell sends it.
        send(himark_event_mouse_up(x, y))
        inputDelegate?.selectionDidChange(self)
        syncFirstResponder(at: point)
    }

    func isEditableText(at point: CGPoint) -> Bool {
        engine.hasTextFocus(bridge: bridgeId) && closestPosition(to: point) != nil
    }

    private func syncFirstResponder(at point: CGPoint? = nil) {
        if let point {
            if isEditableText(at: point) {
                becomeFirstResponder()
            } else {
                resignFirstResponder()
            }
            return
        }

        if engine.hasTextFocus(bridge: bridgeId) {
            becomeFirstResponder()
        }
    }

    func performTextChange(_ body: () -> Bool) {
        inputDelegate?.textWillChange(self)
        inputDelegate?.selectionWillChange(self)
        if body() { request() }
        drainReveal()
        inputDelegate?.selectionDidChange(self)
        inputDelegate?.textDidChange(self)
    }

    private var glide: CGPoint = .zero
    private var glidePoint: CGPoint = .zero
    private var lastTick: CFTimeInterval = 0

    private var lastPan: CGPoint = .zero
    @objc private func onPan(_ gesture: UIPanGestureRecognizer) {
        let t = gesture.translation(in: self)
        let (x, y) = devicePoint(gesture.location(in: self))
        let s = Float(metalLayer.contentsScale)
        let (dx, dy) = (Float(t.x - lastPan.x) * s, Float(t.y - lastPan.y) * s)
        lastPan = gesture.state == .ended ? .zero : t
        if gesture.state == .began { lastPan = .zero; glide = .zero }
        send(himark_event_scroll(x, y, -dx, -dy, UInt32(HIMARK_SCROLL_PHASE_NONE)))

        if gesture.state == .ended {
            glide = gesture.velocity(in: self)
            glidePoint = gesture.location(in: self)
            lastTick = CACurrentMediaTime()
        }
    }

    // ─── hardware-keyboard SPECIAL keys ───
    //
    // Text rides UIKeyInput (insertText/deleteBackward); arrows,
    // home/end, paging, escape and forward-delete never reach that
    // road — they arrive as presses and translate here. Character,
    // return and backspace presses fall through to the input
    // system, or they would land twice.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var unhandled = Set<UIPress>()
        for press in presses {
            guard let key = press.key, let code = Self.specialKey(key.keyCode) else {
                unhandled.insert(press)
                continue
            }
            var mods: UInt32 = 0
            if key.modifierFlags.contains(.shift) { mods |= UInt32(HIMARK_MOD_SHIFT) }
            if key.modifierFlags.contains(.control) { mods |= UInt32(HIMARK_MOD_CONTROL) }
            if key.modifierFlags.contains(.alternate) { mods |= UInt32(HIMARK_MOD_ALT) }
            if key.modifierFlags.contains(.command) { mods |= UInt32(HIMARK_MOD_COMMAND) }
            inputDelegate?.selectionWillChange(self)
            sendKey(code, mods: mods)
            inputDelegate?.selectionDidChange(self)
        }
        if !unhandled.isEmpty {
            super.pressesBegan(unhandled, with: event)
        }
    }

    private static func specialKey(_ code: UIKeyboardHIDUsage) -> UInt32? {
        switch code {
        case .keyboardLeftArrow: UInt32(HIMARK_KEY_LEFT)
        case .keyboardRightArrow: UInt32(HIMARK_KEY_RIGHT)
        case .keyboardUpArrow: UInt32(HIMARK_KEY_UP)
        case .keyboardDownArrow: UInt32(HIMARK_KEY_DOWN)
        case .keyboardHome: UInt32(HIMARK_KEY_HOME)
        case .keyboardEnd: UInt32(HIMARK_KEY_END)
        case .keyboardPageUp: UInt32(HIMARK_KEY_PAGE_UP)
        case .keyboardPageDown: UInt32(HIMARK_KEY_PAGE_DOWN)
        case .keyboardEscape: UInt32(HIMARK_KEY_ESCAPE)
        case .keyboardDeleteForward: UInt32(HIMARK_KEY_DELETE)
        default: nil
        }
    }

    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { (engine.documentLength(bridge: bridgeId) ?? 0) > 0 }

    func dismissKeyboard() { resignFirstResponder() }

    var autocorrectionType: UITextAutocorrectionType = .no
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no

    func insertText(_ text: String) {
        performTextChange {
            text == "\n"
                ? sendKey(UInt32(HIMARK_KEY_ENTER))
                : sendText(text)
        }
    }

    func deleteBackward() {
        performTextChange { sendKey(UInt32(HIMARK_KEY_BACKSPACE)) }
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(paste(_:)) { return UIPasteboard.general.hasStrings }
        if action == #selector(copy(_:)) || action == #selector(cut(_:)) { return true }
        return super.canPerformAction(action, withSender: sender)
    }

    override func paste(_ sender: Any?) {
        guard let text = UIPasteboard.general.string else { return }
        performTextChange { engine.clipboardPaste(bridge: bridgeId, text) }
    }

    override func copy(_ sender: Any?) {
        guard let text = engine.clipboardCopy(bridge: bridgeId) else { return }
        UIPasteboard.general.string = text
    }

    override func cut(_ sender: Any?) {
        performTextChange {
            guard let text = engine.clipboardCut(bridge: bridgeId) else { return false }
            UIPasteboard.general.string = text
            return true
        }
    }
}

extension HimarkBridgeView: UIGestureRecognizerDelegate {
    override func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard gesture is UITapGestureRecognizer else { return true }
        return !isEditableText(at: gesture.location(in: self))
    }
}

extension HimarkBridgeView: UITextInteractionDelegate {
    func interactionShouldBegin(_ interaction: UITextInteraction, at point: CGPoint) -> Bool {
        let editable = isEditableText(at: point)
        // The interaction claims the tap INSTEAD of our recognizer,
        // and on a custom view it does not reliably seat the
        // responder itself — the keyboard follows the tap here.
        if editable { becomeFirstResponder() }
        return editable
    }
}
