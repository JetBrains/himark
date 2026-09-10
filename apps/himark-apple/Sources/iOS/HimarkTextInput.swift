import UIKit

final class HimarkTextPosition: UITextPosition {
    let offset: UInt32

    init(_ offset: UInt32) {
        self.offset = offset
    }

    override func isEqual(_ object: Any?) -> Bool {
        (object as? HimarkTextPosition)?.offset == offset
    }

    override var hash: Int { Int(offset) }
    override var description: String { "HimarkTextPosition(\(offset))" }
}

final class HimarkTextRange: UITextRange {
    let from: UInt32
    let to: UInt32

    init(_ from: UInt32, _ to: UInt32) {
        self.from = min(from, to)
        self.to = max(from, to)
    }

    convenience init(_ range: HimarkRange) {
        self.init(range.start, range.start &+ range.length)
    }

    var himark: HimarkRange { HimarkRange(start: from, length: to &- from) }

    override var start: UITextPosition { HimarkTextPosition(from) }
    override var end: UITextPosition { HimarkTextPosition(to) }
    override var isEmpty: Bool { from == to }
    override var description: String { "HimarkTextRange(\(from)..<\(to))" }
}

final class HimarkSelectionRect: UITextSelectionRect {
    private let box: CGRect
    private let first: Bool
    private let last: Bool

    init(rect: CGRect, containsStart: Bool, containsEnd: Bool) {
        box = rect
        first = containsStart
        last = containsEnd
    }

    override var rect: CGRect { box }
    override var writingDirection: NSWritingDirection { .leftToRight }
    override var containsStart: Bool { first }
    override var containsEnd: Bool { last }
    override var isVertical: Bool { false }
}

extension HimarkView: UITextInput {
    private func offset(_ position: UITextPosition) -> UInt32? {
        (position as? HimarkTextPosition)?.offset
    }

    private func range(_ range: UITextRange) -> HimarkRange? {
        (range as? HimarkTextRange)?.himark
    }

    private var documentLength: UInt32 {
        engine.documentLength(window: windowId) ?? 0
    }

    var beginningOfDocument: UITextPosition { HimarkTextPosition(0) }
    var endOfDocument: UITextPosition { HimarkTextPosition(documentLength) }

    func text(in range: UITextRange) -> String? {
        guard let range = self.range(range) else { return nil }
        return range.length == 0 ? "" : engine.substring(window: windowId, range)
    }

    func replace(_ range: UITextRange, withText text: String) {
        guard let range = self.range(range) else { return }
        if engine.text(window: windowId, text, replacing: range) { request() }
    }

    var selectedTextRange: UITextRange? {
        get {
            engine.selectedRange(window: windowId).map(HimarkTextRange.init)
        }
        set {
            guard let range = newValue.flatMap({ self.range($0) }) else { return }
            if engine.setSelectedRange(window: windowId, range) { request() }
        }
    }

    var markedTextRange: UITextRange? {
        engine.markedRange(window: windowId).map(HimarkTextRange.init)
    }

    func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        let text = markedText ?? ""
        let selected = HimarkRange(start: UInt32(max(0, selectedRange.location)),
                                   length: UInt32(max(0, selectedRange.length)))

