#if os(iOS)
import SwiftUI
import UIKit

/// The iOS editing surface: a `UITextView` with the same answers-in-the-text
/// model as the Mac editor.
///
/// The splice logic is deliberately a close port of the Mac coordinator's
/// rather than a shared abstraction — the two text stacks agree on
/// `NSTextStorage` but differ everywhere around it, and the Mac path is
/// verified in use. Keep the two in step by hand until a third platform
/// forces the refactor.
struct EditorViewIOS: UIViewRepresentable {
    @Binding var text: String
    var fileURL: URL?

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> UITextView {
        let textView = UITextView()
        textView.font = TypographyIOS.body
        textView.backgroundColor = .systemBackground
        textView.alwaysBounceVertical = true
        textView.textContainerInset = UIEdgeInsets(top: 16, left: 10, bottom: 16, right: 10)

        // Every automatic substitution off, same reasoning as the Mac: a
        // smart quote or auto-capital turns a working line into a syntax
        // error the author cannot see.
        textView.autocorrectionType = .no
        textView.autocapitalizationType = .none
        textView.smartQuotesType = .no
        textView.smartDashesType = .no
        textView.smartInsertDeleteType = .no
        textView.spellCheckingType = .no
        textView.keyboardType = .asciiCapable

        textView.delegate = context.coordinator
        textView.text = text
        context.coordinator.fileURL = fileURL
        context.coordinator.restoreViewState(in: textView)
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
        return textView
    }

    func updateUIView(_ textView: UITextView, context: Context) {
        context.coordinator.fileURL = fileURL
        guard textView.text != text else { return }
        textView.text = text
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        private var parent: EditorViewIOS
        var fileURL: URL?
        private var answerRegions: [(range: NSRange, isError: Bool)] = []
        private var lastAnswerByLine: [Int: String] = [:]
        private var isSplicing = false
        private var scheduled: DispatchWorkItem?
        private var statePersist: DispatchWorkItem?

        init(_ parent: EditorViewIOS) {
            self.parent = parent
        }

        // MARK: View state (shared mechanism with the Mac: an xattr)

        func restoreViewState(in textView: UITextView) {
            guard let url = fileURL, let state = DocumentViewState.load(from: url) else {
                return
            }
            let length = (textView.text as NSString).length
            textView.selectedRange = NSRange(
                location: min(max(0, state.cursor), length), length: 0)
        }

        private func persistViewStateSoon(for textView: UITextView) {
            statePersist?.cancel()
            let item = DispatchWorkItem { [weak self, weak textView] in
                guard let self, let textView, let url = self.fileURL else { return }
                DocumentViewState(scale: 1, cursor: textView.selectedRange.location)
                    .save(to: url)
            }
            statePersist = item
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: item)
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            guard !isSplicing else { return }
            applyTypingAttributes(textView)
            persistViewStateSoon(for: textView)
        }

        /// Line classification from the most recent highlight.
        private var lastLines: [LineInfo] = []

