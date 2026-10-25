// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import UIKit

/// The phone (the iPhone artboards of the typographic redesign):
/// chrome is WORDS and HAIRLINES — no glass, no pills, no icons. A
/// 44pt bar on top where there is something to say (the session and
/// New over the chat; the way back over a pushed screen), four
/// section words along the bottom in place of the tab bar, and
/// between them the CONTENT, every piece of it rendered
/// by the engine through a session-scoped bridge in the phone's own
/// theme pair. Every size and ink the native words wear comes from
/// that pair's `shell` section (`ShellTheme`).
struct RootView: View {
    @ObservedObject var host: AppHost
    @Environment(\.colorScheme) private var scheme
    @StateObject private var keyboard = KeyboardWatcher()

    // The dev screenshot road: pick the starting section by env.
    @State private var section: AppSection =
        AppSection(rawValue: ProcessInfo.processInfo.environment["HIMARK_START_TAB"] ?? "")
        ?? .chat
    /// The sections seen so far: each mounts on its first visit and
    /// then STAYS (hidden, not torn down), so coming back finds the
    /// bridge, its scroll position and any pushed screen in place.
    @State private var visited: Set<AppSection> = []

    var body: some View {
        let theme = host.shell
        ZStack {
            ForEach(AppSection.allCases) { section in
                if visited.contains(section) {
                    NavigationStack { section.screen(host: host) }
                        .opacity(section == self.section ? 1 : 0)
                        .allowsHitTesting(section == self.section)
                        .zIndex(section == self.section ? 1 : 0)
                }
            }
        }
        // The nav takes its band off the stacks as PADDING, not as a
        // safe-area inset: a NavigationStack keeps UIKit's own insets
        // and would lay its content under the words. The words make
        // way for the keyboard: a composer writes against the keys,
        // not against a bar hanging over them.
        .padding(.bottom, keyboard.up ? 0 : theme.navHeight)
        .overlay(alignment: .bottom) {
            if !keyboard.up {
                SectionNav(theme: theme, current: $section)
            }
        }
        .background(theme.ground.ignoresSafeArea())
        .onAppear {
            visited.insert(section)
            host.kickoff()
            host.follow(appearance: scheme)
        }
        .onChange(of: section) { visited.insert($0) }
        .onChange(of: scheme) { host.follow(appearance: $0) }
    }
}

/// The four sections, in the nav's order.
enum AppSection: String, CaseIterable, Identifiable {
    case chat, changes, history, files

    var id: String { rawValue }

    /// The nav's word.
    var word: String {
        switch self {
        case .chat: "Chat"
        case .changes: "Changes"
        case .history: "History"
        case .files: "Files"
        }
    }

    /// The tab a push lands on for this section — the split chat's
    /// halves push on the chat tab.
    var tab: HimarkEngine.SessionViewKind {
        switch self {
        case .chat: .chat
        case .changes: .changes
        case .history: .history
        case .files: .files
        }
    }

    @ViewBuilder
    func screen(host: AppHost) -> some View {
        switch self {
        case .chat: ChatScreen(host: host)
        case .changes: ChangesScreen(host: host)
        case .history: HistoryScreen(host: host)
        case .files: FilesScreen(host: host)
        }
    }
}

/// Whether the keyboard stands — the nav yields to it.
final class KeyboardWatcher: ObservableObject {
    @Published var up = false
    private var observers: [NSObjectProtocol] = []

    init() {
        let center = NotificationCenter.default
        observers = [
            center.addObserver(
                forName: UIResponder.keyboardWillShowNotification, object: nil, queue: .main
            ) { [weak self] _ in self?.up = true },
            center.addObserver(
                forName: UIResponder.keyboardWillHideNotification, object: nil, queue: .main
            ) { [weak self] _ in self?.up = false },
        ]
    }

    deinit {
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
    }
}

// ─── the words ───

/// One line of text in a ramp role and an ink.
struct Word: View {
    let theme: ShellTheme
    let role: ShellTheme.Role
    let ink: Color
    let text: String

    var body: some View {
        Text(text)
            .font(theme.font(role))
            .tracking(role.tracking)
            .foregroundStyle(ink)
            .lineLimit(1)
    }
}

/// A hairline, the bar's and the nav's only drawn chrome.
struct Hairline: View {
    let theme: ShellTheme

    var body: some View {
        Rectangle().fill(theme.rule).frame(height: theme.hairline)
    }
}

/// The 44pt BAR: what leads (a back word or the context), the title,
/// one action word at the trailing edge, a hairline under it all.
struct WordBar<Leading: View, Trailing: View>: View {
    let theme: ShellTheme
    @ViewBuilder let leading: () -> Leading
    @ViewBuilder let trailing: () -> Trailing

