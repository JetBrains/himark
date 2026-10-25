// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import Metal
import UIKit

/// The iOS app on the library architecture (docs/himark-core.md):
/// SwiftUI owns the wireframe — tabs, navigation, the glass — and
/// the engine owns the content, met at session-scoped bridges.
/// There is NO workbench and no window here: the engine runs
/// windowless, sessions come off the catalog, and each tab mints
/// its view by place.
@main
struct HimarkApp: App {
    @StateObject private var host = AppHost()

    var body: some Scene {
        WindowGroup {
            RootView(host: host)
        }
    }
}

/// The ONE engine and its loop wiring, app-wide: wake drains on
/// main, effect wake runs the worker; the drain also watches the
/// catalog epoch and publishes the session list the chrome reads.
final class AppHost: NSObject, ObservableObject {
    let engine = HimarkEngine()
    let device = MTLCreateSystemDefaultDevice()!

    struct Selected: Equatable {
        let host: UInt64
        let uri: String
        var title: String
    }

    /// One pushed screen, as himark asked for it: the place handle
    /// is the host's to mint from and release when the screen pops.
    struct Push: Identifiable, Equatable {
        let id = UUID()
        let place: HimarkPlace
        let kind: UInt32
        let title: String
        let origin: UInt64

        static func == (a: Push, b: Push) -> Bool { a.id == b.id }
    }

    @Published var sessions: [HimarkEngine.CatalogSession] = []
    @Published var current: Selected?
    /// The pending push per TAB — the navigate callback routes by
    /// the origin bridge's tab.
    @Published var pushes: [HimarkEngine.SessionViewKind: Push] = [:]

    private var epoch: UInt64 = 0
    private var redraws: [ObjectIdentifier: () -> Void] = [:]
    private var tabs: [UInt64: HimarkEngine.SessionViewKind] = [:]
    private var hostInstalled = false
    private var kicked = false

    private let executor = DispatchQueue(label: "dev.himark.executor", qos: .userInteractive)

    override init() {
        super.init()
        // The engine took its callbacks AT CREATION; the seat hooks
        // them to this host now that it stands.
        engine.seat.onWake = { [weak self] in
            DispatchQueue.main.async { self?.drain() }
        }
        engine.seat.onEffectWake = { [weak self] in
            guard let self else { return }
            self.executor.async { [weak self] in self?.engine.runPending() }
        }
        // The navigate callback: fired mid-batch on the main thread —
        // record the push and return; the screen mints later.
        engine.seat.onNavigate = { [weak self] origin, place, kind, title, _ in
            guard kind == UInt32(HIMARK_PLACE_DOCUMENT) || kind == UInt32(HIMARK_PLACE_DIFF)
            else { return false }
            guard let self else { return false }
            let push = Push(place: place, kind: kind, title: title, origin: origin.raw)
            DispatchQueue.main.async { self.land(push) }
            return true
        }
        let engine = self.engine
        executor.async { engine.runPending() }
    }

    private func land(_ push: Push) {
        let origin = tabs[push.origin] ?? .chat
        // The split chat's parts live on the CHAT tab.
        let tab: HimarkEngine.SessionViewKind =
            (origin == .transcript || origin == .composer) ? .chat : origin
        if let standing = pushes[tab] { engine.release(place: standing.place) }
        pushes[tab] = push
    }

    /// The pushed screen popped: drop the record and the handle.
    func popped(_ push: Push) {
        for (tab, standing) in pushes where standing.id == push.id {
            pushes[tab] = nil
        }
        engine.release(place: push.place)
    }

    /// Boot: install the platform services and run the first batch
    /// so the resident lanes connect and the catalog fills.
    func kickoff() {
        installHostIfNeeded()
        guard !kicked else { return }
        kicked = true
        engine.sync()
        syncCatalog()
    }

    private func drain() {
        if engine.drain() {
            for redraw in redraws.values { redraw() }
        }
        syncCatalog()
    }

    private func syncCatalog() {
        let now = engine.sessionsEpoch()
        guard now != epoch else { return }
        epoch = now
        let rows = engine.sessions().sorted { $0.modified > $1.modified }
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
        guard engine.enterSession(host: row.host, uri: row.uri) else { return }
        current = Selected(host: row.host, uri: row.uri, title: row.title)
    }

    func newSession() {
        engine.newSession()
    }

    func mint(_ kind: HimarkEngine.SessionViewKind) -> HimarkBridge {
        guard let selected = current else {
            NSLog("himark: mint \(kind) — no current session")
            return HimarkBridge(raw: 0)
        }
        let bridge = engine.mint(kind, host: selected.host, uri: selected.uri)
        if bridge.raw != 0 { tabs[bridge.raw] = kind }
        return bridge
    }

    func unmount(bridge: HimarkBridge) {
        tabs.removeValue(forKey: bridge.raw)
        engine.unmount(bridge: bridge)
    }

    func register(_ view: HimarkBridgeView) {
        redraws[ObjectIdentifier(view)] = { [weak view] in view?.request() }
        // The fan-out rides down to the overlay layers the view
        // spawns (and theirs): an unadopted layer paints once and
        // then freezes through every store change.
        view.adoptIntoRedraws = { [weak self] content in self?.register(content) }
        view.orphanFromRedraws = { [weak self] content in self?.unregister(content) }
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

    func dismissKeyboard() {
        UIApplication.shared.sendAction(
            #selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }

    private func installHostIfNeeded() {
        guard !hostInstalled, let presenter = Self.rootViewController() else { return }
        hostInstalled = true
        HostBridge.install(engine: engine, presenter: presenter)
    }

    private static func rootViewController() -> UIViewController? {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow }?
            .rootViewController
    }
}
