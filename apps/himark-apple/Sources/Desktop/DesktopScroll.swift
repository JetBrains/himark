// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import AppKit
import SwiftUI

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
    let pinsToBottom: Bool
    private var pinned = true

    /// `minter` runs until it answers a live bridge: a session view
    /// can be minted BEFORE the catalog serves the session (the
    /// mint door refuses with 0, worth retrying after the epoch
    /// moves) — the drain fanout is exactly that retry clock.
    init(host: DesktopHost, pinsToBottom: Bool = false, minter: @escaping () -> HimarkBridge) {
        self.host = host
        self.minter = minter
        self.pinsToBottom = pinsToBottom
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
        view.seatRevealHandler { [weak self] rect in self?.reveal(rect) }
        scroll.documentView = view
        host.register(view)
        content = view
        needsLayout = true
        return true
    }

    func teardown() {
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
        guard let content else { return }
        // Seed a provisional viewport-sized frame: the real height
        // arrives from the view's measure, which only runs once a
        // visible band exists to render.
        var frame = content.frame
        frame.size.width = scroll.contentSize.width
        if frame.size.height < scroll.contentSize.height {
            frame.size.height = scroll.contentSize.height
        }
        if frame != content.frame {
            content.frame = frame
            content.request()
        }
    }

    private func contentMeasured(_ size: CGSize) {
        guard let content, size.height.isFinite else { return }
        let height = max(size.height, scroll.contentSize.height)
        if content.frame.size.height != height
            || content.frame.size.width != scroll.contentSize.width {
            content.frame = NSRect(
                x: 0, y: 0, width: scroll.contentSize.width, height: height)
            if pinsToBottom && pinned {
                scrollToBottom()
            }
        }
    }

    private func scrollToBottom() {
        guard let content else { return }
        let y = max(0, content.frame.height - scroll.contentSize.height)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    @objc private func boundsChanged() {
        content?.renderNow()
    }

    /// Pinning follows the USER's hand only — a bounds change is
    /// just as often the stream growing the content, and deciding
    /// there yanks the scroll out from under a reading user.
    @objc private func liveScrollBegan() {
        pinned = false
        content?.setSyncPresent(true)
    }

    @objc private func liveScrollEnded() {
        pinned = nearBottom
        content?.setSyncPresent(false)
    }

    private var nearBottom: Bool {
        scroll.contentView.bounds.minY + scroll.contentSize.height
            >= (content?.frame.height ?? 0) - 40
    }

    /// The host's half of Reveal: the view asked, the scroller
    /// moves. The rect arrives in content points — pad it so the
    /// caret never kisses the very edge.
    private func reveal(_ rect: CGRect) {
        let target = CGRect(
            x: rect.minX, y: rect.minY - 8,
            width: max(rect.width, 2), height: rect.height + 16)
        _ = content?.scrollToVisible(target)
    }
}

/// A scrolled, session-minted bridge as a SwiftUI pane.
struct MintedScrollBridge: NSViewRepresentable {
    let host: DesktopHost
    let kind: HimarkEngine.SessionViewKind
    var pinsToBottom = false

    func makeNSView(context: Context) -> DesktopScrollHost {
        DesktopScrollHost(host: host, pinsToBottom: pinsToBottom) { [weak host] in
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

    func makeNSView(context: Context) -> DesktopScrollHost {
        DesktopScrollHost(host: host) { [weak host] in
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
        return true
    }

    func teardown() {
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

    init(host: DesktopHost, bridge: HimarkBridge) {
        self.host = host
        super.init(frame: .zero)
        self.bridge = bridge
        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, device: host.device, bridge: bridge)
        view.autoresizingMask = [.width, .height]
        addSubview(view)
        host.register(view)
        content = view
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
