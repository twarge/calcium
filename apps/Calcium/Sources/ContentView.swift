import SwiftUI

struct ContentView: View {
    @Binding var text: String

    var body: some View {
        EditorView(text: $text)
            .frame(minWidth: 480, minHeight: 320)
            .background(WindowChrome())
    }
}
