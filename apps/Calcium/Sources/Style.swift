#if os(macOS)
import AppKit

/// The document's face — the fonts, the colours, and the pass that applies
/// them — in one place, because two targets wear it: the editor and the
/// Quick Look extension. A .calcium file looks the same in Finder's preview
/// as it does open, and it can only stay that way if there is exactly one
/// implementation to drift from.
enum Styling {

    /// Applies the document's styling in place: base attributes over the
    /// whole text, then per line its kind, the engine's token colours,
    /// redefinition marks and comment tails. Attributes only — never
    /// characters — so an editor's undo and typing position are untouched.
    ///
    /// Returns the ranges the spelling checker may look at: prose and
    /// heading lines, and the comment tails of code lines. The editor feeds
    /// them to proofing; the preview has no checker and drops them.
    @discardableResult
    static func apply(
        lines: [LineInfo],
        tokens tokenLines: [[TokenSpan]],
        to storage: NSMutableAttributedString,
        scale: CGFloat
    ) -> [NSRange] {
        let whole = NSRange(location: 0, length: storage.length)
        storage.setAttributes(
            [
                .font: Typography.body(scale),
                .foregroundColor: NSColor.labelColor,
                // 0 disables ligatures; 1 is the font's defaults.
                .ligature: Typography.ligatures ? 1 : 0,
            ], range: whole)

        let text = storage.string as NSString
        var index = 0
        var checkable: [NSRange] = []
        text.enumerateSubstrings(in: whole, options: [.byLines, .substringNotRequired]) {
            _, lineRange, _, _ in
            defer { index += 1 }
            let line = lines.indices.contains(index) ? lines[index] : nil
            switch line?.kind ?? .code {
            case .heading:
                storage.addAttribute(
                    .font,
                    value: Typography.proseHeading(scale, level: line?.level ?? 1),
                    range: lineRange)
                checkable.append(lineRange)
            case .prose:
                // Prose in the system's own face, so sentences read as
                // sentences and code reads as code — full-strength in light,
                // set back in dark where solid white would glare.
                storage.addAttributes(
                    [
                        .font: Typography.prose(scale),
                        .foregroundColor: Palette.prose,
                    ], range: lineRange)
                inlineMarkdown(storage, in: lineRange, scale: scale)
                checkable.append(lineRange)
            case .code:
                // Colour by the engine's own tokens: numbers, keywords,
                // the defined name — ordinary names stay the text colour.
                if let spans = tokenLines.indices.contains(index)
                    ? tokenLines[index] : nil
                {
                    for span in spans {
                        let range = NSRange(
                            location: lineRange.location + span.o, length: span.l)
                        guard NSMaxRange(range) <= NSMaxRange(lineRange),
                              let color = Palette.token(span.c)
                        else { continue }
                        storage.addAttribute(.foregroundColor, value: color, range: range)
                    }
                }
                // A code line's comment tail is prose too, as far as the
                // spelling checker is concerned.
                if let offset = line?.comment {
                    let start = lineRange.location + offset
                    if NSMaxRange(lineRange) > start {
                        checkable.append(
                            NSRange(
                                location: start,
                                length: NSMaxRange(lineRange) - start))
                    }
                }
            case .raw:
                // A fence's verbatim markup: monospace like code, set back
                // like prose, no token colours — the engine does not read it.
                storage.addAttribute(
                    .foregroundColor, value: NSColor.secondaryLabelColor, range: lineRange)
            }
            // A redefined name gets a dotted orange underline: shadowing
            // the built-in table (`T = 125 degC` over the tesla) or an
            // earlier definition is legal and often deliberate, but it is
            // the kind of thing worth noticing out of the corner of an eye.
            if let mark = line?.redefines, mark.count == 2 {
                let range = NSRange(location: lineRange.location + mark[0], length: mark[1])
                if NSMaxRange(range) <= storage.length {
                    storage.addAttributes(
                        [
                            .underlineStyle: NSUnderlineStyle.thick
                                .union(.patternDot).rawValue,
                            .underlineColor: NSColor.systemOrange,
                        ], range: range)
                }
            }
            // A trailing `#` comment, from wherever the engine says it
            // starts — a `#` inside a string or a `#?` query is not one.
            if let offset = line?.comment {
                let start = lineRange.location + offset
                let length = NSMaxRange(lineRange) - start
                if length > 0, NSMaxRange(lineRange) <= storage.length {
                    storage.addAttribute(
                        .foregroundColor, value: Palette.comment,
                        range: NSRange(location: start, length: length))
                }
            }
        }
        return checkable
    }

