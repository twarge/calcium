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

struct CalciumShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: EvaluateCalciumIntent(),
            phrases: ["Evaluate \(.applicationName)"],
            shortTitle: "Evaluate",
            systemImageName: "equal.circle")
    }
}
