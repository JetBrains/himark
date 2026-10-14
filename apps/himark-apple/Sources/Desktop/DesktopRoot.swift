// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import AppKit

/// The ISLANDS window (docs/himark-islands-*.png): the document
/// content runs FULL-BLEED behind everything; the chat lives in a
/// full-height glass island on the left (traffic lights in its
/// header), the tool rail in an island on the right, and a floating
/// pill names the open document. Collapsed, the chat leaves a
/// semaphore mini-island top-left and a message capsule bottom-left;
/// the rail leaves its icon strip. Every change of shape animates.
struct DesktopRoot: View {
    @ObservedObject var host: DesktopHost
    // Screenshot-loop hooks: HIMARK_UI=chat-collapsed|rail-collapsed|sessions
    @State private var chatOpen =
        ProcessInfo.processInfo.environment["HIMARK_UI"] != "chat-collapsed"
    @State private var sessionsOpen =
        ProcessInfo.processInfo.environment["HIMARK_UI"] == "sessions"
    @State private var railOpen =
        ProcessInfo.processInfo.environment["HIMARK_UI"] != "rail-collapsed"
    @State private var railTool: RailTool = {
        switch ProcessInfo.processInfo.environment["HIMARK_UI"] {
        case "rail-search": .search
        case "rail-history": .history
        case "rail-comments": .comments
        case "rail-files": .files
        default: .changes
        }
    }()
    @State private var paletteOpen =
        ProcessInfo.processInfo.environment["HIMARK_UI"] == "palette"
    @State private var goToOpen =
        ProcessInfo.processInfo.environment["HIMARK_UI"] == "go-to-file"
    @Namespace private var islands
    /// The system appearance drives BOTH halves: the engine's theme
    /// (theme.json / theme-light.json carry the content) and the
    /// Swift glass around it, tuned to the light mocks
    /// (docs/himark-islands-*-light.png).
    @Environment(\.colorScheme) private var colorScheme
    private var dark: Bool { colorScheme == .dark }

    private var spring: Animation { .spring(response: 0.35, dampingFraction: 0.85) }

    /// Above this window width the open rail stops FLOATING over
    /// the document and becomes a SPLIT — the document ends at its
    /// edge, the card's glass goes behind-window (the modern mac
    /// sidebar: same inset rounded panel, desktop showing through).
    /// Below it, today's overlay. The rail itself never moves or
    /// remounts at the crossing; only the document's right edge and
    /// the backdrop change.
    private static let railSplitAt: CGFloat = 1280
    @State private var windowWidth: CGFloat = 0
    private var railSplit: Bool {
        railOpen && windowWidth >= Self.railSplitAt
    }

    /// The open chat column's width — what the chat is laid at
    /// whether it is showing or folded. The splitter drags it.
    @State private var chatWidth: CGFloat = 560
    @State private var chatDragBase: CGFloat?

