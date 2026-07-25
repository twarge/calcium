import AppKit
import SwiftUI

/// The editing surface.
///
/// Answers live *in* the text, written in after each `=>`. Type `1+2=>` and the
/// answer appears after the caret, which stays where you left it — so Return
/// carries on to the next line and typing carries on where you were.
///
/// Nothing is locked. The caret goes anywhere, including past the answer, and
/// any edit is allowed. Backspacing into an answer deletes a character that is
/// immediately written back, so the visible effect is simply that the caret
/// steps left. Protecting the answer would take more machinery and read worse:
/// letting it be overwritten and restored gets the same result for free.
///
/// What that does demand is exact bookkeeping of the insertion point across a
/// splice, which is what `adjust(_:for:)` is for.
struct EditorView: NSViewRepresentable {
    @Binding var text: String

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        // The system factory, deliberately. A hand-built TextKit stack looks
        // equivalent but never gets its first paint — documents opened blank
        // until they were clicked.
        let scrollView = NSTextView.scrollableTextView()
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.borderType = .noBorder

        guard let textView = scrollView.documentView as? NSTextView else { return scrollView }
        configure(textView)
        textView.delegate = context.coordinator
        textView.string = text

        // Not synchronously: publishing answers is a state change and this is
        // still SwiftUI's view-building pass.
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        // Only touch the text when it genuinely differs. Assigning `string`
        // while the user is typing would collapse the selection and clear undo.
        guard textView.string != text else { return }
        textView.string = text
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
    }

    private func configure(_ textView: NSTextView) {
        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = Typography.body
        textView.textContainerInset = CGSize(width: 14, height: 14)

        // Every automatic substitution is off. This is a document of
        // expressions: a smart quote, an em dash, or the system's
        // double-space-inserts-a-period will each turn a working line into a
        // syntax error, and the author will not see why.
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isAutomaticTextCompletionEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.smartInsertDeleteEnabled = false
        textView.enabledTextCheckingTypes = 0

        // The same find bar TextEdit uses: find, replace, replace all, and
        // incremental highlighting as you type the search term.
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
    }

    // MARK: - Coordinator

    final class Coordinator: NSObject, NSTextViewDelegate {
        private var parent: EditorView
        /// Where the answers currently sit, for styling.
        private var answerRegions: [(range: NSRange, isError: Bool)] = []
        /// The answer text last written to each line, so that deleting a `=>`
        /// can take its answer with it.
        private var lastAnswerByLine: [Int: String] = [:]
        /// True while we are splicing, so our own edits are not mistaken for
        /// the user's.
        private var isSplicing = false
        /// The pending recompute, cancelled and rescheduled on every keystroke.
        private var scheduled: DispatchWorkItem?
        /// The text view's own undo stack.
        ///
        /// Without this it shares the window's, which under `DocumentGroup` is
        /// also where SwiftUI records every write to the document binding — and
        /// this editor writes to that binding on every keystroke. The two
        /// interleave and Cmd-Z ends up doing nothing at all.
        private let undoManager = UndoManager()

        init(_ parent: EditorView) {
            self.parent = parent
        }

        // MARK: Editing

        func undoManager(for view: NSTextView) -> UndoManager? { undoManager }

        func textDidChange(_ notification: Notification) {
            guard !isSplicing, let textView = notification.object as? NSTextView else { return }
            parent.text = textView.string
            scheduleRefresh(of: textView)
        }

        /// Return steps over the answer rather than through it.
        ///
        /// After typing `1+2=>` the caret sits between the arrow and the
        /// answer, which is where the author left it. Splitting the line there
        /// would strand the answer on the next line, so Return goes to the end
        /// of the line first — which is what the author meant by it.
        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            guard selector == #selector(NSResponder.insertNewline(_:)) else { return false }
            let caret = textView.selectedRange()
            guard caret.length == 0,
                  let line = answerLine(at: caret.location, in: textView),
                  caret.location >= line.afterArrow,
                  caret.location < line.contentsEnd
            else { return false }
            textView.setSelectedRange(NSRange(location: line.contentsEnd, length: 0))
            return false // let the text view insert the newline at the new spot
        }

        /// Waits for a pause before writing answers back.
        ///
        /// This is the price of keeping answers in the text: rewriting the
        /// buffer between two keystrokes disturbs the text view's input
        /// handling and characters go missing. Recomputing on the next runloop
        /// turn is not enough, because that still lands in the middle of a
        /// burst of typing. So we wait for the typing to stop.
        private func scheduleRefresh(of textView: NSTextView) {
            scheduled?.cancel()
            let item = DispatchWorkItem { [weak self, weak textView] in
                guard let self, let textView else { return }
                // Never interrupt an in-progress input method composition.
                guard !textView.hasMarkedText() else {
                    self.scheduleRefresh(of: textView)
                    return
                }
                self.refresh(textView)
            }
            scheduled = item
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: item)
        }

        /// The `=>` geometry of the line containing `location`, read from the
        /// live text, or `nil` if that line has no answer on it yet.
        ///
        /// Live, not cached: `answerRegions` is a refresh out of date while the
        /// user is typing, and acting on stale offsets moves the caret to the
        /// wrong place.
        private func answerLine(at location: Int, in textView: NSTextView)
            -> (arrowStart: Int, afterArrow: Int, contentsEnd: Int)?
        {
            let text = textView.string as NSString
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

        // MARK: Recomputing

        func refresh(_ textView: NSTextView) {
            // The engine ignores whatever follows a `=>`, so the buffer can be
            // handed over as-is; no need to strip the previous answers first.
            let answers = Engine.evaluate(textView.string)
            splice(answers, into: textView)
            highlight(textView)
            parent.text = textView.string
        }

        /// Replaces the text after each `=>` with its freshly computed answer.
        private func splice(_ answers: [Answer], into textView: NSTextView) {
            guard let storage = textView.textStorage else { return }
            let text = storage.string as NSString
            let lines = lineRanges(in: text)

            // Work out every edit first, then apply them back-to-front so that
            // earlier offsets stay valid as we go.
            var edits: [(range: NSRange, replacement: String)] = []

            // Deleting the `=>` should take its answer with it, rather than
            // leaving the digits behind as ordinary text. Only the caret's own
            // line is considered: that is where an arrow can have just been
            // deleted, and it sidesteps line numbers shifting underneath us.
            let caretLine = lineIndex(of: textView.selectedRange().location, in: lines)
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

                let afterArrow = line.location + body.distance(
                    from: body.startIndex, to: arrow.upperBound)
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

            var selection = textView.selectedRange()
            if !edits.isEmpty {
                isSplicing = true
                // Answers are not the author's edits, so they must not land on
                // the undo stack — and, just as importantly, must not shift the
                // ranges of edits already recorded there. Undo has to step over
                // the user's own typing and nothing else.
                undoManager.disableUndoRegistration()
                defer { undoManager.enableUndoRegistration() }
                storage.beginEditing()
                for edit in edits.sorted(by: { $0.range.location > $1.range.location }) {
                    storage.replaceCharacters(in: edit.range, with: edit.replacement)
                    selection = adjust(selection, for: edit)
                }
                storage.endEditing()
                isSplicing = false
            }

            // Record where the answers ended up, for styling and for keeping
            // the caret out of them. This has to happen *before* the selection
            // is restored: `setSelectedRange` runs through the snapping rule
            // below, and against stale offsets it drags the caret backwards by
            // however much the splice shifted the line.
            let updated = storage.string as NSString
            let freshLines = lineRanges(in: updated)
            answerRegions = answers.compactMap { answer in
                guard answer.line >= 0, answer.line < freshLines.count else { return nil }
                let line = freshLines[answer.line]
                let body = updated.substring(with: line)
                guard let arrow = body.range(of: "=>") else { return nil }
                let afterArrow = line.location + body.distance(
                    from: body.startIndex, to: arrow.upperBound)
                let length = NSMaxRange(line) - afterArrow
                guard length > 0 else { return nil }
                return (NSRange(location: afterArrow, length: length), answer.error)
            }

            if !edits.isEmpty {
                selection.location = max(0, min(selection.location, updated.length))
                selection.length = min(selection.length, updated.length - selection.location)
                textView.setSelectedRange(selection)
            }
        }

        /// Moves the insertion point to account for one splice.
        ///
        /// Three cases, and the boundaries are what make the editor feel right:
        ///
        ///  * **At or before the edit** — unchanged. An answer written at the
        ///    caret therefore appears *after* it: type `1+2=>` and the caret
        ///    stays put with ` 3` to its right.
        ///  * **Inside the edit, up to and including its far end** — the offset
        ///    into the answer is kept, clamped to the new length. This is what
        ///    turns a backspace into a step left: the character is deleted and
        ///    written straight back, leaving only the caret moved.
        ///  * **Past the edit** — shifted by the change in length.
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

        /// Applies attributes only — never characters — so undo and the typing
        /// position are untouched.
        private func highlight(_ textView: NSTextView) {
            guard let storage = textView.textStorage else { return }
            let whole = NSRange(location: 0, length: storage.length)
            storage.beginEditing()
            storage.setAttributes(
                [.font: Typography.body, .foregroundColor: NSColor.labelColor], range: whole)

            let text = storage.string as NSString
            text.enumerateSubstrings(in: whole, options: [.byLines, .substringNotRequired]) {
                _, lineRange, _, _ in
                let line = text.substring(with: lineRange)
                let trimmed = line.trimmingCharacters(in: .whitespaces)

                if trimmed.hasPrefix("#") {
                    storage.addAttribute(.font, value: Typography.heading, range: lineRange)
                    return
                }
                // Prose sits back a little so the calculations carry the page.
                let indented = line.hasPrefix(" ") || line.hasPrefix("\t")
                if !indented && !line.contains("=>") && !trimmed.isEmpty {
                    storage.addAttribute(
                        .foregroundColor, value: NSColor.secondaryLabelColor, range: lineRange)
                }
            }

            // The answers themselves: set back from the text the author wrote,
            // so it stays obvious which is which.
            for region in answerRegions where NSMaxRange(region.range) <= storage.length {
                storage.addAttributes(
                    [
                        .font: Typography.answer,
                        .foregroundColor: region.isError
                            ? NSColor.systemRed : NSColor.secondaryLabelColor,
                    ], range: region.range)
            }
            storage.endEditing()
        }

        /// Which line a character offset falls on.
        private func lineIndex(of location: Int, in lines: [NSRange]) -> Int? {
            lines.firstIndex { location >= $0.location && location <= NSMaxRange($0) }
        }

        /// Line ranges excluding terminators, matching how the engine counts
        /// lines: no trailing empty line after a final newline.
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
    }
}

enum Typography {
    static let body = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    static let heading = NSFont.monospacedSystemFont(ofSize: 13, weight: .bold)
    static let answer = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
}
