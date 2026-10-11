// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI

/// One palette row: a native shell verb or an engine command.
struct PaletteEntry: Identifiable {
    let id: String
    let name: String
    let shortcut: String?
    let run: () -> Void
}

/// The command palette (docs/himark-islands-command-palette.png):
/// a floating glass sheet over the document floor — native verbs
/// and the engine's registered commands in one list.
struct CommandPalette: View {
    let entries: [PaletteEntry]
    let dismiss: () -> Void
    @State private var query = ""
    @State private var picked = 0
    @FocusState private var focused: Bool

    private var matches: [PaletteEntry] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return entries }
        return entries.filter { $0.name.lowercased().contains(needle) }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(">")
                    .font(.system(size: 14, design: .monospaced))
                    .foregroundStyle(.tertiary)
                TextField("", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .focused($focused)
                    .onSubmit(runPicked)
                Text("\(matches.count) commands")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 16)
            .frame(height: 44)

            Divider().overlay(.white.opacity(0.08))

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(spacing: 1) {
                        ForEach(Array(matches.enumerated()), id: \.element.id) { index, entry in
                            Button {
                                entry.run()
                                dismiss()
                            } label: {
                                HStack {
                                    Text(entry.name)
                                        .font(.system(size: 13))
                                    Spacer()
                                    if let shortcut = entry.shortcut {
                                        Text(shortcut)
                                            .font(.system(size: 12))
                                            .foregroundStyle(.tertiary)
                                    }
                                }
                                .padding(.horizontal, 16)
                                .padding(.vertical, 9)
                                .background(
                                    index == picked
                                        ? AnyShapeStyle(.white.opacity(0.08))
                                        : AnyShapeStyle(.clear),
                                    in: RoundedRectangle(cornerRadius: 8))
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .id(index)
                        }
                    }
                    .padding(8)
                }
                .frame(maxHeight: 380)
                .onChange(of: picked) { index in
                    proxy.scrollTo(index)
                }
            }

            Divider().overlay(.white.opacity(0.08))
            HStack(spacing: 14) {
                hint("↑↓", "Navigate")
                hint("⏎", "Run")
                hint("esc", "Dismiss")
                Spacer()
            }
            .padding(.horizontal, 16)
            .frame(height: 34)
        }
        .frame(width: 620)
        .background(IslandBackground(cornerRadius: 14))
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .onAppear { focused = true }
        .onChange(of: query) { _ in picked = 0 }
        .onExitCommand(perform: dismiss)
        .onMoveCommand { direction in
            switch direction {
            case .down: picked = min(picked + 1, max(matches.count - 1, 0))
            case .up: picked = max(picked - 1, 0)
            default: break
            }
        }
    }

    private func runPicked() {
        guard matches.indices.contains(picked) else { return }
        matches[picked].run()
        dismiss()
    }

    private func hint(_ key: String, _ label: String) -> some View {
        HStack(spacing: 5) {
            Text(key)
                .font(.system(size: 11, design: .monospaced))
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: 4))
            Text(label).font(.system(size: 11)).foregroundStyle(.tertiary)
        }
        .foregroundStyle(.secondary)
    }
}
