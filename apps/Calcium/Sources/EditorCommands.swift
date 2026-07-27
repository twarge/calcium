import Foundation

/// Editor commands travel by notification: menu items and toolbar controls
/// cannot see the focused coordinator, so they broadcast and the key
/// window's editor acts.
extension Notification.Name {
    static let calciumToggleComment = Notification.Name("calciumToggleComment")
    static let calciumIndent = Notification.Name("calciumIndent")
    static let calciumOutdent = Notification.Name("calciumOutdent")
    /// userInfo: `["line": Int]`, 0-based.
    static let calciumJumpToLine = Notification.Name("calciumJumpToLine")
}
