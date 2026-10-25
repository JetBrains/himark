// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit

/// The OVERLAY HOST, UIKit side — the macOS `OverlayHostView`'s
/// reconcile, ported: the one view a bridge's exported overlay
/// layers hang in. It stands at any level (a wrapping host wins;
/// the window hosts the rest), empty space passes touches through.
///
/// The engine emits overlays immediate-mode; the host retains one
/// view per overlay per OWNER, keyed by the overlay's STABLE
/// IDENTITY (its host name + which of that host's overlays it is).
/// Per owner tick: one epoch read — on change, diff the emitted
/// identities against the open layers. New opens, absent closes,
/// moved repositions. Each overlay's content is ANOTHER bridge,
/// hosted as a plain subview of this host at the content slice; an
/// overlay's own overlays become nested layers for free. The
/// outside-tap watch is the host itself: while a popover stands,
/// a touch outside every layer and owner is the dismissal — it
/// sends Escape into the owner's focus chain and is spent (the
/// iOS half of macOS's local event monitor).
final class OverlayHostView: UIView {
    typealias Entry = (
        view: HimarkBridgeView, bridge: HimarkBridge, kind: HimarkOverlayKind
    )

    /// One owner's standing layers: the epoch its presence was last
    /// reconciled at and the open layers by identity.
    private final class Client {
        var epoch: UInt64 = 0
        var layers: [String: Entry] = [:]
        weak var owner: HimarkBridgeView?
    }

    private var clients: [ObjectIdentifier: Client] = [:]