    var body: some View {
        ZStack {
            WindowDresser().frame(width: 0, height: 0)

            // ── the split: chat column beside the document floor ──
            HStack(spacing: 0) {
                // The chat is ALWAYS mounted and ALWAYS laid at its
                // open width. Folding it used to remove it from the
                // tree, so every reopen built the transcript and
                // composer bridges again from nothing; and a width
                // that animates is a width the engine re-lays on
                // every frame of the spring. The content keeps its
                // target width throughout — only the frame around it
                // animates, and clips. Same law as the rail
                // (docs/mounted-parity.md).
                HStack(spacing: 0) {
                    ChatPaneColumn(host: host)
                        .frame(width: chatWidth)
                    Rectangle()
                        .fill(dark ? Color.white.opacity(0.07) : Color.black.opacity(0.08))
                        .frame(width: 1)
                }
                .frame(width: chatOpen ? chatWidth + 1 : 0, alignment: .leading)
                .clipped()
                .opacity(chatOpen ? 1 : 0)
                // The SPLITTER'S HAND: the line is one pixel, the
                // grab zone is not — a strip straddling the boundary
                // drags the column's width.
                .overlay(alignment: .trailing) {
                    if chatOpen {
                        SplitterHandle(
                            drag: { delta in
                                let base = chatDragBase ?? chatWidth
                                chatDragBase = base
                                chatWidth = min(max(base + delta, 360), 840)
                            },
                            ended: { chatDragBase = nil })
                            .frame(width: 10)
                            .offset(x: 5)
                    }
                }
                // The chat floor's GLASS — behind the clip, not in
                // it: a backdrop inside .clipped() rasterizes to
                // flat paint. Sized by the folding frame, so it
                // folds with the column.
                .background {
                    ZStack {
                        DesktopGlass(material: .sidebar)
                        (dark
                            ? Color(red: 0.036, green: 0.036, blue: 0.046)
                            : Color(red: 0.918, green: 0.92, blue: 0.93))
                            .opacity(0.5)
                    }
                    .ignoresSafeArea()
                }
                Group {
                    if let opened = host.document {
                        DocumentPane(host: host, opened: opened)
                    } else {
                        EmptyDocumentHint(chatOpen: false, railOpen: railOpen)
                    }
                }
                // SPLIT: the document ends at the rail's edge — the
                // rail's rect is identical in both modes, so the
                // crossing moves the document's edge and nothing
                // else. The splitter marks the edge the way a real
                // split view would.
                .overlay(alignment: .trailing) {
                    if railSplit {
                        Rectangle()
                            .fill(.primary.opacity(0.12))
                            .frame(width: 1)
                            .ignoresSafeArea()
                    }
                }
                .padding(.trailing, railSplit ? RailIsland.railWidth + 24 : 0)
            }
            .ignoresSafeArea()
        }
        .overlay(alignment: .topLeading) {
            // THE island: semaphore + session selector, always
            // afloat — it GROWS into the session list.
            SessionIsland(
                host: host,
                open: $sessionsOpen,
                chatOpen: $chatOpen)
                .padding(12)
        }
        .overlay(alignment: .bottomLeading) {
            if !chatOpen {
                MessageCapsule(host: host) {
                    withAnimation(spring) { chatOpen = true }
                }
                .transition(.move(edge: .leading).combined(with: .opacity))
                .padding(12)
            }
        }
        .overlay(alignment: .top) {
            if let opened = host.document {
                DocumentPill(host: host, opened: opened)
                    .padding(.top, 10)
            }
        }
        .overlay(alignment: .topTrailing) {
            // The island needs a CONCRETE height to hold its content
            // at: a content frame that merely fills what it is
            // offered gets re-proposed — and so re-laid — on every
            // frame of the spring.
            GeometryReader { room in
                RailIsland(
                    host: host,
                    room: room.size.height - 24,
                    open: railOpen,
                    split: railSplit,
                    tool: railTool,
                    pick: { tool in
                        withAnimation(spring) {
                            if railOpen && tool == railTool {
                                railOpen = false
                            } else {
                                railTool = tool
                                railOpen = true
                            }
                        }
                    })
                    .padding(12)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
            }
        }
        .overlay(alignment: .topLeading) {
            if goToOpen {
                // The mock's go-to is a near-full-window glass
                // sheet below the toolbar, not a corner island —
                // and the TOPMOST layer: it covers the rail.
                PlainMintedBridge(host: host, kind: .peeker, grabsFocus: true)
                    .id("peeker-\(host.current?.uri ?? "")")
                    .padding(6)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .background(IslandBackground(cornerRadius: 16))
                    .padding([.leading, .trailing, .bottom], 16)
                    .padding(.top, 56)
                    .transition(.opacity.combined(with: .scale(scale: 0.98, anchor: .topLeading)))
                    .onExitCommand {
                        withAnimation(.easeOut(duration: 0.15)) { goToOpen = false }
                    }
            }
        }
        .overlay(alignment: .top) {
            if paletteOpen {
                // The palette is the ENGINE's — the registry walk,
                // the query, the filtered list and the keys all live
                // in `plugins/palette`, which the workbench opens as
                // a modal and this shell mounts as a place. Swift
                // contributes the glass and nothing else.
                PlainMintedBridge(host: host, kind: .palette, grabsFocus: true)
                    // Re-mint when the session arrives: a mint
                    // before the catalog settles has no session to
                    // name and answers bridge 0 (the peeker's id
                    // carries the same weight).
                    .id("palette-\(host.current?.uri ?? "")")
                    .frame(width: 620, height: 460)
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .background(IslandBackground(cornerRadius: 16))
                    .padding(.top, 64)
                    .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .top)))
                    .onExitCommand {
                        withAnimation(.easeOut(duration: 0.15)) { paletteOpen = false }
                    }
            }
        }
        .onAppear { host.setTheme(dark: dark) }
        .onChange(of: colorScheme) { scheme in
            host.setTheme(dark: scheme == .dark)
        }
        .onChange(of: host.dismissals) { _ in
            withAnimation(.easeOut(duration: 0.15)) {
                goToOpen = false
                paletteOpen = false
                sessionsOpen = false
            }
        }
        .onChange(of: host.document) { _ in
            withAnimation(.easeOut(duration: 0.15)) { goToOpen = false }
        }
        .frame(minWidth: 720, minHeight: 480)
        .background {
            // The mode input: the window's width, nothing else.
            GeometryReader { geo in
                Color.clear
                    .onAppear { windowWidth = geo.size.width }
                    .onChange(of: geo.size.width) { width in windowWidth = width }
            }
        }
        // The light floor CONTINUES the document's white — the strip
        // behind the split rail is the same paper, not a grey band.
        .background(
            dark
                ? Color(red: 0.043, green: 0.043, blue: 0.055)
                : Color(red: 0.995, green: 0.995, blue: 0.995))
        .ignoresSafeArea()
        .background {
            // The palette's and the peeker's summons.
            Button("") {
                withAnimation(.easeOut(duration: 0.15)) { paletteOpen.toggle() }
            }
            .keyboardShortcut("p", modifiers: [.command, .shift])
            .hidden()
            Button("") {
                withAnimation(.easeOut(duration: 0.15)) { goToOpen.toggle() }
            }
            .keyboardShortcut("p", modifiers: .command)
            .hidden()
            // The mock's semaphore island carries no fold button —
            // the chat folds from the keyboard.
            Button("") { withAnimation(spring) { chatOpen.toggle() } }
                .keyboardShortcut("1", modifiers: .command)
                .hidden()
        }
    }

}

