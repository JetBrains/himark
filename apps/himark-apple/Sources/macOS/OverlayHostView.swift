// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import AppKit

/// The OVERLAY HOST: the one view a bridge's exported overlay
/// layers hang in — extracted from `HimarkBridgeView` so a host can
/// stand at ANY level, not just the window. The desktop's window
/// canvas is one (SwiftUI lays it last, so it composites above
/// every island and pill); a scrolled pane WRAPS its scroller in
/// one, so a Cover band pins over that pane's viewport and a
/// popover clips to the pane exactly as before. Empty space passes
/// every click through.
///
/// The engine emits overlays immediate-mode; the host retains one
/// view per overlay per OWNER, keyed by the overlay's STABLE
/// IDENTITY (its host name + which of that host's overlays it is) —
/// the exported set reorders as views mount and scroll, and keying
/// by position swapped a layer's content under the pointer. Per
/// owner tick: one epoch read — on change, diff the emitted
/// identities against the open layers. New opens, absent closes,
/// moved repositions. Each overlay's content is ANOTHER bridge (the
/// engine re-derives the overlay alone), hosted as a plain subview
/// of this host, above everything, at the content slice. An
/// overlay's own overlays become nested layers for free. No
/// NSPopover: a popover is a second window, and a second window
/// means its own appearance clock, dropped first presents, a
/// content that stays on screen after its overlay left, and wheel
/// events that never reach the pane beneath.
final class OverlayHostView: NSView {
    typealias Entry = (
        view: HimarkBridgeView, bridge: HimarkBridge, kind: HimarkOverlayKind
    )

    /// One owner's standing layers: the epoch its presence was last
    /// reconciled at, the open layers by identity, and the outside
    /// click monitor that plays the rest of the window for it.
    private final class Client {
        var epoch: UInt64 = 0
        var layers: [String: Entry] = [:]
        var outsideClickMonitor: Any?
    }

    private var clients: [ObjectIdentifier: Client] = [:]

