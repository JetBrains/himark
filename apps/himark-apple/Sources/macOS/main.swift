import AppKit
import Metal

final class AppDelegate: NSObject, NSApplicationDelegate {
    let device = MTLCreateSystemDefaultDevice()!
    private(set) var engine: HimarkEngine!

    private var windows: [NSWindow] = []

    private let executor = DispatchQueue(label: "dev.himark.executor", qos: .userInteractive)

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
        true
    }

    func bootEngine() {
        engine = HimarkEngine()

        engine.setChromeClearance(130.0)
        let ctx = Unmanaged.passUnretained(self).toOpaque()
        engine.setWake(context: ctx) { ctx in
            let delegate = Unmanaged<AppDelegate>.fromOpaque(ctx!).takeUnretainedValue()
            DispatchQueue.main.async { delegate.drain() }
        }
        engine.setEffectWake(context: ctx) { ctx in
            let delegate = Unmanaged<AppDelegate>.fromOpaque(ctx!).takeUnretainedValue()
            delegate.executor.async { delegate.engine.runPending() }
        }

        HostBridge.install(engine: engine) { [weak self] in self?.repaintAll() }

        let engine = engine!
        executor.async { engine.runPending() }
    }

    private func drain() {
        guard engine.drain() else { return }
        repaintAll()
    }

    private func repaintAll() {
        for window in windows {
            (window.contentView as? HimarkView)?.request()
        }
    }

    @discardableResult
    func makeWindow() -> NSWindow {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 860),
            styleMask: [.titled, .closable, .resizable, .miniaturizable, .fullSizeContentView],
            backing: .buffered, defer: false)
        window.title = "himark"

        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        window.isReleasedWhenClosed = false
        window.center()

        let view = HimarkView(frame: window.contentView!.bounds, engine: engine, device: device)
        view.autoresizingMask = [.width, .height]
        window.contentView = view
        window.makeFirstResponder(view)
        window.makeKeyAndOrderFront(nil)
        windows.append(window)
        return window
    }

    @objc func newWindow(_ sender: Any?) {
        makeWindow()
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)

