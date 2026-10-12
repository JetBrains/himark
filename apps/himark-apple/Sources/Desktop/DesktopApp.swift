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
        engine.setShell(context: ctx) { ctx, _, place, kind, title, titleLen in
            let host = Unmanaged<DesktopHost>.fromOpaque(ctx!).takeUnretainedValue()
            if kind == UInt32(HIMARK_PLACE_DISMISS) {
                DispatchQueue.main.async { host.dismissals += 1 }
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
            let opened = Opened(place: place, kind: kind, title: name)
            DispatchQueue.main.async { host.land(opened) }
            return true
        }
        let engine = self.engine
        executor.async { engine.runPending() }
    }

    private func land(_ opened: Opened) {
        if let standing = document { engine.release(place: standing.place) }
        document = opened
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
