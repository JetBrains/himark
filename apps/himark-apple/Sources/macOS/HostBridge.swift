import AppKit

final class HostBridge {
    private let engine: HimarkEngine
    private let onRepaint: () -> Void

    private let files = DispatchQueue(label: "himark.host.files", qos: .userInitiated)

    static func install(engine: HimarkEngine, onRepaint: @escaping () -> Void) {
        let bridge = HostBridge(engine: engine, onRepaint: onRepaint)
        let ctx = Unmanaged.passRetained(bridge).toOpaque()
        let callbacks = HimarkHostCallbacks(
            ctx: ctx,
            pick_files: { ctx, request, window in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()
                DispatchQueue.main.async { bridge.pickFiles(request: request, window: window) }
            },
            pick_save: { ctx, request, suggested, suggestedLen in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()

                let name: String
                if let suggested {
                    let bytes = UnsafeRawBufferPointer(start: suggested, count: Int(suggestedLen))
                    name = String(decoding: bytes, as: UTF8.self)
                } else {
                    name = "document.md"
                }
                DispatchQueue.main.async { bridge.pickSave(request: request, suggested: name) }
            },
            fetch_document: { ctx, request, location in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()

                let segments = decodeLocation(location)
                bridge.fetch(request: request, segments: segments)
            },
            store_document: { ctx, request, location, text, textLen in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()
                let segments = decodeLocation(location)
                let copied: String
                if let text {
                    let bytes = UnsafeRawBufferPointer(start: text, count: Int(textLen))
                    copied = String(decoding: bytes, as: UTF8.self)
                } else {
                    copied = ""
                }
                bridge.store(request: request, segments: segments, text: copied)
            },
            list_directory: { ctx, request, location in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()
                let segments = decodeLocation(location)
                bridge.list(request: request, segments: segments)
            },

            subscribe: nil,
            unsubscribe: nil,
            set_clipboard: { _, text, textLen in

                guard let text else { return }
                let bytes = UnsafeRawBufferPointer(start: text, count: Int(textLen))
                let copied = String(decoding: bytes, as: UTF8.self)
                DispatchQueue.main.async {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(copied, forType: .string)
                }
            })
        himark_set_host(engine.raw, callbacks)
    }

    private init(engine: HimarkEngine, onRepaint: @escaping () -> Void) {
        self.engine = engine
        self.onRepaint = onRepaint
    }

    private func pickFiles(request: UInt64, window: UInt64) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        let locations = panel.runModal() == .OK ? pickedDocumentLocations(in: panel.urls) : []
        answerPicked(engine: engine, request: request, locations: locations)
        onRepaint()
    }

    private func pickSave(request: UInt64, suggested: String) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = suggested
        var location: LocationSpec?
        if panel.runModal() == .OK, let url = panel.url {
            ScopedRoots.shared.remember(url)
            location = locationSpec(of: url, kind: ResourceKinds.document)
        }
        answerPickedFolder(engine: engine, request: request, location: location)
        onRepaint()
    }

    private func fetch(request: UInt64, segments: [String]) {
        files.async { [self] in
            let text = fetchDocument(segments: segments)
            DispatchQueue.main.async {
                answerFetched(engine: self.engine, request: request, text: text)
                self.onRepaint()
            }
        }
    }

    private func store(request: UInt64, segments: [String], text: String) {
        files.async { [self] in
            let stored = storeDocument(segments: segments, text: text)
            DispatchQueue.main.async {
                _ = himark_host_stored(self.engine.raw, request, stored)
                self.onRepaint()
            }
        }
    }

    private func list(request: UInt64, segments: [String]) {
        files.async { [self] in
            let entries = listDirectory(segments: segments)
            DispatchQueue.main.async {
                answerListed(engine: self.engine, request: request, entries: entries)
                self.onRepaint()
            }
        }
    }
}
