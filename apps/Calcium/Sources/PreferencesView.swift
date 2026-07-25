#if os(macOS)
import SwiftUI

/// The Settings window (⌘,).
///
/// Three settings, which is all the app has that is genuinely configurable:
/// the type size that ⌘0 returns to, whether Fira Code's ligatures draw, and
/// whether the title bar hides until hovered. Everything else the format
/// itself decides per-document (`@precision`, `@group`) and does not belong
/// in app-wide preferences.
struct PreferencesView: View {
    @AppStorage("baseFontSize") private var fontSize = 13.0
    @AppStorage("ligatures") private var ligatures = true
    @AppStorage("hideTitleBar") private var hideTitleBar = true

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
                Toggle("Hide the title bar until the pointer is over it", isOn: $hideTitleBar)
            } header: {
                Text("Window")
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .fixedSize()
    }
}
#endif
