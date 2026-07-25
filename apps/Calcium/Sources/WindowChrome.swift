#if os(macOS)
import AppKit
import SwiftUI

/// Distraction-free window chrome, on the pattern proven in Typeset.
///
/// The hiding itself happens in SwiftUI: `ContentView` toggles the window
/// toolbar *by value* — `.toolbar(_:for: .windowToolbar)` — which collapses
/// the whole title-bar area, traffic lights included, through the system's
/// own mechanism. Two earlier attempts here did it in AppKit and each failed
/// instructively: a tracking-area *subview* was reconciled away by SwiftUI,
/// and alpha-hiding the traffic lights silently broke ⌘W and ⌘M, because
/// `performClose:` works by simulating a click on the button it could no
/// longer find.
///
/// What remains in AppKit is only watching the pointer: a tracking *area*
/// added to the window's content view — areas are not subviews, so SwiftUI
/// leaves them alone — with hysteresis and a debounced hide so the chrome
/// neither strobes at the boundary nor vanishes mid-reach.
struct WindowChrome: NSViewRepresentable {
    var isEnabled: Bool
    /// Fires when the auto-hide state flips; SwiftUI reacts by value.
    var onChromeHiddenChange: (Bool) -> Void

    func makeNSView(context: Context) -> NSView {
        ChromeView(isEnabled: isEnabled, onChromeHiddenChange: onChromeHiddenChange)
    }

    func updateNSView(_ view: NSView, context: Context) {
        guard let chrome = view as? ChromeView else { return }
        chrome.onChromeHiddenChange = onChromeHiddenChange
        chrome.isEnabled = isEnabled
    }
}

private final class ChromeView: NSView {
    var isEnabled: Bool {
        didSet {
            guard oldValue != isEnabled else { return }
            applyMode()
        }
    }
    var onChromeHiddenChange: (Bool) -> Void

    private weak var configuredWindow: NSWindow?
    private weak var trackingView: NSView?
    private var trackingArea: NSTrackingArea?
    private var chromeHidden = false
    private var hideTask: Task<Void, Never>?

    /// The strip below the window's top edge that reveals the chrome.
    private let revealHeight: CGFloat = 40
    /// Once shown, it stays until the pointer drops this much further, so the
    /// boundary does not strobe.
    private let revealHysteresis: CGFloat = 32

    init(isEnabled: Bool, onChromeHiddenChange: @escaping (Bool) -> Void) {
        self.isEnabled = isEnabled
        self.onChromeHiddenChange = onChromeHiddenChange
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    deinit {
        hideTask?.cancel()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        applyMode()
    }

    private func applyMode() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            guard let window = self.window else { return }
            if self.configuredWindow !== window {
                self.removeTracking()
                self.configuredWindow = window
                self.chromeHidden = false
                window.styleMask.insert(.fullSizeContentView)
                window.titlebarAppearsTransparent = true
                window.titlebarSeparatorStyle = .none
            }
            guard self.isEnabled else {
                self.removeTracking()
                self.setChromeHidden(false)
                return
            }
            self.installTracking()
            self.updateForPointer()
        }
    }

    private func installTracking() {
        guard let contentView = configuredWindow?.contentView else { return }
        guard trackingArea == nil || trackingView !== contentView else { return }
        removeTracking()
        // An area on the content view, owned by us: no subview enters
        // SwiftUI's hierarchy, so there is nothing for it to reconcile away.
        let area = NSTrackingArea(
            rect: .zero,
            options: [
                .activeInKeyWindow, .enabledDuringMouseDrag, .inVisibleRect,
                .mouseEnteredAndExited, .mouseMoved,
            ],
            owner: self)
        contentView.addTrackingArea(area)
        trackingArea = area
        trackingView = contentView
    }

    private func removeTracking() {
        if let trackingArea, let trackingView {
            trackingView.removeTrackingArea(trackingArea)
        }
        trackingArea = nil
        trackingView = nil
    }

    override func mouseEntered(with event: NSEvent) { updateForPointer() }
    override func mouseMoved(with event: NSEvent) { updateForPointer() }
    override func mouseDragged(with event: NSEvent) { updateForPointer() }
    override func mouseExited(with event: NSEvent) { updateForPointer() }

    private func updateForPointer() {
        guard isEnabled, let window = configuredWindow else { return }
        if pointerInRevealArea(of: window) {
            hideTask?.cancel()
            hideTask = nil
            setChromeHidden(false)
        } else {
            scheduleHide()
        }
    }

    /// The hide waits out brief excursions — revealing the title bar shifts
    /// what is under the pointer, and reacting instantly strobes.
    private func scheduleHide() {
        guard !chromeHidden, hideTask == nil else { return }
        hideTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(350))
            guard let self, !Task.isCancelled else { return }
            self.hideTask = nil
            guard self.isEnabled, let window = self.configuredWindow,
                  !self.pointerInRevealArea(of: window)
            else { return }
            self.setChromeHidden(true)
        }
    }

    private func pointerInRevealArea(of window: NSWindow) -> Bool {
        let point = NSEvent.mouseLocation
        let frame = window.frame
        guard point.x >= frame.minX, point.x <= frame.maxX else { return false }
        // Inclusive at the top edge: `NSRect.contains` excludes max-Y, which
        // is exactly where the pointer rests on a revealed title bar.
        let fromTop = frame.maxY - point.y
        guard fromTop >= 0 else { return false }
        let limit = chromeHidden ? revealHeight : revealHeight + revealHysteresis
        return fromTop <= limit
    }

    private func setChromeHidden(_ hidden: Bool) {
        guard chromeHidden != hidden else { return }
        chromeHidden = hidden
        onChromeHiddenChange(hidden)
    }
}
#endif
