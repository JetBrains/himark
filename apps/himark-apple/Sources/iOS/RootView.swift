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
    let tab: HimarkEngine.SessionViewKind

    var body: some View {
        if let push = host.pushes[tab] {
            PlaceScreen(host: host, push: push)
        }
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
            .navigationDestination(isPresented: pushBinding(host, .chat)) {
                PushedDestination(host: host, tab: .chat)
            }
    }
}

struct ChangesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        MintedBridge(host: host, kind: .changes)
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(isPresented: pushBinding(host, .changes)) {
                PushedDestination(host: host, tab: .changes)
            }
    }
}

struct FilesScreen: View {
    @ObservedObject var host: AppHost

    var body: some View {
        MintedBridge(host: host, kind: .files)
            .navigationTitle("Files")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(isPresented: pushBinding(host, .files)) {
                PushedDestination(host: host, tab: .files)
            }
    }
}

/// The session picker — the nav title with a chevron, a menu of
/// the catalog underneath.
struct SessionPicker: View {
    @ObservedObject var host: AppHost

    var body: some View {
        Menu {
            if host.sessions.isEmpty {
                // An EMPTY Menu never presents — the picker would
                // read as dead. Name the real state instead.
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
            host.unmount(bridge: bridge)
            bridge = HimarkBridge(raw: 0)
        }
    }
}

/// A pushed place screen: the engine asked for this navigation;
/// the bridge mints from the handed place and the handle releases
/// when the screen pops.
struct PlaceScreen: View {
    @ObservedObject var host: AppHost
    let push: AppHost.Push

    var body: some View {
        PlaceBridge(host: host, place: push.place)
            .navigationTitle(push.title)
            .navigationBarTitleDisplayMode(.inline)
    }
}

struct PlaceBridge: UIViewRepresentable {
    let host: AppHost
    let place: HimarkPlace

    func makeUIView(context: Context) -> PlaceBridgeHost {
        PlaceBridgeHost(host: host, place: place)
    }

    func updateUIView(_ view: PlaceBridgeHost, context: Context) {}

    static func dismantleUIView(_ view: PlaceBridgeHost, coordinator: ()) {
        view.teardown()
    }
}

final class PlaceBridgeHost: UIView {
    private let host: AppHost
    private var bridge = HimarkBridge(raw: 0)
    private var child: HimarkBridgeView?

    init(host: AppHost, place: HimarkPlace) {
        self.host = host
        super.init(frame: .zero)
        backgroundColor = .black
        bridge = host.engine.mint(place: place)
        guard bridge.raw != 0 else { return }
        let view = HimarkBridgeView(
            frame: bounds, engine: host.engine, bridge: bridge, device: host.device)
        view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        addSubview(view)
        host.register(view)
        child = view
    }

    required init?(coder: NSCoder) { fatalError() }

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
