// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit

/// The NATIVE-SCROLL document screen: a PLAIN UIScrollView whose
/// content view is an ordinary `HimarkBridgeView` embedded at
/// content size (docs/ui/imba-ffi.md, the stage road). The bridge
/// view renders its visible band and owns all input — taps, the
/// keyboard, IME — exactly as it does full-bleed; this host only
/// sizes the content, insets for the keyboard, and reveals the
/// caret after edits.
final class StageScrollHost: UIView, UIScrollViewDelegate {
    private let host: AppHost
    private var bridge = HimarkBridge(raw: 0)
    private let scroll = UIScrollView()
    private var content: HimarkBridgeView?

    init(host: AppHost, place: HimarkPlace) {
        self.host = host
        super.init(frame: .zero)
        backgroundColor = .black
        bridge = host.engine.mint(place: place)

        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.alwaysBounceVertical = true
        scroll.delegate = self
        addSubview(scroll)
        scroll.frame = bounds

        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: CGRect(origin: .zero, size: bounds.size),
            engine: host.engine, bridge: bridge, device: host.device,
            scrollEmbedded: true)
        view.onContentMeasured = { [weak self] size in self?.contentMeasured(size) }
        view.onEdited = { [weak self] in self?.revealCaret() }
        scroll.addSubview(view)
        host.register(view)
        content = view

        let center = NotificationCenter.default
        center.addObserver(self, selector: #selector(keyboardChanged(_:)),
                           name: UIResponder.keyboardWillChangeFrameNotification, object: nil)
        center.addObserver(self, selector: #selector(keyboardChanged(_:)),
                           name: UIResponder.keyboardWillHideNotification, object: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    func teardown() {
        if let content {
            host.unregister(content)
            content.removeFromSuperview()
        }
        content = nil
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let content else { return }
        // Seed a provisional viewport-sized frame: the real height
        // arrives from the view's measure, which only runs once a
        // visible band exists to render.
        var frame = content.frame
        frame.size.width = bounds.width
        if frame.size.height < bounds.height { frame.size.height = bounds.height }
        if frame != content.frame {
            content.frame = frame
            scroll.contentSize = frame.size
            content.request()
        }
    }

    private func contentMeasured(_ size: CGSize) {
        guard let content else { return }
        let height = max(size.height, bounds.height)
        if content.frame.size.height != height || scroll.contentSize.height != height {
            content.frame = CGRect(x: 0, y: 0, width: bounds.width, height: height)
            scroll.contentSize = content.frame.size
        }
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        content?.renderNow()
    }

    @objc private func keyboardChanged(_ note: Notification) {
        guard let frame = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?
            .cgRectValue
        else { return }
        let overlap = max(0, bounds.maxY - convert(frame, from: nil).minY)
        scroll.contentInset.bottom = overlap
        scroll.verticalScrollIndicatorInsets.bottom = overlap
        if overlap > 0 { revealCaret() }
    }

    /// Scroll the caret's rect into the visible band — the host's
    /// half of Reveal: the engine answers geometry in content
    /// coordinates, the scroller moves.
    private func revealCaret() {
        guard bridge.raw != 0, let content,
              let range = host.engine.selectedRange(bridge: bridge),
              let rect = host.engine.firstRect(bridge: bridge, range)
        else { return }
        let scale = content.pixelScale
        let target = CGRect(
            x: CGFloat(rect.x) / scale,
            y: CGFloat(rect.y) / scale - 8,
            width: max(CGFloat(rect.width) / scale, 2),
            height: CGFloat(rect.height) / scale + 16)
        scroll.scrollRectToVisible(target, animated: false)
    }
}