// ─── window dressing: no titlebar, no native buttons ───

/// The islands OWN the window chrome: full-size content, hidden
/// titlebar, native buttons hidden — the chat island header draws
/// the semaphore and performs the window verbs.
struct WindowDresser: NSViewRepresentable {
    final class Dressing: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window else { return }
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.styleMask.insert(.fullSizeContentView)
            window.isMovableByWindowBackground = true
            for kind: NSWindow.ButtonType in [.closeButton, .miniaturizeButton, .zoomButton] {
                window.standardWindowButton(kind)?.isHidden = true
            }
            // Harness hook: HIMARK_APPEARANCE=light|dark pins the
            // window appearance — the screenshot loop runs on a
            // machine whose system setting it cannot change.
            switch ProcessInfo.processInfo.environment["HIMARK_APPEARANCE"] {
            case "light": window.appearance = NSAppearance(named: .aqua)
            case "dark": window.appearance = NSAppearance(named: .darkAqua)
            default: break
            }
            // Harness hook: HIMARK_DEBUG_FRAME="w,h" squeezes the
            // window at launch (probe loop has no assistive access).
            if let ask = ProcessInfo.processInfo.environment["HIMARK_DEBUG_FRAME"] {
                let parts = ask.split(separator: ",").compactMap { Double($0) }
                if parts.count == 2 {
                    DispatchQueue.main.async {
                        window.setFrame(
                            NSRect(x: 0, y: 200, width: parts[0], height: parts[1]),
                            display: true)
                    }
                }
            }
        }
    }

    func makeNSView(context: Context) -> Dressing { Dressing() }
    func updateNSView(_ view: Dressing, context: Context) {}
}

