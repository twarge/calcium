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

    /// A suggestion row — plus, on iPhone, two 42-point key rows — and
    /// margins. Stated up front; self-sizing reads as zero at attach time.
    private let height: CGFloat

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
        self.height = keys ? 145 : 46
        // The height must be stated up front, in the frame and in
        // `intrinsicContentSize` below. Deriving it from the internal
        // constraints alone reads as zero while the input system attaches
        // the view, and the rows end up layered behind the keyboard
        // instead of docked above it.
        super.init(
            frame: CGRect(origin: .zero, size: CGSize(width: 0, height: height)),
            inputViewStyle: .keyboard)
        allowsSelfSizing = true

        let column = UIStackView()
        column.axis = .vertical
        column.spacing = 7
        column.translatesAutoresizingMaskIntoConstraints = false
        addSubview(column)
        // The bottom edge yields rather than break during the transient
        // zero-height passes the input system runs while attaching.
        let bottom = column.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -6)
        bottom.priority = UILayoutPriority(999)
        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            bottom,
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
        ])

        suggestionRow.axis = .horizontal
        suggestionRow.spacing = 6
        suggestionRow.distribution = .fillEqually
        suggestionRow.heightAnchor.constraint(equalToConstant: 32).isActive = true
        column.addArrangedSubview(suggestionRow)

        if keys {
            for titles in Self.rows {
                let row = UIStackView()
                row.axis = .horizontal
                row.spacing = 6
                row.distribution = .fillEqually
                row.heightAnchor.constraint(equalToConstant: 42).isActive = true
                for key in titles {
                    row.addArrangedSubview(button(for: key))
                }
                column.addArrangedSubview(row)
            }
        }
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
                    .font: TypographyIOS.body.withSize(15),
                    .foregroundColor: UIColor.label,
                ])
            if !item.value.isEmpty {
                title.append(
                    NSAttributedString(
                        string: "  " + item.value,
                        attributes: [
                            .font: TypographyIOS.body.withSize(13),
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
        CGSize(width: UIView.noIntrinsicMetric, height: height)
    }

    /// The system key click, same as the keyboard's own.
    var enableInputClicksWhenVisible: Bool { true }

    private func button(for key: String) -> UIButton {
        var config = UIButton.Configuration.plain()
        // The editor's own face, so `=>` ligates to ⇒ here exactly as it
        // does in the text.
        config.attributedTitle = AttributedString(
            key,
            attributes: AttributeContainer([
                .font: TypographyIOS.body.withSize(20),
                .foregroundColor: UIColor.label,
            ]))
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
}
#endif
