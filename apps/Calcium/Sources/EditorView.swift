import AppKit
import SwiftUI

/// The editing surface: an `NSTextView` with the answers drawn down the right
/// hand side, each one aligned to the line its `=>` sits on.
struct EditorView: NSViewRepresentable {
    @Binding var text: String
    /// Answers flow *out* only. They are derived from the text, so the editor
    /// owns them; handing them back in as a binding meant `updateNSView` could
    /// overwrite a fresh result with the stale value SwiftUI still held.
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

        let gutter = AnswerGutter(textView: textView)
        textView.addSubview(gutter)
        context.coordinator.gutter = gutter

        textView.string = text
        context.coordinator.highlight(textView)

        // Not synchronously: publishing answers is a state change and this is
        // still SwiftUI's view-building pass.
        DispatchQueue.main.async { context.coordinator.recompute(from: textView) }
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        // Only touch the text when it genuinely differs. Assigning `string`
        // while the user is typing would collapse the selection and clear undo.
        guard textView.string != text else { return }
        textView.string = text
        context.coordinator.highlight(textView)
        DispatchQueue.main.async { context.coordinator.recompute(from: textView) }
    }

    private func configure(_ textView: NSTextView) {
        textView.isRichText = false
        textView.allowsUndo = true
        textView.font = Typography.body
        textView.textContainerInset = CGSize(width: 12, height: 14)

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

    final class Coordinator: NSObject, NSTextViewDelegate {
        private var parent: EditorView
        var gutter: AnswerGutter?
        /// Coalesces recomputation so a burst of keystrokes costs one pass.
        private var pending = false

        init(_ parent: EditorView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            parent.text = textView.string
            highlight(textView)
            guard !pending else { return }
            pending = true
            // Evaluating a whole document is fast but not free, and typing
            // arrives faster than it is worth answering. One pass per runloop
            // turn keeps the feedback live without recomputing per character.
            DispatchQueue.main.async { [weak textView] in
                self.pending = false
                guard let textView else { return }
                self.recompute(from: textView)
            }
        }

        func recompute(from textView: NSTextView) {
            let fresh = Engine.evaluate(textView.string)
            gutter?.answers = fresh
            parent.onAnswersChanged(fresh)
        }

        /// Applies attributes only — never characters — so undo and the typing
        /// position are untouched.
        func highlight(_ textView: NSTextView) {
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
                // The arrow is punctuation, not content.
                if let arrow = line.range(of: "=>") {
                    let offset = line.distance(from: line.startIndex, to: arrow.lowerBound)
                    let range = NSRange(location: lineRange.location + offset, length: 2)
                    if NSMaxRange(range) <= storage.length {
                        storage.addAttribute(
                            .foregroundColor, value: NSColor.tertiaryLabelColor, range: range)
                    }
                }
            }
            storage.endEditing()
            gutter?.needsDisplay = true
        }
    }
}

enum Typography {
    static let body = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    static let heading = NSFont.monospacedSystemFont(ofSize: 13, weight: .bold)
    static let answer = NSFont.monospacedSystemFont(ofSize: 12, weight: .medium)
}

/// The answer column: a transparent overlay over the text view.
///
/// A subview rather than custom drawing inside `NSTextView`, because the text
/// view is layer-backed in a scroll view and its `draw(_:)` is never called —
/// AppKit takes the `updateLayer` path instead. As a subview it scrolls with
/// the text for free and shares its coordinate space.
final class AnswerGutter: NSView {
    private unowned let textView: NSTextView

    var answers: [Answer] = [] {
        didSet { if answers != oldValue { needsDisplay = true } }
    }

    private let preferredWidth: CGFloat = 220
    private let gap: CGFloat = 16

