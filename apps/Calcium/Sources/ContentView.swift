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