/// The three dots, drawn by the island that hosts them.
struct TrafficDots: View {
    @Environment(\.controlActiveState) private var active

    var body: some View {
        HStack(spacing: 8) {
            dot(.init(red: 1.00, green: 0.38, blue: 0.35)) { NSApp.keyWindow?.performClose(nil) }
            dot(.init(red: 1.00, green: 0.78, blue: 0.25)) { NSApp.keyWindow?.miniaturize(nil) }
            dot(.init(red: 0.35, green: 0.80, blue: 0.36)) { NSApp.keyWindow?.zoom(nil) }
        }
    }

    private func dot(_ color: Color, verb: @escaping () -> Void) -> some View {
        Button(action: verb) {
            Circle()
                .fill(active == .key ? color : Color.primary.opacity(0.2))
                .frame(width: 12, height: 12)
        }
        .buttonStyle(.plain)
    }
}

// ─── THE island: semaphore + session selector ───

/// Floats over the chat column (or the editor when the chat is
/// collapsed). Closed: dots + the session pill. Open: it GROWS
/// into the session list — same island, more of it.
struct SessionIsland: View {
    @ObservedObject var host: DesktopHost
    @Binding var open: Bool
    @Binding var chatOpen: Bool

    private var spring: Animation { .spring(response: 0.35, dampingFraction: 0.85) }

    var body: some View {
        // Closed: dots, a rule, the session face, nothing else —
        // the mock's pill, at the ONE island radius every card
        // shares; open, it grows into the sessions card.
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                TrafficDots()
                Rectangle()
                    .fill(.primary.opacity(0.12))
                    .frame(width: 1, height: 12)
                SessionButton(host: host, open: $open)
                if open {
                    Spacer(minLength: 0)
                }
            }
            .padding(.leading, 16)
            .padding(.trailing, 18)
            .frame(height: 44)

            if open {
                // The sessions browser is the ENGINE's drawer — the
                // same hosts/folders/sessions tree, with the same
                // speed search, the workbench stands in a side slot.
                // The island paints the glass; none of the list is
                // written twice.
                PlainMintedBridge(host: host, kind: .sessions, grabsFocus: true)
                    .id("sessions-\(host.current?.uri ?? "")")
                    .frame(height: 656)
            }
        }
        .frame(width: open ? 536 : nil)
        .frame(maxHeight: open ? 700 : 44, alignment: .top)
        .fixedSize(horizontal: !open, vertical: false)
        .clipShape(RoundedRectangle(cornerRadius: IslandBackground.radius))
        .background(IslandBackground())
    }
}

/// "📦 New Session ⌄" — the session face toggle.
struct SessionButton: View {
    @ObservedObject var host: DesktopHost
    @Binding var open: Bool

