import Foundation
import CalciumFFI

/// One computed answer, positioned by the source line its `=>` sits on.
struct Answer: Decodable, Equatable {
    let line: Int
    let text: String
    let error: Bool
}

enum LineKind: String, Decodable {
    case heading, code, prose
}

/// How one source line reads, as the engine sees it.
struct LineInfo: Decodable {
    let kind: LineKind
    /// UTF-16 offset of a trailing `#` comment within the line.
    let comment: Int?
    /// UTF-16 offset of a `#?` autocomplete request.
    let query: Int?
}

/// The Rust engine, behind a Swift-shaped door.
///
/// The whole interface is `String -> String`, so there is no shared state to
/// keep in sync and nothing to tear down. Each call is independent.
enum Engine {

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

    /// The document with answers written in after each `=>`. This is what goes
    /// to disk.
    static func materializingAnswers(in source: String) -> String {
        call(calcium_rewrite, source) ?? source
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
