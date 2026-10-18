// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import Metal
import AppKit

/// The NEW macOS app on the library architecture (docs/himark-core.md),
/// the desktop twin of the iOS shell: SwiftUI owns the wireframe —
/// the toolbar pills, the panes, the inspector — and the engine owns
/// the content, met at session-scoped bridges. There is NO workbench
/// here: the engine runs windowless, sessions come off the catalog,
/// and each pane mints its view by place.
@main
struct HimarkDesktopApp: App {
    @StateObject private var host = DesktopHost()

    var body: some Scene {
        WindowGroup {
            DesktopRoot(host: host)
                .onAppear { host.kickoff() }
        }
        .defaultSize(width: 1_600, height: 1_000)
        // Without this the OS still mounts its titlebar GLASS over
        // the content top — the empty slab across squeezed windows.
        .windowStyle(.hiddenTitleBar)
    }
}

/// The ONE engine and its loop wiring, app-wide: wake drains on
/// main, effect wake runs the worker; the drain also watches the
/// catalog epoch and publishes the session list the chrome reads.
final class DesktopHost: NSObject, ObservableObject {
    let engine = HimarkEngine()
    let device = MTLCreateSystemDefaultDevice()!

    struct Selected: Equatable {
        let host: UInt64
        let uri: String
        var title: String
    }

    /// One opened document, as himark asked for it: the place handle
    /// is the host's to mint from and release when the pane closes.
    struct Opened: Identifiable, Equatable {
        let id = UUID()
        let place: HimarkPlace
        let kind: UInt32
        let title: String
        /// A deliberate jump moves the keyboard INTO the landing;
        /// a passive show (an agent opening files) must not.
        let focus: Bool

        static func == (a: Opened, b: Opened) -> Bool { a.id == b.id }
    }

    @Published var sessions: [HimarkEngine.CatalogSession] = []
    @Published var current: Selected?
    /// Bumped when a mounted modal asks to CLOSE — overlays fold.
    @Published var dismissals = 0
    /// The document pane's standing content — on the desktop every
    /// navigate lands in the CENTER pane, there are no pushes.
    @Published var document: Opened?

    private var epoch: UInt64 = 0
    private var redraws: [ObjectIdentifier: () -> Void] = [:]
    private var hostInstalled = false
    private var kicked = false

    private let executor = DispatchQueue(label: "dev.himark.desktop-executor", qos: .userInteractive)

    override init() {
        super.init()
        // Overlay layers claim their wheels through the desktop's
        // router, ahead of the scroll hosts beneath them.
        HimarkBridgeView.claimWheels = { view, deliver in
            WheelRouter.shared.register(view, deliver: deliver)
        }
        HimarkBridgeView.releaseWheels = { view in WheelRouter.shared.unregister(view) }
        HimarkBridgeView.overlayHost = { window in
            window.contentView.flatMap(OverlayCanvasView.find)
        }
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        engine.setWake(context: ctx) { ctx in
            let host = Unmanaged<DesktopHost>.fromOpaque(ctx!).takeUnretainedValue()
            DispatchQueue.main.async { host.drain() }
        }
        engine.setEffectWake(context: ctx) { ctx in
            let host = Unmanaged<DesktopHost>.fromOpaque(ctx!).takeUnretainedValue()
            host.executor.async { host.engine.runPending() }
        }
        // The navigate callback: fired mid-batch on the main thread —
        // record the opened place and return; the pane mints later.
        engine.setShell(context: ctx) { ctx, _, place, kind, title, titleLen, focus in
            let host = Unmanaged<DesktopHost>.fromOpaque(ctx!).takeUnretainedValue()
            if kind == UInt32(HIMARK_PLACE_DISMISS) {
                DispatchQueue.main.async { host.dismissals += 1 }
                return true
            }
            if kind == UInt32(HIMARK_PLACE_CHAT) {
                // "Show this session" — the sessions drawer picked a
                // row and the engine entered it. Which session the
                // place names rides the handle.
                let selected = DesktopHost.Selected(
                    host: host.engine.placeSessionHost(place),
                    uri: host.engine.placeSessionUri(place),
                    title: "")
                host.engine.release(place: place)
                DispatchQueue.main.async { host.showSession(selected) }
                return true
            }
            guard kind == UInt32(HIMARK_PLACE_DOCUMENT) || kind == UInt32(HIMARK_PLACE_DIFF)
                || kind == UInt32(HIMARK_PLACE_CANVAS)
            else { return false }
            var name = ""
            if let title, titleLen > 0 {
                name = String(
                    bytes: UnsafeRawBufferPointer(
                        start: UnsafeRawPointer(title), count: Int(titleLen)),
                    encoding: .utf8) ?? ""
            }
            let opened = Opened(place: place, kind: kind, title: name, focus: focus)
            DispatchQueue.main.async { host.land(opened) }
            return true
        }
        let engine = self.engine
        executor.async { engine.runPending() }
    }

