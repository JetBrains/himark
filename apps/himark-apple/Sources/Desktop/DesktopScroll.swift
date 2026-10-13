// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import AppKit
import SwiftUI

/// WHEEL ROUTING: SwiftUI hit-tests clicks by its own z-order, but
/// raw scrollWheel goes to the topmost APPKIT sibling — and the
/// full-bleed document floor (added whenever a pane opens) lands
/// above the overlay islands, eating their wheels. This monitor
/// restores the VISUAL order: islands first, floor by fallthrough.
final class WheelRouter {
    static let shared = WheelRouter()
    private var monitor: Any?
    private var targets: [(view: NSView, deliver: (NSEvent) -> Void)] = []

    func install() {
        guard monitor == nil else { return }
        let trace = ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil
        monitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            guard let self, let window = event.window else { return event }
            for target in self.targets.reversed() {
                guard let view = target.view as NSView?,
                      view.window === window,
                      !view.isHiddenOrHasHiddenAncestor
                else { continue }
                let point = view.convert(event.locationInWindow, from: nil)
                if view.bounds.contains(point) {
                    if trace { NSLog("wheel routed to \(type(of: view)) dy \(event.scrollingDeltaY)") }
                    target.deliver(event)
                    return nil
                }
            }
            if trace { NSLog("wheel fell through at \(event.locationInWindow)") }
            return event
        }
    }

    func register(_ view: NSView, deliver: @escaping (NSEvent) -> Void) {
        install()
        targets.append((view, deliver))
    }

    func unregister(_ view: NSView) {
        targets.removeAll { $0.view === view }
    }
}

/// A session view minted per session and scrolled NATIVELY — the
/// desktop twin of the iOS stage hosts: an NSScrollView whose
/// document view IS the embedded bridge; the engine measures, the
/// scroller owns the physics, reveals land here.
final class DesktopScrollHost: NSView {
    private let host: DesktopHost
    private let minter: () -> HimarkBridge
    private(set) var bridge = HimarkBridge(raw: 0)
    private(set) var content: HimarkBridgeView?
    let scroll = NSScrollView()
    /// A deliberate landing moves the keyboard here on mint.
    private let grabsFocus: Bool

    /// `minter` runs until it answers a live bridge: a session view
    /// can be minted BEFORE the catalog serves the session (the
    /// mint door refuses with 0, worth retrying after the epoch
    /// moves) — the drain fanout is exactly that retry clock.
    init(
        host: DesktopHost,
        grabsFocus: Bool = false,
        minter: @escaping () -> HimarkBridge
    ) {
        self.host = host
        self.minter = minter
        self.grabsFocus = grabsFocus
        super.init(frame: .zero)

        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.automaticallyAdjustsContentInsets = false
        addSubview(scroll)
        scroll.frame = bounds

        NotificationCenter.default.addObserver(
            self, selector: #selector(boundsChanged),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            self, selector: #selector(liveScrollBegan),
            name: NSScrollView.willStartLiveScrollNotification, object: scroll)
        NotificationCenter.default.addObserver(
            self, selector: #selector(liveScrollEnded),
            name: NSScrollView.didEndLiveScrollNotification, object: scroll)
        WheelRouter.shared.register(self) { [weak self] event in
            guard let self else { return }
            self.lastUserScroll = Date()
            // A standing overlay (completion popup) takes the wheel
            // before the scroller's physics do.
            if self.content?.offerWheel(event) == true { return }
            if ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil {
                let before = self.scroll.contentView.bounds.origin.y
                self.scroll.scrollWheel(with: event)
                let after = self.scroll.contentView.bounds.origin.y
                NSLog("native scroll \(before) -> \(after) doc \(Int(self.content?.frame.height ?? -1)) clip \(Int(self.scroll.contentSize.height))")
            } else {
                self.scroll.scrollWheel(with: event)
            }
        }
        // HIMARK_DEBUG_NSSCROLL: programmatic offset sanity — no
        // wheels involved at all.
        if ProcessInfo.processInfo.environment["HIMARK_DEBUG_NSSCROLL"] != nil {
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
                guard let self else { return }
                let before = self.scroll.contentView.bounds.origin.y
                self.scroll.contentView.scroll(to: NSPoint(x: 0, y: 300))
                self.scroll.reflectScrolledClipView(self.scroll.contentView)
                let after = self.scroll.contentView.bounds.origin.y
                NSLog("programmatic bridge \(self.bridge.raw): \(before) -> \(after) doc \(Int(self.content?.frame.height ?? -1)) clip \(Int(self.scroll.contentSize.height))")
            }
        }

