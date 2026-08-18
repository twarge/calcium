import Foundation
import CalciumFFI

/// One computed answer, positioned by the source line its `=>` sits on.
struct Answer: Decodable, Equatable {
    let line: Int
    let text: String
    let error: Bool
}

enum LineKind: String, Decodable {
    case heading, code, prose, raw
}

/// How one source line reads, as the engine sees it.
struct LineInfo: Decodable {
    let kind: LineKind
    /// UTF-16 offset of a trailing `#` comment within the line.
    let comment: Int?
    /// UTF-16 offset of a `#?` autocomplete request.
    let query: Int?
    /// UTF-16 offset and length of a name this line redefines.
    let redefines: [Int]?
    /// Heading depth, for headings: 1 for `#`, 2 for `##`, capped at 6.
    let level: Int?
    /// For a continuation line, the line its block starts on. A whole-line
    /// insertion — a directive — belongs above that line, not mid-statement.
    let block: Int?
}

/// One coloured span within a source line, in UTF-16 units.
struct TokenSpan: Decodable {
    enum Class: String, Decodable {
        case num, str, op, kw, fn, def, name, dir
    }
    let o: Int
    let l: Int
    let c: Class
}

/// One completion candidate: a name in scope, with its current value.
struct Completion: Decodable, Equatable {
    let name: String
    /// Rendered as an answer would be; empty for prelude names.
    let value: String
    /// Defined by this document, as opposed to the prelude.
    let doc: Bool
}

/// One sampled `plot(...)`, positioned below the source line it sits on.
/// The engine has already swept the expressions; what arrives here is
/// nothing but labeled series of finite points, ready to draw.
struct PlotData: Decodable, Equatable {
    struct Series: Decodable, Equatable {
        /// The argument as the document wrote it, for the legend.
        let label: String
        /// A dense sampled curve, as opposed to literal data worth marking.
        let swept: Bool
        /// `[x, y]` pairs; a missing sample is a gap in the curve.
        let points: [[Double]]
    }
    let line: Int
    /// The swept variable, when one exists — the x-axis label.
    let x: String?
    /// The unit the sweep carried — `s` for a `0..1.5s` domain — so the
    /// axis reads `t (s)`.
    let xUnit: String?
    /// The unit an `in` conversion asked the series be expressed in —
    /// `pA` for `plot(i(t) in pA, ...)` — shown on the vertical axis.
    let yUnit: String?
    let series: [Series]
}

/// The colourable roles of a document, shared by the styling passes on
/// both platforms and by the preferences that edit them. Raw values name
/// the UserDefaults keys: `color.variable.light` holds a `#RRGGBB`
/// override, and absence means the designed palette.
enum ColorRole: String, CaseIterable {
    case prose, variable, answer, number, string, keyword, function, definition, comment
    case series1, series2, series3, series4, series5, series6

    /// The text roles, as the Colors preferences list them.
    static var text: [ColorRole] { allCases.filter { !$0.isSeries } }
    /// The chart series cycle, in drawing order.
    static var series: [ColorRole] { allCases.filter(\.isSeries) }
    var isSeries: Bool { rawValue.hasPrefix("series") }

    var label: String {
        switch self {
        case .prose: "Prose"
        case .variable: "Variables"
        case .answer: "Results"
        case .number: "Numbers"
        case .string: "Strings"
        case .keyword: "Keywords"
        case .function: "Functions"
        case .definition: "Definitions"
        case .comment: "Comments"
        case .series1: "Series 1"
        case .series2: "Series 2"
        case .series3: "Series 3"
        case .series4: "Series 4"
        case .series5: "Series 5"
        case .series6: "Series 6"
        }
    }

