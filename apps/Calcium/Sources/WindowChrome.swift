import AppKit
import SwiftUI

/// Distraction-free window chrome: the title bar is invisible until the
/// pointer moves into where it would be, then it fades in — buttons, title,
/// document proxy — and fades back out when the pointer leaves.
///
/// SwiftUI's `DocumentGroup` never hands over the `NSWindow`, so this rides in
/// as a background view, walks up to the window once it is in one, and does
/// its work in AppKit. The pointer is watched with an event monitor rather
/// than a tracking-area view: anything added into SwiftUI's hierarchy is
/// liable to be reconciled away on its next layout pass, and an event monitor
/// is out of its reach.
struct WindowChrome: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { ChromeView() }
    func updateNSView(_ view: NSView, context: Context) {}
}

private final class ChromeView: NSView {
    private var monitor: Any?
    private var chromeVisible = false

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else { return }

        // Content under the whole frame; the title bar drawn over it, at the
        // moment not at all.
        window.styleMask.insert(.fullSizeContentView)
        window.titlebarAppearsTransparent = true
        window.titlebarSeparatorStyle = .none
        window.acceptsMouseMovedEvents = true
        apply(visible: false, animated: false)

        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved]) {
            [weak self] event in
            self?.pointerMoved(event)
            return event
        }
    }

    deinit {
        if let monitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    private func pointerMoved(_ event: NSEvent) {
        guard let window else { return }
        // Only this window's events; a move in another window hides ours.
        let inside = event.window === window
            && window.frame.height - event.locationInWindow.y <= titleBarHeight
            && event.locationInWindow.y <= window.frame.height
        setChrome(visible: inside)
    }

    /// How tall the hidden title bar is, measured rather than assumed: the
    /// gap between the frame and the content layout area.
    private var titleBarHeight: CGFloat {
        guard let window else { return 28 }
        let height = window.frame.height - window.contentLayoutRect.height
        return height > 0 ? height : 28
    }

    private func setChrome(visible: Bool) {
        guard visible != chromeVisible else { return }
        chromeVisible = visible
        apply(visible: visible, animated: true)
    }

    private func apply(visible: Bool, animated: Bool) {
        guard let window else { return }
        window.titleVisibility = visible ? .visible : .hidden
        let buttons: [NSWindow.ButtonType] = [.closeButton, .miniaturizeButton, .zoomButton]
        let change = {
            for kind in buttons {
                let button = window.standardWindowButton(kind)
                (animated ? button?.animator() : button)?.alphaValue = visible ? 1 : 0
            }
        }
        if animated {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.18
                change()
            }
        } else {
            change()
        }
    }
}
