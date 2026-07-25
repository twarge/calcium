import SwiftUI

@main
struct CalciumApp: App {

    init() {
        // The `NSTextView` properties are not enough on their own: the smart
        // substitutions are driven by user defaults read from the global
        // domain, and period substitution has no per-view property at all.
        // Writing them into *this app's* domain overrides the global values,
        // which is exactly what the per-app override is for.
        //
        // This matters more here than in a prose editor. Double-space becoming
        // ". " turns `3 +  =>` into `3 +. =>`, a straight quote becoming a
        // curly one turns a string literal into a syntax error, and `--`
        // becoming an em dash breaks subtraction. Each one leaves the author
        // staring at a line that looks right and does not work.
        for key in [
            "NSAutomaticPeriodSubstitutionEnabled",
            "NSAutomaticQuoteSubstitutionEnabled",
            "NSAutomaticDashSubstitutionEnabled",
            "NSAutomaticTextReplacementEnabled",
            "NSAutomaticSpellingCorrectionEnabled",
            "NSAutomaticCapitalizationEnabled",
            "NSAutomaticTextCompletionEnabled",
        ] {
            UserDefaults.standard.set(false, forKey: key)
        }
    }

    var body: some Scene {
        DocumentGroup(newDocument: CalciumDocument()) { file in
            ContentView(text: file.$document.text)
        }
        .defaultSize(width: 900, height: 620)
        .commands {
            // The format has no use for rich text, and these would only put
            // characters in the buffer that the parser cannot read back.
            CommandGroup(replacing: .textFormatting) {}
            CommandGroup(replacing: .help) {
                Link("Calcium Reference", destination: URL(string: "https://github.com/twarge/calcium")!)
            }
        }
    }
}
