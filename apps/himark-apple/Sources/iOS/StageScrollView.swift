// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import UIKit
import Metal
import QuartzCore

/// The NATIVE-SCROLL document screen (docs/ui/imba-ffi.md, the
/// stage road): UIScrollView owns the physics and the viewport;
/// the content renders through staged values — layout at
/// (width, ∞), content size from the sized stage, realize + paint
/// per visible band. No scroll event ever enters the engine.
final class StageScrollHost: UIView, UIScrollViewDelegate {
    private let host: AppHost
    private var bridge = HimarkBridge(raw: 0)

    private let scroll = UIScrollView()
    private let render = StageRenderView()
    private var surface: SkiaMetalSurface!
    private var link: CADisplayLink?
    private var needsRedraw = true

    init(host: AppHost, place: HimarkPlace) {
        self.host = host
        super.init(frame: .zero)
        backgroundColor = .black
        bridge = host.engine.mint(place: place)

        render.metalLayer.device = host.device
        render.metalLayer.pixelFormat = .bgra8Unorm
        render.metalLayer.framebufferOnly = false
        render.metalLayer.contentsScale = UIScreen.main.scale
        surface = SkiaMetalSurface(layer: render.metalLayer, device: host.device)

        // The render view sits UNDER the transparent scroll view:
        // the scroll view supplies physics, indicators and touch
        // routing; the band repaints as the offset moves.
        render.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(render)
        scroll.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        scroll.backgroundColor = .clear
        scroll.delegate = self
        scroll.alwaysBounceVertical = true
        addSubview(scroll)
        render.frame = bounds
        scroll.frame = bounds

        let tap = UITapGestureRecognizer(target: self, action: #selector(onTap))
        scroll.addGestureRecognizer(tap)

        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        self.link = link
        host.registerRedraw(self) { [weak self] in self?.needsRedraw = true }
    }

    required init?(coder: NSCoder) { fatalError() }

    func teardown() {
        link?.invalidate()
        link = nil
        host.unregisterRedraw(self)
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }

    private var pixelScale: CGFloat { render.metalLayer.contentsScale }

    override func layoutSubviews() {
        super.layoutSubviews()
        needsRedraw = true
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        needsRedraw = true
    }

    @objc private func tick() {
        if needsRedraw {
            needsRedraw = false
            renderPass()
        }
    }

    /// One pass of the staged pipeline: layout → measure → realize
    /// the visible band → paint. A paint that CHANGED state means
    /// the engine quiesced the stages — the next pass re-derives.
    private func renderPass() {
        guard bridge.raw != 0, bounds.width > 0, bounds.height > 0 else { return }
        let engine = host.engine.raw
        let scale = pixelScale
        let wPx = Float(bounds.width * scale)
        let vhPx = Float(bounds.height * scale)

        let layout = himark_bridge_layout(engine, bridge, wPx, Float.infinity)
        guard layout.raw != 0 else { return }
        var cw: Float = 0
        var ch: Float = 0
        _ = himark_layout_size(engine, layout, &cw, &ch)
        let contentHeight = max(CGFloat(ch) / scale, bounds.height)
        if abs(scroll.contentSize.height - contentHeight) > 0.5 {
            scroll.contentSize = CGSize(width: bounds.width, height: contentHeight)
        }

        let offsetPx = Float(max(scroll.contentOffset.y, 0) * scale)
        let frame = himark_layout_realize(engine, layout, 0, offsetPx, wPx, vhPx)
        guard frame.raw != 0 else { return }

        render.metalLayer.drawableSize = CGSize(width: CGFloat(wPx), height: CGFloat(vhPx))
        guard let canvas = surface.begin(pixelWidth: Int32(wPx), pixelHeight: Int32(vhPx))
        else { return }
        let changed = himark_frame_paint(engine, frame, canvas, 0, offsetPx, wPx, vhPx)
        surface.end()
        if changed {
            // The paint performed commands: the engine quiesced the
            // bridge's stages, this frame handle is already gone.
            needsRedraw = true
        } else {
            himark_frame_release(engine, frame)
        }
    }

    /// A tap in CONTENT coordinates: derive a fresh stage for the
    /// standing viewport, send down + up (a tap is a click), and
    /// let the next pass repaint whatever changed.
    @objc private func onTap(_ gesture: UITapGestureRecognizer) {
        guard bridge.raw != 0 else { return }
        let engine = host.engine.raw
        let scale = pixelScale
        let point = gesture.location(in: scroll)
        let x = Float(point.x * scale)
        let y = Float(point.y * scale)
        let wPx = Float(bounds.width * scale)
        let vhPx = Float(bounds.height * scale)
        let offsetPx = Float(max(scroll.contentOffset.y, 0) * scale)

        let layout = himark_bridge_layout(engine, bridge, wPx, Float.infinity)
        guard layout.raw != 0 else { return }
        let frame = himark_layout_realize(engine, layout, 0, offsetPx, wPx, vhPx)
        guard frame.raw != 0 else { return }

        let down = himark_event_mouse_down(x, y, 0, 1)
        var changed = himark_frame_event(engine, frame, down, 0, offsetPx, wPx, vhPx)
        himark_event_release(down)
        if !changed {
            let up = himark_event_mouse_up(x, y)
            changed = himark_frame_event(engine, frame, up, 0, offsetPx, wPx, vhPx)
            himark_event_release(up)
            if !changed { himark_frame_release(engine, frame) }
        }
        needsRedraw = true
    }
}

/// The band's drawable: a bare metal-layer view pinned to the
/// visible bounds.
final class StageRenderView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }
}
