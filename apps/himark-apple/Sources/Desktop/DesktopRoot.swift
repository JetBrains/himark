// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI

/// The window per the design (himark-empty-1600 … himark-680):
/// chat column, document pane, inspector card — grown slice by
/// slice. This slice: the chat column and the session picker.
enum InspectorTab: String, CaseIterable {
    case changes = "Changes"
    case files = "Files"
}

/// The narrow window's one pane (himark-680): chat or the document,
/// switched by the titlebar's segmented pill.
enum NarrowFace {
    case chat
    case document
}

struct DesktopRoot: View {
    @ObservedObject var host: DesktopHost
    @State private var inspectorShown = true
    @State private var inspectorTab: InspectorTab = .changes

    @State private var narrowFace: NarrowFace = .chat

    var body: some View {
        GeometryReader { geometry in
            let narrow = geometry.size.width < 860
            let roomy = geometry.size.width >= 1_160

            Group {
                if narrow {
                    // ONE pane (himark-680): the segmented pill picks
                    // the face; an open document pulls itself front.
                    if narrowFace == .document, let opened = host.document {
                        DocumentPane(host: host, opened: opened)
                    } else {
                        ChatColumn(host: host).id(host.current?.uri)
                    }
                } else {
                    HSplitView {
                        ChatColumn(host: host)
                            .id(host.current?.uri)
                            .frame(minWidth: 420, idealWidth: 560, maxWidth: .infinity)
                        if let opened = host.document {
                            DocumentPane(host: host, opened: opened)
                                .frame(minWidth: 440, maxWidth: .infinity)
                        } else if roomy {
                            EmptyDocumentPane()
                                .frame(minWidth: 440, maxWidth: .infinity)
                        }
                    }
                }
            }
            .overlay(alignment: .topTrailing) {
                if inspectorShown && !narrow {
                    InspectorCard(host: host, tab: $inspectorTab)
                        .padding(12)
                }
            }
            .toolbar {
                ToolbarItem(placement: .navigation) { sessionPicker }
                ToolbarItem(placement: .principal) {
                    if narrow, let opened = host.document {
                        NarrowFacePill(opened: opened, face: $narrowFace)
                    } else if let opened = host.document {
                        DocumentPill(host: host, opened: opened)
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        inspectorShown.toggle()
                    } label: {
                        Image(systemName: "sidebar.right")
                    }
                    .disabled(narrow)
                }
            }
            .onChange(of: host.document) { opened in
                // A fresh navigate fronts the document on the phone-
                // narrow window, exactly like the iOS push.
                if opened != nil { narrowFace = .document }
                if opened == nil { narrowFace = .chat }
            }
        }
        .frame(minWidth: 480, minHeight: 420)
        .background(Color(red: 0.055, green: 0.055, blue: 0.07))
        .navigationTitle("")
    }

    private var sessionPicker: some View {
        Menu {
            Button("New Session…") { host.newSession() }
            if !host.sessions.isEmpty {
                Divider()
                ForEach(host.sessions, id: \.uri) { row in
                    Button {
                        host.select(row)
                    } label: {
                        if row.uri == host.current?.uri {
                            Label(pickerTitle(row.title), systemImage: "checkmark")
                        } else {
                            Text(pickerTitle(row.title))
                        }
                    }
                }
            }
        } label: {
            Label(host.current.map { pickerTitle($0.title) } ?? "New Session",
                  systemImage: "shippingbox")
        }
    }

    private func pickerTitle(_ title: String) -> String {
        title.isEmpty ? "Untitled" : String(title.prefix(42))
    }
}

/// Transcript over the composer — both himark-rendered; the
/// transcript scrolls natively and stays pinned to the bottom as
/// turns stream, the composer sits at its engine-measured height.
struct ChatColumn: View {
    let host: DesktopHost
    @State private var composerHeight: CGFloat = 120

    var body: some View {
        VStack(spacing: 0) {
            MintedScrollBridge(host: host, kind: .transcript, pinsToBottom: true)
            ComposerBridge(host: host, height: $composerHeight)
                .frame(height: composerHeight)
        }
    }
}

/// The center pane: the document or diff himark asked the shell to
/// open, scrolled natively. One standing document for now — a new
/// navigate replaces it.
struct DocumentPane: View {
    let host: DesktopHost
    let opened: DesktopHost.Opened

    var body: some View {
        PlaceScrollBridge(host: host, place: opened.place)
            .id(opened.id)
    }
}

/// The titlebar's document pill: name plus a close affordance.
struct DocumentPill: View {
    let host: DesktopHost
    let opened: DesktopHost.Opened

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: opened.kind == UInt32(HIMARK_PLACE_DIFF)
                ? "plus.forwardslash.minus" : "doc.text")
                .foregroundStyle(.secondary)
            Text(opened.title.isEmpty ? "Untitled" : opened.title)
                .font(.system(.body, design: .monospaced))
            Button {
                host.closedDocument(opened)
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(.quaternary.opacity(0.5), in: Capsule())
    }
}

/// The floating inspector card (himark-empty-1600): segmented
/// Changes/Files over the session trees, himark-rendered and
/// scrolled natively. Row taps ride the same navigate road the
/// agent uses — a change opens its diff, a file opens its editor.
struct InspectorCard: View {
    let host: DesktopHost
    @Binding var tab: InspectorTab

    var body: some View {
        VStack(spacing: 8) {
            Picker("", selection: $tab) {
                ForEach(InspectorTab.allCases, id: \.self) { tab in
                    Text(tab.rawValue).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.horizontal, 10)
            .padding(.top, 10)

            MintedScrollBridge(
                host: host,
                kind: tab == .changes ? .changes : .files)
                .id("\(host.current?.uri ?? "")-\(tab.rawValue)")
        }
        .frame(width: 320)
        .frame(maxHeight: .infinity)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .strokeBorder(.white.opacity(0.08)))
    }
}

/// The wide window with nothing open (himark-empty-1600): the
/// pane explains itself instead of sitting black.
struct EmptyDocumentPane: View {
    var body: some View {
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
    }
}

/// The narrow titlebar's face switch (himark-680): Chat on the
/// left, the open document on the right.
struct NarrowFacePill: View {
    let opened: DesktopHost.Opened
    @Binding var face: NarrowFace

    var body: some View {
        Picker("", selection: $face) {
            Label("Chat", systemImage: "bubble.left").tag(NarrowFace.chat)
            Text(opened.title.isEmpty ? "Untitled" : opened.title)
                .tag(NarrowFace.document)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 420)
    }
}
