import AppKit
import Metal
import QuartzCore

final class HimarkView: NSView, NSTextInputClient {
    private let engine: HimarkEngine

    let windowId: UInt64
    private var surface: SkiaMetalSurface!

    private var caLink: CADisplayLink?
    private var linkRunning = false
    private var needsRedraw = true

    private var idleTicks = 0

    private static let idleGraceTicks = 30

    init(frame: NSRect, engine: HimarkEngine, device: MTLDevice) {
        self.engine = engine
        self.windowId = engine.addWindow()
        super.init(frame: frame)
        wantsLayer = true
        let metal = CAMetalLayer()
        metal.device = device
        metal.pixelFormat = .bgra8Unorm
        metal.framebufferOnly = false
        metal.contentsScale = NSScreen.main?.backingScaleFactor ?? 2.0
        layer = metal

        surface = SkiaMetalSurface(layer: metal, device: device)
        startDisplayLink()
    }

    required init?(coder: NSCoder) { fatalError() }
    deinit { caLink?.invalidate() }

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

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

    private func render() {
        guard surface != nil else { return }
        let scale = window?.backingScaleFactor ?? metalLayer.contentsScale
        metalLayer.contentsScale = scale
        let w = Int32(bounds.width * scale)
        let h = Int32(bounds.height * scale)
        metalLayer.drawableSize = CGSize(width: CGFloat(w), height: CGFloat(h))

        if engine.render(window: windowId, into: surface,
                         pixelWidth: w, pixelHeight: h, scale: Float(scale)) {
            request()
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

    private func devicePoint(_ event: NSEvent) -> (Float, Float) {
        let p = convert(event.locationInWindow, from: nil)
        let s = metalLayer.contentsScale
        return (Float(p.x * s), Float((bounds.height - p.y) * s))
    }

    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        let count = UInt32(clamping: event.clickCount)
        let handled = engine.mouseDown(
            window: windowId, x: x, y: y, mods: himarkMods(event), clickCount: count
        )
        if handled { request() }
        window?.makeFirstResponder(self)

        if !handled && y <= engine.toolbarHeight() {
            window?.performDrag(with: event)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        if engine.mouseDrag(window: windowId, x: x, y: y, mods: himarkMods(event)) { request() }
    }

    override func mouseUp(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        if engine.mouseUp(window: windowId, x: x, y: y) { request() }
    }

    override func mouseMoved(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        if engine.mouseMove(window: windowId, x: x, y: y) { request() }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(
            rect: bounds,
            options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect],
            owner: self,
            userInfo: nil
        ))
    }

    private func himarkMods(_ event: NSEvent) -> UInt32 {
        var mods: UInt32 = 0
        if event.modifierFlags.contains(.shift) { mods |= UInt32(HIMARK_MOD_SHIFT) }
        if event.modifierFlags.contains(.control) { mods |= UInt32(HIMARK_MOD_CONTROL) }
        if event.modifierFlags.contains(.option) { mods |= UInt32(HIMARK_MOD_ALT) }
        if event.modifierFlags.contains(.command) { mods |= UInt32(HIMARK_MOD_COMMAND) }
        return mods
    }

    override func scrollWheel(with event: NSEvent) {
        let (x, y) = devicePoint(event)
        let s = Float(metalLayer.contentsScale)
        let perLine: Float = event.hasPreciseScrollingDeltas ? 1.0 : Self.lineScroll
        if engine.scroll(window: windowId, x: x, y: y,
                         dx: Float(-event.scrollingDeltaX) * perLine * s,
                         dy: Float(-event.scrollingDeltaY) * perLine * s) { request() }
    }

