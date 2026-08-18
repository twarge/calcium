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
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // The Services menu: select Calcium text in any app, Compute
        // Calcium, and the selection comes back with its answers written
        // in — doc::rewrite as a system verb.
        NSApp.servicesProvider = ComputeService()
    }
}

/// Answers `Compute Calcium` and `Convert Calcium to Typst` from the
/// Services menu.
final class ComputeService: NSObject {
    @objc func computeCalcium(
        _ pasteboard: NSPasteboard, userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        guard let text = pasteboard.string(forType: .string) else {
            error.pointee = "No text in the selection." as NSString
            return
        }
        pasteboard.clearContents()
        pasteboard.setString(Engine.materializingAnswers(in: text), forType: .string)
    }

    @objc func convertToTypst(
        _ pasteboard: NSPasteboard, userData: String?,
        error: AutoreleasingUnsafeMutablePointer<NSString>
    ) {
        guard let text = pasteboard.string(forType: .string) else {
            error.pointee = "No text in the selection." as NSString
            return
        }
        pasteboard.clearContents()
        pasteboard.setString(Engine.typstMarkup(of: text), forType: .string)
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
            // The stock Format menu speaks rich text, which the parser cannot
            // read back. This one speaks the document's own marks — Markdown
            // emphasis on prose — and the engine's `@` directives, inserted
            // above the calculation the caret is in.
            CommandGroup(replacing: .textFormatting) {
                Button("Bold") { CommandBus.shared.send(.toggleMark("**")) }
                    .keyboardShortcut("b", modifiers: .command)
                Button("Italic") { CommandBus.shared.send(.toggleMark("_")) }
                    .keyboardShortcut("i", modifiers: .command)
                Button("Code") { CommandBus.shared.send(.toggleMark("`")) }
                    .keyboardShortcut("c", modifiers: [.command, .shift])
                Button("Add Link") { CommandBus.shared.send(.insertLink) }
                    .keyboardShortcut("k", modifiers: .command)
                Divider()
                Button("Heading 1") { CommandBus.shared.send(.heading(1)) }
                    .keyboardShortcut("1", modifiers: [.command, .option])
                Button("Heading 2") { CommandBus.shared.send(.heading(2)) }
                    .keyboardShortcut("2", modifiers: [.command, .option])
                Button("Heading 3") { CommandBus.shared.send(.heading(3)) }
                    .keyboardShortcut("3", modifiers: [.command, .option])
                Button("Blockquote") { CommandBus.shared.send(.blockquote) }
                    .keyboardShortcut("b", modifiers: [.command, .shift])
                Divider()
                // Each inserts the directive with its value selected, ready
                // to be typed over. The default values are the engine's own.
                Menu("Insert Directive") {
                    Button("Precision") {
                        CommandBus.shared.send(.insertDirective("@precision = 4"))
                    }
                    Button("Significant Figures") {
                        CommandBus.shared.send(.insertDirective("@sigfigs"))
                    }
                    Button("Digit Grouping") {
                        CommandBus.shared.send(.insertDirective("@group = false"))
                    }
                    Divider()
                    Button("Point Decimal (1,234.5)") {
                        CommandBus.shared.send(.insertDirective("@en-US"))
                    }
                    Button("Comma Decimal (1.234,5)") {
                        CommandBus.shared.send(.insertDirective("@de-DE"))
                    }
                    Button("French Spacing (1 234,5)") {
                        CommandBus.shared.send(.insertDirective("@fr-FR"))
                    }
                }
            }
            #if os(macOS)
            CommandGroup(after: .pasteboard) {
                FindCommands()
            }
            // Typst, in the File menu where exporting belongs. Both travel
            // over the bus because only the key window's editor has the text.
            CommandGroup(after: .importExport) {
                Button("Export to Typst…") { CommandBus.shared.send(.exportTypst) }
                Button("Typeset PDF") { CommandBus.shared.send(.typesetPDF) }
                    .keyboardShortcut("p", modifiers: [.command, .option])
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
