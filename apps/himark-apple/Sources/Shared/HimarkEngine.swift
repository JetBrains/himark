// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import QuartzCore
import Metal

/// The engine's ONE animation clock. Every view used to drive
/// `himark_animation_tick` from its own display link — N engine-wide
/// ticks per frame, and the verdict ("something moved this frame")
/// answered only the view that happened to ask first, so a transition
/// that changed ANOTHER view's content never scheduled that view's
/// frame (the stuck brace highlight). The clock advances the engine
/// once per display frame: the first caller runs the tick, same-frame
/// followers read the frame's cached verdict, and a moving frame is
/// fanned out to EVERY adopted view — the engine's answer is global,
/// so the frame it owes is global too.
final class EngineFrameClock {
    private let advance: (Double) -> Bool
    private var tickedMs = -Double.infinity
    private var verdict = false
    private var fanout: [ObjectIdentifier: () -> Void] = [:]

    init(advance: @escaping (Double) -> Bool) {
        self.advance = advance
    }

    func adopt(_ owner: AnyObject, request: @escaping () -> Void) {
        fanout[ObjectIdentifier(owner)] = request
    }

    func orphan(_ owner: AnyObject) {
        fanout.removeValue(forKey: ObjectIdentifier(owner))
    }

    func tick(nowMs: Double, from owner: AnyObject) -> Bool {
        guard nowMs > tickedMs else { return verdict }
        tickedMs = nowMs
        verdict = advance(nowMs)
        if verdict {
            // The caller acts on the returned verdict itself; the
            // rest hear it as a request, which also resumes a
            // paused link.
            let caller = ObjectIdentifier(owner)
            for (id, request) in fanout where id != caller {
                request()
            }
        }
        return verdict
    }
}

final class HimarkEngine {
    private let engine: OpaquePointer

    var raw: OpaquePointer { engine }

    /// Minted lazily so `self` is whole; main-thread only, like
    /// every tick road.
    private(set) lazy var frameClock = EngineFrameClock { [unowned self] nowMs in
        self.tick(nowMs: nowMs)
    }

    /// Reveal handlers by bridge — ANY road may drain the engine's
    /// parked reveal (an animation tick is engine-wide), so the
    /// drain routes to the owning view instead of guessing that
    /// the drainer is the owner. Rect in device pixels.
    /// HOW a reveal places: minimally into view, or the rect's
    /// origin pinned to the viewport corner (a canvas row reveal).
    enum RevealPlacement {
        case visible
        case topLeft
    }

    private var revealHandlers: [UInt64: (CGRect, RevealPlacement) -> Void] = [:]

    func onReveal(bridge: HimarkBridge, _ handler: @escaping (CGRect, RevealPlacement) -> Void) {
        revealHandlers[bridge.raw] = handler
    }

    func clearReveal(bridge: HimarkBridge) {
        revealHandlers.removeValue(forKey: bridge.raw)
    }

    func drainReveal() {
        // Parked reveals queue PER BRIDGE now — drain them all;
        // one flooding caret-follow must not starve the rest.
        while true {
            var owner: UInt64 = 0
            var x: Float = 0, y: Float = 0, w: Float = 0, h: Float = 0
            var placement: UInt32 = 0
            guard himark_take_reveal(engine, &owner, &x, &y, &w, &h, &placement) else { return }
            if ProcessInfo.processInfo.environment["HIMARK_TRACE_REVEAL"] != nil {
                NSLog("[reveal] drain: owner \(owner) placement \(placement) handler? \(revealHandlers[owner] != nil)")
            }
            guard let handler = revealHandlers[owner] else {
                print("[reveal] parked by bridge \(owner) — no handler seated")
                continue
            }
            handler(CGRect(x: CGFloat(x), y: CGFloat(y),
                           width: CGFloat(w), height: CGFloat(h)),
                    placement == 1 ? .topLeft : .visible)
        }
    }

    init() {
        engine = himark_create()
    }

    deinit {
        himark_destroy(engine)
    }

    func addWindow() -> HimarkWindow {
        himark_add_window(engine)
    }

    func setChromeClearance(_ width: Float) {
        himark_set_chrome_clearance(engine, width)
    }

