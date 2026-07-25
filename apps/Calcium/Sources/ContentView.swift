import SwiftUI

struct ContentView: View {
    @Binding var text: String
    @State private var answers: [Answer] = []

    private var errorCount: Int { answers.filter(\.error).count }

    var body: some View {
        VStack(spacing: 0) {
            EditorView(text: $text) { answers = $0 }
            Divider()
            statusBar
        }
        .frame(minWidth: 620, minHeight: 420)
    }

    private var statusBar: some View {
        HStack(spacing: 12) {
            Label("\(answers.count)", systemImage: "equal.square")
                .help("\(answers.count) answers in this document")
            if errorCount > 0 {
                Label("\(errorCount)", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
                    .help("\(errorCount) lines could not be computed")
            }
            Spacer()
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 5)
        .background(.bar)
    }
}