    static let traceOverlay =
        ProcessInfo.processInfo.environment["HIMARK_TRACE_OVERLAY"] != nil

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
    }

    /// A host WRAPPING content (the scrolled pane road): the content
    /// fills the host; layers stack above it.
    convenience init(wrapping content: NSView) {
        self.init(frame: content.frame)
        content.frame = bounds
        content.autoresizingMask = [.width, .height]
        addSubview(content)
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === self ? nil : hit
    }

    /// The host standing in a window, found from its content view.
    static func find(in view: NSView) -> OverlayHostView? {
        if let host = view as? OverlayHostView { return host }
        for child in view.subviews {
            if let host = find(in: child) { return host }
        }
        return nil
    }

    // ─── presence and geometry ───

    /// One owner's per-tick reconcile — the logic that lived in
    /// `HimarkBridgeView.reconcileOverlays`, hosted here.
    func reconcile(for owner: HimarkBridgeView) {
        let (epoch, pixelAnchors) = owner.engine.overlays(bridge: owner.bridgeId)
        let key = ObjectIdentifier(owner)
        let client = clients[key] ?? Client()
        if client.layers.isEmpty && epoch == client.epoch { return }
        clients[key] = client
        let epochMoved = epoch != client.epoch
        if Self.traceOverlay, epochMoved {
            let rects = pixelAnchors.map {
                "\($0.host)(\(Int($0.rect.minX)),\(Int($0.rect.minY)) \(Int($0.rect.width))x\(Int($0.rect.height)) \($0.kind))"
            }.joined(separator: " ")
            NSLog("[overlay] bridge \(owner.bridgeId.raw): epoch \(epoch) (seen \(client.epoch)) anchors [\(rects)] layers \(client.layers.count)")
        }
        let s = owner.metalLayer.contentsScale
        // The stable identity per index: the host name and which of
        // that host's overlays this is — the engine's mounts pick by
        // the same pair, so a layer keeps ITS overlay while the set
        // reorders around it.
        var seen: [String: Int] = [:]
        let anchors = pixelAnchors.enumerated().map { index, anchor in
            let within = seen[anchor.host, default: 0]
            seen[anchor.host] = within + 1
            return (
                identity: "\(anchor.host)#\(within)",
                index: index,
                kind: anchor.kind,
                rect: CGRect(
                    x: anchor.rect.minX / s, y: anchor.rect.minY / s,
                    width: anchor.rect.width / s, height: anchor.rect.height / s)
            )
        }
        // PRESENCE moves with the epoch: absent identities close,
        // new ones open.
        if epochMoved {
            let standing = Set(anchors.map(\.identity))
            for (identity, entry) in client.layers where !standing.contains(identity) {
                if Self.traceOverlay {
                    NSLog("[overlay] close \(identity) (absent)")
                }
                close(entry, of: owner)
                client.layers[identity] = nil
            }
            client.epoch = epoch
            for anchor in anchors where client.layers[anchor.identity] == nil {
                let minted = owner.engine.mintOverlay(bridge: owner.bridgeId, index: anchor.index)
                if Self.traceOverlay {
                    NSLog("[overlay] open \(anchor.identity) anchor \(anchor.rect) \(anchor.kind) -> bridge \(minted.raw)")
                }
                guard minted.raw != 0 else { continue }
                let content = owner.mintOverlayContent(minted)
                addSubview(content, positioned: .above, relativeTo: nil)
                HimarkBridgeView.claimWheels?(content) { [weak content] event in
                    content?.scrollWheel(with: event)
                }
                client.layers[anchor.identity] = (content, minted, anchor.kind)
            }
        }
        // GEOMETRY is reconciled EVERY pass while anything is open:
        // the anchor moves with the parent's layout and scroll, the
        // size with the overlay's own content (a completion list
        // filters without touching its anchor). Measure is a
        // one-pass stage; overlays are few and short-lived.
        for anchor in anchors {
            guard let entry = client.layers[anchor.identity] else { continue }
            // Refresh the slice: lay (stashes the content rect),
            // release, read. The rect is the layer's size; its
            // origin is the content view's offset into the shared
            // coordinate space.
            let layout = himark_bridge_layout(
                owner.engine.raw, entry.bridge,
                Float(owner.bounds.width * s), Float(owner.bounds.height * s))
            if layout.raw != 0 {
                himark_layout_release(owner.engine.raw, layout)
            }
            var cx: Float = 0
            var cy: Float = 0
            var cw: Float = 0
            var ch: Float = 0
            let haveRect = himark_overlay_content_rect(
                owner.engine.raw, entry.bridge, &cx, &cy, &cw, &ch)
            if Self.traceOverlay {
                NSLog("[overlay] geometry \(anchor.identity) bridge \(entry.bridge.raw): rect \(haveRect ? "(\(cx),\(cy) \(cw)x\(ch))" : "NONE")")
            }
            guard haveRect else {
                // The overlay stands empty this frame. A fresh
                // popover has nothing to show yet — stay hidden. A
                // Cover band that already showed keeps its last
                // frame instead: the emptiness is a transient
                // re-derivation beat, and blinking through it was
                // the flicker.
                if entry.kind == .popover {
                    entry.view.isHidden = true
                }
                continue
            }
            let rect = CGRect(
                x: CGFloat(cx) / s, y: CGFloat(cy) / s,
                width: CGFloat(cw) / s, height: CGFloat(ch) / s)
            entry.view.overlayOrigin = rect.origin
            var frame: NSRect
            switch anchor.kind {
            case .sticky:
                // A COVER layer stands where its content slice lies
                // in the owner, re-anchored to the LIVE viewport:
                // the engine pinned the slice to the viewport it
                // last realized, but between engine passes the
                // host's scroller has moved on — the band leads the
                // flick like everything else. X is the slice's own
                // (the scroll stripe hugs the right edge; the file
                // band spans from the left).
                let liveY = owner.visibleRect.minY + (rect.minY - anchor.rect.minY)
                let origin = convert(NSPoint(x: rect.minX, y: liveY), from: owner)
                frame = NSRect(
                    x: origin.x, y: origin.y, width: rect.width, height: rect.height)
                // The pinned band can never usefully exceed its
                // host — and a stale whole-content band must not
                // become a 17k-pixel Metal texture.
                frame = frame.intersection(bounds)
                guard !frame.isEmpty else { continue }
            case .popover:
                // WHERE the layer stands is the host's call, as it
                // was the popover's: the engine fitted the slice
                // inside its own pane (a 100pt composer cannot hold
                // a 160pt list — the fit clamps to its top), and
                // the slice's origin only names which pixels it
                // holds. Hang the layer off the ANCHOR, in window
                // coordinates: on the side the engine preferred,
                // flipped when the window has no room there,
                // clamped inside the host.
                // In WINDOW coordinates (y up, whatever the host's
                // flip — SwiftUI's hosting view is flipped, a plain
                // content view is not), then back into the host for
                // the frame.
                let anchorInWindow = owner.convert(owner.contentRect(anchor.rect), to: nil)
                let room = convert(bounds, to: nil)
                let preferBelow = rect.midY > anchor.rect.midY
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
                frame = convert(
                    NSRect(x: x, y: y, width: rect.width, height: rect.height), from: nil)
            }
            if Self.traceOverlay {
                NSLog("[overlay] layer \(anchor.identity) at \(frame)")
            }
            if entry.view.superview !== self {
                addSubview(entry.view, positioned: .above, relativeTo: nil)
            }
            if entry.view.frame != frame {
                entry.view.frame = frame
                entry.view.request()
            }
            if entry.view.isHidden {
                entry.view.isHidden = false
                entry.view.request()
            }
            // A moved epoch means the overlay RE-DERIVED: a pinned
            // band keeps its frame while its content follows the
            // scroll — the layer owes a repaint even when its
            // geometry stood still.
            if epochMoved {
                entry.view.request()
            }
        }
        reconcileOutsideClickWatch(for: owner, client: client)
    }

    /// The owner's standing POPOVER bridges — the scrim road
    /// forwards its clicks into each of them. A Cover band carries
    /// no scrim: its layer takes its own clicks, the pane keeps the
    /// rest.
    func popoverBridges(for owner: HimarkBridgeView) -> [HimarkBridge] {
        clients[ObjectIdentifier(owner)]?.layers.values
            .filter { $0.kind == .popover }
            .map(\.bridge) ?? []
    }

    /// The owner leaves the window — every layer folds with it.
    func closeAll(for owner: HimarkBridgeView) {
        let key = ObjectIdentifier(owner)
        guard let client = clients[key] else { return }
        for (_, entry) in client.layers {
            close(entry, of: owner)
        }
        client.layers.removeAll()
        if let monitor = client.outsideClickMonitor {
            NSEvent.removeMonitor(monitor)
            client.outsideClickMonitor = nil
        }
        clients[key] = nil
    }

    /// Take a closed overlay's layer out, unmount its bridge and
    /// orphan its view from the redraw fan-out.
    private func close(_ entry: Entry, of owner: HimarkBridgeView) {
        HimarkBridgeView.releaseWheels?(entry.view)
        entry.view.removeFromSuperview()
        owner.orphanFromRedraws?(entry.view)
        owner.engine.unmount(bridge: entry.bridge)
    }

    // ─── the outside-click watch ───
    //
    // The scrim covers only the OWNER's pane; shell-app's covers
    // the whole window. While popovers stand, a local monitor plays
    // the rest of the window: any mouse-down outside the overlay
    // layers and the owner sends Escape into its focus chain — the
    // combo or completion closes itself, the epoch moves, reconcile
    // closes the layers — and the click continues to whatever it
    // hit. A click INSIDE any overlay layer (the owner's, or a
    // nested one's) is that layer's own.
    private func reconcileOutsideClickWatch(for owner: HimarkBridgeView, client: Client) {
        // Only a POPOVER is dismissable — a standing Cover band
        // must not turn every outside click into an Escape.
        if !client.layers.values.contains(where: { $0.kind == .popover }) {
            if let monitor = client.outsideClickMonitor {
                NSEvent.removeMonitor(monitor)
                client.outsideClickMonitor = nil
            }
            return
        }
        guard client.outsideClickMonitor == nil else { return }
        client.outsideClickMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown]) {
            [weak self, weak owner] event in
            guard let self, let owner,
                  !self.popoverBridges(for: owner).isEmpty else { return event }
            guard event.window === owner.window else { return event }
            if let content = event.window?.contentView,
               let frame = content.superview {
                let hit = content.hitTest(frame.convert(event.locationInWindow, from: nil))
                var walk: NSView? = hit
                while let view = walk {
                    if let bridge = view as? HimarkBridgeView,
                       bridge === owner || bridge.overlayOrigin != nil {
                        return event
                    }
                    walk = view.superview
                }
            }
            if Self.traceOverlay {
                NSLog("[overlay] outside click -> escape to bridge \(owner.bridgeId.raw)")
            }
            _ = owner.sendKey(UInt32(HIMARK_KEY_ESCAPE))
            return event
        }
    }
}