    var body: some View {
        Button {
            withAnimation(.spring(response: 0.3, dampingFraction: 0.85)) { open.toggle() }
        } label: {
            // Flat in the capsule — the mock draws no pill within
            // the pill.
            HStack(spacing: 7) {
                Image(systemName: "shippingbox")
                    .foregroundStyle(.secondary)
                Text(host.current.map { title($0.title) } ?? "New Session")
                    .font(.system(size: 13, weight: .medium))
                Image(systemName: open ? "chevron.up" : "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
                    .padding(.leading, 3)
            }
            .padding(.vertical, 5)
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }

    private func title(_ title: String) -> String {
        title.isEmpty ? "New Session" : String(title.prefix(34))
    }
}

/// The chat SPLIT: a plain column — transcript under the floating
/// island, composer at its measured height.
struct ChatPaneColumn: View {
    @ObservedObject var host: DesktopHost

    var body: some View {
        ChatColumn(host: host)
            .id(host.current?.uri)
            .padding(.top, 60)
    }
}

/// "💬 Message the agent" — the folded chat.
struct MessageCapsule: View {
    @ObservedObject var host: DesktopHost
    let expand: () -> Void

    var body: some View {
        Button(action: expand) {
            HStack(spacing: 8) {
                Image(systemName: "bubble.left")
                Text("Message the agent")
                    .font(.system(size: 13, weight: .medium))
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(IslandBackground(cornerRadius: 22))
        }
        .buttonStyle(.plain)
    }
}

// ─── the rail island ───

enum RailTool: CaseIterable {
    case changes, files, search, history, comments

    var symbol: String {
        switch self {
        case .changes: "list.bullet.indent"
        case .files: "doc.text"
        case .search: "magnifyingglass"
        case .history: "clock.arrow.circlepath"
        case .comments: "bubble.left"
        }
    }
}

struct RailIsland: View {
    @ObservedObject var host: DesktopHost
    /// The height available to the island — the content is held at
    /// this whether the rail is showing it or not.
    let room: CGFloat
    let open: Bool
    /// SPLIT: the document ends at this island's edge instead of
    /// running behind it, and the glass goes behind-window. The
    /// island's own rect is the same either way.
    let split: Bool
    let tool: RailTool
    let pick: (RailTool) -> Void

    /// The open rail's width — the content is laid at this always.
    static let railWidth: CGFloat = 320
    private var railWidth: CGFloat { Self.railWidth }
    private let stripHeight: CGFloat = 46

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ForEach(RailTool.allCases, id: \.self) { candidate in
                    Button {
                        pick(candidate)
                    } label: {
                        Image(systemName: candidate.symbol)
                            .font(.system(size: 13))
                            .foregroundStyle(
                                open && candidate == tool
                                    ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
                            .frame(maxWidth: open ? .infinity : nil)
                            .frame(width: open ? nil : 38, height: 30)
                            .background(
                                open && candidate == tool
                                    ? AnyShapeStyle(.primary.opacity(0.1))
                                    : AnyShapeStyle(.clear),
                                in: RoundedRectangle(cornerRadius: 8))
                            // The WHOLE button answers the press,
                            // not just the glyph's pixels.
                            .contentShape(RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)

            // Folded, the content must contribute ZERO size in BOTH
            // axes — the bridge view fills whatever width it is
            // proposed, and an unbounded proposal would stretch the
            // collapsed pill across the window. It stays MOUNTED
            // through the fold (the growth animates over live rows).
            //
            // A sidebar SLIDES; it does not re-wrap. Animating the
            // content's own width handed it a new size on every
            // frame of the spring, so every frame re-wrapped a tree
            // that should simply have moved — the shake. The inner
            // frame is constant, open or folded; only the outer one
            // animates, and clips. The view is retained and laid at
            // one size throughout, and the spring just moves a clip
            // over it. Folded, the outer frame is zero in both axes,
            // so the collapsed pill still contributes nothing.
            RailContent(host: host, tool: tool)
                .frame(width: railWidth, height: max(room - stripHeight, 1))
                .frame(
                    width: open ? railWidth : 0,
                    height: open ? max(room - stripHeight, 1) : 0,
                    alignment: .topLeading)
                .clipped()
                .opacity(open ? 1 : 0)
        }
        .frame(width: open ? railWidth : nil)
        // The CONTENT clips to the card shape; the backdrop sits
        // outside the clip and rounds itself (the glass dies inside
        // a SwiftUI clip). Same island either way — split only
        // changes what the glass samples: the desktop (nothing of
        // ours behind) instead of the document it floats on.
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .background(
            IslandBackground(cornerRadius: 16, backdrop: split ? .desktop : .content))
    }
}

/// A splitter's GRAB STRIP, in AppKit: the window is movable by
/// its background and a SwiftUI hit area still counts as
/// background — dragging the splitter dragged the whole window. An
/// NSView refuses that claim (`mouseDownCanMoveWindow`), carries
/// the resize cursor, and hands the drag delta back.
struct SplitterHandle: NSViewRepresentable {
    let drag: (CGFloat) -> Void
    let ended: () -> Void

    final class Handle: NSView {
        var drag: ((CGFloat) -> Void)?
        var ended: (() -> Void)?
        private var startX: CGFloat = 0

        override var mouseDownCanMoveWindow: Bool { false }

        override func resetCursorRects() {
            addCursorRect(bounds, cursor: .resizeLeftRight)
        }

        override func mouseDown(with event: NSEvent) {
            startX = NSEvent.mouseLocation.x
        }

        override func mouseDragged(with event: NSEvent) {
            drag?(NSEvent.mouseLocation.x - startX)
        }

        override func mouseUp(with event: NSEvent) {
            ended?()
        }
    }

    func makeNSView(context: Context) -> Handle {
        let view = Handle()
        view.drag = drag
        view.ended = ended
        return view
    }

    func updateNSView(_ view: Handle, context: Context) {
        view.drag = drag
        view.ended = ended
    }
}

/// BEHIND-WINDOW glass: the desktop blurred into a panel, the
/// modern mac sidebar's backdrop. Must never sit inside a SwiftUI
/// clip, mask or opacity — those rasterize the layer and the
/// backdrop sampling dies; it rounds itself via `maskImage`.
struct DesktopGlass: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .sidebar
    var cornerRadius: CGFloat = 0

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = .behindWindow
        view.state = .active
        if cornerRadius > 0 {
            view.maskImage = Self.mask(radius: cornerRadius)
        }
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {
        // The effect view follows the SYSTEM appearance on its own
        // schedule — pin it to the scheme the rest of the chrome is
        // drawn with, so the glass and the text flip together.
        view.appearance = NSAppearance(
            named: context.environment.colorScheme == .dark ? .darkAqua : .aqua)
    }

    /// A stretchable rounded-rect mask — the vibrancy region obeys
    /// the island's shape (a SwiftUI clip does not reach the
    /// backdrop sampling).
    private static func mask(radius: CGFloat) -> NSImage {
        let side = radius * 2 + 1
        let image = NSImage(
            size: NSSize(width: side, height: side),
            flipped: false
        ) { rect in
            NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
            return true
        }
        image.capInsets = NSEdgeInsets(top: radius, left: radius, bottom: radius, right: radius)
        image.resizingMode = .stretch
        return image
    }
}

struct RailContent: View {
    // OBSERVED, not just held: the bridges below are keyed on the
    // current session, and a plain reference leaves this body
    // skipped when the session changes (same pointer, same tool) —
    // the keys were never re-read, so a session pick left every
    // rail face on the old repo until a tab switch rebuilt them.
    @ObservedObject var host: DesktopHost
    let tool: RailTool

    var body: some View {
        switch tool {
        case .changes:
            // Desktop rows open the CANVAS (user law).
            MintedScrollBridge(host: host, kind: .changesCanvas)
                .id("\(host.current?.uri ?? "")-changes")
        case .files:
            MintedScrollBridge(host: host, kind: .files)
                .id("\(host.current?.uri ?? "")-files")
        case .search:
            MintedScrollBridge(host: host, kind: .search)
                .id("\(host.current?.uri ?? "")-search")
        case .history:
            MintedScrollBridge(host: host, kind: .history)
                .id("\(host.current?.uri ?? "")-history")
        case .comments:
            MintedScrollBridge(host: host, kind: .comments)
                .id("\(host.current?.uri ?? "")-comments")
        }
    }
}

// ─── shared island dressings ───

/// THE island look — ONE backdrop for every floating card: the
/// rail, the composer, the pills, the sheets. Glass below, a wash
/// of the scheme's paper over it (white in light — the wash sets
/// the tone, the glass only bleeds what is behind), a hairline and
/// a soft shadow. `.desktop` samples BEHIND THE WINDOW (a split
/// sidebar, the composer over the chat floor); `.content` blurs
/// the document the island floats on — a floating card showing the
/// desktop through a window with content under it reads as a hole.
/// Self-rounded: never put it inside a clip or an opacity, those
/// rasterize the glass to flat paint.
struct IslandBackground: View {
    /// THE island radius — every island wears this one; islands
    /// with different corners stop reading as the same family.
    static let radius: CGFloat = 16

    enum Backdrop { case content, desktop }
    var cornerRadius: CGFloat = IslandBackground.radius
    var backdrop: Backdrop = .content
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let dark = scheme == .dark
        ZStack {
            switch backdrop {
            case .content:
                RoundedRectangle(cornerRadius: cornerRadius)
                    .fill(.ultraThinMaterial)
            case .desktop:
                DesktopGlass(material: .sidebar, cornerRadius: cornerRadius)
            }
            RoundedRectangle(cornerRadius: cornerRadius)
                .fill(
                    dark
                        ? Color(red: 0.08, green: 0.08, blue: 0.1).opacity(0.35)
                        : Color.white.opacity(0.28))
        }
        .overlay(
            RoundedRectangle(cornerRadius: cornerRadius)
                .strokeBorder(.primary.opacity(0.08)))
        .shadow(color: .black.opacity(dark ? 0.45 : 0.16), radius: 22, y: 8)
    }
}

/// Transcript over the composer — both himark-rendered.
struct ChatColumn: View {
    let host: DesktopHost
    @Environment(\.colorScheme) private var scheme
    @State private var composerHeight: CGFloat = 120

    var body: some View {
        VStack(spacing: 0) {
            MintedScrollBridge(host: host, kind: .transcript)
            // The composer rides in its OWN rounded well, the
            // mock's square island — chrome in Swift, content in
            // the engine.
            ComposerBridge(host: host, height: $composerHeight)
                .frame(height: composerHeight)
                // The composer is an ISLAND — the same card as the
                // rail, glass and all.
                .clipShape(RoundedRectangle(cornerRadius: IslandBackground.radius))
                .background(IslandBackground(backdrop: .desktop))
                .padding(.horizontal, 12)
                .padding(.bottom, 12)
                .padding(.top, 6)
        }
    }
}

/// The center pane: full-bleed document or diff content.
struct DocumentPane: View {
    let host: DesktopHost
    let opened: DesktopHost.Opened
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        Group {
            // EVERY landing rides the scrolled host — the canvas was
            // descrollered with the rest (docs/mounted-parity.md):
            // content views own no scroll, this scroller owns it.
            PlaceScrollBridge(host: host, place: opened.place, grabsFocus: opened.focus)
                .id(opened.id)
        }
        // The engine floors nothing now — the pane does; the color
        // is the active theme's paper.
        .background(
            scheme == .dark
                ? Color(red: 0.051, green: 0.051, blue: 0.063)
                : Color(red: 0.995, green: 0.995, blue: 0.995))
    }
}

/// The floating glass pill naming the open document.
struct DocumentPill: View {
    let host: DesktopHost
    let opened: DesktopHost.Opened

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: opened.kind == UInt32(HIMARK_PLACE_DOCUMENT)
                ? "doc.text" : "plus.forwardslash.minus")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Text(opened.title.isEmpty ? "Untitled" : opened.title)
                .font(.system(size: 12, design: .monospaced))
            Button {
                host.closedDocument(opened)
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(IslandBackground())
    }
}

struct EmptyDocumentHint: View {
    let chatOpen: Bool
    let railOpen: Bool

    var body: some View {
        HStack(spacing: 0) {
            if chatOpen { Spacer().frame(width: 584) }
            VStack(spacing: 10) {
                Image(systemName: "doc.badge.plus")
                    .font(.system(size: 34, weight: .light))
                    .foregroundStyle(.tertiary)
                Text("No file open")
                    .font(.title3.weight(.semibold))
                    .foregroundStyle(.secondary)
                Text("Pick a change on the right, or ask the agent to open something.")
                    .font(.callout)
                    .foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            if railOpen { Spacer().frame(width: 344) }
        }
    }
}
