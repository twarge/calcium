import SwiftUI

#if os(macOS)
import AppKit
#endif
#if os(iOS)
import UIKit
#endif


#if os(macOS)
/// The Find menu, matching TextEdit's.
///
/// `NSTextView` already implements all of this through `NSTextFinder`; what it
/// needs is menu items that send `performTextFinderAction:` with the right
/// tag, which `Button` cannot express on its own. Hence the throwaway
/// `NSMenuItem` used purely to carry the tag to the responder chain.
private struct FindCommands: View {
    var body: some View {
        Menu("Find") {
            command("Find…", .showFindInterface, "f", [.command])
            command("Find and Replace…", .showReplaceInterface, "f", [.command, .option])
            command("Find Next", .nextMatch, "g", [.command])
            command("Find Previous", .previousMatch, "g", [.command, .shift])
            Divider()
            command("Use Selection for Find", .setSearchString, "e", [.command])
            Button("Jump to Selection") {
                NSApp.sendAction(
                    #selector(NSResponder.centerSelectionInVisibleArea(_:)), to: nil, from: nil)
            }
            .keyboardShortcut("j", modifiers: [.command])
        }
    }

    private func command(
        _ title: String, _ action: NSTextFinder.Action,
        _ key: KeyEquivalent, _ modifiers: EventModifiers
    ) -> some View {
        Button(title) { perform(action) }
            .keyboardShortcut(key, modifiers: modifiers)
    }

    private func perform(_ action: NSTextFinder.Action) {
        let sender = NSMenuItem()
        sender.tag = action.rawValue
        NSApp.sendAction(
            #selector(NSResponder.performTextFinderAction(_:)), to: nil, from: sender)
    }
}
#endif

#if os(macOS)
/// Removes the Format menu.
///
/// `CommandGroup(replacing: .textFormatting) {}` empties it, but SwiftUI
/// offers no way to remove the menu itself, so an empty "Format" is left in
/// the menu bar. It is deleted here at the AppKit level — and again whenever
/// a window becomes main, because SwiftUI rebuilds the main menu as scenes
/// come and go.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        AppDelegate.removeFormatMenu()
        NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeMainNotification, object: nil, queue: .main
        ) { _ in
            // Window notifications arrive on the main thread.
            MainActor.assumeIsolated { AppDelegate.removeFormatMenu() }
        }
    }

    private static func removeFormatMenu() {
        // Async: at the moment of the notification SwiftUI may not have
        // finished rebuilding the menu it is about to hand us.
        DispatchQueue.main.async {
            guard let menu = NSApp.mainMenu else { return }
            while let index = menu.items.firstIndex(where: {
                $0.title == NSLocalizedString("Format", comment: "")
            }) {
                menu.removeItem(at: index)
            }
        }
    }
}

#endif

@main
struct CalciumApp: App {
    #if os(macOS)
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    #endif

    init() {
        // The `NSTextView` properties are not enough on their own: the smart
        // substitutions are driven by user defaults read from the global
        // domain, and period substitution has no per-view property at all.
        // Writing them into *this app's* domain overrides the global values,
        // which is exactly what the per-app override is for.
        //
        // This matters more here than in a prose editor. Double-space becoming
        // ". " turns `3 +  =>` into `3 +. =>`, a straight quote becoming a
        // curly one turns a string literal into a syntax error, and `--`
        // becoming an em dash breaks subtraction. Each one leaves the author
        // staring at a line that looks right and does not work.
        // Spelling correction is deliberately absent: the editor manages it
        // per-view, offered in prose and withheld from calculations.
        for key in [
            "NSAutomaticPeriodSubstitutionEnabled",
            "NSAutomaticQuoteSubstitutionEnabled",
            "NSAutomaticDashSubstitutionEnabled",
            "NSAutomaticTextReplacementEnabled",
            "NSAutomaticCapitalizationEnabled",
            "NSAutomaticTextCompletionEnabled",
        ] {
            UserDefaults.standard.set(false, forKey: key)
        }
    }

    var body: some Scene {
        DocumentGroup(newDocument: CalciumDocument()) { file in
            ContentView(text: file.$document.text, fileURL: file.fileURL)
        }
        .defaultSize(width: 900, height: 620)
        .commands {
            // The format has no use for rich text, and these would only put
            // characters in the buffer that the parser cannot read back.
            CommandGroup(replacing: .textFormatting) {}
            #if os(macOS)
            CommandGroup(after: .pasteboard) {
                FindCommands()
            }
            #endif
            // Line commands, delivered over the command bus to the key
            // window's editor. Comment toggling only touches indented lines — an
            // unindented leading `#` would be a heading — and indenting is
            // meaningful here: it is what makes a line a calculation.
            CommandGroup(after: .pasteboard) {
                Divider()
                Button("Toggle Comment") { CommandBus.shared.send(.toggleComment) }
                    .keyboardShortcut("/", modifiers: .command)
                Button("Indent") { CommandBus.shared.send(.indent) }
                    .keyboardShortcut("]", modifiers: .command)
                Button("Outdent") { CommandBus.shared.send(.outdent) }
                    .keyboardShortcut("[", modifiers: .command)
            }
            #if os(iOS)
            // On the Mac, ⌘N already means "new document window" and the
            // system provides it. On iPad it is unclaimed, and a fresh scene
            // is the nearest equivalent: it opens on the document browser,
            // ready to create or open. Requires the multiple-scenes opt-in
            // in the Info.plist to do anything.
            CommandGroup(after: .newItem) {
                Button("New Window") {
                    UIApplication.shared.activateSceneSession(
                        for: UISceneSessionActivationRequest())
                }
                .keyboardShortcut("n", modifiers: .command)
            }
            #endif
            CommandGroup(replacing: .help) {
                Link("Calcium Reference", destination: URL(string: "https://github.com/twarge/calcium")!)
            }
        }

        #if os(macOS)
        Settings {
            PreferencesView()
        }
        #endif
    }
}