    func setWake(context: UnsafeMutableRawPointer,
                 callback: @escaping @convention(c) (UnsafeMutableRawPointer?) -> Void) {
        himark_set_wake(engine, callback, context)
    }

    func setEffectWake(context: UnsafeMutableRawPointer,
                       callback: @escaping @convention(c) (UnsafeMutableRawPointer?) -> Void) {
        himark_set_effect_wake(engine, callback, context)
    }

    @discardableResult
    func drain() -> Bool { himark_drain(engine) }

    func runPending() { himark_run_pending(engine) }

    // ─── the session surface: query + control ───

    struct CatalogSession: Equatable {
        let host: UInt64
        let uri: String
        let title: String
        let provider: String
        let status: Int64
        let modified: String
    }

    func sessionsEpoch() -> UInt64 { himark_sessions_epoch(engine) }

    /// The registered command verbs — palette material.
    func commands() -> [(id: String, name: String)] {
        let handle = himark_commands_copy(engine)
        guard handle.raw != 0 else { return [] }
        defer { himark_commands_release(engine, handle) }
        let count = himark_commands_len(engine, handle)
        return (0..<count).compactMap { index in
            guard
                let id = sizedString({ himark_command_id(engine, handle, index, $0, $1) }),
                let name = sizedString({ himark_command_name(engine, handle, index, $0, $1) })
            else { return nil }
            return (id, name)
        }
    }

    @discardableResult
    func sync() -> Bool { himark_sync(engine) }

    func sessions() -> [CatalogSession] {
        let catalog = himark_sessions_copy(engine)
        guard catalog.raw != 0 else { return [] }
        defer { himark_catalog_release(engine, catalog) }
        let count = himark_catalog_sessions(engine, catalog)
        var rows: [CatalogSession] = []
        rows.reserveCapacity(Int(count))
        for index in 0..<count {
            rows.append(CatalogSession(
                host: himark_catalog_session_host(engine, catalog, index),
                uri: sizedString { himark_catalog_session_uri(engine, catalog, index, $0, $1) } ?? "",
                title: sizedString { himark_catalog_session_title(engine, catalog, index, $0, $1) } ?? "",
                provider: sizedString { himark_catalog_session_provider(engine, catalog, index, $0, $1) } ?? "",
                status: himark_catalog_session_status(engine, catalog, index),
                modified: sizedString { himark_catalog_session_modified(engine, catalog, index, $0, $1) } ?? ""
            ))
        }
        return rows
    }

    @discardableResult
    func enterSession(host: UInt64, uri: String) -> Bool {
        uri.withCString { himark_session_enter(engine, host, $0, UInt(strlen($0))) }
    }

    @discardableResult
    func newSession(host: UInt64 = 0) -> Bool { himark_session_new(engine, host) }

    // ─── places and the minting door ───

    enum SessionViewKind {
        case chat, changes, files
        /// The split chat: the transcript as a native scroller's
        /// content, the composer band standalone.
        case transcript, composer
        /// The desktop changes tree: rows open the DIFF CANVAS.
        case changesCanvas
        /// The rail parity faces.
        case history, search, comments
        /// The go-to-file face: the peeker, whole.
        case peeker
        /// The command palette and the sessions browser — engine
        /// views, both: the shell paints the glass and nothing else.
        case palette, sessions
    }

