// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import AppKit
import Metal

/// The workbench WINDOW as a view: the degenerate case of the
/// bridge architecture — one `HimarkBridgeView` whose mounted view
/// is the whole workbench composition, plus the window-only
/// dressing: titlebar drag and double-click on the chrome miss,
/// and the menu bar's command actions.
final class HimarkView: HimarkBridgeView {
    var windowId: UInt64 { bridgeId }

    init(frame: NSRect, engine: HimarkEngine, device: MTLDevice) {
        let window = engine.addWindow()
        super.init(frame: frame, engine: engine, device: device,
                   bridge: engine.windowBridge(window: window))
    }

    required init?(coder: NSCoder) { fatalError() }

    override func mouseDown(with event: NSEvent) {
        let handled = routeMouseDown(event)

        if !handled {
            let (_, y) = devicePoint(event)
            if y <= engine.toolbarHeight() {
                if event.clickCount == 2 {
                    titlebarDoubleClick()
                } else {
                    window?.performDrag(with: event)
                }
            }
        }
    }

    private func titlebarDoubleClick() {
        switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
        case "Minimize": window?.performMiniaturize(nil)
        case "None": break
        default: window?.performZoom(nil)
        }
    }

    @objc func openDocument(_ sender: Any?) {
        if engine.perform(window: windowId, command: "file.open") { request() }
    }

    @objc func saveDocument(_ sender: Any?) {
        if engine.perform(window: windowId, command: "file.save") { request() }
    }

    @objc func saveDocumentAs(_ sender: Any?) {
        guard let text = engine.documentText(window: windowId) else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "document.md"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? text.write(to: url, atomically: true, encoding: .utf8)
    }

    @objc func newScratch(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.new-document") { request() }
    }

    @objc func openDemoWall(_ sender: Any?) {
        engine.openDemoWall(window: windowId)
        request()
    }

    @objc func openDemo(_ sender: Any?) {
        engine.openDemo(window: windowId)
        request()
    }

    @objc func togglePeeker(_ sender: Any?) {
        if engine.perform(window: windowId, command: "peeker.toggle") { request() }
    }

    @objc func togglePalette(_ sender: Any?) {
        if engine.perform(window: windowId, command: "palette.toggle") { request() }
    }

    @objc func toggleWorkspaceTree(_ sender: Any?) {
        if engine.perform(window: windowId, command: "files.tree") { request() }
    }

    @objc func toggleToc(_ sender: Any?) {
        if engine.perform(window: windowId, command: "toc.toggle") { request() }
    }

    @objc func toggleChangesView(_ sender: Any?) {
        if engine.perform(window: windowId, command: "changes.view") { request() }
    }

    @objc func toggleAgentsSwitcher(_ sender: Any?) {
        if engine.perform(window: windowId, command: "agent.toggle-agents") { request() }
    }

    @objc func splitPane(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.split-pane") { request() }
    }

    @objc func openTerminal(_ sender: Any?) {
        if engine.perform(window: windowId, command: "terminal.open") { request() }
    }

    @objc func openSearch(_ sender: Any?) {
        if engine.perform(window: windowId, command: "search.focus") { request() }
    }

    @objc func openFind(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.open") { request() }
    }

    @objc func findNext(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.next") { request() }
    }

    @objc func findPrevious(_ sender: Any?) {
        if engine.perform(window: windowId, command: "find.previous") { request() }
    }

    @objc func closeWidget(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.close") { request() }
    }

    @objc func closePane(_ sender: Any?) {
        if engine.perform(window: windowId, command: "workbench.close-pane") { request() }
    }

    @objc func goBack(_ sender: Any?) {
        if engine.perform(window: windowId, command: "navigation.back") { request() }
    }

    @objc func goForward(_ sender: Any?) {
        if engine.perform(window: windowId, command: "navigation.forward") { request() }
    }

    @objc func toggleTheme(_ sender: Any?) {
        if engine.perform(window: windowId, command: "theme.toggle") { request() }
    }
}
