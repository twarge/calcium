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
        // TextKit 1, matching the Mac editor and for the same reason: the
        // per-keystroke attribute rewrites and behind-the-view splices this
        // editor performs crash TextKit 2's viewport layout
        // (`NSTextContentStorage locationFromLocation:withOffset:` with a
        // null location). iOS has the explicit opt-out initializer.
        let textView = UITextView(usingTextLayoutManager: false)
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
        // The system find panel (with replace; the view is editable).
        textView.isFindInteractionEnabled = true

        // Links open on tap; the recognizer's delegate admits only touches
        // that land on one, so ordinary taps place the caret untouched.
        let tap = UITapGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.tappedLink(_:)))
        tap.delegate = context.coordinator
        textView.addGestureRecognizer(tap)

        textView.delegate = context.coordinator
        textView.text = text
        context.coordinator.fileURL = fileURL
        context.coordinator.restoreViewState(in: textView)
        // The view fills the window and insets itself under the keyboard —
        // UIKit's own mechanism, instead of SwiftUI resizing the frame,
        // which left the editor short of the window on iPadOS after the
        // keyboard dismissed. The keyboard frame arrives in screen
        // coordinates on the main thread; the box re-enters the actor.
        let box = MainActorWeak(textView)
        NotificationCenter.default.addObserver(
            forName: UIResponder.keyboardWillChangeFrameNotification,
            object: nil, queue: .main
        ) { note in
            let end = (note.userInfo?[UIResponder.keyboardFrameEndUserInfoKey]
                as? NSValue)?.cgRectValue
            MainActor.assumeIsolated {
                guard let textView = box.value, let end,
                      textView.window != nil else { return }
                let keyboard = textView.convert(end, from: nil)
                let overlap = max(0, textView.bounds.maxY - keyboard.minY)
                textView.contentInset.bottom = overlap
                textView.verticalScrollIndicatorInsets.bottom = overlap
            }
        }
        context.coordinator.installCommands(for: textView)
        let coordinator = context.coordinator
        Task { coordinator.refresh(textView) }
        return textView
    }

    func updateUIView(_ textView: UITextView, context: Context) {
        context.coordinator.fileURL = fileURL
        guard textView.text != text else { return }
        textView.text = text
        let coordinator = context.coordinator
        Task { coordinator.refresh(textView) }
    }

    @MainActor
    final class Coordinator: NSObject, UITextViewDelegate, UIGestureRecognizerDelegate {
        private var parent: EditorViewIOS
        var fileURL: URL?
        private var answerRegions: [(range: NSRange, isError: Bool)] = []
        private var lastAnswerByLine: [Int: String] = [:]
        private var isSplicing = false
        private var scheduled: Task<Void, Never>?
        private var statePersist: Task<Void, Never>?

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
            statePersist = Task { [weak self, weak textView] in
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled, let self, let textView,
                      let url = self.fileURL else { return }
                DocumentViewState(scale: 1, cursor: textView.selectedRange.location)
                    .save(to: url)
            }
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            guard !isSplicing else { return }
            applyTypingAttributes(textView)
            persistViewStateSoon(for: textView)
        }

        /// The calculator rows above the keyboard: operators and digits,
        /// inserted through the ordinary editing pipeline so each tap
        /// evaluates like a keystroke.
        ///
        /// Attached on first focus, not at construction. Built while the
        /// view was still windowless, the input system's first activation
        /// came up with the accessory but no keyboard; by the time editing
        /// begins the input session is real, and a reload seats both.
        func textViewDidBeginEditing(_ textView: UITextView) {
            guard textView.inputAccessoryView == nil else { return }
            // The calculator rows are for the iPhone, whose letters
            // keyboard hides digits behind a mode switch. The iPad
            // keyboard has its own number row, and with a hardware
            // keyboard the rows would float as clutter — there, only the
            // completion strip rides along.
            let accessory = KeypadAccessory(
                for: textView,
                keys: textView.traitCollection.userInterfaceIdiom == .phone)
            accessory.onPick = { [weak self, weak textView] pick in
                guard let self, let textView else { return }
                self.accept(pick, in: textView)
            }
            keypad = accessory
            textView.inputAccessoryView = accessory
            textView.reloadInputViews()
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

        /// Half a 60 Hz frame: what one keystroke may cost before answers
        /// stop being computed inline. Same rule as the Mac coordinator.
        private static let inlineBudget: TimeInterval = 0.008
        /// What the last refresh actually cost.
        private var evalCost: TimeInterval = 0

        func textViewDidChange(_ textView: UITextView) {
            guard !isSplicing else { return }
            scheduled?.cancel()
            // Answers land on the keystroke itself, with the same three
            // fallbacks to the pause as the Mac: marked text, an undo or
            // redo replay, and a document that last measured too slow.
            let undo = textView.undoManager
            let undoBusy = (undo?.isUndoing ?? false) || (undo?.isRedoing ?? false)
            if !undoBusy, textView.markedTextRange == nil, evalCost < Self.inlineBudget {
                refresh(textView)
                updateCompletions(in: textView)
                return
            }
            parent.text = textView.text
            // Restyle even while evaluation waits.
            highlight(textView, lines: Engine.lines(of: textView.text))
            scheduled = Task { [weak self, weak textView] in
                try? await Task.sleep(for: .milliseconds(120))
                guard !Task.isCancelled, let self, let textView else { return }
                guard textView.markedTextRange == nil else { return }
                self.refresh(textView)
            }
            updateCompletions(in: textView)
        }

        /// Return steps over the answer rather than through it, the same
        /// rule as the Mac editor: after typing `1+1=>` the caret sits
        /// between the arrow and the answer, and splitting the line there
        /// would strand the answer on the next line. Unlike AppKit, UIKit
        /// inserts at the range it announced — moving the selection alone
        /// changes nothing — so the newline is declined and redone by hand
        /// from the end of the line. Return also continues Markdown list
        /// markers on prose lines, and ends the list on an empty item.
        func textView(
            _ textView: UITextView,
            shouldChangeTextIn range: NSRange,
            replacementText text: String
        ) -> Bool {
            guard text == "\n", range.length == 0 else { return true }
            if let line = answerLine(at: range.location, in: textView),
               range.location >= line.afterArrow,
               range.location < line.contentsEnd
            {
                textView.selectedRange = NSRange(location: line.contentsEnd, length: 0)
                // Through insertText, so the editing pipeline — delegate
                // calls included — runs as if the key landed there.
                // Re-entry is finite: the moved caret no longer sits inside
                // an answer.
                textView.insertText("\n")
                return false
            }
            switch listContinuation(at: range.location, in: textView) {
            case .continue(let marker):
                textView.insertText("\n" + marker)
                return false
            case .terminate(let markerRange):
                // Return on an empty item ends the list: the marker goes,
                // the newline does not.
                textView.textStorage.replaceCharacters(in: markerRange, with: "")
                textView.selectedRange = NSRange(location: markerRange.location, length: 0)
                refresh(textView)
                return false
            case .none:
                return true
            }
        }

        private enum ListAction {
            case `continue`(String)
            case terminate(NSRange)
            case none
        }

        private static let listContinuationRegex = try! NSRegularExpression(
            pattern: "^(\\s*)([-*>]|\\d+\\.)( +)")

        private func listContinuation(at caret: Int, in textView: UITextView) -> ListAction {
            let text = textView.text as NSString
            let index = lineNumber(at: caret, in: text)
            guard lastLines.indices.contains(index), lastLines[index].kind == .prose else {
                return .none
            }
            var lineStart = 0
            var contentsEnd = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: &contentsEnd,
                for: NSRange(location: caret, length: 0))
            let line = text.substring(
                with: NSRange(location: lineStart, length: contentsEnd - lineStart))
            let full = NSRange(location: 0, length: (line as NSString).length)
            guard let match = Self.listContinuationRegex.firstMatch(in: line, range: full),
                  caret >= lineStart + match.range.length
            else { return .none }
            if match.range.length == full.length {
                return .terminate(NSRange(location: lineStart, length: full.length))
            }
            let bullet = (line as NSString).substring(with: match.range(at: 2))
            let next = Int(bullet.dropLast()).map { "\($0 + 1)." } ?? bullet
            let indent = (line as NSString).substring(with: match.range(at: 1))
            let gap = (line as NSString).substring(with: match.range(at: 3))
            return .continue(indent + next + gap)
        }

        /// Which line a character offset sits on.
        private func lineNumber(at location: Int, in text: NSString) -> Int {
            var lineStart = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: nil,
                for: NSRange(location: min(location, text.length), length: 0))
            return text.substring(to: lineStart).components(separatedBy: "\n").count - 1
        }

        /// The `=>` geometry of the line containing `location`, read from
        /// the live text; `nil` if the line has no answer text after its
        /// arrow. A port of the Mac coordinator's.
        private func answerLine(at location: Int, in textView: UITextView)
            -> (arrowStart: Int, afterArrow: Int, contentsEnd: Int)?
        {
            let text = textView.text as NSString
            guard location >= 0, location <= text.length else { return nil }

            var lineStart = 0
            var contentsEnd = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: &contentsEnd,
                for: NSRange(location: location, length: 0))
            let body = text.substring(
                with: NSRange(location: lineStart, length: contentsEnd - lineStart))
            guard let arrow = body.range(of: "=>") else { return nil }

            let arrowStart = lineStart + body.distance(from: body.startIndex, to: arrow.lowerBound)
            let afterArrow = lineStart + body.distance(from: body.startIndex, to: arrow.upperBound)
            guard contentsEnd > afterArrow else { return nil }
            return (arrowStart, afterArrow, contentsEnd)
        }

        func refresh(_ textView: UITextView) {
            let started = CFAbsoluteTimeGetCurrent()
            let answers = Engine.evaluate(textView.text)
            splice(answers, into: textView)
            highlight(textView, lines: Engine.lines(of: textView.text))
            parent.text = textView.text
            evalCost = CFAbsoluteTimeGetCurrent() - started
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
                let undo = textView.undoManager
                undo?.disableUndoRegistration()
                storage.beginEditing()
                for edit in edits.sorted(by: { $0.range.location > $1.range.location }) {
                    storage.replaceCharacters(in: edit.range, with: edit.replacement)
                    selection = adjust(selection, for: edit)
                }
                storage.endEditing()
                // UIKit owns this undo manager, and editing the storage
                // behind the view's back can make it reset itself —
                // removeAllActions re-enables registration as a side effect —
                // after which an unconditional enableUndoRegistration throws.
                // Re-enable only if our disable is still in force.
                if let undo, !undo.isUndoRegistrationEnabled {
                    undo.enableUndoRegistration()
                }
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

        // MARK: Inline Markdown (port of the Mac coordinator's)

        private static let codeSpanRegex = try! NSRegularExpression(pattern: "`[^`\n]+`")
        private static let boldRegex = try! NSRegularExpression(pattern: "\\*\\*[^*\n]+\\*\\*")
        private static let italicRegex = try! NSRegularExpression(
            pattern: "(?<=^|[\\s(])_[^_\n]+_(?=$|[\\s).,;:!?])")
        private static let linkRegex = try! NSRegularExpression(
            pattern: "\\[([^\\]\n]+)\\]\\(([^)\\s]+)\\)")
        private static let listMarkerRegex = try! NSRegularExpression(
            pattern: "^\\s*(?:[-*>]|\\d+\\.)\\s")

        private func applyInlineMarkdown(_ storage: NSTextStorage, in lineRange: NSRange) {
            let text = storage.string as NSString
            let line = text.substring(with: lineRange)
            let full = NSRange(location: 0, length: (line as NSString).length)
            let dim = UIColor.tertiaryLabel

            if let marker = Self.listMarkerRegex.firstMatch(in: line, range: full) {
                storage.addAttribute(
                    .foregroundColor, value: dim,
                    range: NSRange(
                        location: lineRange.location + marker.range.location,
                        length: marker.range.length))
            }

            var codeSpans: [NSRange] = []
            for match in Self.codeSpanRegex.matches(in: line, range: full) {
                codeSpans.append(match.range)
                let range = NSRange(
                    location: lineRange.location + match.range.location,
                    length: match.range.length)
                storage.addAttributes(
                    [.font: TypographyIOS.body, .foregroundColor: UIColor.label],
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

            for match in Self.boldRegex.matches(in: line, range: full)
            where outsideCode(match.range) {
                let range = NSRange(
                    location: lineRange.location + match.range.location,
                    length: match.range.length)
                storage.addAttribute(.font, value: TypographyIOS.proseBold, range: range)
                for marks in [
                    NSRange(location: range.location, length: 2),
                    NSRange(location: NSMaxRange(range) - 2, length: 2),
                ] {
                    storage.addAttribute(.foregroundColor, value: dim, range: marks)
                }
            }

            for match in Self.italicRegex.matches(in: line, range: full)
            where outsideCode(match.range) {
                let range = NSRange(
                    location: lineRange.location + match.range.location,
                    length: match.range.length)
                storage.addAttribute(.font, value: TypographyIOS.proseItalic, range: range)
                for mark in [range.location, NSMaxRange(range) - 1] {
                    storage.addAttribute(
                        .foregroundColor, value: dim,
                        range: NSRange(location: mark, length: 1))
                }
            }

            for match in Self.linkRegex.matches(in: line, range: full)
            where outsideCode(match.range) {
                let whole = NSRange(
                    location: lineRange.location + match.range.location,
                    length: match.range.length)
                storage.addAttribute(.foregroundColor, value: dim, range: whole)
                let title = NSRange(
                    location: lineRange.location + match.range(at: 1).location,
                    length: match.range(at: 1).length)
                storage.addAttribute(.foregroundColor, value: UIColor.link, range: title)
                let target = (line as NSString).substring(with: match.range(at: 2))
                if let url = URL(string: target),
                   url.scheme == "http" || url.scheme == "https"
                {
                    storage.addAttribute(.link, value: url, range: title)
                }
            }
        }

        // MARK: Links

        /// The link under a point, if the point is actually on its glyphs —
        /// `closestPosition` alone would snap taps in empty space to the
        /// nearest character and open links nobody touched.
        private func link(at point: CGPoint, in textView: UITextView) -> URL? {
            let inset = textView.textContainerInset
            let location = CGPoint(x: point.x - inset.left, y: point.y - inset.top)
            let layout = textView.layoutManager
            let index = layout.characterIndex(
                for: location, in: textView.textContainer,
                fractionOfDistanceBetweenInsertionPoints: nil)
            guard index < textView.textStorage.length else { return nil }
            let glyphs = layout.glyphRange(
                forCharacterRange: NSRange(location: index, length: 1),
                actualCharacterRange: nil)
            guard layout.boundingRect(forGlyphRange: glyphs, in: textView.textContainer)
                .insetBy(dx: -2, dy: -2).contains(location)
            else { return nil }
            return textView.textStorage.attribute(.link, at: index, effectiveRange: nil)
                as? URL
        }

        /// Claims only touches that land on a link; every other tap places
        /// the caret as usual. An editable text view never opens its own
        /// links, so this is the whole mechanism.
        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch
        ) -> Bool {
            guard let textView = gestureRecognizer.view as? UITextView else { return false }
            return link(at: touch.location(in: textView), in: textView) != nil
        }

        @objc func tappedLink(_ recognizer: UITapGestureRecognizer) {
            guard let textView = recognizer.view as? UITextView,
                  let url = link(at: recognizer.location(in: textView), in: textView)
            else { return }
            UIApplication.shared.open(url)
        }

        // MARK: Line commands (port of the Mac coordinator's)

        func installCommands(for textView: UITextView) {
            CommandBus.shared.register { [weak self, weak textView] command in
                guard let self, let textView else { return false }
                if case .preferencesChanged = command {
                    // Applies regardless of focus: the settings sheet has
                    // the editor resigned while it is up.
                    refresh(textView)
                    return false
                }
                guard textView.isFirstResponder else { return false }
                switch command {
                case .toggleComment:
                    self.transformSelectedLines(textView) { self.toggledComment($0) }
                case .indent:
                    self.transformSelectedLines(textView) { $0.isEmpty ? nil : "    " + $0 }
                case .outdent:
                    self.transformSelectedLines(textView) { line in
                        if line.hasPrefix("\t") { return String(line.dropFirst()) }
                        var trimmed = line
                        var removed = 0
                        while removed < 4, trimmed.hasPrefix(" ") {
                            trimmed.removeFirst()
                            removed += 1
                        }
                        return removed > 0 ? trimmed : nil
                    }
                case .jump, .preferencesChanged:
                    return false // jump is macOS-only; preferences handled above
                }
                return true
            }
        }

        private func toggledComment(_ line: String) -> String? {
            let indent = String(line.prefix { $0 == " " || $0 == "\t" })
            guard !indent.isEmpty, indent.count < line.count else { return nil }
            let rest = String(line.dropFirst(indent.count))
            if rest.hasPrefix("# ") { return indent + String(rest.dropFirst(2)) }
            if rest.hasPrefix("#") { return indent + String(rest.dropFirst(1)) }
            return indent + "# " + rest
        }

        private func transformSelectedLines(
            _ textView: UITextView, _ transform: (String) -> String?
        ) {
            let text = textView.text as NSString
            let span = text.lineRange(for: textView.selectedRange)
            let block = text.substring(with: span)
            let endsWithNewline = block.hasSuffix("\n")
            var lines = block.components(separatedBy: "\n")
            if endsWithNewline { lines.removeLast() }
            var replacement = lines.map { transform($0) ?? $0 }.joined(separator: "\n")
            if endsWithNewline { replacement.append("\n") }
            guard replacement != block else { return }
            textView.textStorage.replaceCharacters(in: span, with: replacement)
            let kept = (replacement as NSString).length - (endsWithNewline ? 1 : 0)
            textView.selectedRange = NSRange(location: span.location, length: max(0, kept))
            refresh(textView)
        }

        // MARK: Completions

        /// The keypad shows suggestions in its top row; set when the keypad
        /// attaches on first focus.
        weak var keypad: KeypadAccessory?

        private func wordPrefix(at caret: Int, in text: NSString) -> (NSRange, String)? {
            guard caret > 0, caret <= text.length else { return nil }
            let isWord = { (unit: unichar) -> Bool in
                guard let scalar = Unicode.Scalar(unit) else { return false }
                let ch = Character(scalar)
                return ch.isLetter || ch.isNumber || ch == "_"
            }
            if caret < text.length, isWord(text.character(at: caret)) { return nil }
            var start = caret
            while start > 0, isWord(text.character(at: start - 1)) {
                start -= 1
            }
            guard start < caret else { return nil }
            let word = text.substring(with: NSRange(location: start, length: caret - start))
            guard let first = word.first, first.isLetter || first == "_" else { return nil }
            return (NSRange(location: start, length: caret - start), word)
        }

        func updateCompletions(in textView: UITextView) {
            guard let keypad else { return }
            guard UserDefaults.standard.object(forKey: "completions") as? Bool ?? true else {
                keypad.showSuggestions([])
                return
            }
            let caret = textView.selectedRange
            let text = textView.text as NSString
            guard caret.length == 0,
                  let (_, word) = wordPrefix(at: caret.location, in: text),
                  word.count >= 2
            else {
                keypad.showSuggestions([])
                return
            }
            let line = lineNumber(at: caret.location, in: text)
            guard lastLines.indices.contains(line), lastLines[line].kind == .code else {
                keypad.showSuggestions([])
                return
            }
            var hits = Engine.completions(of: textView.text, line: line, prefix: word)
            hits.removeAll { $0.name == word }
            keypad.showSuggestions(Array(hits.prefix(4)))
        }

        func accept(_ pick: Completion, in textView: UITextView) {
            let caret = textView.selectedRange
            guard let (range, _) = wordPrefix(at: caret.location, in: textView.text as NSString)
            else { return }
            textView.selectedRange = range
            textView.insertText(pick.name)
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
            let tokenLines = Engine.tokens(of: storage.string)
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
                    self.applyInlineMarkdown(storage, in: lineRange)
                case .code:
                    // Colour by the engine's own tokens, as on the Mac.
                    if let spans = tokenLines.indices.contains(index)
                        ? tokenLines[index] : nil
                    {
                        for span in spans {
                            let range = NSRange(
                                location: lineRange.location + span.o, length: span.l)
                            guard NSMaxRange(range) <= NSMaxRange(lineRange),
                                  let color = PaletteIOS.token(span.c)
                            else { continue }
                            storage.addAttribute(
                                .foregroundColor, value: color, range: range)
                        }
                    }
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
    static var proseBold: UIFont {
        if proseUsesSystemFont { return .boldSystemFont(ofSize: baseSize) }
        return UIFont(name: "FiraCode-Bold", size: baseSize)
            ?? .monospacedSystemFont(ofSize: baseSize, weight: .bold)
    }
    static var proseItalic: UIFont {
        let base = prose
        guard let descriptor = base.fontDescriptor.withSymbolicTraits(.traitItalic) else {
            return base
        }
        return UIFont(descriptor: descriptor, size: base.pointSize)
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

    /// Code colours by token class, matching the Mac palette.
    static func token(_ class: TokenSpan.Class) -> UIColor? {
        switch `class` {
        case .num: .systemBlue
        case .str: .systemBrown
        case .kw: .systemPurple
        case .fn: .systemPink
        case .def: .systemTeal
        case .dir: .systemPurple
        case .op: .secondaryLabel
        case .name: nil
        }
    }
}
#endif
