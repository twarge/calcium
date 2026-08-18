import Foundation

/// Editor commands travel over a small main-actor bus rather than
/// NotificationCenter: under Swift 6, notification closures are @Sendable
/// and cannot carry the main-actor coordinators.
enum EditorCommand {
    case toggleComment
    case indent
    case outdent
    case jump(line: Int)
    /// Format > Bold/Italic/Code: the marker pair toggled around the
    /// selection — `**`, `_`, or `` ` ``, as the prose styler draws them.
    case toggleMark(String)
    /// Format > Heading 1–3: the `#` prefix set, switched, or removed on
    /// the selected lines.
    case heading(Int)
    /// Format > Blockquote: a leading `> ` toggled on the selected lines.
    case blockquote
    /// Format > Link: the selection wrapped as `[title](url)`.
    case insertLink
    /// Format > Insert Directive: a `@directive` line placed above the
    /// calculation the caret is in.
    case insertDirective(String)
    /// Preferences changed; restyle open documents.
    case preferencesChanged
    /// File > Export to Typst…, macOS only.
    case exportTypst
    /// File > Typeset PDF, macOS only.
    case typesetPDF
}

@MainActor
final class CommandBus {
    static let shared = CommandBus()

    /// A handler returns true if it acted — it belonged to the key window —
    /// which stops delivery; `preferencesChanged` goes to everyone.
    private var handlers: [(EditorCommand) -> Bool] = []

    func register(_ handler: @escaping (EditorCommand) -> Bool) {
        handlers.append(handler)
    }

    func send(_ command: EditorCommand) {
        if case .preferencesChanged = command {
            for handler in handlers {
                _ = handler(command)
            }
            return
        }
        for handler in handlers where handler(command) {
            break
        }
    }
}

/// A weak reference allowed across @Sendable boundaries — KVO handlers and
/// system callbacks documented to run on the main thread — where the callee
/// re-enters the actor explicitly with `MainActor.assumeIsolated`.
struct MainActorWeak<T: AnyObject>: @unchecked Sendable {
    weak var value: T?
    init(_ value: T?) { self.value = value }
}
