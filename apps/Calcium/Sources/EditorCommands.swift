import Foundation

/// Editor commands travel over a small main-actor bus rather than
/// NotificationCenter: under Swift 6, notification closures are @Sendable
/// and cannot carry the main-actor coordinators.
enum EditorCommand {
    case toggleComment
    case indent
    case outdent
    case jump(line: Int)
    /// Preferences changed; restyle open documents.
    case preferencesChanged
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