    // MARK: Inline Markdown

    /// `**bold**`, `_italic_`, `` `code` ``, `[text](url)`, and list
    /// markers, styled in place on prose lines. The marks stay visible —
    /// hiding syntax would fight the caret model — they just step back.
    private static let codeSpanRegex = try! NSRegularExpression(pattern: "`[^`\n]+`")
    private static let boldRegex = try! NSRegularExpression(pattern: "\\*\\*[^*\n]+\\*\\*")
    private static let italicRegex = try! NSRegularExpression(
        pattern: "(?<=^|[\\s(])_[^_\n]+_(?=$|[\\s).,;:!?])")
    private static let linkRegex = try! NSRegularExpression(
        pattern: "\\[([^\\]\n]+)\\]\\(([^)\\s]+)\\)")
    private static let listMarkerRegex = try! NSRegularExpression(
        pattern: "^\\s*(?:[-*>]|\\d+\\.)\\s")

    private static func inlineMarkdown(
        _ storage: NSMutableAttributedString, in lineRange: NSRange, scale: CGFloat
    ) {
        let text = storage.string as NSString
        let line = text.substring(with: lineRange)
        let full = NSRange(location: 0, length: (line as NSString).length)
        let dim = NSColor.tertiaryLabelColor

        if let marker = listMarkerRegex.firstMatch(in: line, range: full) {
            storage.addAttribute(
                .foregroundColor, value: dim,
                range: NSRange(
                    location: lineRange.location + marker.range.location,
                    length: marker.range.length))
        }

        // Code spans claim their territory first; emphasis does not
        // reach inside them.
        var codeSpans: [NSRange] = []
        for match in codeSpanRegex.matches(in: line, range: full) {
            codeSpans.append(match.range)
            let range = NSRange(
                location: lineRange.location + match.range.location,
                length: match.range.length)
            storage.addAttributes(
                [.font: Typography.body(scale), .foregroundColor: NSColor.labelColor],
                range: range)
            for tick in [range.location, NSMaxRange(range) - 1] {
                storage.addAttribute(
                    .foregroundColor, value: dim,
                    range: NSRange(location: tick, length: 1))
            }
        }
        let outsideCode = { (candidate: NSRange) in
            !codeSpans.contains { NSIntersectionRange($0, candidate).length > 0 }
        }

        for match in boldRegex.matches(in: line, range: full)
        where outsideCode(match.range) {
            let range = NSRange(
                location: lineRange.location + match.range.location,
                length: match.range.length)
            storage.addAttribute(.font, value: Typography.proseBold(scale), range: range)
            for marks in [
                NSRange(location: range.location, length: 2),
                NSRange(location: NSMaxRange(range) - 2, length: 2),
            ] {
                storage.addAttribute(.foregroundColor, value: dim, range: marks)
            }
        }

        for match in italicRegex.matches(in: line, range: full)
        where outsideCode(match.range) {
            let range = NSRange(
                location: lineRange.location + match.range.location,
                length: match.range.length)
            storage.addAttribute(.font, value: Typography.proseItalic(scale), range: range)
            for mark in [range.location, NSMaxRange(range) - 1] {
                storage.addAttribute(
                    .foregroundColor, value: dim,
                    range: NSRange(location: mark, length: 1))
            }
        }

        for match in linkRegex.matches(in: line, range: full)
        where outsideCode(match.range) {
            let whole = NSRange(
                location: lineRange.location + match.range.location,
                length: match.range.length)
            storage.addAttribute(.foregroundColor, value: dim, range: whole)
            let title = NSRange(
                location: lineRange.location + match.range(at: 1).location,
                length: match.range(at: 1).length)
            storage.addAttribute(.foregroundColor, value: NSColor.linkColor, range: title)
            let target = (line as NSString).substring(with: match.range(at: 2))
            if let url = URL(string: target), url.scheme != nil {
                storage.addAttribute(.link, value: url, range: title)
            }
        }
    }
}

enum Palette {
    /// A role's colour: the user's preference when one is set, the
    /// designed palette otherwise. Resolved through a fresh dynamic
    /// colour on every ask, so a preference edit repaints on the next
    /// styling pass and a theme flip re-resolves on its own.
    static func color(_ role: ColorRole) -> NSColor {
        NSColor(name: nil) { appearance in
            let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(hex: role.hex(dark: dark)) ?? .labelColor
        }
    }

    static var comment: NSColor { color(.comment) }
    static var prose: NSColor { color(.prose) }
    static var answer: NSColor { color(.answer) }