    static let traceOverlay =
        ProcessInfo.processInfo.environment["HIMARK_TRACE_OVERLAY"] != nil

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .clear
    }

    required init?(coder: NSCoder) { fatalError() }

    /// The host standing in a view tree, found from its root.
    static func find(in view: UIView) -> OverlayHostView? {
        if let host = view as? OverlayHostView { return host }
        for child in view.subviews {
            if let host = find(in: child) { return host }
        }
        return nil
    }

    /// Empty space passes through — except while a POPOVER stands:
    /// then a touch outside every layer and outside every owner's
    /// pane is the dismissal, claimed by the host itself. A touch
    /// on the owner's pane stays the owner's (its scrim road
    /// forwards into the popover bridges and decides there).
    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        let hit = super.hitTest(point, with: event)
        if hit !== self { return hit }
        guard popoversStand else { return nil }
        for client in clients.values {
            guard let owner = client.owner, owner.window != nil else { continue }
            if owner.bounds.contains(convert(point, to: owner)) {
                return nil
            }
        }
        return self
    }

    private var popoversStand: Bool {
        clients.values.contains { client in
            client.layers.values.contains { $0.kind == .popover }
        }
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        // Reached only through the scrim road of `hitTest`: the
        // outside tap closes what stands — Escape to every owner
        // with popovers; the combo or completion closes itself, the
        // epoch moves, reconcile folds the layers.
        for client in clients.values {
            guard let owner = client.owner,
                  client.layers.values.contains(where: { $0.kind == .popover })
            else { continue }
            if Self.traceOverlay {
                NSLog("[overlay] outside tap -> escape to bridge \(owner.bridgeId.raw)")
            }
            _ = owner.sendKey(UInt32(HIMARK_KEY_ESCAPE))
        }
    }

    // ─── presence and geometry ───

    /// One owner's per-tick reconcile — the macOS logic, with the
    /// coordinate math in UIKit's one (y-down) space.
    func reconcile(for owner: HimarkBridgeView) {
        let (epoch, pixelAnchors) = owner.engine.overlays(bridge: owner.bridgeId)
        let key = ObjectIdentifier(owner)
        let client = clients[key] ?? Client()
        if client.layers.isEmpty && epoch == client.epoch { return }
        client.owner = owner
        clients[key] = client
        let epochMoved = epoch != client.epoch
        if Self.traceOverlay, epochMoved {
            let rects = pixelAnchors.map {
                "\($0.host)(\(Int($0.rect.minX)),\(Int($0.rect.minY)) \(Int($0.rect.width))x\(Int($0.rect.height)) \($0.kind))"
            }.joined(separator: " ")
            NSLog("[overlay] bridge \(owner.bridgeId.raw): epoch \(epoch) (seen \(client.epoch)) anchors [\(rects)] layers \(client.layers.count)")
        }
        let s = owner.pixelScale
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
                addSubview(content)
                client.layers[anchor.identity] = (content, minted, anchor.kind)
                // Layers stack above whatever the window grew since.
                superview?.bringSubviewToFront(self)
            }
        }
        // GEOMETRY is reconciled EVERY pass while anything is open:
        // the anchor moves with the parent's layout and scroll, the
        // size with the overlay's own content (a completion list
        // filters without touching its anchor).
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
                // frame: the emptiness is a transient re-derivation
                // beat, and blinking through it was the flicker.
                if entry.kind == .popover {
                    entry.view.isHidden = true
                }
                continue
            }
            let rect = CGRect(
                x: CGFloat(cx) / s, y: CGFloat(cy) / s,
                width: CGFloat(cw) / s, height: CGFloat(ch) / s)
            entry.view.overlayOrigin = rect.origin
            var frame: CGRect
            switch anchor.kind {
            case .sticky:
                // A COVER layer stands where its content slice lies
                // in the owner, re-anchored to the LIVE viewport:
                // the engine pinned the slice to the viewport it
                // last realized, but between engine passes the
                // host's scroller has moved on. X is the slice's
                // own (the scroll stripe hugs the right edge; the
                // file band spans from the left).
                let liveY = owner.visibleContentRect().minY + (rect.minY - anchor.rect.minY)
                let origin = convert(CGPoint(x: rect.minX, y: liveY), from: owner)
                frame = CGRect(
                    x: origin.x, y: origin.y, width: rect.width, height: rect.height)
                // The pinned band can never usefully exceed its
                // host — and a stale whole-content band must not
                // become a 17k-pixel Metal texture.
                frame = frame.intersection(bounds)
                guard !frame.isEmpty else { continue }
            case .popover:
                // WHERE the layer stands is the host's call: the
                // engine fitted the slice inside its own pane, and
                // the slice's origin only names which pixels it
                // holds. Hang the layer off the ANCHOR, on the side
                // the engine preferred, flipped when the host has
                // no room there, clamped inside the host. UIKit is
                // y-down throughout — below is maxY.
                let anchorInHost = convert(anchor.rect, from: owner)
                let preferBelow = rect.midY > anchor.rect.midY
                let belowY = anchorInHost.maxY
                let aboveY = anchorInHost.minY - rect.height
                let fitsBelow = belowY + rect.height <= bounds.maxY
                let fitsAbove = aboveY >= bounds.minY
                let y: CGFloat = switch (preferBelow, fitsBelow, fitsAbove) {
                case (true, true, _), (false, true, false): belowY
                case (false, _, true), (true, false, true): aboveY
                default: max(bounds.minY, min(belowY, bounds.maxY - rect.height))
                }
                let x = max(bounds.minX, min(anchorInHost.minX, bounds.maxX - rect.width))
                frame = CGRect(x: x, y: y, width: rect.width, height: rect.height)
            }
            if Self.traceOverlay {
                NSLog("[overlay] layer \(anchor.identity) at \(frame)")
            }
            if entry.view.superview !== self {
                addSubview(entry.view)
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
    }

    /// The owner's standing POPOVER bridges — the scrim road
    /// forwards its taps into each of them. A Cover band carries
    /// no scrim: its layer takes its own touches, the pane keeps
    /// the rest.
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
        clients[key] = nil
    }

    /// Take a closed overlay's layer out, unmount its bridge and
    /// orphan its view from the redraw fan-out.
    private func close(_ entry: Entry, of owner: HimarkBridgeView) {
        entry.view.removeFromSuperview()
        owner.orphanFromRedraws?(entry.view)
        owner.engine.unmount(bridge: entry.bridge)
    }
}
