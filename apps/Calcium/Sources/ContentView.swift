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
    var body: some View {
        // No `.ignoresSafeArea(.keyboard)` here, deliberately: SwiftUI's
        // keyboard avoidance is what constrains the editor to end above the
        // keyboard and its keypad rows. A raw UITextView does no keyboard
        // avoidance of its own, so opting out left the text running on
        // beneath them.
        EditorViewIOS(text: $text, fileURL: fileURL)
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
                        NotificationCenter.default.post(
                            name: .calciumJumpToLine, object: nil,
                            userInfo: ["line": heading.line])
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
