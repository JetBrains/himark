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
final class HimarkBridgeView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    let engine: HimarkEngine
    let bridgeId: HimarkBridge

    weak var inputDelegate: UITextInputDelegate?
    var markedTextStyle: [NSAttributedString.Key: Any]?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)

    var pixelScale: CGFloat { metalLayer.contentsScale }
    private var surface: SkiaMetalSurface!
    private var link: CADisplayLink?
    private var needsRedraw = true

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    init(frame: CGRect, engine: HimarkEngine, bridge: HimarkBridge, device: MTLDevice) {
        self.engine = engine
        self.bridgeId = bridge
        super.init(frame: frame)
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = false
        metalLayer.contentsScale = UIScreen.main.scale

        surface = SkiaMetalSurface(layer: metalLayer, device: device)

        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        self.link = link

        let pan = UIPanGestureRecognizer(target: self, action: #selector(onPan))
        addGestureRecognizer(pan)

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
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit { link?.invalidate() }

    // ─── the event road: mint, dispatch, release ───

    @discardableResult
    func send(_ event: HimarkEvent) -> Bool {
        let pointed = engine.bridgeEvent(bridge: bridgeId, event: event)
        himark_event_release(event)
        if pointed.changed { request() }
        return pointed.changed
    }

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

    @objc private func tick() {
        let animating = engine.tick(nowMs: CACurrentMediaTime() * 1000.0)

        // The hand-rolled momentum: imba's scroller owns the physics
        // for now, so the flick decays here. This retires when the
        // content bridges move into real UIScrollViews (the stage
        // road — docs/ui/imba-ffi.md, touch section).
        if glide != .zero {
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
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        render()
    }

    private func render() {
        guard surface != nil, UIApplication.shared.applicationState == .active else { return }
        let scale = window?.screen.scale ?? UIScreen.main.scale
        metalLayer.contentsScale = scale
        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        guard w > 0, h > 0 else { return }
        metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))

        if engine.renderBridge(bridge: bridgeId, into: surface,
                               pixelWidth: w, pixelHeight: h, scale: Float(scale)) {
            needsRedraw = true
        }
    }

    func request() { needsRedraw = true }

    func performCommand(_ id: String) {
        if engine.perform(bridge: bridgeId, command: id) { request() }
    }

    func syncFocus() { syncFirstResponder() }

    func documentText() -> String? { engine.documentText(bridge: bridgeId) }

    private func devicePoint(_ p: CGPoint) -> (Float, Float) {
        let s = Float(metalLayer.contentsScale)
        return (Float(p.x) * s, Float(p.y) * s)
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        glide = .zero
    }

    @objc private func onTap(_ gesture: UITapGestureRecognizer) {
        let point = gesture.location(in: self)
        let (x, y) = devicePoint(point)
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
        isEditableText(at: point)
    }
}
