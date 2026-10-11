// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI
import UIKit

/// The native wireframe (design 2026-10-10): a session-centric
/// app — the picker up top, three tabs, native navigation and
/// glass — with every piece of CONTENT rendered by himark through
/// a session-scoped bridge. The composer stays the square imba
/// one inside the chat bridge.
struct RootView: View {
    @ObservedObject var host: AppHost

    // The dev screenshot road: pick the starting tab by env.
    @State private var tab: String =
        ProcessInfo.processInfo.environment["HIMARK_START_TAB"] ?? "chat"

    var body: some View {
        TabView(selection: $tab) {
            NavigationStack { ChatScreen(host: host) }
                .tabItem { Label("Chat", systemImage: "bubble.left") }
                .tag("chat")
            NavigationStack { ChangesScreen(host: host) }
                .tabItem { Label("Changes", systemImage: "arrow.triangle.branch") }
                .tag("changes")
            NavigationStack { FilesScreen(host: host) }
                .tabItem { Label("Files", systemImage: "folder") }
                .tag("files")
        }
        .onAppear { host.kickoff() }
        .preferredColorScheme(.dark)
    }
}

struct ChatScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        MintedBridge(host: host, kind: .chat)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    SessionPicker(host: host)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { host.newSession() } label: {
                        Image(systemName: "square.and.pencil")
                    }
                }
            }
    }
}

struct ChangesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        MintedBridge(host: host, kind: .changes)
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
    }
}

struct FilesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        MintedBridge(host: host, kind: .files)
            .navigationTitle("Files")
            .navigationBarTitleDisplayMode(.inline)
    }
}

/// The session picker — the nav title with a chevron, a menu of
/// the catalog underneath.
struct SessionPicker: View {
    @ObservedObject var host: AppHost

    var body: some View {
        Menu {
            ForEach(host.sessions, id: \.uri) { row in
                Button {
                    host.select(row)
                } label: {
                    if row.title.isEmpty {
                        Text("Untitled")
                    } else {
                        Text(row.title)
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Text(title)
                    .font(.headline)
                    .foregroundStyle(.primary)
                Image(systemName: "chevron.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(.secondary)
            }
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
        backgroundColor = .black
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
            host.engine.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }
}
