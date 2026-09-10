import Foundation

enum ResourceKinds {
    static let document = "document"
    static let directory = "dir"
}

struct LocationSpec {
    let kind: String
    let segments: [String]
}

func locationSpec(of url: URL, kind: String) -> LocationSpec {
    LocationSpec(kind: kind, segments: url.pathComponents.filter { $0 != "/" })
}

func fileURL(forSegments segments: [String]) -> URL {
    URL(fileURLWithPath: "/" + segments.joined(separator: "/"))
}

final class ScopedRoots {
    static let shared = ScopedRoots()
    private var roots: [String: URL] = [:]

    func remember(_ url: URL) {
        guard roots[url.path] == nil else { return }
        if url.startAccessingSecurityScopedResource() {
            roots[url.path] = url
        }
    }
}

func pickedDocumentLocations(in urls: [URL]) -> [LocationSpec] {
    var locations: [LocationSpec] = []
    let fileManager = FileManager.default

    for url in urls {
        ScopedRoots.shared.remember(url)
        var isDirectory: ObjCBool = false
        guard fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
            continue
        }
        if isDirectory.boolValue {
            locations.append(locationSpec(of: url, kind: ResourceKinds.directory))
        } else if isOpenable(url) {
            locations.append(locationSpec(of: url, kind: ResourceKinds.document))
        }
    }
    return locations
}

let binaryishExtensions: Set<String> = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "ico", "icns", "heic", "pdf", "zip",
    "gz", "tgz", "tar", "xz", "7z", "rar", "jar", "class", "o", "a", "so", "dylib", "dll",
    "exe", "bin", "dmg", "ttf", "otf", "woff", "woff2", "mp3", "mp4", "m4a", "mov", "avi",
    "mkv", "wav", "flac", "ogg", "db", "sqlite",
]

func isOpenable(_ url: URL) -> Bool {
    let ext = url.pathExtension.lowercased()
    return ext.isEmpty || !binaryishExtensions.contains(ext)
}

func fetchDocument(segments: [String]) -> String? {
    try? String(contentsOf: fileURL(forSegments: segments), encoding: .utf8)
}

func storeDocument(segments: [String], text: String) -> Bool {
    (try? text.write(to: fileURL(forSegments: segments), atomically: true, encoding: .utf8))
        != nil
}

func listDirectory(segments: [String]) -> [LocationSpec]? {
    let url = fileURL(forSegments: segments)
    guard
        let children = try? FileManager.default.contentsOfDirectory(
            at: url, includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles])
    else { return nil }
    var directories: [URL] = []
    var files: [URL] = []
    for child in children {
        let isDirectory =
            (try? child.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory ?? false
        if isDirectory {
            directories.append(child)
        } else if isOpenable(child) {
            files.append(child)
        }
    }
    let byName = { (a: URL, b: URL) in
        a.lastPathComponent.localizedStandardCompare(b.lastPathComponent) == .orderedAscending
    }
    return directories.sorted(by: byName).map { locationSpec(of: $0, kind: ResourceKinds.directory) }
        + files.sorted(by: byName).map { locationSpec(of: $0, kind: ResourceKinds.document) }
}

func decodeLocation(_ location: UnsafePointer<HimarkLocation>?) -> [String] {
    guard let location = location?.pointee else { return [] }
    var segments: [String] = []
    for index in 0..<Int(location.segment_count) {
        let segment = location.segments[index]
        guard let ptr = segment.ptr else { continue }
        let bytes = UnsafeRawBufferPointer(start: ptr, count: Int(segment.len))
        segments.append(String(decoding: bytes, as: UTF8.self))
    }
    return segments
}

func withAbiLocations<T>(
    _ specs: [LocationSpec],
    _ body: (UnsafePointer<HimarkLocation>?, UInt) -> T
) -> T {
    var cstrings: [UnsafeMutablePointer<CChar>] = []
    var segmentBuffers: [UnsafeMutablePointer<HimarkStr>] = []
    defer {
        cstrings.forEach { free($0) }
        segmentBuffers.forEach { $0.deallocate() }
    }
    func abiStr(_ string: String) -> HimarkStr {
        guard let dup = strdup(string) else { return HimarkStr(ptr: nil, len: 0) }
        cstrings.append(dup)
        return HimarkStr(ptr: dup, len: UInt(strlen(dup)))
    }
    var locations: [HimarkLocation] = []
    for spec in specs {
        let buffer = UnsafeMutablePointer<HimarkStr>.allocate(
            capacity: max(1, spec.segments.count))
        segmentBuffers.append(buffer)
        for (index, segment) in spec.segments.enumerated() {
            buffer[index] = abiStr(segment)
        }
        locations.append(
            HimarkLocation(
                kind: abiStr(spec.kind),
                authority: abiStr("local"),
                segments: buffer,
                segment_count: UInt(spec.segments.count)))
    }
    return locations.withUnsafeBufferPointer { body($0.baseAddress, UInt(specs.count)) }
}

func answerPicked(engine: HimarkEngine, request: UInt64, locations: [LocationSpec]) {
    withAbiLocations(locations) { base, count in
        _ = himark_host_picked(engine.raw, request, base, count)
    }
}

func answerPickedFolder(engine: HimarkEngine, request: UInt64, location: LocationSpec?) {
    switch location {
    case let .some(location):
        withAbiLocations([location]) { base, _ in
            _ = himark_host_picked_folder(engine.raw, request, base)
        }
    case .none:
        _ = himark_host_picked_folder(engine.raw, request, nil)
    }
}

func answerFetched(engine: HimarkEngine, request: UInt64, text: String?) {
    guard let text else {
        _ = himark_host_fetched(engine.raw, request, nil, 0)
        return
    }
    text.withCString { cstr in
        _ = himark_host_fetched(engine.raw, request, cstr, UInt(strlen(cstr)))
    }
}

func answerListed(engine: HimarkEngine, request: UInt64, entries: [LocationSpec]?) {
    switch entries {
    case let .some(entries):
        withAbiLocations(entries) { base, count in
            _ = himark_host_listed(engine.raw, request, base, count)
        }
    case .none:
        _ = himark_host_listed(engine.raw, request, nil, 0)
    }
}
