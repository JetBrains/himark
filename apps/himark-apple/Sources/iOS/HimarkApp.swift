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

    @Published var sessions: [HimarkEngine.CatalogSession] = []
    @Published var current: Selected?

    private var epoch: UInt64 = 0
    private var redraws: [ObjectIdentifier: () -> Void] = [:]
    private var hostInstalled = false
    private var kicked = false

    private let executor = DispatchQueue(label: "dev.himark.executor", qos: .userInteractive)

    override init() {
        super.init()
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        engine.setWake(context: ctx) { ctx in
            let host = Unmanaged<AppHost>.fromOpaque(ctx!).takeUnretainedValue()
            DispatchQueue.main.async { host.drain() }
        }
        engine.setEffectWake(context: ctx) { ctx in
            let host = Unmanaged<AppHost>.fromOpaque(ctx!).takeUnretainedValue()
            host.executor.async { host.engine.runPending() }
        }
        let engine = self.engine
        executor.async { engine.runPending() }
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
        return engine.mint(kind, host: selected.host, uri: selected.uri)
    }

    func register(_ view: HimarkBridgeView) {
        redraws[ObjectIdentifier(view)] = { [weak view] in view?.request() }
    }

    func unregister(_ view: HimarkBridgeView) {
        redraws.removeValue(forKey: ObjectIdentifier(view))
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
