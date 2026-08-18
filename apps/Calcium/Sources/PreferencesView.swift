import SwiftUI

/// The preferences — the Settings window (⌘,) on the Mac, a sheet behind
/// the gear on iOS. In the app on both platforms, deliberately: nothing
/// here needs the Settings app, and a document editor's options belong
/// where the documents are.
///
/// Only what is genuinely configurable app-wide: the type size, the faces
/// and ligatures, completions, and — on the Mac — proofing and the title
/// bar. Everything else the format itself decides per-document
/// (`@precision`, `@group`) and does not belong in app-wide preferences.
struct PreferencesView: View {
    @AppStorage("baseFontSize") private var fontSize = 13.0
    @AppStorage("ligatures") private var ligatures = true
    @AppStorage("proseSystemFont") private var proseSystemFont = true
    @AppStorage("hideTitleBar") private var hideTitleBar = true
    @AppStorage("proseSpelling") private var proseSpelling = true
    @AppStorage("proseAutocorrect") private var proseAutocorrect = true
    @AppStorage("starterText") private var starterText = true
    @AppStorage("completions") private var completions = true
    /// Bumped by the reset button so the colour wells rebuild from the
    /// designed palette rather than holding their edited state.
    @State private var paletteEdition = 0

    var body: some View {
        #if os(macOS)
        form
            .frame(width: 420)
            .fixedSize()
        #else
        form
        #endif
    }

    private var form: some View {
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
                Grid(alignment: .leading, verticalSpacing: 6) {
                    GridRow {
                        Text("")
                        Text("Light").font(.callout).foregroundStyle(.secondary)
                        Text("Dark").font(.callout).foregroundStyle(.secondary)
                    }
                    ForEach(ColorRole.text, id: \.rawValue) { role in
                        GridRow {
                            Text(role.label)
                                .gridColumnAlignment(.leading)
                            ColorWell(role: role, dark: false)
                            ColorWell(role: role, dark: true)
                        }
                    }
                    Divider()
                    ForEach(ColorRole.series, id: \.rawValue) { role in
                        GridRow {
                            Text(role.label)
                                .gridColumnAlignment(.leading)
                            ColorWell(role: role, dark: false)
                            ColorWell(role: role, dark: true)
                        }
                    }
                }
                .id(paletteEdition)
                Button("Use Designed Palette") {
                    for role in ColorRole.allCases {
                        UserDefaults.standard.removeObject(forKey: role.key(dark: false))
                        UserDefaults.standard.removeObject(forKey: role.key(dark: true))
                    }
                    paletteEdition += 1
                    CommandBus.shared.send(.preferencesChanged)
                }
                Text("Each role in each theme, chart series included; open documents repaint as you pick.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Colors")
            }

            Section {
                Toggle("Complete names while typing", isOn: $completions)
                Text("Suggestions with current values appear as you type a name in a calculation.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } header: {
                Text("Completions")
            }

            #if os(macOS)
            // Proofing rides NSTextView's per-range checking; UIKit offers
            // no equivalent gate, so iOS has no spelling to configure.
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
            #endif

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
        // Open documents restyle live. Explicit sends rather than observing
        // UserDefaults: notification closures are @Sendable under Swift 6
        // and cannot carry the main-actor coordinators.
        .onChange(of: paletteEdition) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: fontSize) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: ligatures) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseSystemFont) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseSpelling) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: proseAutocorrect) { CommandBus.shared.send(.preferencesChanged) }
        .onChange(of: completions) { CommandBus.shared.send(.preferencesChanged) }
    }
}

/// One colour well: a role in one theme. Edits write `#RRGGBB` to the
/// role's UserDefaults key and repaint open documents; the designed
/// palette is simply the absence of a key.
private struct ColorWell: View {
    let role: ColorRole
    let dark: Bool
    @State private var color: Color

    init(role: ColorRole, dark: Bool) {
        self.role = role
        self.dark = dark
        _color = State(initialValue: Color(hex: role.hex(dark: dark)) ?? .primary)
    }

    var body: some View {
        ColorPicker("", selection: $color, supportsOpacity: false)
            .labelsHidden()
            .onChange(of: color) {
                guard let hex = color.hexString else { return }
                UserDefaults.standard.set(hex, forKey: role.key(dark: dark))
                CommandBus.shared.send(.preferencesChanged)
            }
    }
}

extension Color {
    /// `#RRGGBB`, the spelling the palette keys hold.
    init?(hex: String) {
        var value: UInt64 = 0
        let text = hex.dropFirst(hex.hasPrefix("#") ? 1 : 0)
        guard text.count == 6, Scanner(string: String(text)).scanHexInt64(&value) else {
            return nil
        }
        self.init(
            .sRGB,
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255,
            opacity: 1)
    }

    var hexString: String? {
        #if os(macOS)
        guard let resolved = NSColor(self).usingColorSpace(.sRGB) else { return nil }
        let red = resolved.redComponent, green = resolved.greenComponent
        let blue = resolved.blueComponent
        #else
        var red: CGFloat = 0, green: CGFloat = 0, blue: CGFloat = 0, alpha: CGFloat = 0
        guard UIColor(self).getRed(&red, green: &green, blue: &blue, alpha: &alpha) else {
            return nil
        }
        #endif
        let byte = { (component: CGFloat) in
            Int((component * 255).rounded()).clamped(to: 0...255)
        }
        return String(format: "#%02X%02X%02X", byte(red), byte(green), byte(blue))
    }
}

extension Int {
    fileprivate func clamped(to range: ClosedRange<Int>) -> Int {
        Swift.min(Swift.max(self, range.lowerBound), range.upperBound)
    }
}