func buildMenu() -> NSMenu {
    let mainMenu = NSMenu()

    let appItem = NSMenuItem()
    mainMenu.addItem(appItem)
    let appMenu = NSMenu()
    appMenu.addItem(withTitle: "Quit himark",
                    action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
    appItem.submenu = appMenu

    let fileItem = NSMenuItem()
    mainMenu.addItem(fileItem)
    let fileMenu = NSMenu(title: "File")
    fileMenu.addItem(withTitle: "New",
                     action: #selector(HimarkView.newScratch(_:)), keyEquivalent: "n")
    fileMenu.addItem(withTitle: "New Window",
                     action: #selector(AppDelegate.newWindow(_:)), keyEquivalent: "N")
    fileMenu.addItem(withTitle: "Open…",
                     action: #selector(HimarkView.openDocument(_:)), keyEquivalent: "o")
    fileMenu.addItem(withTitle: "Save",
                     action: #selector(HimarkView.saveDocument(_:)), keyEquivalent: "s")
    fileMenu.addItem(withTitle: "Save As…",
                     action: #selector(HimarkView.saveDocumentAs(_:)), keyEquivalent: "S")
    fileMenu.addItem(.separator())
    fileMenu.addItem(withTitle: "Open Demo Document",
                     action: #selector(HimarkView.openDemo(_:)), keyEquivalent: "")
    fileMenu.addItem(withTitle: "Open Demo Wall of Text",
                     action: #selector(HimarkView.openDemoWall(_:)), keyEquivalent: "")
    fileItem.submenu = fileMenu

    let editItem = NSMenuItem()
    mainMenu.addItem(editItem)
    let editMenu = NSMenu(title: "Edit")
    editMenu.addItem(withTitle: "Undo",
                     action: #selector(HimarkView.undoEdit(_:)), keyEquivalent: "z")
    let redo = editMenu.addItem(withTitle: "Redo",
                                action: #selector(HimarkView.redoEdit(_:)), keyEquivalent: "Z")
    redo.keyEquivalentModifierMask = [.command, .shift]
    editMenu.addItem(.separator())
    editMenu.addItem(withTitle: "Cut",
                     action: #selector(HimarkView.cut(_:)), keyEquivalent: "x")
    editMenu.addItem(withTitle: "Copy",
                     action: #selector(HimarkView.copy(_:)), keyEquivalent: "c")
    editMenu.addItem(withTitle: "Paste",
                     action: #selector(HimarkView.paste(_:)), keyEquivalent: "v")
    editMenu.addItem(.separator())
    editMenu.addItem(withTitle: "Select All",
                     action: #selector(HimarkView.selectAll(_:)), keyEquivalent: "a")
    editItem.submenu = editMenu

    let viewItem = NSMenuItem()
    mainMenu.addItem(viewItem)
    let viewMenu = NSMenu(title: "View")
    viewMenu.addItem(withTitle: "Toggle Peeker",
                     action: #selector(HimarkView.togglePeeker(_:)), keyEquivalent: "p")
    viewMenu.addItem(withTitle: "Table of Contents",
                     action: #selector(HimarkView.toggleToc(_:)), keyEquivalent: "t")
    viewMenu.addItem(withTitle: "Workspace Tree",
                     action: #selector(HimarkView.toggleWorkspaceTree(_:)), keyEquivalent: "T")
    viewMenu.addItem(withTitle: "Changes",
                     action: #selector(HimarkView.toggleChangesView(_:)), keyEquivalent: "r")
    viewMenu.addItem(withTitle: "Switch Workspace",
                     action: #selector(HimarkView.toggleWorkspaceSwitcher(_:)), keyEquivalent: "U")
    viewMenu.addItem(withTitle: "Command Palette",
                     action: #selector(HimarkView.togglePalette(_:)), keyEquivalent: "P")
    viewMenu.addItem(withTitle: "Split Pane",
                     action: #selector(HimarkView.splitPane(_:)), keyEquivalent: "d")
    viewMenu.addItem(withTitle: "Find",
                     action: #selector(HimarkView.openFind(_:)), keyEquivalent: "f")
    viewMenu.addItem(withTitle: "Find Next",
                     action: #selector(HimarkView.findNext(_:)), keyEquivalent: "g")
    viewMenu.addItem(withTitle: "Find Previous",
                     action: #selector(HimarkView.findPrevious(_:)), keyEquivalent: "G")
    viewMenu.addItem(withTitle: "Find in Files",
                     action: #selector(HimarkView.openSearch(_:)), keyEquivalent: "F")
    viewMenu.addItem(withTitle: "New Terminal",
                     action: #selector(HimarkView.openTerminal(_:)), keyEquivalent: "T")
    viewMenu.addItem(withTitle: "Close",
                     action: #selector(HimarkView.closeWidget(_:)), keyEquivalent: "w")
    viewMenu.addItem(withTitle: "Close Pane",
                     action: #selector(HimarkView.closePane(_:)), keyEquivalent: "W")
    viewMenu.addItem(.separator())
    viewMenu.addItem(withTitle: "Toggle Light/Dark Theme",
                     action: #selector(HimarkView.toggleTheme(_:)), keyEquivalent: "L")
    viewItem.submenu = viewMenu

    let goItem = NSMenuItem()
    mainMenu.addItem(goItem)
    let goMenu = NSMenu(title: "Go")
    goMenu.addItem(withTitle: "Back",
                   action: #selector(HimarkView.goBack(_:)), keyEquivalent: "[")
    goMenu.addItem(withTitle: "Forward",
                   action: #selector(HimarkView.goForward(_:)), keyEquivalent: "]")
    goItem.submenu = goMenu

    return mainMenu
}
app.mainMenu = buildMenu()

delegate.bootEngine()
delegate.makeWindow()

app.activate(ignoringOtherApps: true)
app.run()
