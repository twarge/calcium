import AppKit

/// The editor's styled text, without an editor: the same engine calls and the
/// same `Styling` pass, at scale 1 — a preview has no pinch.
enum PreviewRenderer {

    static func render(_ source: String) -> NSAttributedString {
        let styled = NSMutableAttributedString(string: source)
        Styling.apply(
            lines: Engine.lines(of: source),
            tokens: Engine.tokens(of: source),
            to: styled, scale: 1)

        // The answers are already in the text — the app materialises them on
        // save — so styling them is a matter of colouring each answer's range. The
        // engine says which lines carry answers, and which of those erred;
        // this mirrors the editor's answer-region colouring.
        let text = styled.string as NSString
        let lineRanges = text.engineLineRanges()
        for answer in Engine.evaluate(source) {
            guard lineRanges.indices.contains(answer.line) else { continue }
            // The saved file already holds its answers, so the engine's
            // range for each one is the range to colour.
            guard let range = answer.range(on: lineRanges[answer.line]),
                  range.length > 0 else { continue }
            styled.addAttribute(
                .foregroundColor,
                value: answer.error ? NSColor.systemRed : Palette.answer,
                range: range)
        }
        return styled
    }
}