        if !mintContent() {
            host.registerRedraw(self) { [weak self] in
                guard let self else { return }
                if self.mintContent() { self.host.unregisterRedraw(self) }
            }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    private func mintContent() -> Bool {
        guard content == nil else { return true }
        let bridge = minter()
        guard bridge.raw != 0 else { return false }
        self.bridge = bridge
        let view = HimarkBridgeView(
            frame: NSRect(origin: .zero, size: scroll.contentSize),
            engine: host.engine, device: host.device, bridge: bridge,
            scrollEmbedded: true)
        view.onContentMeasured = { [weak self] size in self?.contentMeasured(size) }
        view.seatRevealHandler { [weak self] rect, placement in self?.reveal(rect, placement) }
        scroll.documentView = view
        host.register(view)
        if grabsFocus {
            // The window may still be seating the pane this turn —
            // claim the keyboard once it is in the hierarchy.
            DispatchQueue.main.async { [weak view] in
                view?.window?.makeFirstResponder(view)
            }
        }
        content = view
        needsLayout = true
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil {
            NSLog("minted bridge \(bridge.raw): flipped \(view.isFlipped) embedded \(view.scrollEmbedded)")
        }
        // DEBUG harness: HIMARK_DEBUG_CLICK="x,y,delay" clicks this
        // bridge at content points once, for screenshot loops; a
        // leading `v` ("vx,y,delay") makes the point VIEWPORT
        // relative — what is on screen after a reveal, wherever the
        // scroll sits. HIMARK_DEBUG_TEXT="delay:text" types.
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_CLICK"] {
            for one in spec.split(separator: ";") {
                let viewportRelative = one.hasPrefix("v")
                let numbers = viewportRelative ? String(one.dropFirst()) : String(one)
                let parts = numbers.split(separator: ",").compactMap { Double($0) }
                guard parts.count == 3 else { continue }
                DispatchQueue.main.asyncAfter(deadline: .now() + parts[2]) { [weak view, weak self] in
                    guard let view else { return }
                    let s = Float(view.metalLayer.contentsScale)
                    var (x, y) = (Float(parts[0]) * s, Float(parts[1]) * s)
                    if viewportRelative, let self {
                        y += Float(self.scroll.contentView.bounds.minY) * s
                    }
                    view.send(himark_event_mouse_down(x, y, 0, 1))
                    view.send(himark_event_mouse_up(x, y))
                }
            }
        }
        // HIMARK_DEBUG_WHEEL="x,y,dy,delay;..." — synthetic wheel
        // bursts OFFERED at viewport-relative content points: the
        // nested-scroll road (a popup's list, an inlay's preview)
        // without accessibility permissions.
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_WHEEL"] {
            for one in spec.split(separator: ";") {
                let parts = one.split(separator: ",").compactMap { Double($0) }
                guard parts.count == 4 else { continue }
                DispatchQueue.main.asyncAfter(deadline: .now() + parts[3]) { [weak view, weak self] in
                    guard let view, let self else { return }
                    let y = parts[1] + Double(self.scroll.contentView.bounds.minY)
                    for step in 0..<4 {
                        let consumed = view.offer(
                            at: NSPoint(x: parts[0], y: y),
                            dx: 0,
                            dy: Float(parts[2]),
                            phase: step == 0 ? UInt32(HIMARK_SCROLL_PHASE_BEGAN)
                                : UInt32(HIMARK_SCROLL_PHASE_CHANGED))
                        NSLog("[wheel-harness] step \(step) at (\(parts[0]),\(Int(y))) dy \(parts[2]) consumed \(consumed)")
                    }
                }
            }
        }
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_TEXT"] {
            let halves = spec.split(separator: ":", maxSplits: 1)
            if halves.count == 2, let delay = Double(halves[0]) {
                let text = String(halves[1])
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak view] in
                    guard let view else { return }
                    text.withCString {
                        view.send(himark_event_text($0, UInt(strlen($0))))
                    }
                }
            }
        }
        return true
    }

    func teardown() {
        WheelRouter.shared.unregister(self)
        host.unregisterRedraw(self)
        if let content {
            host.unregister(content)
            content.removeFromSuperview()
        }
        content = nil
        if bridge.raw != 0 {
            host.engine.clearReveal(bridge: bridge)
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }

    override func layout() {
        super.layout()
        // Autoresizing from a zero-sized FOLD leaves the scroller
        // degenerate (the rail's tree sat unscrollable until a
        // remount) — the frame is restated, not inherited.
        if scroll.frame != bounds {
            scroll.frame = bounds
        }
        guard let content else { return }
        // Seed from the LAST MEASURE when one exists; a provisional
        // viewport-sized frame otherwise — the real height arrives
        // from the view's measure once a visible band renders.
        var frame = content.frame
        frame.size.width = scroll.contentSize.width
        frame.size.height = max(measured, scroll.contentSize.height)
        if frame != content.frame {
            content.frame = frame
            content.request()
        }
    }

    /// The last content measure — layout re-applies it after a
    /// fold/expand, when no fresh band has rendered yet.
    private var measured: CGFloat = 0

    private func contentMeasured(_ size: CGSize) {
        if size.height.isFinite { measured = size.height }
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil {
            NSLog("measured bridge \(bridge.raw): content \(Int(size.height)) clip \(Int(scroll.contentSize.height)) doc \(Int(content?.frame.height ?? -1))")
        }
        guard let content, size.height.isFinite else { return }
        let height = max(size.height, scroll.contentSize.height)
        if content.frame.size.height != height
            || content.frame.size.width != scroll.contentSize.width {
            content.frame = NSRect(
                x: 0, y: 0, width: scroll.contentSize.width, height: height)
            // Tail-follow is NOT decided here: the engine arms a
            // tail reveal when a view at the tail grows
            // (docs/mounted-parity.md) — this host only applies
            // reveals, same as the workbench scroller.
        }
    }

    private var dumpedJump = false
    @objc private func boundsChanged() {
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_SCROLL"] != nil {
            let y = scroll.contentView.bounds.origin.y
            let doc = content?.frame.height ?? -1
            let clip = scroll.contentSize.height
            NSLog("bounds bridge \(bridge.raw): y \(Int(y)) doc \(Int(doc)) clip \(Int(clip))")
            if !dumpedJump, doc > clip * 2, abs(y - (doc - clip)) < 2 {
                dumpedJump = true
                NSLog("JUMP STACK:\n\(Thread.callStackSymbols.joined(separator: "\n"))")
            }
        }
        content?.renderNow()
    }

    @objc private func liveScrollBegan() {
        lastUserScroll = Date()
        content?.setSyncPresent(true)
    }

    @objc private func liveScrollEnded() {
        lastUserScroll = Date()
        content?.setSyncPresent(false)
    }

    /// The last moment the USER drove this scroller (live drag or a
    /// routed wheel, momentum included).
    private var lastUserScroll = Date.distantPast

    /// The host's half of Reveal: the view asked, the scroller
    /// moves. The rect arrives in content points — pad it so the
    /// caret never kisses the very edge.
    private func reveal(_ rect: CGRect, _ placement: HimarkEngine.RevealPlacement) {
        // An anchor correction must not fight the user's hand: while
        // a gesture is fresh, drop the ask — the engine re-emits
        // until it is satisfied or superseded.
        if Date().timeIntervalSince(lastUserScroll) < 0.3 {
            return
        }
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_REVEAL"] != nil {
            NSLog("[reveal] host scrolls bridge \(bridge.raw) to \(rect) \(placement) doc \(Int(content?.frame.height ?? -1)) clip \(Int(scroll.contentSize.height)) at \(scroll.contentView.bounds.origin.y)")
        }
        switch placement {
        case .topLeft:
            // The rect's origin becomes the viewport's corner — a
            // canvas pins the revealed row's top, not "somewhere
            // visible" (minimal scrolling never satisfies it).
            let top = min(rect.minY, max(0, (content?.frame.height ?? 0) - scroll.contentSize.height))
            scroll.contentView.scroll(to: NSPoint(x: 0, y: max(0, top)))
            scroll.reflectScrolledClipView(scroll.contentView)
        case .visible:
            let target = CGRect(
                x: rect.minX, y: rect.minY - 8,
                width: max(rect.width, 2), height: rect.height + 16)
            _ = content?.scrollToVisible(target)
        }
        // Keep the clock running through the landing: the engine
        // disarms a satisfied reveal on a TICK (`Revealed` rides
        // the clock road). A scroll that never wakes the link
        // leaves the ask armed — and the next tick, resumed by a
        // stray mouse move, yanked the viewport back to the row.
        content?.request()
    }
}

