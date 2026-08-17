import AppIntents

/// The engine as a verb for Shortcuts, Spotlight, and Apple Intelligence:
/// text in, every `=>` answered, text out. String -> String, like
/// everything else about the engine.
struct EvaluateCalciumIntent: AppIntent {
    static let title: LocalizedStringResource = "Evaluate Calcium"
    static let description = IntentDescription(
        "Computes every => in Calcium text and returns the text with its answers written in.")

    @Parameter(title: "Text", inputOptions: String.IntentInputOptions(multiline: true))
    var text: String

    static var parameterSummary: some ParameterSummary {
        Summary("Evaluate \(\.$text)")
    }

    func perform() async throws -> some IntentResult & ReturnsValue<String> {
        .result(value: Engine.materializingAnswers(in: text))
    }
}

/// The Typst converter as a verb: Calcium text in, Typst markup out, with
/// every answer computed fresh. In Shortcuts this chains into Run Shell
/// Script — which runs outside the app's sandbox — so a two-step shortcut
/// turns a document into a PDF with the user's own `typst`.
struct ConvertToTypstIntent: AppIntent {
    static let title: LocalizedStringResource = "Convert Calcium to Typst"
    static let description = IntentDescription(
        "Converts Calcium text to Typst markup, with every => computed and units typeset.")

    @Parameter(title: "Text", inputOptions: String.IntentInputOptions(multiline: true))
    var text: String

    static var parameterSummary: some ParameterSummary {
        Summary("Convert \(\.$text) to Typst")
    }

    func perform() async throws -> some IntentResult & ReturnsValue<String> {
        .result(value: Engine.typstMarkup(of: text))
    }
}

struct CalciumShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: EvaluateCalciumIntent(),
            phrases: ["Evaluate \(.applicationName)"],
            shortTitle: "Evaluate",
            systemImageName: "equal.circle")
        AppShortcut(
            intent: ConvertToTypstIntent(),
            phrases: ["Convert \(.applicationName) to Typst"],
            shortTitle: "To Typst",
            systemImageName: "doc.richtext")
    }
}
