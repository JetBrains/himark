// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

import SwiftUI

/// The native wireframe, v0: the workbench bridge full-bleed with
/// a native bottom bar. The session-centric screens of the design
/// (picker + Chat/Changes/Files tabs, one bridge each) replace
/// this as the session surface lands — the bar below is already
/// the pattern: SwiftUI chrome speaking commands, imba rendering
/// content.
struct RootView: View {
    @ObservedObject var host: AppHost

    var body: some View {
        WorkbenchBridge(host: host)
            .ignoresSafeArea(.container, edges: .top)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                bar
            }
            .onAppear { host.installHostIfNeeded() }
            .preferredColorScheme(.dark)
    }

    private var bar: some View {
        HStack {
            Menu {
                Button { host.perform("workbench.new-document") } label: {
                    Label("New", systemImage: "doc")
                }
                Button { host.perform("file.open") } label: {
                    Label("Open…", systemImage: "folder")
                }
                Button { host.saveDocument() } label: {
                    Label("Save…", systemImage: "square.and.arrow.down")
                }
                Button { host.perform("agent.toggle-agents") } label: {
                    Label("Sessions", systemImage: "person.2")
                }
                Divider()
                Button { host.perform("peeker.toggle"); host.workbenchView?.syncFocus() } label: {
                    Label("Peeker", systemImage: "filemenu.and.selection")
                }
                Button { host.perform("files.tree") } label: {
                    Label("Workspace Tree", systemImage: "sidebar.left")
                }
                Button { host.perform("workbench.split-pane") } label: {
                    Label("Split Pane", systemImage: "rectangle.split.2x1")
                }
                Button { host.perform("workbench.close-pane") } label: {
                    Label("Close Pane", systemImage: "xmark.rectangle")
                }
            } label: {
                Image(systemName: "line.3.horizontal")
                    .frame(width: 44, height: 44)
            }

            Spacer()

            Button {
                host.perform("search.open")
                host.workbenchView?.syncFocus()
            } label: {
                Image(systemName: "magnifyingglass")
                    .frame(width: 44, height: 44)
            }

            Button { host.workbenchView?.dismissKeyboard() } label: {
                Image(systemName: "keyboard.chevron.compact.down")
                    .frame(width: 44, height: 44)
            }
        }
        .padding(.horizontal, 8)
        .background(.bar)
    }
}