    func mint(_ kind: SessionViewKind, host: UInt64, uri: String) -> HimarkBridge {
        let place: HimarkPlace = uri.withCString { cstr in
            switch kind {
            case .chat: himark_place_chat(engine, host, cstr, UInt(strlen(cstr)))
            case .changes: himark_place_changes(engine, host, cstr, UInt(strlen(cstr)))
            case .changesCanvas:
                himark_place_changes_canvas(engine, host, cstr, UInt(strlen(cstr)))
            case .history: himark_place_history(engine, host, cstr, UInt(strlen(cstr)))
            case .search: himark_place_search(engine, host, cstr, UInt(strlen(cstr)))
            case .comments: himark_place_comments(engine, host, cstr, UInt(strlen(cstr)))
            case .peeker: himark_place_peeker(engine, host, cstr, UInt(strlen(cstr)))
            case .palette: himark_place_palette(engine, host, cstr, UInt(strlen(cstr)))
            case .sessions: himark_place_sessions(engine, host, cstr, UInt(strlen(cstr)))
            case .files: himark_place_files(engine, host, cstr, UInt(strlen(cstr)))
            case .transcript:
                himark_place_chat_transcript(engine, host, cstr, UInt(strlen(cstr)))
            case .composer:
                himark_place_chat_composer(engine, host, cstr, UInt(strlen(cstr)))
            }
        }
        guard place.raw != 0 else { return HimarkBridge(raw: 0) }
        defer { himark_place_release(engine, place) }
        let bridge = himark_bridge_mint(engine, place)
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_REVEAL"] != nil {
            NSLog("[reveal] mint: \(kind) -> bridge \(bridge.raw)")
        }
        return bridge
    }

    func unmount(bridge: HimarkBridge) { himark_bridge_unmount(engine, bridge) }

    /// Mint from a place HANDLE — the navigate callback's road: the
    /// engine handed the place over, the host mints when its screen
    /// is up and releases when the screen pops.
    func mint(place: HimarkPlace) -> HimarkBridge {
        let bridge = himark_bridge_mint(engine, place)
        if ProcessInfo.processInfo.environment["HIMARK_TRACE_REVEAL"] != nil {
            NSLog("[reveal] mint(place) -> bridge \(bridge.raw)")
        }
        return bridge
    }

    /// WHICH SESSION a claimed place names — the shell re-points
    /// its views at it.
    func placeSessionHost(_ place: HimarkPlace) -> UInt64 {
        himark_place_session_host(engine, place)
    }

    func placeSessionUri(_ place: HimarkPlace) -> String {
        sizedString { himark_place_session_uri(engine, place, $0, $1) } ?? ""
    }

    /// A bridge's exported WINDOW overlays: the popover presence
    /// the shell reconciles against. Epoch moves on change only.
    func overlays(bridge: HimarkBridge) -> (epoch: UInt64, anchors: [CGRect]) {
        var epoch: UInt64 = 0
        let count = Int(himark_bridge_overlays(engine, bridge, &epoch))
        var anchors: [CGRect] = []
        anchors.reserveCapacity(count)
        for index in 0..<count {
            var x: Float = 0
            var y: Float = 0
            var w: Float = 0
            var h: Float = 0
            if himark_bridge_overlay_anchor(engine, bridge, UInt(index), &x, &y, &w, &h) {
                anchors.append(CGRect(
                    x: CGFloat(x), y: CGFloat(y),
                    width: CGFloat(w), height: CGFloat(h)))
            }
        }
        return (epoch, anchors)
    }

    /// Mount the POPOVER's bridge for overlay #index of `bridge`.
    func mintOverlay(bridge: HimarkBridge, index: Int) -> HimarkBridge {
        himark_overlay_mint(engine, bridge, UInt(index))
    }

    func release(place: HimarkPlace) {
        himark_place_release(engine, place)
    }

    /// The EXPLICIT tree-follow: the host landed this navigation —
    /// the session's files tree stands on the landed location.
    func treeFollow(place: HimarkPlace) {
        _ = himark_tree_follow(engine, place)
    }

    func setShell(context: UnsafeMutableRawPointer, onNavigate: HimarkNavigateFn) {
        himark_set_shell(engine, context, onNavigate)
    }

    // ─── the normalized bridge surface: handles, never pointers ───
    // One event is minted once (himark_event_*), may be dispatched
    // to any number of bridges, and is released explicitly.

    func windowBridge(window: HimarkWindow) -> HimarkBridge {
        himark_window_bridge(engine, window)
    }

    @discardableResult
    func bridgeEvent(bridge: HimarkBridge, event: HimarkEvent) -> HimarkPointed {
        himark_bridge_event(engine, bridge, event)
    }

    /// One bridge paint's outcome — the shell's retry contract.
    enum RenderPass {
        /// Painted, and the draw changed state: another pass is owed.
        case reconciling
        /// Painted, quiet.
        case painted
        /// No drawable this frame: NOTHING painted. The pass must be
        /// retried, or the view shows stale pixels until the next
        /// event happens to request a frame — conflating this with
        /// "painted, quiet" was the stuck-highlight bug.
        case starved
    }

    func renderBridge(bridge: HimarkBridge, into surface: SkiaMetalSurface,
                      pixelWidth w: Int32, pixelHeight h: Int32, scale: Float,
                      coupled: Bool = true) -> RenderPass {
        guard let canvas = surface.begin(pixelWidth: w, pixelHeight: h, coupled: coupled)
        else { return .starved }
        let reconciling = himark_bridge_draw(engine, bridge, canvas, Float(w), Float(h), scale)
        surface.end()
        return reconciling ? .reconciling : .painted
    }

    @discardableResult func tick(nowMs: Double) -> Bool { himark_animation_tick(engine, nowMs) }
    func toolbarHeight() -> Float {
        himark_toolbar_height(engine)
    }

    @discardableResult
    func text(bridge: HimarkBridge, _ string: String, replacing range: HimarkRange) -> Bool {
        string.withCString { himark_text_input_replacing(engine, bridge, $0, UInt(strlen($0)), range) }
    }

    func clipboardCopy(bridge: HimarkBridge) -> String? {
        sizedString { out, cap in himark_copy(engine, bridge, out, cap) }
    }

    func clipboardCut(bridge: HimarkBridge) -> String? {
        sizedString { out, cap in himark_cut(engine, bridge, out, cap) }
    }

    @discardableResult
    func clipboardPaste(bridge: HimarkBridge, _ string: String) -> Bool {
        string.withCString { himark_paste(engine, bridge, $0, UInt(strlen($0))) }
    }

    private func sizedString(_ call: (UnsafeMutablePointer<CChar>?, UInt) -> UInt) -> String? {
        let needed = Int(call(nil, 0))
        if needed == 0 { return nil }
        var buffer = [CChar](repeating: 0, count: needed)
        let written = Int(buffer.withUnsafeMutableBufferPointer { call($0.baseAddress, UInt(needed)) })
        guard written > 0, written <= needed else { return nil }
        return buffer.prefix(written).withUnsafeBufferPointer {
            $0.withMemoryRebound(to: UInt8.self) { String(decoding: $0, as: UTF8.self) }
        }
    }

    @discardableResult
    func perform(bridge: HimarkBridge, command id: String) -> Bool {
        id.withCString { himark_perform_command(engine, bridge, $0) }
    }

    func openDemo(bridge: HimarkBridge) { himark_open_demo(engine, bridge) }
    func openDemoWall(bridge: HimarkBridge) { himark_open_demo_wall(engine, bridge) }

    func openDocument(bridge: HimarkBridge, name: String, source: String, primary: Bool = true) {
        name.withCString { namePtr in
            source.withCString { sourcePtr in
                _ = himark_open_document(
                    engine, bridge, namePtr, UInt(strlen(namePtr)),
                    sourcePtr, UInt(strlen(sourcePtr)), primary)
            }
        }
    }

    func documentText(bridge: HimarkBridge) -> String? {
        let range = HimarkRange(start: 0, length: UInt32.max)
        let needed = Int(himark_substring(engine, bridge, range, nil, 0))
        if needed == 0 { return nil }
        var buffer = [CChar](repeating: 0, count: needed)
        let written = Int(himark_substring(engine, bridge, range, &buffer, UInt(needed)))
        guard written > 0 else { return nil }
        return buffer.prefix(written).withUnsafeBufferPointer {
            $0.withMemoryRebound(to: UInt8.self) { String(decoding: $0, as: UTF8.self) }
        }
    }

    func hasTextFocus(bridge: HimarkBridge) -> Bool { himark_has_text_focus(engine, bridge) }
    func hasMarkedText(bridge: HimarkBridge) -> Bool { himark_has_marked_text(engine, bridge) }

    @discardableResult
    func setMarkedText(bridge: HimarkBridge, _ string: String,
                       selected: HimarkRange, replacement: HimarkRange?) -> Bool {
        string.withCString { p in
            himark_set_marked_text(engine, bridge, p, UInt(strlen(p)), selected,
                                   replacement != nil,
                                   replacement ?? HimarkRange(start: 0, length: 0))
        }
    }

    @discardableResult
    func unmarkText(bridge: HimarkBridge) -> Bool { himark_unmark_text(engine, bridge) }

    func markedRange(bridge: HimarkBridge) -> HimarkRange? {
        var r = HimarkRange(start: 0, length: 0)
        return himark_marked_range(engine, bridge, &r) ? r : nil
    }

    func selectedRange(bridge: HimarkBridge) -> HimarkRange? {
        var r = HimarkRange(start: 0, length: 0)
        return himark_selected_range(engine, bridge, &r) ? r : nil
    }

    func firstRect(bridge: HimarkBridge, _ range: HimarkRange) -> HimarkRect? {
        var rect = HimarkRect(x: 0, y: 0, width: 0, height: 0)
        return himark_first_rect(engine, bridge, range, &rect) ? rect : nil
    }

    func charIndex(bridge: HimarkBridge, x: Float, y: Float) -> Int? {
        let index = himark_char_index_at(engine, bridge, x, y)
        return index < 0 ? nil : Int(index)
    }

    func substring(bridge: HimarkBridge, _ range: HimarkRange) -> String? {
        let needed = Int(himark_substring(engine, bridge, range, nil, 0))
        guard needed > 0 else { return nil }
        var buffer = [CChar](repeating: 0, count: needed)
        let written = Int(himark_substring(engine, bridge, range, &buffer, UInt(needed)))
        guard written == needed else { return nil }
        return buffer.withUnsafeBufferPointer {
            $0.withMemoryRebound(to: UInt8.self) { String(decoding: $0, as: UTF8.self) }
        }
    }

    @discardableResult
    func setSelectedRange(bridge: HimarkBridge, _ range: HimarkRange) -> Bool {
        himark_set_selected_range(engine, bridge, range)
    }

    func documentLength(bridge: HimarkBridge) -> UInt32? {
        var length: UInt32 = 0
        return himark_document_length(engine, bridge, &length) ? length : nil
    }

    @discardableResult
    func revealSelection(bridge: HimarkBridge) -> Bool { himark_reveal_selection(engine, bridge) }

    func selectionRects(bridge: HimarkBridge, _ range: HimarkRange) -> [HimarkRect] {
        let needed = Int(himark_selection_rects(engine, bridge, range, nil, 0))
        guard needed > 0 else { return [] }
        var rects = [HimarkRect](repeating: HimarkRect(x: 0, y: 0, width: 0, height: 0),
                                 count: needed)
        let written = Int(himark_selection_rects(engine, bridge, range, &rects, UInt(needed)))
        return written == needed ? rects : []
    }
}

