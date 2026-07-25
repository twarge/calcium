import AppKit
import SwiftUI

/// The editing surface.
///
/// Answers live *in* the text, written in after each `=>` as you type. That is
/// what makes the file self-describing: what you see is what is on disk, with
/// no separate display layer to keep in step.
///
/// The cost is that the app edits the buffer behind the user, which is exactly
/// the thing a text editor must not get wrong. Three rules keep it honest:
///
///  * answers are spliced through the text storage directly, so they never
///    enter the undo stack — undo steps over the user's own edits, never ours;
///  * the insertion point is adjusted for every splice that happens before it,
///    so typing never jumps;
///  * an answer is not editable, and the caret will not sit inside one.
struct EditorView: NSViewRepresentable {
    @Binding var text: String
    var onAnswersChanged: ([Answer]) -> Void

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
    }

    // MARK: - Coordinator

    final class Coordinator: NSObject, NSTextViewDelegate {
        private var parent: EditorView
        /// Where the answers currently sit, for styling and for refusing edits.
        private var answerRegions: [(range: NSRange, isError: Bool)] = []
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

        /// An answer is not the author's text, so it is not theirs to edit.
        /// A change that reaches outside one — selecting whole lines and
        /// deleting them, say — is allowed through and simply recomputed.
        ///
        /// Judged from the live text, not from `answerRegions`. Those are one
        /// refresh out of date while the user types, and a stale range here
        /// silently *rejects* keystrokes: after typing two characters before
        /// the arrow, the caret sits exactly where the previous answer began.
        func textView(
            _ textView: NSTextView, shouldChangeTextIn range: NSRange,
            replacementString: String?
        ) -> Bool {
            if isSplicing { return true }
            return !isInsideAnswer(range, in: textView)
        }

        /// Whether `range` lies entirely in the answer part of its line.
        private func isInsideAnswer(_ range: NSRange, in textView: NSTextView) -> Bool {
            guard let line = answerLine(at: range.location, in: textView) else { return false }
            return range.location >= line.afterArrow && NSMaxRange(range) <= line.contentsEnd
        }

        /// Keeps the caret out of the answers.
        ///
        /// It snaps to *before* the `=>`, not after it. After the arrow is
        /// still the engine's text, so pressing End on an answered line would
        /// leave the caret somewhere it cannot type. Before the arrow is where
        /// the author's expression ends, which is what End should mean.
        func textView(
            _ textView: NSTextView,
            willChangeSelectionFromCharacterRange old: NSRange,
            toCharacterRange new: NSRange
        ) -> NSRange {
            guard new.length == 0 else { return new } // a drag-selection may span
            guard let barrier = answerBarrier(at: new.location, in: textView) else { return new }
            return NSRange(location: barrier, length: 0)
        }

        /// Where the caret belongs if `location` has landed in a line's answer,
        /// or `nil` if it is somewhere the author owns.
        private func answerBarrier(at location: Int, in textView: NSTextView) -> Int? {
            guard let line = answerLine(at: location, in: textView) else { return nil }
            return location > line.arrowStart ? line.arrowStart : nil
        }

        /// The `=>` geometry of the line containing `location`, read from the
        /// live text, or `nil` if that line has no answer on it yet.
        ///
        /// Everything that has to reason about where an answer sits goes
        /// through here. The cached `answerRegions` are a refresh out of date
        /// while the user is typing, and acting on them silently swallows
        /// keystrokes and drags the caret backwards.
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
            // Nothing to guard until an answer has actually been written; the
            // author must be able to sit right after an arrow they just typed.
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
            parent.onAnswersChanged(answers)
        }

        /// Replaces the text after each `=>` with its freshly computed answer.
        private func splice(_ answers: [Answer], into textView: NSTextView) {
            guard let storage = textView.textStorage else { return }
            let text = storage.string as NSString
            let lines = lineRanges(in: text)

            // Work out every edit first, then apply them back-to-front so that
            // earlier offsets stay valid as we go.
            var edits: [(range: NSRange, replacement: String)] = []
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
                    let delta = (edit.replacement as NSString).length - edit.range.length
                    if NSMaxRange(edit.range) <= selection.location {
                        // The edit was entirely before the caret; shift it.
                        selection.location += delta
                    } else if edit.range.location < selection.location {
                        // The caret was inside the answer being replaced.
                        selection.location = edit.range.location
                        selection.length = 0
                    }
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
                if let arrow = line.range(of: "=>") {
                    let offset = line.distance(from: line.startIndex, to: arrow.lowerBound)
                    let range = NSRange(location: lineRange.location + offset, length: 2)
                    if NSMaxRange(range) <= storage.length {
                        storage.addAttribute(
                            .foregroundColor, value: NSColor.tertiaryLabelColor, range: range)
                    }
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
