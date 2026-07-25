#if os(macOS)
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
    var fileURL: URL?

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

        // Pinch on the trackpad scales the type. The recognizer spares us a
        // text-view subclass, which would cost the system factory and its
        // correct first paint.
        let pinch = NSMagnificationGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.pinched(_:)))
        textView.addGestureRecognizer(pinch)
        context.coordinator.installZoomShortcuts(for: textView)
        context.coordinator.followPreferences(of: textView)

        // Not synchronously: publishing answers is a state change and this is
        // still SwiftUI's view-building pass.
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        context.coordinator.fileURL = fileURL
        // Only touch the text when it genuinely differs. Assigning `string`
        // while the user is typing would collapse the selection and clear undo.
        guard textView.string != text else { return }
        textView.string = text
        DispatchQueue.main.async { context.coordinator.refresh(textView) }
    }

    private func configure(_ textView: NSTextView) {
        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = Typography.body(1)
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
        /// Resolves `#?` requests against the on-device model.
        private let autocomplete = Autocomplete()

        init(_ parent: EditorView) {
            self.parent = parent
        }

        // MARK: Per-document view state

        /// Where this document lives, for the view-state extended attribute.
        var fileURL: URL?
        /// This document's zoom. Multiplies the Preferences font size.
        private var scale: CGFloat = 1
        /// Debounces state writes; arrow keys should not each cost an xattr.
        private var statePersist: DispatchWorkItem?

        func restoreViewState(in textView: NSTextView) {
            guard let url = fileURL, let state = DocumentViewState.load(from: url) else {
                return
            }
            scale = CGFloat(state.scale)
            let length = (textView.string as NSString).length
            textView.setSelectedRange(
                NSRange(location: min(max(0, state.cursor), length), length: 0))
        }

        private func persistViewStateSoon(for textView: NSTextView) {
            statePersist?.cancel()
            let item = DispatchWorkItem { [weak self, weak textView] in
                guard let self, let textView, let url = self.fileURL else { return }
                DocumentViewState(
                    scale: Double(self.scale),
                    cursor: textView.selectedRange().location
                ).save(to: url)
            }
            statePersist = item
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4, execute: item)
        }

        func textViewDidChangeSelection(_ notification: Notification) {
            guard !isSplicing, let textView = notification.object as? NSTextView else { return }
            persistViewStateSoon(for: textView)
        }

        // MARK: Zoom

        /// Scale at the moment the pinch began.
        private var pinchBase: CGFloat = 1

        @objc func pinched(_ recognizer: NSMagnificationGestureRecognizer) {
            guard let textView = recognizer.view as? NSTextView else { return }
            switch recognizer.state {
            case .began:
                pinchBase = scale
            case .changed:
                scale = pinchBase * (1 + recognizer.magnification)
                rescale(textView)
            default:
                break
            }
        }

        /// ⌘+ / ⌘− / ⌘0, through the same path as the pinch. Installed as a
        /// local monitor because the coordinator is not in the responder
        /// chain, and a text-view subclass would cost the system factory.
        private var keyMonitor: Any?

        /// Re-styles when Preferences change, so an open document follows the
        /// font-size slider live.
        func followPreferences(of textView: NSTextView) {
            NotificationCenter.default.addObserver(
                forName: UserDefaults.didChangeNotification, object: nil, queue: .main
            ) { [weak self, weak textView] _ in
                guard let self, let textView else { return }
                self.rescale(textView)
            }
        }

        func installZoomShortcuts(for textView: NSTextView) {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) {
                [weak self, weak textView] event in
                guard let self, let textView,
                      textView.window?.isKeyWindow == true,
                      event.modifierFlags.contains(.command),
                      !event.modifierFlags.contains(.shift),
                      let key = event.charactersIgnoringModifiers
                else { return event }
                switch key {
                case "=", "+": self.zoom(textView, by: 1.1)
                case "-": self.zoom(textView, by: 1 / 1.1)
                case "0":
                    self.scale = 1
                    self.rescale(textView)
                default: return event
                }
                return nil
            }
        }

        deinit {
            if let keyMonitor {
                NSEvent.removeMonitor(keyMonitor)
            }
        }

        func zoom(_ textView: NSTextView, by factor: CGFloat) {
            scale *= factor
            rescale(textView)
        }

        private func rescale(_ textView: NSTextView) {
            scale = min(max(scale, 0.5), 4)
            textView.font = Typography.body(scale)
            highlight(textView, lines: Engine.lines(of: textView.string))
            persistViewStateSoon(for: textView)
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
            let lines = Engine.lines(of: textView.string)
            highlight(textView, lines: lines)
            // The delegate always runs on the main thread; say so to the
            // compiler, which cannot see it.
            MainActor.assumeIsolated {
                autocomplete.resolveFirstQuery(in: textView, lines: lines)
            }
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
                storage.beginEditing()
                for edit in edits.sorted(by: { $0.range.location > $1.range.location }) {
                    storage.replaceCharacters(in: edit.range, with: edit.replacement)
                    selection = adjust(selection, for: edit)
                }
                storage.endEditing()

                // Tell the text view its text changed. Mutating the storage
                // directly bypasses the path that normally does this, so
                // layout below the edit is never invalidated: the rest of the
                // page stops drawing until a scroll or a caret move forces it
                // back. Called while `isSplicing` still holds, so the
                // notification is not mistaken for the user's own edit.
                textView.didChangeText()

                undoManager.enableUndoRegistration()
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
        private func highlight(_ textView: NSTextView, lines: [LineInfo]) {
            guard let storage = textView.textStorage else { return }
            let whole = NSRange(location: 0, length: storage.length)
            storage.beginEditing()
            storage.setAttributes(
                [
                    .font: Typography.body(scale),
                    .foregroundColor: NSColor.labelColor,
                    // 0 disables ligatures; 1 is the font's defaults.
                    .ligature: Typography.ligatures ? 1 : 0,
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
                        .font, value: Typography.heading(self.scale), range: lineRange)
                case .prose:
                    // Prose sits back a little so the calculations carry the page.
                    storage.addAttribute(
                        .foregroundColor, value: NSColor.secondaryLabelColor, range: lineRange)
                case .code:
                    break
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

            // The answers themselves: set back from the text the author wrote,
            // so it stays obvious which is which.
            for region in answerRegions where NSMaxRange(region.range) <= storage.length {
                storage.addAttributes(
                    [
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

enum Palette {
    /// A slate grey-blue: clearly an aside, without the flatness of plain grey,
    /// and distinct from the grey the answers use. Resolved per appearance so
    /// it holds its weight in both themes.
    static let comment = NSColor(name: "comment") { appearance in
        let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return dark
            ? NSColor(srgbRed: 0.46, green: 0.55, blue: 0.66, alpha: 1)
            : NSColor(srgbRed: 0.38, green: 0.47, blue: 0.58, alpha: 1)
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