    var body: some View {
        HStack(spacing: theme.wordGap) {
            leading()
            Spacer(minLength: 0)
            trailing()
        }
        .padding(.horizontal, theme.inset)
        .frame(maxWidth: .infinity)
        .frame(height: theme.barHeight)
        .overlay(alignment: .bottom) { Hairline(theme: theme) }
        .background(theme.ground)
    }
}

/// A bar's ACTION — a word in the quiet ink, the whole bar's height
/// to press; it reaches past the inset so the word itself keeps the
/// edge (the artboards' negative margin).
struct ActionWord: View {
    let theme: ShellTheme
    let text: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Word(theme: theme, role: theme.word, ink: theme.ink3, text: text)
                .padding(.horizontal, theme.actionPad)
                .frame(height: theme.barHeight)
        }
        .buttonStyle(.plain)
        .padding(.trailing, -theme.actionPad)
    }
}

/// The BACK word: "‹ Files" — the section the screen came from.
struct BackWord: View {
    let theme: ShellTheme
    let text: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Word(theme: theme, role: theme.word, ink: theme.ink3, text: "‹ \(text)")
                .padding(.horizontal, theme.actionPad)
                .frame(height: theme.barHeight)
        }
        .buttonStyle(.plain)
        .padding(.leading, -theme.actionPad)
    }
}

/// The bottom NAV: four words, equal columns, the current one by
/// weight and ink alone; a hairline over them, the home indicator's
/// own air under (the safe area).
struct SectionNav: View {
    let theme: ShellTheme
    @Binding var current: AppSection

    var body: some View {
        HStack(spacing: 0) {
            ForEach(AppSection.allCases) { section in
                let here = section == current
                Button {
                    current = section
                } label: {
                    Word(
                        theme: theme,
                        role: here ? theme.tabActive : theme.tab,
                        ink: here ? theme.ink : theme.ink3,
                        text: section.word)
                    .frame(maxWidth: .infinity)
                    .frame(height: theme.navHeight)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, theme.navInset)
        .frame(height: theme.navHeight)
        .overlay(alignment: .top) { Hairline(theme: theme) }
        .background(theme.ground)
    }
}

// ─── the screens ───

/// The push binding for one tab: the engine's navigate callback
/// lands a `Push` for the tab; popping clears it and releases the
/// place handle.
private func pushBinding(_ host: AppHost, _ tab: HimarkEngine.SessionViewKind)
    -> Binding<Bool>
{
    Binding(
        get: { host.pushes[tab] != nil },
        set: { presented in
            if !presented, let standing = host.pushes[tab] {
                host.popped(standing)
            }
        }
    )
}

/// One tab's pushed destination — empty until the engine lands a
/// push for this tab.
private struct PushedDestination: View {
    @ObservedObject var host: AppHost
    let section: AppSection

    var body: some View {
        if let push = host.pushes[section.tab] {
            PlaceScreen(host: host, push: push, back: section.word)
        }
    }
}

/// The session's name leads the chat bar; pressing it lists the
/// catalog. "New" at the trailing edge starts a session.
struct ChatScreen: View {
    @ObservedObject var host: AppHost
    // A seed the engine's measure replaces on the first layout.
    @State private var composerHeight: CGFloat = 96

    var body: some View {
        let theme = host.shell
        VStack(spacing: 0) {
            WordBar(theme: theme) {
                SessionWord(host: host)
            } trailing: {
                ActionWord(theme: theme, text: "New") { host.newSession() }
            }
            ScrolledMintedBridge(host: host, kind: .transcript)
            ComposerBridge(host: host, height: $composerHeight)
                .frame(height: composerHeight)
        }
        .toolbar(.hidden, for: .navigationBar)
        .navigationDestination(isPresented: pushBinding(host, .chat)) {
            PushedDestination(host: host, section: .chat)
        }
    }
}

struct ChangesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        SectionScreen(host: host, section: .changes, kind: .changes)
    }
}

struct HistoryScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        SectionScreen(host: host, section: .history, kind: .history)
    }
}

struct FilesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        SectionScreen(host: host, section: .files, kind: .files)
    }
}

/// A section whose whole body is one natively scrolled session view.
/// No bar names it: the nav's standing word already does, so the
/// content starts right under the status bar.
private struct SectionScreen: View {
    @ObservedObject var host: AppHost
    let section: AppSection
    let kind: HimarkEngine.SessionViewKind