/// A scrolled, session-minted bridge as a SwiftUI pane.
struct MintedScrollBridge: NSViewRepresentable {
    let host: DesktopHost
    let kind: HimarkEngine.SessionViewKind

    func makeNSView(context: Context) -> DesktopScrollHost {
        DesktopScrollHost(host: host) { [weak host] in
            host?.mint(kind) ?? HimarkBridge(raw: 0)
        }
    }

    func updateNSView(_ view: DesktopScrollHost, context: Context) {}

    static func dismantleNSView(_ view: DesktopScrollHost, coordinator: ()) {
        view.teardown()
    }
}

/// A place-minted bridge (document, diff) as a SwiftUI pane.
struct PlaceScrollBridge: NSViewRepresentable {
    let host: DesktopHost
    let place: HimarkPlace
    var grabsFocus = false

    func makeNSView(context: Context) -> DesktopScrollHost {
        DesktopScrollHost(host: host, grabsFocus: grabsFocus) { [weak host] in
            host?.engine.mint(place: place) ?? HimarkBridge(raw: 0)
        }
    }

    func updateNSView(_ view: DesktopScrollHost, context: Context) {}

    static func dismantleNSView(_ view: DesktopScrollHost, coordinator: ()) {
        view.teardown()
    }
}

/// The composer: an UNSCROLLED embedded bridge whose measured
/// height drives the SwiftUI frame — the band is the whole view.
final class DesktopComposerHost: NSView {
    private let host: DesktopHost
    private(set) var bridge = HimarkBridge(raw: 0)
    private(set) var content: HimarkBridgeView?
    var onMeasured: ((CGFloat) -> Void)?