    private static let lineScroll: Float = 48.0

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        render()
    }

    override func viewWillStartLiveResize() {
        super.viewWillStartLiveResize()
        surface.setSyncPresent(true)
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        surface.setSyncPresent(false)
        render()
    }

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        if hasMarkedText() {
            interpretKeyEvents([event])
            return
        }

        if let code = himarkKey(event),
           engine.key(window: windowId, code, mods: himarkMods(event)) {
            request()
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

    @objc func openDocument(_ sender: Any?) {
        if engine.perform(window: windowId, command: "file.open") { request() }
    }

    @objc func saveDocument(_ sender: Any?) {
        if engine.perform(window: windowId, command: "file.save") { request() }
    }

    @objc func saveDocumentAs(_ sender: Any?) {
        guard let text = engine.documentText(window: windowId) else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "document.md"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? text.write(to: url, atomically: true, encoding: .utf8)
    }

    @objc func newScratch(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.new-document") { request() }
    }

    @objc func openDemoWall(_ sender: Any?) {
        engine.openDemoWall(window: windowId)
        request()
    }

    @objc func openDemo(_ sender: Any?) {
        engine.openDemo(window: windowId)
        request()
    }

    @objc func togglePeeker(_ sender: Any?) {
        if engine.perform(window: windowId, command: "peeker.toggle") { request() }
    }

    @objc func togglePalette(_ sender: Any?) {
        if engine.perform(window: windowId, command: "palette.toggle") { request() }
    }

    @objc func toggleWorkspaceTree(_ sender: Any?) {
        if engine.perform(window: windowId, command: "files.tree") { request() }
    }

    @objc func toggleToc(_ sender: Any?) {
        if engine.perform(window: windowId, command: "toc.toggle") { request() }
    }

    @objc func toggleChangesView(_ sender: Any?) {
        if engine.perform(window: windowId, command: "changes.view") { request() }
    }

    @objc func toggleWorkspaceSwitcher(_ sender: Any?) {
        if engine.perform(window: windowId, command: "session.switch") { request() }
    }

    @objc func splitPane(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.split-pane") { request() }
    }

    @objc func openTerminal(_ sender: Any?) {
        if engine.perform(window: windowId, command: "terminal.open") { request() }
    }

    @objc func openSearch(_ sender: Any?) {
        if engine.perform(window: windowId, command: "search.open") { request() }
    }

    @objc func openFind(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.open") { request() }
    }

    @objc func findNext(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.next") { request() }
    }

    @objc func findPrevious(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.previous") { request() }
    }

    @objc func closeWidget(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.close") { request() }
    }

    @objc func closePane(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.close-pane") { request() }
    }

    @objc func goBack(_ sender: Any?) {
        if engine.perform(window: windowId, command: "navigation.back") { request() }
    }

    @objc func goForward(_ sender: Any?) {
        if engine.perform(window: windowId, command: "navigation.forward") { request() }
    }

    @objc func toggleTheme(_ sender: Any?) {
        if engine.perform(window: windowId, command: "theme.toggle") { request() }
    }

    func insertText(_ string: Any, replacementRange: NSRange) {
        let text = plain(string)

        if replacementRange.location != NSNotFound {
            let range = HimarkRange(start: UInt32(max(0, replacementRange.location)),
                                    length: UInt32(max(0, replacementRange.length)))
            if engine.text(window: windowId, text, replacing: range) { request() }
        } else if engine.text(window: windowId, text) { request() }
    }

    func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let selected = HimarkRange(start: UInt32(max(0, selectedRange.location)),
                                   length: UInt32(max(0, selectedRange.length)))
        let replacement = replacementRange.location == NSNotFound ? nil
            : HimarkRange(start: UInt32(max(0, replacementRange.location)),
                          length: UInt32(max(0, replacementRange.length)))
        if engine.setMarkedText(window: windowId, plain(string),
                                selected: selected, replacement: replacement) {
            request()
        }
    }

    func unmarkText() { if engine.unmarkText(window: windowId) { request() } }
    func hasMarkedText() -> Bool { engine.hasMarkedText(window: windowId) }

    func markedRange() -> NSRange { nsRange(engine.markedRange(window: windowId)) }
    func selectedRange() -> NSRange { nsRange(engine.selectedRange(window: windowId)) }

    func attributedSubstring(forProposedRange range: NSRange,
                             actualRange: NSRangePointer?) -> NSAttributedString? {
        guard let s = engine.substring(window: windowId, himarkRange(range)) else { return nil }
        actualRange?.pointee = range
        return NSAttributedString(string: s)
    }

    func firstRect(forCharacterRange range: NSRange,
                   actualRange: NSRangePointer?) -> NSRect {
        guard let rect = engine.firstRect(window: windowId, himarkRange(range)) else { return .zero }
        let s = metalLayer.contentsScale
        let viewRect = NSRect(x: CGFloat(rect.x) / s,
                              y: bounds.height - CGFloat(rect.y + rect.height) / s,
                              width: CGFloat(rect.width) / s,
                              height: CGFloat(rect.height) / s)
        let windowRect = convert(viewRect, to: nil)
        return window?.convertToScreen(windowRect) ?? windowRect
    }

    func characterIndex(for point: NSPoint) -> Int {
        guard let window else { return NSNotFound }
        let viewPoint = convert(window.convertPoint(fromScreen: point), from: nil)
        let s = Float(metalLayer.contentsScale)
        return engine.charIndex(window: windowId, x: Float(viewPoint.x) * s,
                                y: Float(bounds.height - viewPoint.y) * s) ?? NSNotFound
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
        if let key, engine.key(window: windowId, UInt32(key), mods: mods) { request() }
    }

    override func selectAll(_ sender: Any?) {
        if engine.key(window: windowId, UInt32(UnicodeScalar("a").value),
                      mods: UInt32(HIMARK_MOD_COMMAND)) { request() }
    }

    @objc func undoEdit(_ sender: Any?) {
        if engine.key(window: windowId, UInt32(UnicodeScalar("z").value),
                      mods: UInt32(HIMARK_MOD_COMMAND)) { request() }
    }

    @objc func redoEdit(_ sender: Any?) {
        if engine.key(window: windowId, UInt32(UnicodeScalar("z").value),
                      mods: UInt32(HIMARK_MOD_COMMAND | HIMARK_MOD_SHIFT)) { request() }
    }

    @objc func copy(_ sender: Any?) {
        guard let text = engine.clipboardCopy(window: windowId) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @objc func cut(_ sender: Any?) {
        guard let text = engine.clipboardCut(window: windowId) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        request()
    }

    @objc func paste(_ sender: Any?) {
        guard let text = NSPasteboard.general.string(forType: .string) else { return }
        if engine.clipboardPaste(window: windowId, text) { request() }
    }

    private func plain(_ string: Any) -> String {
        (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
    }
    private func nsRange(_ r: HimarkRange?) -> NSRange {
        guard let r else { return NSRange(location: NSNotFound, length: 0) }
        return NSRange(location: Int(r.start), length: Int(r.length))
    }
    private func himarkRange(_ r: NSRange) -> HimarkRange {
        HimarkRange(start: UInt32(max(0, r.location)), length: UInt32(max(0, r.length)))
    }
}
