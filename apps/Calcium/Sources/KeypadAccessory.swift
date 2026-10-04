#if os(iOS)
import UIKit

/// The calculator rows docked above the keyboard: operators and digits,
/// the characters a calculation needs that the letters keyboard hides
/// behind a mode switch.
///
/// An input *accessory*, not a keyboard extension: it rides on top of the
/// system keyboard inside this app only, needs no enabling in Settings,
/// and leaves typing exactly as it was. `UIInputView` with the `.keyboard`
/// style draws the keyboard's own background material, so the rows read
/// as part of the keyboard rather than a toolbar stuck to it.
final class KeypadAccessory: UIInputView, UIInputViewAudioFeedback {
    private weak var textView: UITextView?

    /// `=>` last, where the return key lives on the row below it.
    private static let rows: [[String]] = [
        [".", "+", "-", "*", "/", "(", ")", "=", "=>"],
        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
    ]

    /// The fonts and row heights at the current text size; the height the
    /// view states is summed from these and the rows it holds.
    private var metrics: Metrics
    private let keyRowCount: Int
    private var suggestionRowHeight: NSLayoutConstraint?
    private var keyRowHeights: [NSLayoutConstraint] = []
    private var keyButtons: [(key: String, button: UIButton)] = []

    /// The completion strip, filled by the coordinator as an identifier is
    /// typed: names in scope with their current values, QuickType-style.
    private let suggestionRow = UIStackView()
    var onPick: ((Completion) -> Void)?
    private var suggestions: [Completion] = []

