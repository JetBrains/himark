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

    var body: some View {
        ZStack {
            WindowDresser().frame(width: 0, height: 0)

            // ── the split: chat column beside the document floor ──
            HStack(spacing: 0) {
                if chatOpen {
                    ChatPaneColumn(host: host)
                        .frame(width: 560)
                        .transition(.move(edge: .leading))
                    Rectangle()
                        .fill(.white.opacity(0.07))
                        .frame(width: 1)
                        .ignoresSafeArea()
                }
                Group {
                    if let opened = host.document {
                        DocumentPane(host: host, opened: opened)
                    } else {
                        EmptyDocumentHint(chatOpen: false, railOpen: railOpen)
                    }
                }
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
        .overlay(alignment: .topLeading) {
            if goToOpen {
                PlainMintedBridge(host: host, kind: .peeker, grabsFocus: true)
                    .id("peeker-\(host.current?.uri ?? "")")
                    .frame(width: 1_040)
                    .frame(maxHeight: .infinity)
                    .background(IslandBackground(cornerRadius: 14))
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .padding(.leading, 12)
                    .padding(.bottom, 12)
                    .padding(.top, 64)
                    .transition(.opacity.combined(with: .move(edge: .leading)))
                    .onExitCommand {
                        withAnimation(.easeOut(duration: 0.15)) { goToOpen = false }
                    }
            }
        }
        .overlay(alignment: .top) {
            if paletteOpen {
                CommandPalette(entries: paletteEntries) {
                    withAnimation(.easeOut(duration: 0.15)) { paletteOpen = false }
                }
                .padding(.top, 64)
                .transition(.opacity.combined(with: .scale(scale: 0.97, anchor: .top)))
            }
        }
        .overlay(alignment: .topTrailing) {
            RailIsland(
                host: host,
                open: railOpen,
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
        }
        .onChange(of: host.dismissals) { _ in
            withAnimation(.easeOut(duration: 0.15)) {
                goToOpen = false
                paletteOpen = false
            }
        }
        .onChange(of: host.document) { _ in
            withAnimation(.easeOut(duration: 0.15)) { goToOpen = false }
        }
        .frame(minWidth: 720, minHeight: 480)
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

    /// Native shell verbs first, the engine's registry after.
    private var paletteEntries: [PaletteEntry] {
        var entries: [PaletteEntry] = [
            PaletteEntry(id: "view.toggle-chat", name: "View: Toggle Chat", shortcut: "⌘1") {
                withAnimation(spring) { chatOpen.toggle() }
            },
            PaletteEntry(id: "view.toggle-rail", name: "View: Toggle Changes Rail", shortcut: "⌘2") {
                withAnimation(spring) { railOpen.toggle() }
            },
            PaletteEntry(id: "session.new", name: "Session: New Session", shortcut: "⌘N") {
                host.newSession()
            },
        ]
        let native = Set(entries.map(\.id))
        for (id, name) in host.engine.commands() where !native.contains(id) {
            entries.append(PaletteEntry(id: id, name: name, shortcut: nil) { [weak host] in
                guard let host else { return }
                _ = host.engine.perform(bridge: HimarkBridge(raw: 0), command: id)
            })
        }
        return entries
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
                SessionList(host: host, open: $open)
            }
        }
        .frame(width: open ? 536 : nil)
        .frame(maxHeight: open ? 700 : 44, alignment: .top)
        .fixedSize(horizontal: !open, vertical: false)
        .background(IslandBackground(cornerRadius: open ? 16 : 14))
        .clipShape(RoundedRectangle(cornerRadius: open ? 16 : 14))
    }
}

enum SessionDates {
    static let plain = ISO8601DateFormatter()
    static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    static func parse(_ stamp: String) -> Date? {
        fractional.date(from: stamp) ?? plain.date(from: stamp)
    }
}

struct SessionRow: View {
    let row: HimarkEngine.CatalogSession
    let current: Bool
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title.isEmpty ? "Untitled" : row.title)
                        .font(.system(size: 13, weight: current ? .semibold : .regular))
                        .lineLimit(1)
                    Text(meta)
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
                Spacer()
                if current {
                    Circle().fill(.yellow).frame(width: 6, height: 6)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(
                current ? AnyShapeStyle(.white.opacity(0.07)) : AnyShapeStyle(.clear),
                in: RoundedRectangle(cornerRadius: 8))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private var meta: String {
        guard let date = SessionDates.parse(row.modified) else {
            return row.provider
        }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter.localizedString(for: date, relativeTo: .now)
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

/// The sessions face (himark-islands-sessions-open): new-session
/// row, then the catalog grouped by day.
struct SessionList: View {
    @ObservedObject var host: DesktopHost
    @Binding var open: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                Button {
                    host.newSession()
                    close()
                } label: {
                    HStack {
                        Label("New session", systemImage: "plus")
                            .font(.system(size: 13, weight: .medium))
                        Spacer()
                        Text("⌘N").foregroundStyle(.tertiary)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .keyboardShortcut("n", modifiers: .command)

                ForEach(groups, id: \.title) { group in
                    Text(group.title.uppercased())
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 12)
                        .padding(.top, 14)
                        .padding(.bottom, 4)
                    ForEach(group.rows, id: \.uri) { row in
                        SessionRow(
                            row: row,
                            current: row.uri == host.current?.uri
                        ) {
                            host.select(row)
                            close()
                        }
                    }
                }
            }
            .padding(10)
        }
    }

    private func close() {
        withAnimation(.spring(response: 0.3, dampingFraction: 0.85)) { open = false }
    }

    private struct Group {
        let title: String
        let rows: [HimarkEngine.CatalogSession]
    }

    private var groups: [Group] {
        let calendar = Calendar.current
        var today: [HimarkEngine.CatalogSession] = []
        var yesterday: [HimarkEngine.CatalogSession] = []
        var earlier: [HimarkEngine.CatalogSession] = []
        for row in host.sessions {
            let date = SessionDates.parse(row.modified)
            if let date, calendar.isDateInToday(date) {
                today.append(row)
            } else if let date, calendar.isDateInYesterday(date) {
                yesterday.append(row)
            } else {
                earlier.append(row)
            }
        }
        return [
            Group(title: "Today", rows: today),
            Group(title: "Yesterday", rows: yesterday),
            Group(title: "Earlier", rows: earlier),
        ].filter { !$0.rows.isEmpty }
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
    let open: Bool
    let tool: RailTool
    let pick: (RailTool) -> Void

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
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)

            if open {
                RailContent(host: host, tool: tool)
            }
        }
        .frame(width: open ? 320 : nil)
        .frame(maxHeight: open ? .infinity : nil, alignment: .top)
        .background(IslandBackground())
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }
}

struct RailContent: View {
    let host: DesktopHost
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
            MintedScrollBridge(host: host, kind: .transcript, pinsToBottom: true)
            // The composer rides in its OWN rounded well, the
            // mock's square island — chrome in Swift, content in
            // the engine.
            ComposerBridge(host: host, height: $composerHeight)
                .frame(height: composerHeight)
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
        if opened.kind == UInt32(HIMARK_PLACE_CANVAS) {
            // The canvas scrolls ITSELF — no native scroller around it.
            PlainPlaceBridge(host: host, place: opened.place)
                .id(opened.id)
        } else {
            PlaceScrollBridge(host: host, place: opened.place)
                .id(opened.id)
        }
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