    /// The chart series cycle, resolved like every other role.
    static var series: [NSColor] { ColorRole.series.map(color) }

    /// Code colours by token class, from the engine's own lexer.
    static func token(_ class: TokenSpan.Class) -> NSColor? {
        switch `class` {
        case .num: color(.number)
        case .str: color(.string)
        case .kw, .dir: color(.keyword)
        case .fn: color(.function)
        case .def: color(.definition)
        case .op: .secondaryLabelColor
        case .name: color(.variable)
        }
    }
}

extension NSColor {
    /// `#RRGGBB`, the spelling the palette and the preferences share.
    convenience init?(hex: String) {
        var value: UInt64 = 0
        let text = hex.dropFirst(hex.hasPrefix("#") ? 1 : 0)
        guard text.count == 6, Scanner(string: String(text)).scanHexInt64(&value) else {
            return nil
        }
        self.init(
            srgbRed: CGFloat((value >> 16) & 0xFF) / 255,
            green: CGFloat((value >> 8) & 0xFF) / 255,
            blue: CGFloat(value & 0xFF) / 255,
            alpha: 1)
    }
}

enum Typography {
    /// The size ⌘0 returns to, set in Preferences.
    static var baseSize: CGFloat {
        let saved = UserDefaults.standard.double(forKey: "baseFontSize")
        return saved > 0 ? CGFloat(saved) : 13
    }

    /// Whether Fira Code's ligatures draw, set in Preferences.
    static var ligatures: Bool {
        UserDefaults.standard.object(forKey: "ligatures") as? Bool ?? true
    }

    /// Whether prose and headings are set in the system's proportional face,
    /// leaving Fira Code to the calculations. On by default: most of a
    /// document is sentences, and sentences read better proportional.
    static var proseUsesSystemFont: Bool {
        UserDefaults.standard.object(forKey: "proseSystemFont") as? Bool ?? true
    }

    static func prose(_ scale: CGFloat) -> NSFont {
        proseUsesSystemFont
            ? .systemFont(ofSize: baseSize * min(max(scale, 0.5), 4))
            : body(scale)
    }
    static func proseBold(_ scale: CGFloat) -> NSFont {
        proseUsesSystemFont
            ? .boldSystemFont(ofSize: baseSize * min(max(scale, 0.5), 4))
            : named("FiraCode-Bold", scale, fallback: .bold)
    }
    static func proseItalic(_ scale: CGFloat) -> NSFont {
        NSFontManager.shared.convert(prose(scale), toHaveTrait: .italicFontMask)
    }
    /// Headings step down with depth: `#` largest, `##` smaller, and so on,
    /// levelling out at body size by `####`.
    static func headingMultiplier(_ level: Int) -> CGFloat {
        switch level {
        case ...1: 1.6
        case 2: 1.35
        case 3: 1.15
        default: 1.0
        }
    }

    static func proseHeading(_ scale: CGFloat, level: Int) -> NSFont {
        let size = baseSize * min(max(scale, 0.5), 4) * headingMultiplier(level)
        if proseUsesSystemFont {
            return .systemFont(ofSize: size, weight: .bold)
        }
        return NSFont(name: "FiraCode-Bold", size: size)
            ?? .monospacedSystemFont(ofSize: size, weight: .bold)
    }

    /// Fira Code, bundled in `Resources/Fonts` and registered by
    /// `ATSApplicationFontsPath`.
    ///
    /// Its ligatures happen to line up with this language exactly: `=>`, `!=`,
    /// `>=` and `<=` are drawn as ⇒, ≠, ≥ and ≤ — the symbols the operators
    /// already mean. They are contextual alternates, so they occupy the same
    /// advance width as the characters they replace and the caret still lands
    /// between them.
    ///
    /// The scale is per-document — pinch zoom in one window leaves the others
    /// alone — so the fonts are functions of it rather than statics.
    static func body(_ scale: CGFloat) -> NSFont {
        named("FiraCode-Regular", scale, fallback: .regular)
    }
    static func heading(_ scale: CGFloat) -> NSFont {
        named("FiraCode-Bold", scale, fallback: .bold)
    }

    /// Falls back to the system monospace face if the bundled font is missing,
    /// so a broken resource copy degrades rather than crashes.
    private static func named(
        _ name: String, _ scale: CGFloat, fallback weight: NSFont.Weight
    ) -> NSFont {
        let size = baseSize * min(max(scale, 0.5), 4)
        return NSFont(name: name, size: size)
            ?? .monospacedSystemFont(ofSize: size, weight: weight)
    }
}
#endif
