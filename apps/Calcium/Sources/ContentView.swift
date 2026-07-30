import SwiftUI

struct ContentView: View {
    @Binding var text: String
    var fileURL: URL?

    #if os(macOS)
    @AppStorage("hideTitleBar") private var hideTitleBar = true
    /// Whether the chrome is currently auto-hidden; driven by `WindowChrome`.
    @State private var chromeHidden = false

    var body: some View {
        EditorView(text: $text, fileURL: fileURL)
            // The editor's surface reaches the top of the window; the scroll
            // view pins its own top content inset to the chrome height. With
            // both in place the toolbar overlays the text, so hiding it in
            // distraction-free mode frees its area without anything reflowing.
            .ignoresSafeArea(.container, edges: .top)
            .frame(minWidth: 480, minHeight: 320)
            .background(
                WindowChrome(isEnabled: hideTitleBar) { chromeHidden = $0 }
            )
            // The hide itself, by value: collapsing the window toolbar takes
            // the whole title bar with it through the system's own path, so
            // ⌘W and ⌘M keep working — unlike alpha-hiding the buttons.
            .toolbar(
                hideTitleBar && chromeHidden ? .hidden : .visible,
                for: .windowToolbar)
            .toolbar {
                ToolbarItem {
                    OutlineMenu(text: text)
                }
            }
    }
    #else
    @State private var showingPreferences = false

    var body: some View {
        // The text view manages its own keyboard insets — see the keyboard
        // observer in EditorViewIOS. SwiftUI's own avoidance resized the
        // representable instead and, on iPadOS, left it short of the window
        // after the keyboard dismissed.
        EditorViewIOS(text: $text, fileURL: fileURL)
            .ignoresSafeArea(.keyboard, edges: .bottom)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showingPreferences = true
                    } label: {
                        Image(systemName: "gearshape")
                    }
                    .accessibilityLabel("Settings")
                }
            }
            .sheet(isPresented: $showingPreferences) {
                // A plain detented sheet: the grabber and a downward flick
                // (or a tap outside, on iPad) dismiss it — no chrome, no
                // buttons, the sections speak for themselves.
                PreferencesView()
                    // Chrome-free sheets inset nothing themselves: without
                    // this the first section rides up under the grabber and
                    // the top row clips against the sheet's edge.
                    .contentMargins(.top, 18, for: .scrollContent)
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
            }
    }
    #endif
}

#if os(macOS)
/// The document's headings, as a toolbar menu that jumps.
private struct OutlineMenu: View {
    let text: String

    private struct Heading: Identifiable {
        let line: Int
        let level: Int
        let title: String
        var id: Int { line }
    }

    private var headings: [Heading] {
        text.components(separatedBy: "\n").enumerated().compactMap { index, line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("#"), !trimmed.hasPrefix("#?") else { return nil }
            let level = trimmed.prefix(while: { $0 == "#" }).count
            let title = trimmed.drop(while: { $0 == "#" })
                .trimmingCharacters(in: .whitespaces)
            guard !title.isEmpty else { return nil }
            return Heading(line: index, level: min(level, 6), title: title)
        }
    }

    var body: some View {
        Menu {
            if headings.isEmpty {
                Text("No Headings")
            } else {
                ForEach(headings) { heading in
                    Button(
                        String(repeating: "    ", count: heading.level - 1) + heading.title
                    ) {
                        CommandBus.shared.send(.jump(line: heading.line))
                    }
                }
            }
        } label: {
            Label("Outline", systemImage: "list.bullet")
        }
        .help("Jump to a heading")
    }
}
#endif
