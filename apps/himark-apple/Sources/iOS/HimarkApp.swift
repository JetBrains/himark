// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import Metal
import UIKit

/// The iOS app, rebuilt on the library architecture: SwiftUI owns
/// the wireframe, the engine owns the content, and the two meet at
/// bridges (docs/himark-core.md). v0 mounts the workbench bridge
/// whole — the degenerate case — and the native chrome grows
/// screens as the session surface lands (docs/ui/himark-ffi.md).
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
/// main, effect wake runs the worker — the handed-the-world
/// contract (docs/editor/embedding-api.md), owned here and nowhere
/// in any view.
final class AppHost: NSObject, ObservableObject, UIDocumentPickerDelegate {
    let engine = HimarkEngine()
    let device = MTLCreateSystemDefaultDevice()!

    let windowId: HimarkWindow
    let bridgeId: HimarkBridge

    private let executor = DispatchQueue(label: "dev.himark.executor", qos: .userInteractive)
    private var redraws: [ObjectIdentifier: () -> Void] = [:]
    private var hostInstalled = false

    weak var workbenchView: HimarkBridgeView?

    override init() {
        windowId = engine.addWindow()
        bridgeId = engine.windowBridge(window: windowId)
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

    private func drain() {
        guard engine.drain() else { return }
        for redraw in redraws.values { redraw() }
    }

    func register(_ view: HimarkBridgeView) {
        redraws[ObjectIdentifier(view)] = { [weak view] in view?.request() }
        if view.bridgeId.raw == bridgeId.raw { workbenchView = view }
    }

    func unregister(_ view: HimarkBridgeView) {
        redraws.removeValue(forKey: ObjectIdentifier(view))
    }

    /// The platform services table, installed once a presenter
    /// exists (file pickers need a view controller to hang off).
    func installHostIfNeeded() {
        guard !hostInstalled, let presenter = Self.rootViewController() else { return }
        hostInstalled = true
        HostBridge.install(engine: engine, presenter: presenter)
    }

    func perform(_ id: String) {
        workbenchView?.performCommand(id)
    }

    func saveDocument() {
        guard let text = workbenchView?.documentText(),
              let presenter = Self.rootViewController() else { return }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("document.md")
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
        } catch {
            return
        }
        let picker = UIDocumentPickerViewController(forExporting: [url])
        picker.delegate = self
        presenter.present(picker, animated: true)
    }

    private static func rootViewController() -> UIViewController? {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap { $0.windows }
            .first { $0.isKeyWindow }?
            .rootViewController
    }
}

/// The workbench bridge as a SwiftUI citizen.
struct WorkbenchBridge: UIViewRepresentable {
    let host: AppHost

    func makeUIView(context: Context) -> HimarkBridgeView {
        let view = HimarkBridgeView(frame: .zero, engine: host.engine,
                                    bridge: host.bridgeId, device: host.device)
        host.register(view)
        return view
    }

    func updateUIView(_ view: HimarkBridgeView, context: Context) {}
}
