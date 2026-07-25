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
    }
    #else
    var body: some View {
        EditorViewIOS(text: $text, fileURL: fileURL)
            .ignoresSafeArea(.keyboard, edges: .bottom)
    }
    #endif
}
