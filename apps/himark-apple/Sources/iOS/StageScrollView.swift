// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit
import SwiftUI

/// The NATIVE-SCROLL document screen: a PLAIN UIScrollView whose
/// content view is an ordinary `HimarkBridgeView` embedded at
/// content size (docs/ui/imba-ffi.md, the stage road). The bridge
/// view renders its visible band and owns all input — taps, the
/// keyboard, IME — exactly as it does full-bleed; this host only
/// sizes the content, insets for the keyboard, and reveals the
/// caret after edits.
final class StageScrollHost: UIView, UIScrollViewDelegate {
    private let host: AppHost
    private var bridge = HimarkBridge(raw: 0)
    private let scroll = UIScrollView()
    private var content: HimarkBridgeView?

    convenience init(host: AppHost, place: HimarkPlace) {
        self.init(host: host, bridge: host.engine.mint(place: place))
    }

    init(host: AppHost, bridge: HimarkBridge) {
        self.host = host
        super.init(frame: .zero)
        backgroundColor = .black
        self.bridge = bridge

        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.alwaysBounceVertical = true
        scroll.delegate = self
        addSubview(scroll)
        scroll.frame = bounds

        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: CGRect(origin: .zero, size: bounds.size),
            engine: host.engine, bridge: bridge, device: host.device,
            scrollEmbedded: true)
        view.onContentMeasured = { [weak self] size in self?.contentMeasured(size) }
        view.onReveal = { [weak self] rect, placement in self?.reveal(rect, placement) }
        view.seatRevealHandler()
        scroll.addSubview(view)
        host.register(view)
        content = view

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(keyboardChanged(_:)),
                           name: UIResponder.keyboardWillChangeFrameNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardChanged(_:)),
                           name: UIResponder.keyboardWillHideNotification, object: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    func teardown() {
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

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let content else { return }
        // Seed a provisional viewport-sized frame: the real height
        // arrives from the view's measure, which only runs once a
        // visible band exists to render.
        var frame = content.frame
        frame.size.width = bounds.width
        if frame.size.height < bounds.height { frame.size.height = bounds.height }
        if frame != content.frame {
            content.frame = frame
            scroll.contentSize = frame.size
            content.request()
        }
    }

    private func contentMeasured(_ size: CGSize) {
        guard let content, size.height.isFinite else { return }
        let height = max(size.height, bounds.height)
        if content.frame.size.height != height || scroll.contentSize.height != height {
            content.frame = CGRect(x: 0, y: 0, width: bounds.width, height: height)
            scroll.contentSize = content.frame.size
            // Tail-follow is NOT decided here: the engine arms a
            // tail reveal when a view at the tail grows
            // (docs/mounted-parity.md) — this host only applies
            // reveals, same as every other shell.
        }
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        content?.renderNow()
    }

    @objc private func keyboardChanged(_ note: Notification) {
        guard let frame = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?
            .cgRectValue
        else { return }
        let overlap = max(0, bounds.maxY - convert(frame, from: nil).minY)
        scroll.contentInset.bottom = overlap
        scroll.verticalScrollIndicatorInsets.bottom = overlap
    }

    /// The host's half of Reveal: the view asked, the scroller
    /// moves. The rect arrives in content points — pad it so the
    /// caret never kisses the very edge.
    private func reveal(_ rect: CGRect, _ placement: HimarkEngine.RevealPlacement) {
        // Corrections yield to the user's hand (the macOS host's
        // law): the engine re-emits until satisfied or superseded.
        if scroll.isTracking || scroll.isDragging || scroll.isDecelerating {
            return
        }
        switch placement {
        case .topLeft:
            let top = min(rect.minY, max(0, scroll.contentSize.height - scroll.bounds.height))
            scroll.setContentOffset(CGPoint(x: 0, y: max(0, top)), animated: false)
        case .visible:
            let target = CGRect(
                x: rect.minX, y: rect.minY - 8,
                width: max(rect.width, 2), height: rect.height + 16)
            scroll.scrollRectToVisible(target, animated: false)
        }
        // Keep the clock running through the landing — the engine
        // disarms a satisfied reveal on a TICK (the macOS host's
        // law, same here).
        content?.request()
    }
}

