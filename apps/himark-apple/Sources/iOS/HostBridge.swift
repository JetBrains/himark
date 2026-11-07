// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit
import UniformTypeIdentifiers

final class HostBridge: NSObject, UIDocumentPickerDelegate {
    /// Attached after `himark_create` (the engine) and after the
    /// first scene stands (the presenter) — the callbacks are the
    /// engine's CONSTRUCTION parameters, so the bridge is born
    /// first; a callback firing before attachment answers empty.
    private var engine: HimarkEngine?
    private weak var presenter: UIViewController?

    private var request: HimarkRequest?

    private var pickingFolder = false
    private let files = DispatchQueue(label: "himark.host.files", qos: .userInitiated)

    /// The host's capability callbacks, for `himark_create` — and
    /// the bridge they dispatch into, which wants `attach` once the
    /// engine and the root presenter exist.
    static func prepare() -> (HostBridge, HimarkHostCallbacks) {
        let bridge = HostBridge()
        let ctx = Unmanaged.passRetained(bridge).toOpaque()
        let callbacks = HimarkHostCallbacks(
            ctx: ctx,
            pick_files: { ctx, request, origin in
                guard let ctx else { return }
                let bridge = Unmanaged<HostBridge>.fromOpaque(ctx).takeUnretainedValue()
                DispatchQueue.main.async { bridge.pickFiles(request: request, origin: origin) }
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
            },
            open_url: { _, url, urlLen in
                guard let url else { return }
                let bytes = UnsafeRawBufferPointer(start: url, count: Int(urlLen))
                let raw = String(decoding: bytes, as: UTF8.self)
                DispatchQueue.main.async {
                    guard let parsed = URL(string: raw) else { return }
                    UIApplication.shared.open(parsed)
                }
            })
        return (bridge, callbacks)
    }

    func attach(engine: HimarkEngine, presenter: UIViewController) {
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

    private func pickFiles(request: HimarkRequest, origin _: HimarkBridge) {
        guard let engine else { return }
        guard let presenter, self.request == nil else {
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

    private func fetch(request: HimarkRequest, segments: [String]) {
        files.async { [self] in
            let text = fetchDocument(segments: segments)
            DispatchQueue.main.async {
                guard let engine = self.engine else { return }
                answerFetched(engine: engine, request: request, text: text)
            }
        }
    }

    private func store(request: HimarkRequest, segments: [String], text: String) {
        files.async { [self] in
            let stored = storeDocument(segments: segments, text: text)
            DispatchQueue.main.async {
                guard let engine = self.engine else { return }
                _ = himark_host_stored(engine.raw, request, stored)
            }
        }
    }

    private func list(request: HimarkRequest, segments: [String]) {
        files.async { [self] in
            let entries = listDirectory(segments: segments)
            DispatchQueue.main.async {
                guard let engine = self.engine else { return }
                answerListed(engine: engine, request: request, entries: entries)
            }
        }
    }

    func documentPicker(_ controller: UIDocumentPickerViewController,
                        didPickDocumentsAt urls: [URL]) {
        guard let engine, let request else { return }
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
        self.request = nil
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard let engine, let request else { return }
        if pickingFolder {
            answerPickedFolder(engine: engine, request: request, location: nil)
        } else {
            answerPicked(engine: engine, request: request, locations: [])
        }
        self.request = nil
    }
}
