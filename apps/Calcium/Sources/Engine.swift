import Foundation
import CalciumFFI

/// One computed answer, positioned by the source line its `=>` sits on.
struct Answer: Decodable, Equatable {
    let line: Int
    let text: String
    let error: Bool
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

    /// The document with answers written in after each `=>`. This is what goes
    /// to disk.
    static func materializingAnswers(in source: String) -> String {
        call(calcium_rewrite, source) ?? source
    }

    /// The document with answers removed. This is what the editor holds while
    /// you type, so the buffer never shows an answer twice.
    static func strippingAnswers(from source: String) -> String {
        call(calcium_strip_answers, source) ?? source
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
