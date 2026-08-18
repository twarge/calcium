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
        // save — so styling them is a matter of finding each `=>` tail. The
        // engine says which lines carry answers, and which of those erred;
        // this mirrors the editor's answer-region colouring.
        let text = styled.string as NSString
        var lineRanges: [NSRange] = []
        text.enumerateSubstrings(
            in: NSRange(location: 0, length: text.length),
            options: [.byLines, .substringNotRequired]
        ) { _, lineRange, _, _ in
            lineRanges.append(lineRange)
        }
        for answer in Engine.evaluate(source) {
            guard lineRanges.indices.contains(answer.line) else { continue }
            let line = lineRanges[answer.line]
            let body = text.substring(with: line)
            guard let arrow = body.range(of: "=>") else { continue }
            let afterArrow = line.location
                + body.distance(from: body.startIndex, to: arrow.upperBound)
            let length = NSMaxRange(line) - afterArrow
            guard length > 0 else { continue }
            styled.addAttribute(
                .foregroundColor,
                value: answer.error ? NSColor.systemRed : Palette.answer,
                range: NSRange(location: afterArrow, length: length))
        }
        return styled
    }
}