    init(host: DesktopHost) {
        self.host = host
        super.init(frame: .zero)
        if !mintContent() {
            host.registerRedraw(self) { [weak self] in
                guard let self else { return }
                if self.mintContent() { self.host.unregisterRedraw(self) }
            }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    private func mintContent() -> Bool {
        guard content == nil else { return true }
        let bridge = host.mint(.composer)
        guard bridge.raw != 0 else { return false }
        self.bridge = bridge
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, device: host.device,
            bridge: bridge, scrollEmbedded: true)
        view.autoresizingMask = [.width, .height]
        view.onContentMeasured = { [weak self] size in
            guard size.height.isFinite, size.height > 1 else { return }
            self?.onMeasured?(size.height)
        }
        addSubview(view)
        host.register(view)
        content = view
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_CLICK3"] {
            let parts = spec.split(separator: ",").compactMap { Double($0) }
            if parts.count == 3 {
                DispatchQueue.main.asyncAfter(deadline: .now() + parts[2]) { [weak view] in
                    guard let view else { return }
                    let s = Float(view.metalLayer.contentsScale)
                    view.send(himark_event_mouse_down(Float(parts[0]) * s, Float(parts[1]) * s, 0, 1))
                    view.send(himark_event_mouse_up(Float(parts[0]) * s, Float(parts[1]) * s))
                }
            }
        }
        WheelRouter.shared.register(self) { [weak view] event in
            guard let view else { return }
            let p = view.convert(event.locationInWindow, from: nil)
            let s = Float(view.metalLayer.contentsScale)
            let dy = Float(-event.scrollingDeltaY) * (event.hasPreciseScrollingDeltas ? 1.0 : 48.0)
            view.send(himark_event_scroll(
                Float(p.x) * s, Float(p.y) * s, 0, dy * s, UInt32(HIMARK_SCROLL_PHASE_NONE)))
        }
        return true
    }

    func teardown() {
        WheelRouter.shared.unregister(self)
        host.unregisterRedraw(self)
        if let content {
            host.unregister(content)
            content.removeFromSuperview()
        }
        content = nil
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }
}

struct ComposerBridge: NSViewRepresentable {
    let host: DesktopHost
    @Binding var height: CGFloat

    func makeNSView(context: Context) -> DesktopComposerHost {
        let view = DesktopComposerHost(host: host)
        view.onMeasured = { measured in
            DispatchQueue.main.async {
                if abs(measured - height) > 0.5 { height = measured }
            }
        }
        return view
    }

    func updateNSView(_ view: DesktopComposerHost, context: Context) {}

    static func dismantleNSView(_ view: DesktopComposerHost, coordinator: ()) {
        view.teardown()
    }
}

