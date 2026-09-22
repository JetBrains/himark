// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import QuartzCore
import Metal

final class HimarkEngine {
    private let engine: OpaquePointer

    var raw: OpaquePointer { engine }

    init() {
        engine = himark_create()
    }

    deinit {
        himark_destroy(engine)
    }

    func addWindow() -> UInt64 {
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

    @discardableResult
    func render(window: UInt64, into surface: SkiaMetalSurface,
                pixelWidth w: Int32, pixelHeight h: Int32, scale: Float) -> Bool {
        guard let canvas = surface.begin(pixelWidth: w, pixelHeight: h) else { return false }
        let reconciling = himark_draw(engine, window, canvas, Float(w), Float(h), scale)
        surface.end()
        return reconciling
    }

    @discardableResult func tick(nowMs: Double) -> Bool { himark_animation_tick(engine, nowMs) }
    @discardableResult func key(window: UInt64, _ code: UInt32, mods: UInt32 = 0) -> Bool {
        himark_key_down(engine, window, code, mods)
    }
    @discardableResult func mouseDown(
        window: UInt64, x: Float, y: Float, mods: UInt32 = 0, clickCount: UInt32 = 1
    ) -> Bool {
        himark_mouse_down(engine, window, x, y, mods, clickCount)
    }
    func toolbarHeight() -> Float {
        himark_toolbar_height(engine)
    }
    @discardableResult func mouseDrag(window: UInt64, x: Float, y: Float, mods: UInt32 = 0) -> Bool {
        himark_mouse_drag(engine, window, x, y, mods)
    }
    @discardableResult func mouseUp(window: UInt64, x: Float, y: Float) -> Bool {
        himark_mouse_up(engine, window, x, y)
    }
    @discardableResult func mouseMove(window: UInt64, x: Float, y: Float) -> Bool {
        himark_mouse_move(engine, window, x, y)
    }
    @discardableResult func scroll(window: UInt64, x: Float, y: Float, dx: Float, dy: Float) -> Bool {
        himark_scroll(engine, window, x, y, dx, dy)
    }
    @discardableResult func scrollPhased(
        window: UInt64, x: Float, y: Float, dx: Float, dy: Float, phase: UInt32
    ) -> Bool {
        himark_scroll_phased(engine, window, x, y, dx, dy, phase)
    }

    @discardableResult
    func text(window: UInt64, _ string: String) -> Bool {
        string.withCString { himark_text_input(engine, window, $0, UInt(strlen($0))) }
    }

    @discardableResult
    func text(window: UInt64, _ string: String, replacing range: HimarkRange) -> Bool {
        string.withCString { himark_text_input_replacing(engine, window, $0, UInt(strlen($0)), range) }
    }

    func clipboardCopy(window: UInt64) -> String? {
        sizedString { out, cap in himark_copy(engine, window, out, cap) }
    }

    func clipboardCut(window: UInt64) -> String? {
        sizedString { out, cap in himark_cut(engine, window, out, cap) }
    }

    @discardableResult
    func clipboardPaste(window: UInt64, _ string: String) -> Bool {
        string.withCString { himark_paste(engine, window, $0, UInt(strlen($0))) }
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
    func perform(window: UInt64, command id: String) -> Bool {
        id.withCString { himark_perform_command(engine, window, $0) }
    }

    func openDemo(window: UInt64) { himark_open_demo(engine, window) }
    func openDemoWall(window: UInt64) { himark_open_demo_wall(engine, window) }

    func openDocument(window: UInt64, name: String, source: String, primary: Bool = true) {
        name.withCString { namePtr in
            source.withCString { sourcePtr in
                _ = himark_open_document(
                    engine, window, namePtr, UInt(strlen(namePtr)),
                    sourcePtr, UInt(strlen(sourcePtr)), primary)
            }
        }
    }

    func documentText(window: UInt64) -> String? {
        let range = HimarkRange(start: 0, length: UInt32.max)
        let needed = Int(himark_substring(engine, window, range, nil, 0))
        if needed == 0 { return nil }
        var buffer = [CChar](repeating: 0, count: needed)
        let written = Int(himark_substring(engine, window, range, &buffer, UInt(needed)))
        guard written > 0 else { return nil }
        return buffer.prefix(written).withUnsafeBufferPointer {
            $0.withMemoryRebound(to: UInt8.self) { String(decoding: $0, as: UTF8.self) }
        }
    }

    func hasTextFocus(window: UInt64) -> Bool { himark_has_text_focus(engine, window) }
    func hasMarkedText(window: UInt64) -> Bool { himark_has_marked_text(engine, window) }

    @discardableResult
    func setMarkedText(window: UInt64, _ string: String,
                       selected: HimarkRange, replacement: HimarkRange?) -> Bool {
        string.withCString { p in
            himark_set_marked_text(engine, window, p, UInt(strlen(p)), selected,
                                   replacement != nil,
                                   replacement ?? HimarkRange(start: 0, length: 0))
        }
    }

    @discardableResult
    func unmarkText(window: UInt64) -> Bool { himark_unmark_text(engine, window) }

    func markedRange(window: UInt64) -> HimarkRange? {
        var r = HimarkRange(start: 0, length: 0)
        return himark_marked_range(engine, window, &r) ? r : nil
    }

    func selectedRange(window: UInt64) -> HimarkRange? {
        var r = HimarkRange(start: 0, length: 0)
        return himark_selected_range(engine, window, &r) ? r : nil
    }

    func firstRect(window: UInt64, _ range: HimarkRange) -> HimarkRect? {
        var rect = HimarkRect(x: 0, y: 0, width: 0, height: 0)
        return himark_first_rect(engine, window, range, &rect) ? rect : nil
    }

    func charIndex(window: UInt64, x: Float, y: Float) -> Int? {
        let index = himark_char_index_at(engine, window, x, y)
        return index < 0 ? nil : Int(index)
    }

    func substring(window: UInt64, _ range: HimarkRange) -> String? {
        let needed = Int(himark_substring(engine, window, range, nil, 0))
        guard needed > 0 else { return nil }
        var buffer = [CChar](repeating: 0, count: needed)
        let written = Int(himark_substring(engine, window, range, &buffer, UInt(needed)))
        guard written == needed else { return nil }
        return buffer.withUnsafeBufferPointer {
            $0.withMemoryRebound(to: UInt8.self) { String(decoding: $0, as: UTF8.self) }
        }
    }

    @discardableResult
    func setSelectedRange(window: UInt64, _ range: HimarkRange) -> Bool {
        himark_set_selected_range(engine, window, range)
    }

    func documentLength(window: UInt64) -> UInt32? {
        var length: UInt32 = 0
        return himark_document_length(engine, window, &length) ? length : nil
    }

    @discardableResult
    func revealSelection(window: UInt64) -> Bool { himark_reveal_selection(engine, window) }

    func selectionRects(window: UInt64, _ range: HimarkRange) -> [HimarkRect] {
        let needed = Int(himark_selection_rects(engine, window, range, nil, 0))
        guard needed > 0 else { return [] }
        var rects = [HimarkRect](repeating: HimarkRect(x: 0, y: 0, width: 0, height: 0),
                                 count: needed)
        let written = Int(himark_selection_rects(engine, window, range, &rects, UInt(needed)))
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

    func begin(pixelWidth w: Int32, pixelHeight h: Int32) -> UnsafeMutableRawPointer? {
        skia_metal_begin(skia, w, h)
    }

    func end() {
        skia_metal_end(skia)
    }

    func setSyncPresent(_ sync: Bool) { skia_metal_set_sync(skia, sync) }
}