    /// `keys: false` builds only the suggestion strip — the iPad form,
    /// where the software keyboard has its own number row and a hardware
    /// keyboard would leave the key rows floating as clutter.
    init(for textView: UITextView, keys: Bool) {
        self.textView = textView
        // The text view's traits, not the accessory's own: this view has
        // no window yet, and the height is needed now.
        self.metrics = Metrics(textView.traitCollection)
        self.keyRowCount = keys ? Self.rows.count : 0
        // The height must be stated up front, in the frame and in
        // `intrinsicContentSize` below. Deriving it from the internal
        // constraints alone reads as zero while the input system attaches
        // the view, and the rows end up layered behind the keyboard
        // instead of docked above it.
        super.init(
            frame: CGRect(
                origin: .zero,
                size: CGSize(width: 0, height: metrics.height(keyRows: keyRowCount))),
            inputViewStyle: .keyboard)
        allowsSelfSizing = true

        let column = UIStackView()
        column.axis = .vertical
        column.spacing = Metrics.spacing
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)
        // The bottom edge yields rather than break during the transient
        // zero-height passes the input system runs while attaching.
        let bottom = column.bottomAnchor.constraint(
            equalTo: bottomAnchor, constant: -Metrics.bottom)
        bottom.priority = UILayoutPriority(999)
        // The sides follow the view, not its safe area: the keyboard below
        // spans the full width even where iOS 27.1's vertical bar insets
        // the safe area, and the keys should line up with it.
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: topAnchor, constant: Metrics.top),
            bottom,
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: Metrics.side),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -Metrics.side),
        ])

        suggestionRow.axis = .horizontal
        suggestionRow.spacing = 6
        suggestionRow.distribution = .fillEqually
        let suggestionHeight = suggestionRow.heightAnchor.constraint(
            equalToConstant: metrics.suggestionRow)
        suggestionHeight.isActive = true
        suggestionRowHeight = suggestionHeight
        column.addArrangedSubview(suggestionRow)

        if keys {
            for titles in Self.rows {
                let row = UIStackView()
                row.axis = .horizontal
                row.spacing = 6
                row.distribution = .fillEqually
                let height = row.heightAnchor.constraint(equalToConstant: metrics.keyRow)
                height.isActive = true
                keyRowHeights.append(height)
                for key in titles {
                    let button = self.button(for: key)
                    keyButtons.append((key, button))
                    row.addArrangedSubview(button)
                }
                column.addArrangedSubview(row)
            }
        }

        // Width needs nothing here: the rows divide whatever the keyboard
        // spans, folded or open. Text size changes the rows themselves.
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) {
            (view: KeypadAccessory, _: UITraitCollection) in view.refit()
        }
    }

    /// Rescales the rows to the current text size and restates the height;
    /// the input system resizes the accessory on its next layout pass.
    private func refit() {
        // Windowless between keyboard sessions; the real size arrives with
        // the next attach.
        guard traitCollection.preferredContentSizeCategory != .unspecified else { return }
        let fitted = Metrics(traitCollection)
        guard fitted != metrics else { return }
        metrics = fitted
        suggestionRowHeight?.constant = metrics.suggestionRow
        for height in keyRowHeights {
            height.constant = metrics.keyRow
        }
        for (key, button) in keyButtons {
            button.configuration?.attributedTitle = title(for: key)
        }
        let shown = suggestions
        suggestions = []
        showSuggestions(shown)
        invalidateIntrinsicContentSize()
    }

    /// Replaces the suggestion strip's contents. Empty clears it; the row
    /// keeps its height either way, so the keyboard never jumps.
    func showSuggestions(_ items: [Completion]) {
        guard items != suggestions else { return }
        suggestions = items
        for view in suggestionRow.arrangedSubviews {
            view.removeFromSuperview()
        }
        for (index, item) in items.enumerated() {
            var config = UIButton.Configuration.plain()
            let title = NSMutableAttributedString(
                string: item.name,
                attributes: [
                    .font: metrics.nameFont,
                    .foregroundColor: UIColor.label,
                ])
            if !item.value.isEmpty {
                title.append(
                    NSAttributedString(
                        string: "  " + item.value,
                        attributes: [
                            .font: metrics.valueFont,
                            .foregroundColor: UIColor.secondaryLabel,
                        ]))
            }
            config.attributedTitle = AttributedString(title)
            config.titleLineBreakMode = .byTruncatingTail
            config.background.backgroundColor = .systemFill.withAlphaComponent(0.06)
            config.background.cornerRadius = 6
            config.contentInsets = NSDirectionalEdgeInsets(
                top: 2, leading: 6, bottom: 2, trailing: 6)
            suggestionRow.addArrangedSubview(
                UIButton(
                    configuration: config,
                    primaryAction: UIAction { [weak self] _ in
                        guard let self, self.suggestions.indices.contains(index) else { return }
                        UIDevice.current.playInputClick()
                        self.onPick?(self.suggestions[index])
                    }))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: metrics.height(keyRows: keyRowCount))
    }

    /// The system key click, same as the keyboard's own.
    var enableInputClicksWhenVisible: Bool { true }

    /// The editor's own face, so `=>` ligates to ⇒ here exactly as it
    /// does in the text.
    private func title(for key: String) -> AttributedString {
        AttributedString(
            key,
            attributes: AttributeContainer([
                .font: metrics.keyFont,
                .foregroundColor: UIColor.label,
            ]))
    }

    private func button(for key: String) -> UIButton {
        var config = UIButton.Configuration.plain()
        config.attributedTitle = title(for: key)
        config.background.backgroundColor = .systemFill
        config.background.cornerRadius = 6
        config.contentInsets = .zero
        return UIButton(
            configuration: config,
            primaryAction: UIAction { [weak self] _ in
                UIDevice.current.playInputClick()
                self?.textView?.insertText(key)
            })
    }

    /// The rows' faces and heights at one text size: the designed sizes
    /// at the default size, scaled with Dynamic Type as far as the largest
    /// size short of the accessibility range. The keyboard below doesn't
    /// grow with the text size at all; past that point the rows would
    /// crowd out the document for keys already plainly legible.
    private struct Metrics: Equatable {
        static let top: CGFloat = 8
        static let bottom: CGFloat = 6
        static let side: CGFloat = 6
        static let spacing: CGFloat = 7

        let keyFont: UIFont
        let nameFont: UIFont
        let valueFont: UIFont
        let suggestionRow: CGFloat
        let keyRow: CGFloat

        init(_ traits: UITraitCollection) {
            var size = traits.preferredContentSizeCategory
            if size > .extraExtraExtraLarge { size = .extraExtraExtraLarge }
            let capped = UITraitCollection(preferredContentSizeCategory: size)
            let scale = UIFontMetrics(forTextStyle: .body)
            let face = TypographyIOS.body
            keyFont = scale.scaledFont(for: face.withSize(20), compatibleWith: capped)
            nameFont = scale.scaledFont(for: face.withSize(15), compatibleWith: capped)
            valueFont = scale.scaledFont(for: face.withSize(13), compatibleWith: capped)
            suggestionRow = scale.scaledValue(for: 32, compatibleWith: capped).rounded()
            keyRow = scale.scaledValue(for: 42, compatibleWith: capped).rounded()
        }

        /// The suggestion row, then each key row after its gap, inside the
        /// margins: 144 points on iPhone at the default size, 46 on iPad.
        func height(keyRows: Int) -> CGFloat {
            Self.top + suggestionRow + CGFloat(keyRows) * (Self.spacing + keyRow) + Self.bottom
        }
    }
}
#endif
