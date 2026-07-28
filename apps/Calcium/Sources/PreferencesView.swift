#if os(macOS)
import SwiftUI

/// The Settings window (⌘,).
///
/// Only what is genuinely configurable app-wide: the type size that ⌘0
/// returns to, the faces and ligatures, proofing, and whether the title bar
/// hides until hovered. Everything else the format itself decides
/// per-document (`@precision`, `@group`) and does not belong in app-wide
/// preferences.
struct PreferencesView: View {
    @AppStorage("baseFontSize") private var fontSize = 13.0
    @AppStorage("ligatures") private var ligatures = true
    @AppStorage("proseSystemFont") private var proseSystemFont = true
    @AppStorage("hideTitleBar") private var hideTitleBar = true
    @AppStorage("proseSpelling") private var proseSpelling = true
    @AppStorage("proseAutocorrect") private var proseAutocorrect = true
    @AppStorage("starterText") private var starterText = true
    @AppStorage("completions") private var completions = true

    var body: some View {
        Form {
            Section {
                HStack {
                    Slider(value: $fontSize, in: 9...24, step: 1) {
                        Text("Font size")
                    }
                    Text("\(Int(fontSize)) pt")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .frame(width: 44, alignment: .trailing)
                }
                Toggle("System font for prose", isOn: $proseSystemFont)
                Text("Sentences and headings in the system face; calculations stay in Fira Code.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Toggle("Ligatures", isOn: $ligatures)
                Text(
                    ligatures
                        ? "=> != >= are drawn as ⇒ ≠ ⩾ — the symbols they mean."
                        : "Operators are drawn as the characters you typed."
                )
                .font(.callout)
                .foregroundStyle(.secondary)
            } header: {
                Text("Type")
            }

            Section {
                Toggle("Complete names while typing", isOn: $completions)
                Text("Suggestions with current values appear as you type a name in a calculation.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Completions")
            }

            Section {
                Toggle("Check spelling in prose", isOn: $proseSpelling)
                Toggle("Correct spelling automatically", isOn: $proseAutocorrect)
                    .disabled(!proseSpelling)
                Text("Sentences, headings, and comments are checked; calculations are never touched.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Proofing")
            }

            Section {
                Toggle("Hide the title bar until the pointer is over it", isOn: $hideTitleBar)
            } header: {
                Text("Window")
            }

            Section {
                Toggle("Start new documents with the sample text", isOn: $starterText)
                Text("Off means a new document opens empty.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } header: {
                Text("New Documents")
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .fixedSize()
        // Open documents restyle live. Explicit sends rather than observing
        // UserDefaults: notification closures are @Sendable under Swift 6
        // and cannot carry the main-actor coordinators.
        .onChange(of: fontSize) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: ligatures) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseSystemFont) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseSpelling) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseAutocorrect) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: completions) { CommandBus.shared.send(.preferencesChanged) }
    }
}
#endif