    init(textView: NSTextView) {
        self.textView = textView
        super.init(frame: textView.bounds)
        autoresizingMask = [.width, .height]
        // Follow the text view as it grows with the document.
        textView.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(
            self, selector: #selector(textViewFrameChanged),
            name: NSView.frameDidChangeNotification, object: textView)
        reserveColumn()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    deinit { NotificationCenter.default.removeObserver(self) }

    @objc private func textViewFrameChanged() {
        frame = textView.bounds
        reserveColumn()
        needsDisplay = true
    }

    override var isFlipped: Bool { true }
    /// Never take clicks; the text view underneath owns all interaction.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// Where the column starts, in view coordinates.
    ///
    /// One number decides both where the text stops wrapping and where the
    /// answers are drawn, so the two cannot drift apart. On a narrow window the
    /// column gives way rather than squeezing the text to nothing.
    private var columnOriginX: CGFloat {
        let inset = textView.textContainerInset.width
        let available = bounds.width - (inset * 2)
        let column = min(preferredWidth, max(90, available * 0.4))
        return bounds.width - inset - column
    }

    /// Narrows the text container so wrapped text never runs under the answers.
    private func reserveColumn() {
        guard let container = textView.textContainer else { return }
        container.widthTracksTextView = false
        let width = max(120, columnOriginX - gap - textView.textContainerInset.width)
        if container.size.width != width {
            container.size = CGSize(width: width, height: CGFloat.greatestFiniteMagnitude)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.separatorColor.withAlphaComponent(0.6).setFill()
        NSRect(x: columnOriginX - gap / 2, y: dirtyRect.minY, width: 1, height: dirtyRect.height)
            .fill()

        guard !answers.isEmpty else { return }

        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .right
        paragraph.lineBreakMode = .byTruncatingTail
        // Align baselines, not boxes: the answer is set a size smaller than the
        // body, so matching tops would leave it sitting low.
        let baselineShift = Typography.body.ascender - Typography.answer.ascender
        let columnWidth = bounds.width - textView.textContainerInset.width - columnOriginX

        for answer in answers {
            guard let frame = lineFrame(forLine: answer.line), frame.intersects(dirtyRect) else {
                continue
            }
            NSAttributedString(
                string: answer.text,
                attributes: [
                    .font: Typography.answer,
                    .foregroundColor: answer.error
                        ? NSColor.systemRed : NSColor.secondaryLabelColor,
                    .paragraphStyle: paragraph,
                ]
            ).draw(
                in: NSRect(
                    x: columnOriginX, y: frame.minY + baselineShift,
                    width: columnWidth,
                    height: Typography.body.boundingRectForFont.height + 2))
        }
    }

    /// The rectangle of a 0-based document line, in this view's coordinates.
    private func lineFrame(forLine line: Int) -> NSRect? {
        guard let offset = utf16Offset(ofLine: line) else { return nil }
        let inset = textView.textContainerInset

        if let layoutManager = textView.textLayoutManager,
           let contentManager = layoutManager.textContentManager
        {
            // TextKit 2 lays out lazily; without this every fragment answers
            // with the same origin and the column collapses onto one line.
            layoutManager.ensureLayout(for: layoutManager.documentRange)
            guard let location = contentManager.location(
                    contentManager.documentRange.location, offsetBy: offset),
                  let fragment = layoutManager.textLayoutFragment(for: location)
            else { return nil }
            var frame = fragment.layoutFragmentFrame
            frame.origin.x += inset.width
            frame.origin.y += inset.height
            return frame
        }

        // TextKit 1 fallback.
        guard let legacy = textView.layoutManager, let container = textView.textContainer else {
            return nil
        }
        let glyphRange = legacy.glyphRange(
            forCharacterRange: NSRange(location: offset, length: 0), actualCharacterRange: nil)
        var frame = legacy.boundingRect(forGlyphRange: glyphRange, in: container)
        if frame.height == 0 {
            frame.size.height = legacy.defaultLineHeight(for: Typography.body)
        }
        frame.origin.x += inset.width
        frame.origin.y += inset.height
        return frame
    }

    /// UTF-16 offset of the start of a 0-based line.
    private func utf16Offset(ofLine line: Int) -> Int? {
        guard line >= 0 else { return nil }
        let text = textView.string as NSString
        var current = 0
        var index = 0
        while index < line {
            let range = text.lineRange(for: NSRange(location: current, length: 0))
            let next = NSMaxRange(range)
            if next <= current { return nil }
            current = next
            index += 1
        }
        return current <= text.length ? current : nil
    }
}