    /// The designed palette: earthy inks on paper — olive variables, teal
    /// definitions, indigo results, violet keywords, sage functions, brick
    /// strings, ochre numbers, slate comments — and pastels on dark, where
    /// the same roles lighten to cream, ice, mint and apricot.
    var defaultHex: (light: String, dark: String) {
        switch self {
        case .prose: ("#1D1D1F", "#EDF4FF")
        case .variable: ("#606D27", "#FFF2BB")
        case .answer: ("#3D4BA2", "#8E9CE2")
        case .number: ("#936C22", "#FBD082")
        case .string: ("#8A4C49", "#FFA026")
        case .keyword: ("#6944BA", "#C8B0FF")
        case .function: ("#6A9B7E", "#D6FFB8")
        case .definition: ("#346F7D", "#C2F5FF")
        case .comment: ("#5F6F81", "#798BA5")
        // The chart cycle wears the same inks, at line strength: indigo,
        // ochre, sage, brick, violet, slate — pastel on dark like the text.
        case .series1: ("#3D4BA2", "#8E9CE2")
        case .series2: ("#B0761F", "#FBD082")
        case .series3: ("#3E8E5B", "#D6FFB8")
        case .series4: ("#A04A45", "#FFA026")
        case .series5: ("#6944BA", "#C8B0FF")
        case .series6: ("#5F6F81", "#798BA5")
        }
    }

    func key(dark: Bool) -> String { "color.\(rawValue).\(dark ? "dark" : "light")" }

    /// The hex in force: the user's override, or the designed default.
    func hex(dark: Bool) -> String {
        UserDefaults.standard.string(forKey: key(dark: dark))
            ?? (dark ? defaultHex.dark : defaultHex.light)
    }
}

/// The Rust engine, behind a Swift-shaped door.
///
/// The whole interface is `String -> String`, so there is no shared state to
/// keep in sync and nothing to tear down. Each call is independent.
enum Engine {

    /// Lexical token spans, one array per source line; empty for prose.
    static func tokens(of source: String) -> [[TokenSpan]] {
        guard let json = call(calcium_tokens, source),
              let data = json.data(using: .utf8),
              let spans = try? JSONDecoder().decode([[TokenSpan]].self, from: data)
        else { return [] }
        return spans
    }

    /// Names usable at `line` matching `prefix`: the document's own first,
    /// with current values, then the prelude's.
    static func completions(of source: String, line: Int, prefix: String) -> [Completion] {
        let json: String? = source.withCString { src in
            prefix.withCString { pre in
                guard let raw = calcium_completions(src, UInt32(max(0, line)), pre) else {
                    return nil
                }
                defer { calcium_string_free(raw) }
                return String(cString: raw)
            }
        }
        guard let json, let data = json.data(using: .utf8),
              let hits = try? JSONDecoder().decode([Completion].self, from: data)
        else { return [] }
        return hits
    }

    /// Answers for every `=>` in the document, in source order.
    static func evaluate(_ source: String) -> [Answer] {
        guard let json = call(calcium_evaluate, source),
              let data = json.data(using: .utf8),
              let answers = try? JSONDecoder().decode([Answer].self, from: data)
        else { return [] }
        return answers
    }

    /// How each source line reads, one entry per line.
    ///
    /// Asked of the engine rather than guessed at here. The rule is subtler
    /// than it looks — an unindented `T = 125 degC` is a calculation, while an
    /// unindented sentence ending in a full stop is not — and a second copy of
    /// it in the editor drifts out of step with the one that matters.
    static func lines(of source: String) -> [LineInfo] {
        guard let json = call(calcium_line_kinds, source),
              let data = json.data(using: .utf8),
              let lines = try? JSONDecoder().decode([LineInfo].self, from: data)
        else { return [] }
        return lines
    }

    /// Every `plot(...)` in the document, sampled, in source order.
    static func plots(in source: String) -> [PlotData] {
        guard let json = call(calcium_plots, source),
              let data = json.data(using: .utf8),
              let plots = try? JSONDecoder().decode([PlotData].self, from: data)
        else { return [] }
        return plots
    }

    /// The document with answers written in after each `=>`. This is what goes
    /// to disk.
    static func materializingAnswers(in source: String) -> String {
        call(calcium_rewrite, source) ?? source
    }

    /// The document as Typst markup — prose, display equations with fresh
    /// answers, units through fancy-units — ready for `typst compile`.
    static func typstMarkup(of source: String) -> String {
        call(calcium_typst, source) ?? source
    }

    /// Bridges one `const char * -> char *` call, taking ownership of the
    /// result and always releasing it.
    private static func call(
        _ function: (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?,
        _ source: String
    ) -> String? {
        source.withCString { input in
            guard let raw = function(input) else { return nil }
            defer { calcium_string_free(raw) }
            return String(cString: raw)
        }
    }
}
