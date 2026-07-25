import AppKit
import FoundationModels

/// Resolves `#?` requests with the on-device model.
///
/// Writing `mass of earth = #?` asks for a value; the reply replaces the `#?`.
/// Everything about the flow follows from one fact: the reply becomes the
/// *author's* text, not an answer. So it is inserted through the ordinary
/// editing path where undo records it, and once it lands the `#?` is gone —
/// which is also what makes the whole thing idempotent. No marker, no query.
final class Autocomplete {
    /// Lines already being asked about, keyed by their text, so a slow reply
    /// is not requested twice while the first is still thinking.
    private var inFlight: Set<String> = []
    /// Lines already answered once. Undoing the reply brings the `#?` back,
    /// and without this the next refresh would immediately re-ask and
    /// overwrite the undo — Cmd-Z would appear not to work. Asking again is a
    /// deliberate act: retype the `#?` or change the line.
    private var answered: Set<String> = []

    /// True when the OS offers the model at all: Apple Intelligence enabled,
    /// hardware capable. Checked per call — the user can switch it on midway.
    static var isAvailable: Bool {
        guard #available(macOS 26.0, *) else { return false }
        return SystemLanguageModel.default.isAvailable
    }

    /// Kicks off a request for the first unanswered `#?` in the document.
    ///
    /// One at a time, deliberately: replies land as edits, edits reshuffle
    /// offsets, and the next refresh finds the next query anyway.
    @MainActor
    func resolveFirstQuery(in textView: NSTextView, lines: [LineInfo]) {
        guard #available(macOS 26.0, *), Autocomplete.isAvailable else { return }
        guard let index = lines.firstIndex(where: { $0.query != nil }) else { return }

        let text = textView.string as NSString
        var lineStart = 0
        var found: NSRange? = nil
        var current = 0
        text.enumerateSubstrings(
            in: NSRange(location: 0, length: text.length),
            options: [.byLines, .substringNotRequired]
        ) { _, range, _, stop in
            if current == index {
                lineStart = range.location
                found = range
                stop.pointee = true
            }
            current += 1
        }
        guard let lineRange = found else { return }
        let line = text.substring(with: lineRange)
        guard !inFlight.contains(line), !answered.contains(line), line.contains("#?") else {
            return
        }
        inFlight.insert(line)

        let prompt = Autocomplete.prompt(for: line)
        Task { @MainActor [weak self, weak textView] in
            defer { self?.inFlight.remove(line) }
            guard let reply = await Autocomplete.ask(prompt) else { return }
            guard let textView else { return }
            // Re-locate before touching anything: the document may have moved
            // underneath the request. The line must still say what it said.
            let fresh = textView.string as NSString
            let search = NSRange(location: 0, length: fresh.length)
            let lineNow = fresh.range(of: line, options: [], range: search)
            guard lineNow.location != NSNotFound else { return }
            let marker = (line as NSString).range(of: "#?")
            let target = NSRange(location: lineNow.location + marker.location, length: marker.length)
            // Through `insertText`, the input system's own entry point: the
            // reply is the author's text now, and undo must reliably take it
            // back out — the manual shouldChange/replace/didChange bracket
            // registered undo only intermittently. `insertText` moves the
            // caret to the end of what it inserted, so put it back if it was
            // somewhere unrelated.
            let caret = textView.selectedRange()
            textView.insertText(reply, replacementRange: target)
            self?.answered.insert(line)
            if caret.location < target.location || caret.location > NSMaxRange(target) {
                let shift = (reply as NSString).length - target.length
                let restored = caret.location > NSMaxRange(target)
                    ? NSRange(location: caret.location + shift, length: caret.length)
                    : caret
                textView.setSelectedRange(restored)
            }
            _ = lineStart
        }
    }

    /// One value, no prose — the reply lands in the middle of a calculation.
    private static func prompt(for line: String) -> String {
        let question = line.replacingOccurrences(of: "#?", with: "___")
        return """
        Fill in the blank marked ___ in this line from a plain-text calculation \
        document. Reply with ONLY the value: a number, with units if natural \
        (like "5.972e24 kg" or "299792458 m/s" or "42"). No explanation, no \
        punctuation, no sentence.

        \(question)
        """
    }

    @available(macOS 26.0, *)
    private static func ask(_ prompt: String) async -> String? {
        do {
            let session = LanguageModelSession()
            let reply = try await session.respond(to: prompt).content
                .trimmingCharacters(in: .whitespacesAndNewlines)
            // One line only; a chatty reply would break the document.
            guard !reply.isEmpty, !reply.contains("\n"), reply.count < 80 else { return nil }
            return reply
        } catch {
            return nil
        }
    }
}