/// A place-minted bridge WITHOUT the native scroller — the diff
/// canvas owns its scroll inside the engine (wheels ride the
/// bridge-event road); the view fills the pane and draws whole.
final class PlainBridgeHost: NSView {
    private let host: DesktopHost
    private(set) var bridge = HimarkBridge(raw: 0)
    private(set) var content: HimarkBridgeView?
    var grabsFocus = false

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if grabsFocus, let content, let window {
            DispatchQueue.main.async { window.makeFirstResponder(content) }
        }
    }

    override func layout() {
        super.layout()
        // Autoresizing from a zero seed stays zero — size the child
        // to the REAL bounds whenever they land.
        if let content, content.frame != bounds {
            content.frame = bounds
            content.request()
        }
    }

    init(host: DesktopHost, bridge: HimarkBridge) {
        self.host = host
        super.init(frame: .zero)
        self.bridge = bridge
        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, device: host.device, bridge: bridge)
        view.metalLayer.isOpaque = false
        view.autoresizingMask = [.width, .height]
        addSubview(view)
        host.register(view)
        content = view
        // Wheels over a PLAIN pane go to the ENGINE's scroller —
        // and must win against native hosts lying beneath.
        WheelRouter.shared.register(self) { [weak view] event in
            view?.scrollWheel(with: event)
        }
        // Same debug harness as the scroll host: HIMARK_DEBUG_CLICK2
        // pokes THIS pane (the canvas) once.
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_CLICK2"] {
            let parts = spec.split(separator: ",").compactMap { Double($0) }
            if parts.count == 3 {
                DispatchQueue.main.asyncAfter(deadline: .now() + parts[2]) { [weak view] in
                    guard let view else { return }
                    let s = Float(view.metalLayer.contentsScale)
                    let (x, y) = (Float(parts[0]) * s, Float(parts[1]) * s)
                    view.send(himark_event_mouse_down(x, y, 0, 1))
                    view.send(himark_event_mouse_up(x, y))
                }
            }
        }
        // HIMARK_DEBUG_SCROLL="x,y,dy,delay" — one synthetic wheel
        // gesture into this pane's ENGINE scroller.
        if let spec = ProcessInfo.processInfo.environment["HIMARK_DEBUG_SCROLL"] {
            let parts = spec.split(separator: ",").compactMap { Double($0) }
            if parts.count == 4 {
                DispatchQueue.main.asyncAfter(deadline: .now() + parts[3]) { [weak view] in
                    guard let view else { return }
                    let s = Float(view.metalLayer.contentsScale)
                    let (x, y) = (Float(parts[0]) * s, Float(parts[1]) * s)
                    let dy = Float(parts[2]) * s
                    view.send(himark_event_scroll(x, y, 0, 0, UInt32(HIMARK_SCROLL_PHASE_BEGAN)))
                    view.send(himark_event_scroll(x, y, 0, dy, UInt32(HIMARK_SCROLL_PHASE_CHANGED)))
                    view.send(himark_event_scroll(x, y, 0, 0, UInt32(HIMARK_SCROLL_PHASE_ENDED)))
                }
            }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    func teardown() {
        if let content {
            host.unregister(content)
            content.removeFromSuperview()
        }
        content = nil
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }
}

/// A session view minted UNSCROLLED — the peeker owns its own
/// list scroll and input.
struct PlainMintedBridge: NSViewRepresentable {
    let host: DesktopHost
    let kind: HimarkEngine.SessionViewKind
    var grabsFocus = false

    func makeNSView(context: Context) -> PlainBridgeHost {
        let view = PlainBridgeHost(host: host, bridge: host.mint(kind))
        if grabsFocus { view.grabsFocus = true }
        return view
    }

    func updateNSView(_ view: PlainBridgeHost, context: Context) {}

    static func dismantleNSView(_ view: PlainBridgeHost, coordinator: ()) {
        view.teardown()
    }
}

struct PlainPlaceBridge: NSViewRepresentable {
    let host: DesktopHost
    let place: HimarkPlace

    func makeNSView(context: Context) -> PlainBridgeHost {
        PlainBridgeHost(host: host, bridge: host.engine.mint(place: place))
    }

    func updateNSView(_ view: PlainBridgeHost, context: Context) {}

    static func dismantleNSView(_ view: PlainBridgeHost, coordinator: ()) {
        view.teardown()
    }
}
