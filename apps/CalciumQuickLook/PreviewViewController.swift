import AppKit
import CoreText
import QuickLookUI

/// The Finder/Files preview: the document in a read-only text view, styled
/// exactly as the editor would style it.
final class PreviewViewController: NSViewController, QLPreviewingController {

    /// Fira Code, bundled in the extension and registered by hand: an appex
    /// does not get the app's `ATSApplicationFontsPath` treatment. If
    /// registration fails, `Typography` falls back to the system monospace.
    private static let fontsRegistered: Void = {
        for url in Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: "Fonts")
            ?? []
        {
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }()

    private var textView: NSTextView?

    override func loadView() {
        let scrollView = NSTextView.scrollableTextView()
        scrollView.borderType = .noBorder
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        if let documentView = scrollView.documentView as? NSTextView {
            documentView.isEditable = false
            documentView.textContainerInset = CGSize(width: 14, height: 14)
            textView = documentView
        }
        view = scrollView
    }

    func preparePreviewOfFile(at url: URL) async throws {
        _ = Self.fontsRegistered
        let source = try String(contentsOf: url, encoding: .utf8)
        _ = view // materialise the hierarchy before reaching for the text view
        textView?.textStorage?.setAttributedString(PreviewRenderer.render(source))
    }
}
