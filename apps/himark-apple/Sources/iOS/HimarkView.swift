import UIKit
import Metal
import QuartzCore

final class HimarkView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    private let device = MTLCreateSystemDefaultDevice()!

    var engine: HimarkEngine!

    var windowId: UInt64 = 0

    weak var inputDelegate: UITextInputDelegate?
    var markedTextStyle: [NSAttributedString.Key: Any]?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)

    var pixelScale: CGFloat { metalLayer.contentsScale }
    private var surface: SkiaMetalSurface!
    private var link: CADisplayLink?
    private var needsRedraw = true

    private let executor = DispatchQueue(label: "dev.himark.executor", qos: .userInteractive)

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        metalLayer.device = device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = false
        metalLayer.contentsScale = UIScreen.main.scale

        engine = HimarkEngine()
        windowId = engine.addWindow()
        surface = SkiaMetalSurface(layer: metalLayer, device: device)
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        engine.setWake(context: ctx) { ctx in
            let view = Unmanaged<HimarkView>.fromOpaque(ctx!).takeUnretainedValue()
            DispatchQueue.main.async { view.drain() }
        }
        engine.setEffectWake(context: ctx) { ctx in
            let view = Unmanaged<HimarkView>.fromOpaque(ctx!).takeUnretainedValue()
            view.executor.async { view.engine.runPending() }
        }

        let engineRef = engine!
        executor.async { engineRef.runPending() }

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
    }

    @objc private func resume() { link?.isPaused = false }
    @objc private func pause() { link?.isPaused = true }

    required init?(coder: NSCoder) { fatalError() }
    deinit { link?.invalidate() }

    @objc private func tick() {
        let animating = engine.tick(nowMs: CACurrentMediaTime() * 1000.0)

        if glide != .zero {
            let now = CACurrentMediaTime()
            let dt = min(now - lastTick, 1.0 / 30.0)
            lastTick = now
            let s = Float(metalLayer.contentsScale)
            let (x, y) = devicePoint(glidePoint)
            let dy = Float(glide.y * dt) * s
            if engine.scroll(window: windowId, x: x, y: y, dx: 0, dy: -dy) { needsRedraw = true }
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

    private func drain() { if engine.drain() { needsRedraw = true } }

    override func layoutSubviews() {
        super.layoutSubviews()
        render()
    }

    private func render() {
        guard engine != nil, UIApplication.shared.applicationState == .active else { return }
        let scale = window?.screen.scale ?? UIScreen.main.scale
        metalLayer.contentsScale = scale
        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))

        if engine.render(window: windowId, into: surface,
                         pixelWidth: w, pixelHeight: h, scale: Float(scale)) {
            needsRedraw = true
        }
    }

    func request() { needsRedraw = true }

    func newScratch() {
        if engine.perform(window: windowId, command: "workbench.new-document") { request() }
    }

    func openDemo() {
        engine.openDemo(window: windowId)
        request()
    }

    func toggleAgents() {
        if engine.perform(window: windowId, command: "agent.toggle-agents") { request() }
    }

    func togglePeeker() {
        if engine.perform(window: windowId, command: "peeker.toggle") { request() }
        syncFirstResponder()
    }

    func toggleWorkspaceTree() {
        if engine.perform(window: windowId, command: "files.tree") { request() }
    }

    func toggleWorkspaceSwitcher() {
        if engine.perform(window: windowId, command: "session.switch") { request() }
    }

    func splitPane() {
        if engine.perform(window: windowId, command: "workbench.split-pane") { request() }
    }

    func openSearch() {
        if engine.perform(window: windowId, command: "search.open") { request() }
        syncFirstResponder()
    }

    func closePane() {
        if engine.perform(window: windowId, command: "workbench.close-pane") { request() }
    }

    func installHost(presenter: UIViewController) {
        HostBridge.install(engine: engine, presenter: presenter)
    }

    func openFilePicker() {
        if engine.perform(window: windowId, command: "file.open") { request() }
    }

    func documentText() -> String? { engine.documentText(window: windowId) }

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
        if engine.mouseDown(window: windowId, x: x, y: y) { request() }
        inputDelegate?.selectionDidChange(self)
        syncFirstResponder(at: point)
    }

    func isEditableText(at point: CGPoint) -> Bool {
        engine.hasTextFocus(window: windowId) && closestPosition(to: point) != nil
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

        if engine.hasTextFocus(window: windowId) {
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
        if engine.scroll(window: windowId, x: x, y: y, dx: -dx, dy: -dy) { request() }

        if gesture.state == .ended {
            glide = gesture.velocity(in: self)
            glidePoint = gesture.location(in: self)
            lastTick = CACurrentMediaTime()
        }
    }

    override var canBecomeFirstResponder: Bool { true }
    var hasText: Bool { (engine.documentLength(window: windowId) ?? 0) > 0 }

    func dismissKeyboard() { resignFirstResponder() }

    var autocorrectionType: UITextAutocorrectionType = .no
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no

    func insertText(_ text: String) {
        performTextChange {
            text == "\n"
                ? engine.key(window: windowId, UInt32(HIMARK_KEY_ENTER))
                : engine.text(window: windowId, text)
        }
    }

    func deleteBackward() {
        performTextChange { engine.key(window: windowId, UInt32(HIMARK_KEY_BACKSPACE)) }
    }

    func revealCaretAfterKeyboardChange() {
        if engine.revealSelection(window: windowId) { request() }
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(paste(_:)) { return UIPasteboard.general.hasStrings }
        if action == #selector(copy(_:)) || action == #selector(cut(_:)) { return true }
        return super.canPerformAction(action, withSender: sender)
    }

    override func paste(_ sender: Any?) {
        guard let text = UIPasteboard.general.string else { return }
        performTextChange { engine.clipboardPaste(window: windowId, text) }
    }

    override func copy(_ sender: Any?) {
        guard let text = engine.clipboardCopy(window: windowId) else { return }
        UIPasteboard.general.string = text
    }

    override func cut(_ sender: Any?) {
        performTextChange {
            guard let text = engine.clipboardCut(window: windowId) else { return false }
            UIPasteboard.general.string = text
            return true
        }
    }
}

extension HimarkView: UIGestureRecognizerDelegate {
    override func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
        guard gesture is UITapGestureRecognizer else { return true }
        return !isEditableText(at: gesture.location(in: self))
    }
}

extension HimarkView: UITextInteractionDelegate {
    func interactionShouldBegin(_ interaction: UITextInteraction, at point: CGPoint) -> Bool {
        isEditableText(at: point)
    }
}