final class SkiaMetalSurface {
    private let skia: OpaquePointer

    init(layer: CAMetalLayer, device: MTLDevice) {
        skia = skia_metal_create(Unmanaged.passUnretained(layer).toOpaque(),
                                 Unmanaged.passUnretained(device).toOpaque())
    }

    deinit {
        skia_metal_destroy(skia)
    }

    /// `coupled` must be decided BEFORE the drawable is vended —
    /// it latches the layer's presentsWithTransaction for this
    /// frame's drawable. Pass the same value to `end`.
    func begin(pixelWidth w: Int32, pixelHeight h: Int32, coupled: Bool = true) -> UnsafeMutableRawPointer? {
        skia_metal_begin(skia, w, h, coupled)
    }

    /// ONE PRESENT DISCIPLINE: inside the transaction, on this
    /// thread. The async road (a0c6635e) let a still frame hand its
    /// drawable to the submit queue and return, which read well on
    /// a scroll and was ruinous on a drag: the layer's
    /// `presentsWithTransaction` is a MODE, and flipping it per
    /// frame while a present is still pending leaves
    /// `nextDrawable` waiting out CoreAnimation's one-second
    /// timeout, over and over. Measured over 240 resize steps:
    /// 51.9s mixing the roads, 4.6s coupled throughout.
    func end() {
        skia_metal_end(skia)
    }

}
