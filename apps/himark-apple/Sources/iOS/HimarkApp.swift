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
    // The data root rides the container's Application Support: the
    // node state dir inside it is locked until first unlock and kept
    // out of backups (a backed-up node key would clone this device's
    // tailnet identity).
    let engine = HimarkEngine(dataDir: AppHost.prepareTailnetDataDir())
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

    struct TailnetStatus: Equatable {
        var state: HimarkEngine.TailnetState = .off
        var authURL: String?
    }

    @Published var sessions: [HimarkEngine.CatalogSession] = []
    @Published var hosts: [HimarkEngine.CatalogHost] = []
    @Published var tailnet = TailnetStatus()
    @Published var current: Selected?
    /// The pending push per TAB — the navigate callback routes by
    /// the origin bridge's tab.
    @Published var pushes: [HimarkEngine.SessionViewKind: Push] = [:]
    /// The folder picker the engine asked for (the add-folder road on
    /// a remote host) — presented as a SHEET over whatever stands.
    @Published var picker: Push?
    /// The native chrome's theme — the iOS pair's half for the
    /// standing appearance, re-read when the system switches.
    @Published var shell = ShellTheme.load(dark: true)
    private var dark: Bool?

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
        // Coming back to the foreground: a suspension can wedge the
        // embedded tailnet node mid-handshake — kick it and let the
        // failed hosts redial, instead of waiting for a restart.
        NotificationCenter.default.addObserver(
            forName: UIApplication.didBecomeActiveNotification,
            object: nil, queue: .main
        ) { [weak self] _ in
            guard let self else { return }
            self.engine.tailnetResume()
            self.drain()
        }
        engine.seat.onEffectWake = { [weak self] in
            guard let self else { return }
            self.executor.async { [weak self] in self?.engine.runPending() }
        }
        // A PHONE runs the phone's theme pair (3x metrics, rows that
        // are tap targets); a pad keeps the desktop pair, whose 2x
        // metrics are its own. Declared before the first paint.
        engine.setForm(phone: UIDevice.current.userInterfaceIdiom == .phone)
        // The navigate callback: fired mid-batch on the main thread —
        // record the push and return; the screen mints later. A
        // document, a diff, or a commit's canvas each ride the native
        // stage scroller.
        engine.seat.onNavigate = { [weak self] origin, place, kind, title, _ in
            guard let self else { return false }
            // The folder picker rides its own SHEET (the add-folder
            // road on a remote host); the engine's dismiss pops it.
            if kind == UInt32(HIMARK_PLACE_FOLDER_PICKER) {
                let push = Push(place: place, kind: kind, title: title, origin: origin.raw)
                DispatchQueue.main.async { self.landPicker(push) }
                return true
            }
            if kind == UInt32(HIMARK_PLACE_DISMISS), picker != nil {
                DispatchQueue.main.async { self.dismissPicker() }
                return true
            }
            guard kind == UInt32(HIMARK_PLACE_DOCUMENT) || kind == UInt32(HIMARK_PLACE_DIFF)
                || kind == UInt32(HIMARK_PLACE_CANVAS)
            else { return false }
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

    private func landPicker(_ push: Push) {
        if let standing = picker { engine.release(place: standing.place) }
        picker = push
    }

    /// The engine asked the picker away (escape, the grant landing).
    private func dismissPicker() {
        guard let standing = picker else { return }
        picker = nil
        engine.release(place: standing.place)
    }

    /// The SHEET went (a swipe, the binding clearing): drop the
    /// record and the handle.
    func pickerDismissed(_ push: Push) {
        guard picker?.id == push.id else { return }
        picker = nil
        engine.release(place: push.place)
    }

    /// The system appearance drives BOTH halves: the engine's theme
    /// (the pair's `theme.dark`/`theme.light` commands, so the usual
    /// theme-change propagation runs) and the native words' theme.
    func follow(appearance scheme: ColorScheme) {
        let dark = scheme == .dark
        guard self.dark != dark else { return }
        self.dark = dark
        shell = ShellTheme.load(dark: dark)
        if engine.perform(bridge: HimarkBridge(raw: 0), command: dark ? "theme.dark" : "theme.light") {
            for redraw in redraws.values { redraw() }
        }
    }

    /// Boot: install the platform services and run the first batch
    /// so the resident lanes connect and the catalog fills.
    func kickoff() {
        installHostIfNeeded()
        guard !kicked else { return }
        kicked = true
        engine.sync()
        syncCatalog()
        syncTailnet()
    }

    private func drain() {
        if engine.drain() {
            for redraw in redraws.values { redraw() }
        }
        syncCatalog()
        syncTailnet()
    }

    private func syncTailnet() {
        let now = TailnetStatus(state: engine.tailnetState(), authURL: engine.tailnetAuthURL())
        guard now != tailnet else { return }
        tailnet = now
        // The tap that asked to connect is still waiting for its
        // URL — present the login the moment it exists.
        if wantsLoginPresentation, now.state == .needsLogin {
            openTailnetLogin()
        }
        if now.state == .running {
            wantsLoginPresentation = false
        }
    }

    private var wantsLoginPresentation = false

    /// A tap on a host row: the same kick as foregrounding — revive
    /// the node if wedged and retry every failed host.
    func retryHosts() {
        engine.tailnetResume()
    }

    func connectTailnet() {
        wantsLoginPresentation = true
        engine.tailnetLogin()
        syncTailnet()
    }

    /// Safari carries the Tailscale login; the scanner sees the
    /// node flip to Running when the user returns.
    func openTailnetLogin() {
        guard tailnet.state == .needsLogin,
            let raw = tailnet.authURL, let url = URL(string: raw)
        else { return }
        wantsLoginPresentation = false
        UIApplication.shared.open(url)
    }

    private static func prepareTailnetDataDir() -> String? {
        guard
            let support = FileManager.default.urls(
                for: .applicationSupportDirectory, in: .userDomainMask
            ).first
        else { return nil }
        let root = support.appendingPathComponent("himark", isDirectory: true)
        var node = root.appendingPathComponent("tailnet-client", isDirectory: true)
        try? FileManager.default.createDirectory(
            at: node,
            withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication]
        )
        var exclude = URLResourceValues()
        exclude.isExcludedFromBackup = true
        try? node.setResourceValues(exclude)
        return root.path
    }

    private func syncCatalog() {
        let now = engine.sessionsEpoch()
        guard now != epoch else { return }
        epoch = now
        let rows = engine.sessions().sorted { $0.modified > $1.modified }
        sessions = rows
        let standing = engine.hosts()
        if standing != hosts { hosts = standing }
        if wantsNewest, let newest = rows.first,
            newest.uri != current?.uri || newest.host != current?.host
        {
            // The asked create landed: the newest row is the newborn.
            select(newest)
        } else if current == nil, let newest = rows.first {
            select(newest)
        } else if let selected = current,
                  let row = rows.first(where: { $0.host == selected.host && $0.uri == selected.uri }),
                  row.title != selected.title {
            current?.title = row.title
        }
    }

    func select(_ row: HimarkEngine.CatalogSession) {
        wantsNewest = false
        guard engine.enterSession(host: row.host, uri: row.uri) else { return }
        current = Selected(host: row.host, uri: row.uri, title: row.title)
    }

    /// A create was asked: the catalog hears of the newborn through
    /// the root feed (SessionAdded), and the next catalog change that
    /// puts a DIFFERENT session on top switches to it — creating
    /// otherwise changes nothing on screen.
    private var wantsNewest = false

    func newSession() {
        // iOS runs no local backend: 0 ("the local default") creates
        // nowhere. The current session's host takes it; failing
        // that, the first connected host, then any cataloged one.
        let host = current?.host
            ?? hosts.first(where: { $0.status == 2 })?.id
            ?? sessions.first?.host
        guard let host else {
            NSLog("himark: new session — no host to create on")
            return
        }
        if engine.newSession(host: host) {
            wantsNewest = true
        } else {
            NSLog("himark: new session — the engine refused the create")
        }
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
