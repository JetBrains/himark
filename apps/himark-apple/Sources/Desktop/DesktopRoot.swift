// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI

/// The window per the design (himark-empty-1600 … himark-680):
/// chat column, document pane, inspector card — grown slice by
/// slice. This slice: the chat column and the session picker.
struct DesktopRoot: View {
    @ObservedObject var host: DesktopHost

    var body: some View {
        ChatColumn(host: host)
            .id(host.current?.uri)
            .frame(minWidth: 480, minHeight: 420)
            .background(Color(red: 0.055, green: 0.055, blue: 0.07))
            .toolbar {
                ToolbarItem(placement: .navigation) { sessionPicker }
            }
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