        /// The next character takes the face of the line the caret is on,
        /// rather than arriving monospace and being corrected a beat later.
        private func applyTypingAttributes(_ textView: UITextView) {
            let caret = textView.selectedRange.location
            let text = textView.text as NSString
            guard caret <= text.length else { return }
            var lineStart = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: nil,
                for: NSRange(location: caret, length: 0))
            let index = text.substring(to: lineStart).components(separatedBy: "\n").count - 1
            let kind = lastLines.indices.contains(index) ? lastLines[index].kind : .code
            switch kind {
            case .heading:
                let level = lastLines.indices.contains(index)
                    ? (lastLines[index].level ?? 1) : 1
                textView.typingAttributes = [
                    .font: TypographyIOS.heading(level: level),
                    .foregroundColor: UIColor.label,
                ]
            case .prose:
                textView.typingAttributes = [
                    .font: TypographyIOS.prose, .foregroundColor: UIColor.secondaryLabel,
                ]
            case .code:
                textView.typingAttributes = [
                    .font: TypographyIOS.body, .foregroundColor: UIColor.label,
                    .ligature: TypographyIOS.ligatures ? 1 : 0,
                ]
            }
        }

        // MARK: Editing

        func textViewDidChange(_ textView: UITextView) {
            guard !isSplicing else { return }
            parent.text = textView.text
            // Restyle now; only evaluation waits for the pause.
            highlight(textView, lines: Engine.lines(of: textView.text))
            scheduled?.cancel()
            let item = DispatchWorkItem { [weak self, weak textView] in
                guard let self, let textView else { return }
                guard textView.markedTextRange == nil else { return }
                self.refresh(textView)
            }
            scheduled = item
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: item)
        }

        func refresh(_ textView: UITextView) {
            let answers = Engine.evaluate(textView.text)
            splice(answers, into: textView)
            highlight(textView, lines: Engine.lines(of: textView.text))
            parent.text = textView.text
        }

        // MARK: Splicing (port of the Mac coordinator)

        private func splice(_ answers: [Answer], into textView: UITextView) {
            let storage = textView.textStorage
            let text = storage.string as NSString
            let lines = lineRanges(in: text)

            var edits: [(range: NSRange, replacement: String)] = []
            let caretLine = lineIndex(of: textView.selectedRange.location, in: lines)
            if let caretLine,
               let stale = lastAnswerByLine[caretLine],
               !answers.contains(where: { $0.line == caretLine })
            {
                let line = lines[caretLine]
                let body = text.substring(with: line)
                if !body.contains("=>"), body.hasSuffix(stale) {
                    edits.append(
                        (
                            NSRange(
                                location: NSMaxRange(line) - (stale as NSString).length,
                                length: (stale as NSString).length),
                            ""
                        ))
                }
            }

            for answer in answers {
                guard answer.line >= 0, answer.line < lines.count else { continue }
                let line = lines[answer.line]
                let body = text.substring(with: line)
                guard let arrow = body.range(of: "=>") else { continue }
                let afterArrow =
                    line.location + body.distance(from: body.startIndex, to: arrow.upperBound)
                let existing = NSRange(
                    location: afterArrow, length: NSMaxRange(line) - afterArrow)
                let replacement = answer.text.isEmpty ? "" : " " + answer.text
                if text.substring(with: existing) != replacement {
                    edits.append((existing, replacement))
                }
            }
            lastAnswerByLine = Dictionary(
                answers.map { ($0.line, $0.text.isEmpty ? "" : " " + $0.text) },
                uniquingKeysWith: { first, _ in first })

            var selection = textView.selectedRange
            if !edits.isEmpty {
                isSplicing = true
                textView.undoManager?.disableUndoRegistration()
                storage.beginEditing()
                for edit in edits.sorted(by: { $0.range.location > $1.range.location }) {
                    storage.replaceCharacters(in: edit.range, with: edit.replacement)
                    selection = adjust(selection, for: edit)
                }
                storage.endEditing()
                textView.undoManager?.enableUndoRegistration()
                isSplicing = false
            }

            let updated = storage.string as NSString
            let freshLines = lineRanges(in: updated)
            answerRegions = answers.compactMap { answer in
                guard answer.line >= 0, answer.line < freshLines.count else { return nil }
                let line = freshLines[answer.line]
                let body = updated.substring(with: line)
                guard let arrow = body.range(of: "=>") else { return nil }
                let afterArrow =
                    line.location + body.distance(from: body.startIndex, to: arrow.upperBound)
                let length = NSMaxRange(line) - afterArrow
                guard length > 0 else { return nil }
                return (NSRange(location: afterArrow, length: length), answer.error)
            }

            if !edits.isEmpty {
                selection.location = max(0, min(selection.location, updated.length))
                selection.length = min(selection.length, updated.length - selection.location)
                isSplicing = true
                textView.selectedRange = selection
                isSplicing = false
            }
        }

        private func adjust(_ selection: NSRange, for edit: (range: NSRange, replacement: String))
            -> NSRange
        {
            var selection = selection
            let start = edit.range.location
            let end = NSMaxRange(edit.range)
            let newLength = (edit.replacement as NSString).length
            if selection.location <= start {
                return selection
            }
            if selection.location <= end {
                selection.location = start + min(selection.location - start, newLength)
                selection.length = 0
                return selection
            }
            selection.location += newLength - edit.range.length
            return selection
        }

        // MARK: Highlighting

        private func highlight(_ textView: UITextView, lines: [LineInfo]) {
            lastLines = lines
            let storage = textView.textStorage
            let whole = NSRange(location: 0, length: storage.length)
            storage.beginEditing()
            storage.setAttributes(
                [
                    .font: TypographyIOS.body,
                    .foregroundColor: UIColor.label,
                    .ligature: TypographyIOS.ligatures ? 1 : 0,
                ], range: whole)

            let text = storage.string as NSString
            var index = 0
            text.enumerateSubstrings(in: whole, options: [.byLines, .substringNotRequired]) {
                _, lineRange, _, _ in
                defer { index += 1 }
                let line = lines.indices.contains(index) ? lines[index] : nil
                switch line?.kind ?? .code {
                case .heading:
                    storage.addAttribute(
                        .font, value: TypographyIOS.heading(level: line?.level ?? 1),
                        range: lineRange)
                case .prose:
                    storage.addAttributes(
                        [
                            .font: TypographyIOS.prose,
                            .foregroundColor: UIColor.secondaryLabel,
                        ], range: lineRange)
                case .code:
                    break
                }
                if let mark = line?.redefines, mark.count == 2 {
                    let range = NSRange(location: lineRange.location + mark[0], length: mark[1])
                    if NSMaxRange(range) <= storage.length {
                        storage.addAttributes(
                            [
                                .underlineStyle: NSUnderlineStyle.thick
                                    .union(.patternDot).rawValue,
                                .underlineColor: UIColor.systemOrange,
                            ], range: range)
                    }
                }
                if let offset = line?.comment {
                    let start = lineRange.location + offset
                    let length = NSMaxRange(lineRange) - start
                    if length > 0, NSMaxRange(lineRange) <= storage.length {
                        storage.addAttribute(
                            .foregroundColor, value: PaletteIOS.comment,
                            range: NSRange(location: start, length: length))
                    }
                }
            }

            for region in answerRegions where NSMaxRange(region.range) <= storage.length {
                storage.addAttribute(
                    .foregroundColor,
                    value: region.isError ? UIColor.systemRed : UIColor.secondaryLabel,
                    range: region.range)
            }
            storage.endEditing()
            applyTypingAttributes(textView)
        }

        // MARK: Line geometry

        private func lineRanges(in text: NSString) -> [NSRange] {
            var ranges: [NSRange] = []
            var start = 0
            while start <= text.length {
                var lineEnd = 0
                var contentsEnd = 0
                text.getLineStart(
                    nil, end: &lineEnd, contentsEnd: &contentsEnd,
                    for: NSRange(location: start, length: 0))
                ranges.append(NSRange(location: start, length: contentsEnd - start))
                if lineEnd == start { break }
                start = lineEnd
            }
            if ranges.count > 1, let last = ranges.last, last.length == 0 {
                ranges.removeLast()
            }
            return ranges
        }

        private func lineIndex(of location: Int, in lines: [NSRange]) -> Int? {
            lines.firstIndex { location >= $0.location && location <= NSMaxRange($0) }
        }
    }
}

