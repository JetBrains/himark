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
    /// whether it is showing or folded.
    private let chatWidth: CGFloat = 560

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
                        .fill(.white.opacity(0.07))
                        .frame(width: 1)
                }
                .frame(width: chatOpen ? chatWidth + 1 : 0, alignment: .leading)
                .clipped()
                .opacity(chatOpen ? 1 : 0)
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
                // else.
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
                    .background(IslandBackground(cornerRadius: 16))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
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
                    .background(IslandBackground(cornerRadius: 16))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .padding(.top, 64)
                    .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .top)))
                    .onExitCommand {
                        withAnimation(.easeOut(duration: 0.15)) { paletteOpen = false }
                    }
            }
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
        .background(Color(red: 0.043, green: 0.043, blue: 0.055))
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
                .fill(active == .key ? color : Color.white.opacity(0.2))
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
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                TrafficDots()
                Divider().frame(height: 14).overlay(.white.opacity(0.15))
                SessionButton(host: host, open: $open)
                if open || !chatOpen {
                    Spacer(minLength: 0)
                }
                if !open {
                    Button {
                        withAnimation(spring) { chatOpen.toggle() }
                    } label: {
                        Image(systemName: chatOpen
                            ? "rectangle.lefthalf.inset.filled.arrow.left"
                            : "rectangle.lefthalf.inset.filled")
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 14)
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
        .background(IslandBackground(cornerRadius: 16))
        .clipShape(RoundedRectangle(cornerRadius: 16))
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
            HStack(spacing: 6) {
                Image(systemName: "shippingbox")
                    .foregroundStyle(.secondary)
                Text(host.current.map { title($0.title) } ?? "New Session")
                    .font(.system(size: 13, weight: .medium))
                Image(systemName: open ? "chevron.up" : "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(.white.opacity(0.06), in: Capsule())
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
            .background(Color(red: 0.036, green: 0.036, blue: 0.046))
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
                                    ? AnyShapeStyle(.white.opacity(0.1))
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
        .background {
            // BOTH faces stay mounted and only trade opacity: the
            // crossing must not blink, and swapping views out of
            // the tree is a swap the eye can catch.
            ZStack {
                IslandBackground().opacity(split ? 0 : 1)
                SidebarGlass(cornerRadius: 16)
                    .overlay(
                        RoundedRectangle(cornerRadius: 16)
                            .strokeBorder(.white.opacity(0.08)))
                    .opacity(split ? 1 : 0)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }
}

/// The SPLIT backdrop: the modern mac sidebar's glass — the same
/// inset rounded panel, blurring what is BEHIND THE WINDOW, so with
/// nothing of the document under it the desktop shows through.
struct SidebarGlass: NSViewRepresentable {
    var cornerRadius: CGFloat = 16

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = .sidebar
        view.blendingMode = .behindWindow
        view.state = .active
        // The app paints its own dark theme; the effect view follows
        // the SYSTEM appearance unless pinned — a light sidebar
        // under white tree text is unreadable.
        view.appearance = NSAppearance(named: .darkAqua)
        view.maskImage = Self.mask(radius: cornerRadius)
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}

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

struct IslandBackground: View {
    var cornerRadius: CGFloat = 16

    var body: some View {
        RoundedRectangle(cornerRadius: cornerRadius)
            .fill(Color(red: 0.08, green: 0.08, blue: 0.1).opacity(0.55))
            .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: cornerRadius))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius)
                    .strokeBorder(.white.opacity(0.08)))
            .shadow(color: .black.opacity(0.45), radius: 22, y: 8)
    }
}

/// Transcript over the composer — both himark-rendered.
struct ChatColumn: View {
    let host: DesktopHost
    @State private var composerHeight: CGFloat = 120

    var body: some View {
        VStack(spacing: 0) {
            MintedScrollBridge(host: host, kind: .transcript)
            // The composer rides in its OWN rounded well, the
            // mock's square island — chrome in Swift, content in
            // the engine.
            ComposerBridge(host: host, height: $composerHeight)
                .frame(height: composerHeight)
                .background(
                    RoundedRectangle(cornerRadius: 12)
                        .fill(.white.opacity(0.045)))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .strokeBorder(.white.opacity(0.09)))
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

    var body: some View {
        Group {
            // EVERY landing rides the scrolled host — the canvas was
            // descrollered with the rest (docs/mounted-parity.md):
            // content views own no scroll, this scroller owns it.
            PlaceScrollBridge(host: host, place: opened.place, grabsFocus: opened.focus)
                .id(opened.id)
        }
        // The engine floors nothing now — the pane does.
        .background(Color(red: 0.051, green: 0.051, blue: 0.063))
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
        .background(IslandBackground(cornerRadius: 18))
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