    var body: some View {
        ScrolledMintedBridge(host: host, kind: kind)
        .toolbar(.hidden, for: .navigationBar)
        .navigationDestination(isPresented: pushBinding(host, section.tab)) {
            PushedDestination(host: host, section: section)
        }
    }
}

/// The session's name as the chat bar's title, a menu of the catalog
/// under it.
struct SessionWord: View {
    @ObservedObject var host: AppHost

    var body: some View {
        let theme = host.shell
        Menu {
            if host.sessions.isEmpty {
                // An EMPTY Menu never presents — the word would read
                // as dead. Name the real state instead.
                Button("No sessions — is the agent host running?") {}
                    .disabled(true)
            }
            ForEach(host.sessions, id: \.uri) { row in
                Button {
                    host.select(row)
                } label: {
                    let title = row.title.isEmpty ? "Untitled" : row.title
                    if host.current?.uri == row.uri, host.current?.host == row.host {
                        Label(title, systemImage: "checkmark")
                    } else {
                        Text(title)
                    }
                }
            }
        } label: {
            Word(theme: theme, role: theme.title, ink: theme.ink, text: title)
                .frame(height: theme.barHeight)
        }
    }

    private var title: String {
        guard let current = host.current, !current.title.isEmpty else {
            return "New Session"
        }
        return current.title
    }
}

/// One tab's content: the bridge a place mints for the current
/// session, re-minted when the session switches and retried when
/// the catalog catches up (a fresh session's chat exists only
/// after its entry lands).
struct MintedBridge: UIViewRepresentable {
    @ObservedObject var host: AppHost
    let kind: HimarkEngine.SessionViewKind

    func makeUIView(context: Context) -> MintedBridgeHost {
        MintedBridgeHost(host: host, kind: kind)
    }

    func updateUIView(_ view: MintedBridgeHost, context: Context) {
        view.sync()
    }

    static func dismantleUIView(_ view: MintedBridgeHost, coordinator: ()) {
        view.teardown()
    }
}

/// The UIKit side of one tab: owns the minted bridge's lifecycle
/// and the `HimarkBridgeView` painting it.
final class MintedBridgeHost: UIView {
    private let host: AppHost
    private let kind: HimarkEngine.SessionViewKind

    private var mounted: AppHost.Selected?
    private var bridge = HimarkBridge(raw: 0)
    private var child: HimarkBridgeView?

    init(host: AppHost, kind: HimarkEngine.SessionViewKind) {
        self.host = host
        self.kind = kind
        super.init(frame: .zero)
        // The ground is the theme's, painted by SwiftUI beneath; the
        // engine's band covers it on the first paint.
        backgroundColor = .clear
        sync()
    }

    required init?(coder: NSCoder) { fatalError() }

    func sync() {
        let desired = host.current
        if mounted == desired, bridge.raw != 0 { return }
        teardown()
        mounted = desired
        guard desired != nil else { return }
        bridge = host.mint(kind)
        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, bridge: bridge, device: host.device)
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(view)
        host.register(view)
        child = view
    }

    func teardown() {
        if let child {
            host.unregister(child)
            child.removeFromSuperview()
        }
        child = nil
        if bridge.raw != 0 {
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }
}

/// A pushed place screen: the engine asked for this navigation;
/// the bridge mints from the handed place and the handle releases
/// when the screen pops. Its bar leads with the way back.
struct PlaceScreen: View {
    @ObservedObject var host: AppHost
    let push: AppHost.Push
    let back: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let theme = host.shell
        VStack(spacing: 0) {
            WordBar(theme: theme) {
                BackWord(theme: theme, text: back) { dismiss() }
                Word(theme: theme, role: theme.title, ink: theme.ink, text: push.title)
                    .truncationMode(.middle)
            } trailing: {
                EmptyView()
            }
            // Every pushed place scrolls NATIVELY — documents, the
            // (scroller-free) inline diff and a commit's canvas alike.
            StagePlaceBridge(host: host, place: push.place)
        }
        .toolbar(.hidden, for: .navigationBar)
    }
}

struct StagePlaceBridge: UIViewRepresentable {
    let host: AppHost
    let place: HimarkPlace

    func makeUIView(context: Context) -> StageScrollHost {
        StageScrollHost(host: host, place: place)
    }

    func updateUIView(_ view: StageScrollHost, context: Context) {}

    static func dismantleUIView(_ view: StageScrollHost, coordinator: ()) {
        view.teardown()
    }
}

/// With the system bar hidden behind our word bar, the edge swipe
/// that pops a screen loses its delegate; seating none keeps the
/// phone's own way back.
extension UINavigationController {
    override open func viewDidLoad() {
        super.viewDidLoad()
        interactivePopGestureRecognizer?.delegate = nil
    }
}