        if engine.setMarkedText(window: windowId, text, selected: selected, replacement: nil) {
            request()
        }
    }

    func unmarkText() {
        if engine.unmarkText(window: windowId) { request() }
    }

    func textRange(from fromPosition: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        guard let from = offset(fromPosition), let to = offset(toPosition) else { return nil }
        return HimarkTextRange(from, to)
    }

    func position(from position: UITextPosition, offset delta: Int) -> UITextPosition? {
        guard let base = self.offset(position) else { return nil }
        let moved = Int(base) + delta
        guard moved >= 0, moved <= Int(documentLength) else { return nil }
        return HimarkTextPosition(UInt32(moved))
    }

    func position(from position: UITextPosition,
                  in direction: UITextLayoutDirection,
                  offset delta: Int) -> UITextPosition? {
        switch direction {
        case .left: return self.position(from: position, offset: -delta)
        case .right: return self.position(from: position, offset: delta)
        case .up, .down:

            guard let offset = self.offset(position),
                  let engineRect = engine.firstRect(window: windowId,
                                                    HimarkRange(start: offset, length: 0)),
                  let caret = viewRect(engineRect),
                  caret.height > 0
            else { return nil }
            let step = caret.height * CGFloat(delta) * (direction == .up ? -1 : 1)
            return closestPosition(to: CGPoint(x: caret.midX, y: caret.midY + step))
        @unknown default: return nil
        }
    }

    func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        guard let left = offset(position), let right = offset(other) else { return .orderedSame }
        if left < right { return .orderedAscending }
        return left > right ? .orderedDescending : .orderedSame
    }

    func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int {
        guard let from = offset(from), let to = offset(toPosition) else { return 0 }
        return Int(to) - Int(from)
    }

    func position(within range: UITextRange,
                  farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        switch direction {
        case .left, .up: return range.start
        case .right, .down: return range.end
        @unknown default: return nil
        }
    }

    func characterRange(byExtending position: UITextPosition,
                        in direction: UITextLayoutDirection) -> UITextRange? {
        guard let base = offset(position),
              let other = self.position(from: position, in: direction, offset: 1),
              let moved = offset(other)
        else { return nil }
        return HimarkTextRange(base, moved)
    }

    func baseWritingDirection(for position: UITextPosition,
                              in direction: UITextStorageDirection) -> NSWritingDirection {
        .leftToRight
    }

    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    private func viewRect(_ rect: HimarkRect) -> CGRect? {
        let scale = pixelScale
        guard scale.isFinite, scale > 0 else { return nil }
        let out = CGRect(x: CGFloat(rect.x) / scale,
                         y: CGFloat(rect.y) / scale,
                         width: CGFloat(rect.width) / scale,
                         height: CGFloat(rect.height) / scale)
        guard out.origin.x.isFinite, out.origin.y.isFinite,
              out.width.isFinite, out.height.isFinite
        else { return nil }
        return out
    }

    private var fallbackCaret: CGRect {
        CGRect(x: 0, y: 0, width: 2, height: 24)
    }

    func firstRect(for range: UITextRange) -> CGRect {
        guard let range = self.range(range),
              let rect = engine.firstRect(window: windowId, range),
              let out = viewRect(rect)
        else { return .null }
        return out
    }

    func caretRect(for position: UITextPosition) -> CGRect {
        guard let offset = self.offset(position),
              let rect = engine.firstRect(window: windowId,
                                          HimarkRange(start: offset, length: 0)),
              let out = viewRect(rect)
        else { return fallbackCaret }
        return out
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let asked = self.range(range) else { return [] }
        let rects = engine.selectionRects(window: windowId, asked)
        let finite = rects.compactMap { viewRect($0) }
        return finite.enumerated().map { index, rect in
            HimarkSelectionRect(rect: rect,
                                containsStart: index == 0,
                                containsEnd: index == finite.count - 1)
        }
    }

    func closestPosition(to point: CGPoint) -> UITextPosition? {
        let scale = Float(pixelScale)
        guard let index = engine.charIndex(window: windowId,
                                           x: Float(point.x) * scale,
                                           y: Float(point.y) * scale)
        else { return nil }
        return HimarkTextPosition(UInt32(clamping: index))
    }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        guard let free = closestPosition(to: point).flatMap({ offset($0) }),
              let bounds = self.range(range)
        else { return nil }
        let clamped = min(max(free, bounds.start), bounds.start &+ bounds.length)
        return HimarkTextPosition(clamped)
    }

    func characterRange(at point: CGPoint) -> UITextRange? {
        guard let position = closestPosition(to: point), let at = offset(position) else {
            return nil
        }

        if at < documentLength { return HimarkTextRange(at, at &+ 1) }
        return at > 0 ? HimarkTextRange(at &- 1, at) : HimarkTextRange(at, at)
    }
}