// ─── the split chat's two hosts ───

/// A session view minted per session and scrolled NATIVELY — the
/// transcript (the engine's tail reveal follows the stream), the
/// changes tree, the file tree.
final class MintedScrollHost: UIView {
    private let host: AppHost
    private let kind: HimarkEngine.SessionViewKind
    private var mounted: AppHost.Selected?
    private var child: StageScrollHost?

    init(host: AppHost, kind: HimarkEngine.SessionViewKind) {
        self.host = host
        self.kind = kind
        super.init(frame: .zero)
        backgroundColor = .black
        sync()
    }

    required init?(coder: NSCoder) { fatalError() }

    func sync() {
        let desired = host.current
        if mounted == desired, child != nil { return }
        teardown()
        mounted = desired
        guard desired != nil else { return }
        let bridge = host.mint(kind)
        guard bridge.raw != 0 else { return }
        let view = StageScrollHost(host: host, bridge: bridge)
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.frame = bounds
        addSubview(view)
        child = view
    }

    func teardown() {
        child?.teardown()
        child?.removeFromSuperview()
        child = nil
    }
}

struct ScrolledMintedBridge: UIViewRepresentable {
    @ObservedObject var host: AppHost
    let kind: HimarkEngine.SessionViewKind

    func makeUIView(context: Context) -> MintedScrollHost {
        MintedScrollHost(host: host, kind: kind)
    }

    func updateUIView(_ view: MintedScrollHost, context: Context) {
        view.sync()
    }

    static func dismantleUIView(_ view: MintedScrollHost, coordinator: ()) {
        view.teardown()
    }
}

/// The COMPOSER BAND for the current session, full-bleed at the
/// height the engine measures — SwiftUI sizes the frame from the
/// measure callback, so the band grows with the draft.
final class ComposerHost: UIView {
    private let host: AppHost
    private var mounted: AppHost.Selected?
    private var bridge = HimarkBridge(raw: 0)
    private var child: HimarkBridgeView?
    private var measured: CGFloat = 0

    var onMeasured: ((CGFloat) -> Void)?

    init(host: AppHost) {
        self.host = host
        super.init(frame: .zero)
        backgroundColor = .black
        host.registerRedraw(self) { [weak self] in
            self?.child?.request()
            self?.measure()
        }
        sync()
    }

    required init?(coder: NSCoder) { fatalError() }

    func sync() {
        let desired = host.current
        if mounted == desired, bridge.raw != 0 { return }
        teardown()
        mounted = desired
        guard desired != nil else { return }
        bridge = host.mint(.composer)
        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, bridge: bridge, device: host.device)
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(view)
        host.register(view)
        child = view
        measure()
    }

    func teardown() {
        if let child {
            host.unregister(child)
            child.removeFromSuperview()
        }
        child = nil
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        measure()
    }

    /// The band's natural height off the sized stage — the measure
    /// ask, released right after the read.
    private func measure() {
        guard bridge.raw != 0, bounds.width > 0 else { return }
        let raw = host.engine.raw
        let scale = UIScreen.main.scale
        let layout = himark_bridge_layout(raw, bridge, Float(bounds.width * scale), .infinity)
        guard layout.raw != 0 else { return }
        var cw: Float = 0
        var ch: Float = 0
        _ = himark_layout_size(raw, layout, &cw, &ch)
        himark_layout_release(raw, layout)
        guard ch.isFinite else { return }
        let height = CGFloat(ch) / scale
        if abs(height - measured) > 0.5 {
            measured = height
            onMeasured?(height)
        }
    }
}

struct ComposerBridge: UIViewRepresentable {
    @ObservedObject var host: AppHost
    @Binding var height: CGFloat

    func makeUIView(context: Context) -> ComposerHost {
        let view = ComposerHost(host: host)
        view.onMeasured = { measured in
            DispatchQueue.main.async { height = measured }
        }
        return view
    }

    func updateUIView(_ view: ComposerHost, context: Context) {
        view.sync()
    }

    static func dismantleUIView(_ view: ComposerHost, coordinator: ()) {
        view.teardown()
    }
}
