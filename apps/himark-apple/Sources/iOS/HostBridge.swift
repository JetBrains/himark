import UIKit
import UniformTypeIdentifiers

final class HostBridge: NSObject, UIDocumentPickerDelegate {
    private let engine: HimarkEngine
    private weak var presenter: UIViewController?

    private var request: UInt64 = 0

    private var pickingFolder = false
    private let files = DispatchQueue(label: "himark.host.files", qos: .userInitiated)

    static func install(engine: HimarkEngine, presenter: UIViewController) {
        let bridge = HostBridge(engine: engine, presenter: presenter)
        let ctx = Unmanaged.passRetained(bridge).toOpaque()
        let callbacks = HimarkHostCallbacks(
            ctx: ctx,
            pick_files: { ctx, request, window in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()
                DispatchQueue.main.async { bridge.pickFiles(request: request, window: window) }
            },

            pick_save: nil,
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
                    UIPasteboard.general.string = copied
                }
            })
        himark_set_host(engine.raw, callbacks)
    }

    private init(engine: HimarkEngine, presenter: UIViewController) {
        self.engine = engine
        self.presenter = presenter
    }

    private let openableTypes: [UTType] = [
        UTType(filenameExtension: "md") ?? .plainText,
        UTType(filenameExtension: "rs") ?? .sourceCode,
        .plainText,
        .text,
        .sourceCode,
    ]

    private func pickFiles(request: UInt64, window: UInt64) {
        guard let presenter, self.request == 0 else {
            answerPicked(engine: engine, request: request, locations: [])
            return
        }
        self.request = request
        pickingFolder = false

        let picker = UIDocumentPickerViewController(
            forOpeningContentTypes: openableTypes + [.folder])
        picker.delegate = self
        picker.allowsMultipleSelection = true
        presenter.present(picker, animated: true)
    }

    private func fetch(request: UInt64, segments: [String]) {
        files.async { [self] in
            let text = fetchDocument(segments: segments)
            DispatchQueue.main.async {
                answerFetched(engine: self.engine, request: request, text: text)
            }
        }
    }

    private func store(request: UInt64, segments: [String], text: String) {
        files.async { [self] in
            let stored = storeDocument(segments: segments, text: text)
            DispatchQueue.main.async {
                _ = himark_host_stored(self.engine.raw, request, stored)
            }
        }
    }

    private func list(request: UInt64, segments: [String]) {
        files.async { [self] in
            let entries = listDirectory(segments: segments)
            DispatchQueue.main.async {
                answerListed(engine: self.engine, request: request, entries: entries)
            }
        }
    }

    func documentPicker(_ controller: UIDocumentPickerViewController,
                        didPickDocumentsAt urls: [URL]) {
        guard request != 0 else { return }
        if pickingFolder {
            let location = urls.first.map { url -> LocationSpec in
                ScopedRoots.shared.remember(url)
                return locationSpec(of: url, kind: ResourceKinds.directory)
            }
            answerPickedFolder(engine: engine, request: request, location: location)
        } else {
            answerPicked(
                engine: engine, request: request, locations: pickedDocumentLocations(in: urls))
        }
        request = 0
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard request != 0 else { return }
        if pickingFolder {
            answerPickedFolder(engine: engine, request: request, location: nil)
        } else {
            answerPicked(engine: engine, request: request, locations: [])
        }
        request = 0
    }
}