    private func land(_ opened: Opened) {
        if let standing = document { engine.release(place: standing.place) }
        document = opened
        // Navigation goes THROUGH the host — the perfect spot to
        // command the tree: the files island follows every landing.
        engine.treeFollow(place: opened.place)
    }

    /// The document pane closed: drop the record and the handle.
    func closedDocument(_ opened: Opened) {
        if document?.id == opened.id { document = nil }
        engine.release(place: opened.place)
    }

    /// Boot: install the platform services and run the first batch
    /// so the resident lanes connect and the catalog fills.
    func kickoff() {
        if !hostInstalled {
            hostInstalled = true
            HostBridge.install(engine: engine) { [weak self] in self?.redrawAll() }
        }
        guard !kicked else { return }
        kicked = true
        engine.sync()
        syncCatalog()
    }

    private func drain() {
        if engine.drain() { redrawAll() }
        syncCatalog()
    }

    private func redrawAll() {
        for redraw in redraws.values { redraw() }
    }

    private func syncCatalog() {
        let now = engine.sessionsEpoch()
        guard now != epoch else { return }
        epoch = now
        var rows = engine.sessions().sorted { $0.modified > $1.modified }
        // Screenshot-loop hook: pin the picked session by substring.
        if let pin = ProcessInfo.processInfo.environment["HIMARK_DEBUG_SESSION"],
           let index = rows.firstIndex(where: {
               $0.title.localizedCaseInsensitiveContains(pin)
                   || $0.uri.localizedCaseInsensitiveContains(pin)
           }) {
            rows.swapAt(0, index)
        }
        sessions = rows
        if current == nil, let newest = rows.first {
            select(newest)
        } else if let selected = current,
                  let row = rows.first(where: { $0.host == selected.host && $0.uri == selected.uri }),
                  row.title != selected.title {
            current?.title = row.title
        }
    }

    func select(_ row: HimarkEngine.CatalogSession) {
        guard engine.enterSession(host: row.host, uri: row.uri) else {
            NSLog("himark: enterSession refused for \(row.uri)")
            return
        }
        current = Selected(host: row.host, uri: row.uri, title: row.title)
    }

    /// The engine named a session to show (the drawer's pick): the
    /// catalog carries its title, and every view keyed on the
    /// current session re-mints.
    func showSession(_ selected: Selected) {
        guard !selected.uri.isEmpty else { return }
        var selected = selected
        selected.title = sessions.first { $0.uri == selected.uri }?.title ?? ""
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_NAV"] != nil {
            NSLog("[nav] show session \(selected.uri) — was \(current?.uri ?? "none")")
        }
        current = selected
        // A pick completes the browsing: overlays fold, the same
        // as any mounted modal finishing.
        dismissals += 1
    }

    /// The OS appearance drives the engine's theme — theme.json or
    /// theme-light.json, the registered theme.dark/theme.light
    /// commands (built for exactly this).
    func setTheme(dark: Bool) {
        _ = engine.perform(bridge: HimarkBridge(raw: 0), command: dark ? "theme.dark" : "theme.light")
        redrawAll()
    }

    func newSession() {
        engine.newSession()
    }

    func mint(_ kind: HimarkEngine.SessionViewKind) -> HimarkBridge {
        guard let selected = current else { return HimarkBridge(raw: 0) }
        return engine.mint(kind, host: selected.host, uri: selected.uri)
    }

    func unmount(bridge: HimarkBridge) {
        engine.unmount(bridge: bridge)
    }

    func register(_ view: HimarkBridgeView) {
        redraws[ObjectIdentifier(view)] = { [weak view] in view?.request() }
    }

    func unregister(_ view: HimarkBridgeView) {
        redraws.removeValue(forKey: ObjectIdentifier(view))
    }

    /// The generic redraw seat — any content host (the stage-road
    /// scroller) rides the same drain fanout.
    func registerRedraw(_ owner: AnyObject, _ fn: @escaping () -> Void) {
        redraws[ObjectIdentifier(owner)] = fn
    }

    func unregisterRedraw(_ owner: AnyObject) {
        redraws.removeValue(forKey: ObjectIdentifier(owner))
    }
}
