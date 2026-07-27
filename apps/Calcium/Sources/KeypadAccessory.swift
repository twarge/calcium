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

    /// Two 42-point rows plus their margins.
    private static let height: CGFloat = 105

    init(for textView: UITextView) {
        self.textView = textView
        // The height must be stated up front, in the frame and in
        // `intrinsicContentSize` below. Deriving it from the internal
        // constraints alone reads as zero while the input system attaches
        // the view, and the rows end up layered behind the keyboard
        // instead of docked above it.
        super.init(
            frame: CGRect(origin: .zero, size: CGSize(width: 0, height: Self.height)),
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

        for keys in Self.rows {
            let row = UIStackView()
            row.axis = .horizontal
            row.spacing = 6
            row.distribution = .fillEqually
            row.heightAnchor.constraint(equalToConstant: 42).isActive = true
            for key in keys {
                row.addArrangedSubview(button(for: key))
            }
            column.addArrangedSubview(row)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: Self.height)
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