enum TypographyIOS {
    static var baseSize: CGFloat {
        let saved = UserDefaults.standard.double(forKey: "baseFontSize")
        return saved > 0 ? CGFloat(saved) : 15
    }
    static var ligatures: Bool {
        UserDefaults.standard.object(forKey: "ligatures") as? Bool ?? true
    }
    static var proseUsesSystemFont: Bool {
        UserDefaults.standard.object(forKey: "proseSystemFont") as? Bool ?? true
    }
    static var body: UIFont {
        UIFont(name: "FiraCode-Regular", size: baseSize)
            ?? .monospacedSystemFont(ofSize: baseSize, weight: .regular)
    }
    static func headingMultiplier(_ level: Int) -> CGFloat {
        switch level {
        case ...1: 1.6
        case 2: 1.35
        case 3: 1.15
        default: 1.0
        }
    }
    static func heading(level: Int) -> UIFont {
        let size = baseSize * headingMultiplier(level)
        if proseUsesSystemFont {
            return .systemFont(ofSize: size, weight: .bold)
        }
        return UIFont(name: "FiraCode-Bold", size: size)
            ?? .monospacedSystemFont(ofSize: size, weight: .bold)
    }
    static var prose: UIFont {
        proseUsesSystemFont ? .systemFont(ofSize: baseSize) : body
    }
}

enum PaletteIOS {
    static var comment: UIColor {
        UIColor { traits in
            traits.userInterfaceStyle == .dark
                ? UIColor(red: 0.46, green: 0.55, blue: 0.66, alpha: 1)
                : UIColor(red: 0.38, green: 0.47, blue: 0.58, alpha: 1)
        }
    }
}
#endif
