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

        // The editor ignores the top safe area, so this scroll view reaches
        // the top of the window — which also stops AppKit insetting it under
        // the toolbar automatically. Pin the inset ourselves instead, to the
        // chrome height, and keep it whether or not the chrome is showing:
        // the toolbar then overlays the text, and distraction-free hiding it
        // frees its area without the text ever shifting.
        scrollView.automaticallyAdjustsContentInsets = false
        scrollView.contentInsets = NSEdgeInsets(
            top: Coordinator.fallbackChromeHeight, left: 0, bottom: 0, right: 0)

        guard let textView = scrollView.documentView as? NSTextView else { return scrollView }
        configure(textView)
        context.coordinator.applyProofing(to: textView)
        textView.delegate = context.coordinator
        textView.string = text

        // Pinch on the trackpad scales the type. The recognizer spares us a
        // text-view subclass, which would cost the system factory and its
        // correct first paint.
        let pinch = NSMagnificationGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.pinched(_:)))
        textView.addGestureRecognizer(pinch)
        // Option-drag on a number scrubs its value. The delegate admits the
        // gesture only then, so ordinary drag-selection is untouched.
        let scrub = NSPanGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.scrubbed(_:)))
        scrub.delegate = context.coordinator
        textView.addGestureRecognizer(scrub)
        context.coordinator.installZoomShortcuts(for: textView)
        context.coordinator.followPreferences(of: textView)
        context.coordinator.installCommands(for: textView)

        // Not synchronously: publishing answers is a state change and this is
        // still SwiftUI's view-building pass. By the time this runs the view
        // is in its window, which the inset measurement needs.
        DispatchQueue.main.async {
            context.coordinator.measureChromeInset(of: scrollView)
            context.coordinator.refresh(textView)
        }
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
        // TextKit 1, deliberately. This editor rewrites storage attributes on
        // every keystroke and splices characters in behind the view's back —
        // under the default TextKit 2 stack that eventually trips a known
        // crash, `-[NSTextContentStorage locationFromLocation:withOffset:]
        // received invalid location (null)`, when viewport layout walks a
        // fragment whose locations the edits invalidated. Apple's response to
        // the matching Feedback was to use TextKit 1; reading `layoutManager`
        // is the documented downgrade. Nothing here uses TextKit 2 API.
        _ = textView.layoutManager

        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = Typography.body(1)
        textView.textContainerInset = CGSize(width: 14, height: 14)

        // Every automatic substitution is off. This is a document of
        // expressions: a smart quote, an em dash, or the system's
        // double-space-inserts-a-period will each turn a working line into a
        // syntax error, and the author will not see why. Spelling is the one
        // exception — enabled below per the preference, and confined to prose
        // by the delegate, where the same reasoning runs the other way:
        // sentences deserve the system's proofing, symbols must never get it.
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticTextCompletionEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.smartInsertDeleteEnabled = false

        // The same find bar TextEdit uses: find, replace, replace all, and
        // incremental highlighting as you type the search term.
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
    }

    // MARK: - Coordinator

    final class Coordinator: NSObject, NSTextViewDelegate, NSGestureRecognizerDelegate {
        private var parent: EditorView
        /// Line classification from the most recent highlight, for the typing
        /// attributes: an arrow key should not cost an engine call.
        private var lastLines: [LineInfo] = []
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

        // MARK: Proofing

        /// Spelling and correction only. Quotes, dashes, replacements and the
        /// rest stay off even in prose: they rewrite characters, and a prose
        /// line is one ` = ` away from becoming a calculation.
        static let proseCheckingTypes: NSTextCheckingTypes =
            NSTextCheckingResult.CheckingType.spelling.rawValue
                | NSTextCheckingResult.CheckingType.correction.rawValue

        /// Where the checker may look: prose and heading lines, and the
        /// comment tail of code lines. Rebuilt by every `highlight`, so
        /// exactly as current as the styling.
        private var checkableRanges: [NSRange] = []
        /// The text length those ranges were computed against; a mismatch
        /// means the checker raced an edit, and the answer is to check
        /// nothing rather than the wrong thing.
        private var checkableTextLength = 0

        func applyProofing(to textView: NSTextView) {
            let spelling = UserDefaults.standard.object(forKey: "proseSpelling") as? Bool ?? true
            let correct = UserDefaults.standard.object(forKey: "proseAutocorrect") as? Bool ?? true
            textView.isContinuousSpellCheckingEnabled = spelling
            textView.isAutomaticSpellingCorrectionEnabled = spelling && correct
            textView.enabledTextCheckingTypes = spelling ? Self.proseCheckingTypes : 0
        }

        /// The system checker announces each range it is about to check;
        /// ranges that touch no prose are declined outright.
        func textView(
            _ textView: NSTextView,
            willCheckTextIn range: NSRange,
            options: [NSSpellChecker.OptionKey: Any],
            types checkingTypes: UnsafeMutablePointer<NSTextCheckingTypes>
        ) -> [NSSpellChecker.OptionKey: Any] {
            guard checkableTextLength == (textView.string as NSString).length,
                  checkableRanges.contains(where: { NSIntersectionRange($0, range).length > 0 })
            else {
                checkingTypes.pointee = 0
                return options
            }
            checkingTypes.pointee &= Self.proseCheckingTypes
            return options
        }

        /// A checked range can still straddle prose and code — the checker
        /// works in its own chunks — so each finding is kept only if it lies
        /// wholly inside prose.
        func textView(
            _ textView: NSTextView,
            didCheckTextIn range: NSRange,
            types checkingTypes: NSTextCheckingTypes,
            options: [NSSpellChecker.OptionKey: Any],
            results: [NSTextCheckingResult],
            orthography: NSOrthography,
            wordCount: Int
        ) -> [NSTextCheckingResult] {
            guard checkableTextLength == (textView.string as NSString).length else { return [] }
            return results.filter { result in
                result.range.location != NSNotFound
                    && checkableRanges.contains {
                        result.range.location >= $0.location
                            && NSMaxRange(result.range) <= NSMaxRange($0)
                    }
            }
        }

        // MARK: Chrome inset

        /// The unified title-bar-plus-toolbar height on every recent macOS,
        /// used until the window can be measured.
        static let fallbackChromeHeight: CGFloat = 52

        private var chromeObservation: NSKeyValueObservation?

        /// Measures the window chrome and pins the scroll view's top inset to
        /// it — measured, not assumed, in case a future toolbar style changes
        /// the height. `contentLayoutRect` shrinks to nothing while the
        /// chrome is hidden, so the inset only ever ratchets upward: the
        /// pinned value is the chrome's height when *visible*, which is what
        /// the text must clear.
        func measureChromeInset(of scrollView: NSScrollView) {
            guard chromeObservation == nil, let window = scrollView.window else { return }
            chromeObservation = window.observe(\.contentLayoutRect, options: [.initial]) {
                [weak scrollView] window, _ in
                guard let scrollView, let contentView = window.contentView else { return }
                let measured = contentView.frame.height - window.contentLayoutRect.height
                let current = scrollView.contentInsets.top
                guard measured > current else { return }
                // If the view is resting at the top, keep it resting at the
                // new top rather than leaving the first line under the chrome.
                let atTop = scrollView.contentView.bounds.origin.y <= -(current - 1)
                scrollView.contentInsets.top = measured
                if atTop {
                    scrollView.documentView?.scroll(NSPoint(x: 0, y: -measured))
                }
            }
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
            // Any caret move closes the completion menu; during typing this
            // fires before textDidChange, which reopens it at the new word.
            completionPanel.hide()
            // The next character takes the face of the line the caret is on.
            applyTypingAttributes(textView)
            persistViewStateSoon(for: textView)
        }

        /// Styling for the caret's line, applied ahead of the keystroke.
        ///
        /// `highlight` styles text that exists; this styles text about to
        /// exist. Without it the first character typed on a line arrives in
        /// whatever face the previous edit left behind and is corrected a
        /// moment later — precisely the flicker being avoided.
        private func applyTypingAttributes(_ textView: NSTextView) {
            let caret = textView.selectedRange().location
            let text = textView.string as NSString
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
                    .font: Typography.proseHeading(scale, level: level),
                    .foregroundColor: NSColor.labelColor,
                ]
            case .prose:
                textView.typingAttributes = [
                    .font: Typography.prose(scale),
                    .foregroundColor: NSColor.secondaryLabelColor,
                ]
            case .code:
                textView.typingAttributes = [
                    .font: Typography.body(scale),
                    .foregroundColor: NSColor.labelColor,
                    .ligature: Typography.ligatures ? 1 : 0,
                ]
            }
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
                self.applyProofing(to: textView)
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

        /// What one keystroke may cost before answers stop being computed
        /// inline: half a 60 Hz frame, leaving the other half for the text
        /// view's own work.
        private static let inlineBudget: TimeInterval = 0.008
        /// What the last refresh actually cost — the estimate that decides
        /// whether the next keystroke can afford one.
        private var evalCost: TimeInterval = 0

        func textDidChange(_ notification: Notification) {
            guard !isSplicing, let textView = notification.object as? NSTextView else { return }
            scheduled?.cancel()
            // Answers land on the keystroke itself. Three things rule the
            // inline pass out, and each falls back to the pause: an
            // input-method composition (never interrupt marked text), an
            // undo or redo replay (a splice would shift the ranges the undo
            // stack recorded), and a document that last measured too slow to
            // evaluate between keystrokes.
            let undoBusy = undoManager.isUndoing || undoManager.isRedoing
            if !undoBusy, !textView.hasMarkedText(), evalCost < Self.inlineBudget {
                refreshAnswers(in: textView)
                // `#?` queries still wait for the typing to stop: each reply
                // costs a language-model request, and the in-flight guard is
                // keyed by line text, which changes with every keystroke.
                if lastLines.contains(where: { $0.query != nil }) {
                    scheduleRefresh(of: textView)
                }
            } else {
                parent.text = textView.string
                // Restyle even while evaluation waits: a character typed on
                // a prose line must be born proportional, not corrected to
                // it a beat later.
                highlight(textView, lines: Engine.lines(of: textView.string))
                scheduleRefresh(of: textView)
            }
            updateCompletions(in: textView)
        }

        /// Three interceptions, in order: keys the completion menu claims
        /// while visible; Return stepping over an answer rather than through
        /// it (after typing `1+2=>` the caret sits between the arrow and the
        /// answer, and splitting there would strand the answer on the next
        /// line); and Return continuing a list marker on prose lines.
        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            if completionPanel.isVisible {
                switch selector {
                case #selector(NSResponder.moveDown(_:)):
                    completionPanel.move(1)
                    return true
                case #selector(NSResponder.moveUp(_:)):
                    completionPanel.move(-1)
                    return true
                case #selector(NSResponder.insertTab(_:)),
                     #selector(NSResponder.insertNewline(_:)):
                    if let pick = completionPanel.current {
                        accept(pick, in: textView)
                        return true
                    }
                    completionPanel.hide()
                case #selector(NSResponder.cancelOperation(_:)):
                    completionPanel.hide()
                    return true
                default:
                    break
                }
            }
            guard selector == #selector(NSResponder.insertNewline(_:)) else { return false }
            let caret = textView.selectedRange()
            guard caret.length == 0 else { return false }

            if let line = answerLine(at: caret.location, in: textView),
               caret.location >= line.afterArrow,
               caret.location < line.contentsEnd
            {
                textView.setSelectedRange(NSRange(location: line.contentsEnd, length: 0))
                return false // let the text view insert the newline at the new spot
            }

            switch listContinuation(at: caret.location, in: textView) {
            case .continue(let marker):
                textView.insertText("\n" + marker, replacementRange: caret)
                return true
            case .terminate(let markerRange):
                // Return on an empty item ends the list: the marker goes,
                // the newline does not.
                textView.insertText("", replacementRange: markerRange)
                return true
            case .none:
                return false
            }
        }

        /// What Return should do about a Markdown list on the caret's line.
        private enum ListAction {
            case `continue`(String)
            case terminate(NSRange)
            case none
        }

        private static let listContinuationRegex = try! NSRegularExpression(
            pattern: "^(\\s*)([-*>]|\\d+\\.)( +)")

        private func listContinuation(at caret: Int, in textView: NSTextView) -> ListAction {
            let text = textView.string as NSString
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

        /// Which line a character offset sits on, counting newlines before it.
        private func lineNumber(at location: Int, in text: NSString) -> Int {
            var lineStart = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: nil,
                for: NSRange(location: min(location, text.length), length: 0))
            return text.substring(to: lineStart).components(separatedBy: "\n").count - 1
        }

        /// Waits for a pause before writing answers back — the fallback for
        /// the cases `textDidChange` rules out of the inline pass, and the
        /// path every `#?` query takes.
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

        // MARK: Completions

        /// The floating name menu. Owned per coordinator so two windows
        /// never fight over one panel.
        let completionPanel = CompletionPanel()

        /// The identifier being typed at the caret: its range and text, or
        /// nil when the caret is not at the end of a word.
        private func wordPrefix(at caret: Int, in text: NSString) -> (NSRange, String)? {
            guard caret > 0, caret <= text.length else { return nil }
            let isWord = { (unit: unichar) -> Bool in
                guard let scalar = Unicode.Scalar(unit) else { return false }
                let ch = Character(scalar)
                return ch.isLetter || ch.isNumber || ch == "_"
            }
            // Mid-word completion would replace text the author can see to
            // the right of the caret; only offer at the word's end.
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

        /// Offers names after two typed characters on a code line, values
        /// included, anchored under the word being typed.
        func updateCompletions(in textView: NSTextView) {
            let caret = textView.selectedRange()
            let text = textView.string as NSString
            guard caret.length == 0,
                  let (range, word) = wordPrefix(at: caret.location, in: text),
                  word.count >= 2
            else {
                completionPanel.hide()
                return
            }
            let line = lineNumber(at: caret.location, in: text)
            guard lastLines.indices.contains(line), lastLines[line].kind == .code else {
                completionPanel.hide()
                return
            }
            var hits = Engine.completions(of: textView.string, line: line, prefix: word)
            hits.removeAll { $0.name == word }
            guard !hits.isEmpty else {
                completionPanel.hide()
                return
            }
            let anchor = textView.firstRect(
                forCharacterRange: NSRange(location: range.location, length: 0),
                actualRange: nil)
            completionPanel.onPick = { [weak self, weak textView] pick in
                guard let self, let textView else { return }
                self.accept(pick, in: textView)
            }
            completionPanel.show(Array(hits.prefix(8)), below: anchor, scale: scale)
        }

        /// Inserts a picked name over the typed prefix, through the ordinary
        /// editing path so undo and evaluation both see it.
        func accept(_ pick: Completion, in textView: NSTextView) {
            completionPanel.hide()
            let caret = textView.selectedRange()
            guard let (range, _) = wordPrefix(at: caret.location, in: textView.string as NSString)
            else { return }
            textView.insertText(pick.name, replacementRange: range)
        }

        // MARK: Line commands

        /// Menu commands arrive by notification — the menu cannot see the
        /// focused coordinator — and the key window's editor acts.
        func installCommands(for textView: NSTextView) {
            let center = NotificationCenter.default
            let forKeyWindow = { [weak self, weak textView] (act: @escaping (Coordinator, NSTextView) -> Void) in
                { (_: Notification) in
                    guard let self, let textView,
                          textView.window?.isKeyWindow == true else { return }
                    act(self, textView)
                }
            }
            center.addObserver(
                forName: .calciumToggleComment, object: nil, queue: .main,
                using: forKeyWindow { this, view in
                    this.transformSelectedLines(view) { this.toggledComment($0) }
                })
            center.addObserver(
                forName: .calciumIndent, object: nil, queue: .main,
                using: forKeyWindow { this, view in
                    this.transformSelectedLines(view) { $0.isEmpty ? nil : "    " + $0 }
                })
            center.addObserver(
                forName: .calciumOutdent, object: nil, queue: .main,
                using: forKeyWindow { this, view in
                    this.transformSelectedLines(view) { line in
                        if line.hasPrefix("\t") { return String(line.dropFirst()) }
                        var trimmed = line
                        var removed = 0
                        while removed < 4, trimmed.hasPrefix(" ") {
                            trimmed.removeFirst()
                            removed += 1
                        }
                        return removed > 0 ? trimmed : nil
                    }
                })
            center.addObserver(
                forName: .calciumJumpToLine, object: nil, queue: .main
            ) { [weak self, weak textView] note in
                guard let self, let textView, textView.window?.isKeyWindow == true,
                      let line = note.userInfo?["line"] as? Int
                else { return }
                self.jump(to: line, in: textView)
            }
        }

        /// Puts the caret at the start of `line` and centres it.
        private func jump(to line: Int, in textView: NSTextView) {
            let text = textView.string as NSString
            var offset = 0
            var index = 0
            text.enumerateSubstrings(
                in: NSRange(location: 0, length: text.length),
                options: [.byLines, .substringNotRequired]
            ) { _, range, _, stop in
                if index == line {
                    offset = range.location
                    stop.pointee = true
                }
                index += 1
            }
            textView.window?.makeFirstResponder(textView)
            textView.setSelectedRange(NSRange(location: offset, length: 0))
            textView.centerSelectionInVisibleArea(nil)
        }

        /// If the line is indented and stays one, comments or uncomments it.
        /// Unindented lines are left alone: a leading `#` there is a heading.
        private func toggledComment(_ line: String) -> String? {
            let indent = String(line.prefix { $0 == " " || $0 == "\t" })
            guard !indent.isEmpty, indent.count < line.count else { return nil }
            let rest = String(line.dropFirst(indent.count))
            if rest.hasPrefix("# ") { return indent + String(rest.dropFirst(2)) }
            if rest.hasPrefix("#") { return indent + String(rest.dropFirst(1)) }
            return indent + "# " + rest
        }

        /// Applies a per-line rewrite to every line the selection touches,
        /// as one edit through the undo-registering path. `nil` from the
        /// transform leaves that line untouched.
        private func transformSelectedLines(
            _ textView: NSTextView, _ transform: (String) -> String?
        ) {
            let text = textView.string as NSString
            let span = text.lineRange(for: textView.selectedRange())
            let block = text.substring(with: span)
            let endsWithNewline = block.hasSuffix("\n")
            var lines = block.components(separatedBy: "\n")
            if endsWithNewline { lines.removeLast() }
            var replacement = lines.map { transform($0) ?? $0 }.joined(separator: "\n")
            if endsWithNewline { replacement.append("\n") }
            guard replacement != block,
                  textView.shouldChangeText(in: span, replacementString: replacement)
            else { return }
            textView.textStorage?.replaceCharacters(in: span, with: replacement)
            textView.didChangeText()
            let kept = (replacement as NSString).length - (endsWithNewline ? 1 : 0)
            textView.setSelectedRange(NSRange(location: span.location, length: max(0, kept)))
        }

        // MARK: Value scrubbing

        /// The number being option-dragged: where it sits now, its value
        /// when the drag began, its decimal places, and the last step count.
        private var scrubbing: (range: NSRange, value: Double, decimals: Int, steps: Int)?

        func gestureRecognizerShouldBegin(_ gestureRecognizer: NSGestureRecognizer) -> Bool {
            guard let textView = gestureRecognizer.view as? NSTextView,
                  NSEvent.modifierFlags.contains(.option)
            else { return false }
            return numberToken(at: gestureRecognizer.location(in: textView), in: textView) != nil
        }

        /// Option-drag on a number scrubs it: each few points of travel
        /// steps the last decimal place, and every step re-evaluates, so
        /// dependent answers follow the drag live.
        @objc func scrubbed(_ recognizer: NSPanGestureRecognizer) {
            guard let textView = recognizer.view as? NSTextView else { return }
            switch recognizer.state {
            case .began:
                guard let hit = numberToken(
                    at: recognizer.location(in: textView), in: textView)
                else { return }
                scrubbing = (hit.range, hit.value, hit.decimals, 0)
                undoManager.beginUndoGrouping()
            case .changed:
                guard var scrub = scrubbing else { return }
                let steps = Int(recognizer.translation(in: textView).x / 6)
                guard steps != scrub.steps else { return }
                let value = scrub.value + Double(steps) * pow(10, -Double(scrub.decimals))
                let formatted = String(format: "%.\(scrub.decimals)f", value)
                guard textView.shouldChangeText(
                    in: scrub.range, replacementString: formatted)
                else { return }
                textView.textStorage?.replaceCharacters(in: scrub.range, with: formatted)
                textView.didChangeText()
                scrub.range.length = (formatted as NSString).length
                scrub.steps = steps
                scrubbing = scrub
            default:
                if scrubbing != nil {
                    undoManager.endUndoGrouping()
                    scrubbing = nil
                }
            }
        }

        /// The plain decimal number under a point, or nil. Hex, exponents
        /// and fractions are not scrubbed: their "one step" is not obvious.
        private func numberToken(at point: NSPoint, in textView: NSTextView)
            -> (range: NSRange, value: Double, decimals: Int)?
        {
            let index = textView.characterIndexForInsertion(at: point)
            let text = textView.string as NSString
            guard index >= 0, index < text.length else { return nil }
            let line = lineNumber(at: index, in: text)
            var lineStart = 0
            text.getLineStart(
                &lineStart, end: nil, contentsEnd: nil,
                for: NSRange(location: index, length: 0))
            let local = index - lineStart
            let tokens = Engine.tokens(of: textView.string)
            guard tokens.indices.contains(line),
                  let span = tokens[line].first(where: {
                      $0.c == .num && local >= $0.o && local <= $0.o + $0.l
                  })
            else { return nil }
            let range = NSRange(location: lineStart + span.o, length: span.l)
            let raw = text.substring(with: range)
            guard raw.allSatisfy({ $0.isNumber || $0 == "." }),
                  let value = Double(raw)
            else { return nil }
            let decimals = raw.contains(".") ? raw.split(separator: ".").last!.count : 0
            return (range, value, decimals)
        }

        // MARK: Recomputing

        func refresh(_ textView: NSTextView) {
            let lines = refreshAnswers(in: textView)
            // The delegate always runs on the main thread; say so to the
            // compiler, which cannot see it.
            MainActor.assumeIsolated {
                autocomplete.resolveFirstQuery(in: textView, lines: lines)
            }
        }

        /// Evaluates, splices and restyles — and measures itself, which is
        /// what decides whether the next keystroke can do this inline.
        @discardableResult
        private func refreshAnswers(in textView: NSTextView) -> [LineInfo] {
            let started = CFAbsoluteTimeGetCurrent()
            // The engine ignores whatever follows a `=>`, so the buffer can be
            // handed over as-is; no need to strip the previous answers first.
            let answers = Engine.evaluate(textView.string)
            splice(answers, into: textView)
            let lines = Engine.lines(of: textView.string)
            highlight(textView, lines: lines)
            parent.text = textView.string
            evalCost = CFAbsoluteTimeGetCurrent() - started
            return lines
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
            lastLines = lines
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
            let tokenLines = Engine.tokens(of: storage.string)
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
                        value: Typography.proseHeading(self.scale, level: line?.level ?? 1),
                        range: lineRange)
                    checkable.append(lineRange)
                case .prose:
                    // Prose sits back a little so the calculations carry the
                    // page — and, by default, in the system's own face, so
                    // sentences read as sentences and code reads as code.
                    storage.addAttributes(
                        [
                            .font: Typography.prose(self.scale),
                            .foregroundColor: NSColor.secondaryLabelColor,
                        ], range: lineRange)
                    self.applyInlineMarkdown(storage, in: lineRange)
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
            checkableRanges = checkable
            checkableTextLength = storage.length
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

        private func applyInlineMarkdown(_ storage: NSTextStorage, in lineRange: NSRange) {
            let text = storage.string as NSString
            let line = text.substring(with: lineRange)
            let full = NSRange(location: 0, length: (line as NSString).length)
            let dim = NSColor.tertiaryLabelColor

            if let marker = Self.listMarkerRegex.firstMatch(in: line, range: full) {
                storage.addAttribute(
                    .foregroundColor, value: dim,
                    range: NSRange(
                        location: lineRange.location + marker.range.location,
                        length: marker.range.length))
            }

            // Code spans claim their territory first; emphasis does not
            // reach inside them.
            var codeSpans: [NSRange] = []
            for match in Self.codeSpanRegex.matches(in: line, range: full) {
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

            for match in Self.boldRegex.matches(in: line, range: full)
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

            for match in Self.italicRegex.matches(in: line, range: full)
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

            for match in Self.linkRegex.matches(in: line, range: full)
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

    /// Code colours by token class, from the engine's own lexer. `nil` means
    /// the default text colour — ordinary names stay quiet so the structure
    /// (numbers, the defined name, keywords) is what carries colour.
    static func token(_ class: TokenSpan.Class) -> NSColor? {
        switch `class` {
        case .num: .systemBlue
        case .str: .systemBrown
        case .kw: .systemPurple
        case .fn: .systemPink
        case .def: .systemTeal
        case .dir: .systemPurple
        case .op: .secondaryLabelColor
        case .name: nil
        }
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
