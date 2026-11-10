import Foundation
import CalciumFFI

/// One computed answer, positioned by the source line its `=>` sits on.
struct Answer: Decodable, Equatable {
    let line: Int
    let text: String
    let error: Bool
    /// The UTF-16 range on that line the answer occupies: from just past
    /// the arrow to the end of whatever answer is there now. From the
    /// engine's parser — never found by searching the text for `=>`, which
    /// a string literal can contain and a second statement can repeat.
    let start: Int
    let end: Int

    /// What goes into that range.
    var replacement: String { text.isEmpty ? "" : " " + text }

    /// The range to replace, on a line occupying `line` of the text, or
    /// `nil` if it does not fit — the text is not what was evaluated.
    func range(on line: NSRange) -> NSRange? {
        guard start >= 0, start <= end, end <= line.length else { return nil }
        return NSRange(location: line.location + start, length: end - start)
    }

    /// Where each answer sits once every replacement is written, given the
    /// line ranges of the spliced text. Several answers can share a line,
    /// so each one shifts by what the earlier ones on its line changed.
    static func regions(
        of answers: [Answer], lines: [NSRange]
    ) -> [(range: NSRange, isError: Bool)] {
        var shift: [Int: Int] = [:]
        return answers.compactMap { answer in
            guard lines.indices.contains(answer.line) else { return nil }
            let line = lines[answer.line]
            let length = (answer.replacement as NSString).length
            let moved = shift[answer.line, default: 0]
            shift[answer.line] = moved + length - (answer.end - answer.start)
            let location = line.location + answer.start + moved
            guard length > 0, location + length <= NSMaxRange(line) else { return nil }
            return (NSRange(location: location, length: length), answer.error)
        }
    }
}

/// Where one answer sits on a line, UTF-16: its arrow and its range.
struct AnswerSlot: Decodable, Equatable {
    let line: Int
    let arrow: Int
    let start: Int
    let end: Int
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

    /// Where the answers of `source` sit, parsed but not evaluated — cheap
    /// enough to ask about a single line on a keystroke.
    static func answerSlots(of source: String) -> [AnswerSlot] {
        guard let json = call(calcium_answer_slots, source),
              let data = json.data(using: .utf8),
              let slots = try? JSONDecoder().decode([AnswerSlot].self, from: data)
        else { return [] }
        return slots
    }

    /// Answers for every `=>` in the document, in source order.
    static func evaluate(_ source: String) -> [Answer] {
        guard let json = call(calcium_evaluate, source),
              let data = json.data(using: .utf8),
              let answers = try? JSONDecoder().decode([Answer].self, from: data)
        else { return [] }
        return answers
    }

    /// Answers and sampled plots from a single evaluation — what a refresh
    /// wants, so a document with plots is not computed twice over.
    struct EvaluatedDocument: Decodable {
        let answers: [Answer]
        let plots: [PlotData]
    }

    static func document(_ source: String) -> EvaluatedDocument {
        guard let json = call(calcium_document, source),
              let data = json.data(using: .utf8),
              let document = try? JSONDecoder().decode(EvaluatedDocument.self, from: data)
        else { return EvaluatedDocument(answers: [], plots: []) }
        return document
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
        // The C ABI reads to the first NUL, so a document holding one would
        // come back cut short there — and a save would write the stump.
        // Failing here leaves every caller on its raw-text fallback.
        guard !source.utf8.contains(0) else { return nil }
        return source.withCString { input in
            guard let raw = function(input) else { return nil }
            defer { calcium_string_free(raw) }
            return String(cString: raw)
        }
    }
}

extension NSString {
    /// The contents range of every line, with lines counted as the engine
    /// counts them: broken at `\n` alone, a `\r` before it left out.
    ///
    /// Foundation's own line APIs also break at U+2028, U+2029, U+0085 and a
    /// lone `\r` — a soft break from Ctrl-Return or pasted from a word
    /// processor — and one of those would put every engine line index below
    /// it off by one: answers written after the wrong arrow, colours on the
    /// wrong lines. A trailing empty line is not a line, as in the engine.
    func engineLineRanges() -> [NSRange] {
        var ranges: [NSRange] = []
        var start = 0
        while true {
            let newline = range(
                of: "\n", options: .literal,
                range: NSRange(location: start, length: length - start))
            guard newline.location != NSNotFound else {
                ranges.append(NSRange(location: start, length: length - start))
                break
            }
            var end = newline.location
            if end > start, character(at: end - 1) == 0x0D { end -= 1 }
            ranges.append(NSRange(location: start, length: end - start))
            start = newline.location + 1
        }
        if ranges.count > 1, let last = ranges.last, last.length == 0 {
            ranges.removeLast()
        }
        return ranges
    }

    /// `getLineStart(_:end:contentsEnd:for:)` with lines as the engine
    /// counts them: the line or lines `range` touches, broken at `\n` alone.
    /// What the caret's own line is must agree with what the engine calls
    /// a line, or Return, list continuation and typing attributes act on a
    /// fragment of it after a soft break.
    func getEngineLineStart(
        _ startPointer: UnsafeMutablePointer<Int>?,
        end endPointer: UnsafeMutablePointer<Int>?,
        contentsEnd contentsEndPointer: UnsafeMutablePointer<Int>?,
        for range: NSRange
    ) {
        let location = min(max(range.location, 0), length)
        var start = 0
        if location > 0 {
            let before = self.range(
                of: "\n", options: [.literal, .backwards],
                range: NSRange(location: 0, length: location))
            if before.location != NSNotFound { start = before.location + 1 }
        }
        // The line holding the range's last character, or the caret's.
        let last = range.length > 0 ? min(max(NSMaxRange(range) - 1, location), length) : location
        let after = self.range(
            of: "\n", options: .literal,
            range: NSRange(location: last, length: length - last))
        var end = length
        var contentsEnd = length
        if after.location != NSNotFound {
            end = after.location + 1
            contentsEnd = after.location
            if contentsEnd > start, character(at: contentsEnd - 1) == 0x0D { contentsEnd -= 1 }
        }
        startPointer?.pointee = start
        endPointer?.pointee = end
        contentsEndPointer?.pointee = contentsEnd
    }

    /// `lineRange(for:)` with lines as the engine counts them, terminator
    /// included.
    func engineLineRange(for range: NSRange) -> NSRange {
        var start = 0
        var end = 0
        getEngineLineStart(&start, end: &end, contentsEnd: nil, for: range)
        return NSRange(location: start, length: end - start)
    }

    /// Calls `body` with each line's contents range, lines as the engine
    /// counts them.
    func enumerateEngineLines(_ body: (NSRange) -> Void) {
        engineLineRanges().forEach(body)
    }
}
