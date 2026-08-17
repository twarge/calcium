#if os(macOS)
import AppKit
import UniformTypeIdentifiers

/// Export to Typst, and — once the user installs a one-line helper —
/// typesetting straight to PDF with the `typst` CLI.
///
/// The app is sandboxed, so it cannot run `/opt/homebrew/bin/typst` itself:
/// the sandbox denies both the exec and the network access Typst's package
/// cache wants. `NSUserUnixTask` is the sanctioned way out — a script the
/// user placed in `~/Library/Application Scripts/com.twarge.calcium/` runs
/// outside the sandbox, with the user's own PATH resolution baked in at
/// install time and the user's own package cache.
@MainActor
enum TypstExport {

    /// File > Export to Typst…: the converted document through a save panel.
    static func exportPanel(source: String, documentURL: URL?, window: NSWindow?) {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "typ") ?? .plainText]
        panel.nameFieldStringValue = baseName(of: documentURL) + ".typ"
        let save: (NSApplication.ModalResponse) -> Void = { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try Engine.typstMarkup(of: source)
                    .write(to: url, atomically: true, encoding: .utf8)
            } catch {
                presentError("Could not write the Typst file.",
                             detail: error.localizedDescription)
            }
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: save)
        } else {
            save(panel.runModal())
        }
    }

    /// File > Typeset PDF: convert, compile, and hand the PDF to Preview.
    ///
    /// Two compilers are tried in order. The Typeset app's `Compile Typst to
    /// PDF` service is first — Typeset embeds the compiler in-process, so it
    /// needs no setup at all. The helper script wrapping the `typst` CLI is
    /// second. With neither present, explain the choices.
    static func typeset(source: String, documentURL: URL?) {
        let markup = Engine.typstMarkup(of: source)
        let base = baseName(of: documentURL)
        if typesetWithTypesetApp(markup, base: base) {
            return
        }
        guard let script = helperScript() else {
            offerSetup()
            return
        }
        let folder = FileManager.default.temporaryDirectory
        let input = folder.appendingPathComponent(base + ".typ")
        let output = folder.appendingPathComponent(base + ".pdf")
        let log = folder.appendingPathComponent(base + ".typst.log")
        do {
            try markup.write(to: input, atomically: true, encoding: .utf8)
            FileManager.default.createFile(atPath: log.path, contents: nil)
            let logHandle = try FileHandle(forWritingTo: log)
            let task = try NSUserUnixTask(url: script)
            task.standardOutput = logHandle
            task.standardError = logHandle
            task.execute(withArguments: ["compile", input.path, output.path]) { error in
                // The task calls back on its own queue; the alerts and
                // NSWorkspace belong to the main actor.
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        finished(error: error, output: output, log: log)
                    }
                }
            }
        } catch {
            presentError("Could not run the Typst helper.",
                         detail: error.localizedDescription)
        }
    }

    private static func finished(error: Error?, output: URL, log: URL) {
        if error == nil, FileManager.default.fileExists(atPath: output.path) {
            NSWorkspace.shared.open(output)
            return
        }
        let report = (try? String(contentsOf: log, encoding: .utf8))?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        presentError(
            "Typst could not typeset the document.",
            detail: report.isEmpty
                ? (error?.localizedDescription ?? "No output was produced.")
                : report)
    }

    /// Compiles through the Typeset app's pasteboard service, if Typeset is
    /// installed: markup out, PDF data back, no sandbox gymnastics — the
    /// compiler runs inside Typeset. Returns false when the service is
    /// unavailable or declined the job.
    private static func typesetWithTypesetApp(_ markup: String, base: String) -> Bool {
        let pasteboard = NSPasteboard.withUniqueName()
        pasteboard.clearContents()
        pasteboard.setString(markup, forType: .string)
        guard NSPerformService("Compile Typst to PDF", pasteboard),
              let data = pasteboard.data(forType: .pdf)
        else { return false }
        let output = FileManager.default.temporaryDirectory
            .appendingPathComponent(base + ".pdf")
        do {
            try data.write(to: output)
        } catch {
            presentError("Could not save the typeset PDF.",
                         detail: error.localizedDescription)
            return true
        }
        NSWorkspace.shared.open(output)
        return true
    }

    /// The user-installed helper, if there is one.
    private static func helperScript() -> URL? {
        guard let folder = try? FileManager.default.url(
            for: .applicationScriptsDirectory, in: .userDomainMask,
            appropriateFor: nil, create: false)
        else { return nil }
        let script = folder.appendingPathComponent("typst")
        return FileManager.default.isExecutableFile(atPath: script.path) ? script : nil
    }

    /// Resolves `typst` with the user's shell PATH at install time and bakes
    /// the result in, because the helper later runs with a minimal PATH.
    private static let setupCommand =
        #"mkdir -p ~/Library/Application\ Scripts/com.twarge.calcium && "# +
        #"printf '#!/bin/sh\nexec %s "$@"\n' "$(command -v typst)" "# +
        #"> ~/Library/Application\ Scripts/com.twarge.calcium/typst && "# +
        #"chmod +x ~/Library/Application\ Scripts/com.twarge.calcium/typst"#

    private static func offerSetup() {
        let alert = NSAlert()
        alert.messageText = "Set Up Typst Typesetting"
        alert.informativeText = """
            The easy way: install the Typeset app, and Calcium hands documents \
            to its built-in Typst compiler with no setup at all.

            The CLI way: Calcium is sandboxed, so it can only run the typst \
            command through a helper script you install once. With Typst \
            installed (brew install typst), paste this into Terminal, then \
            choose Typeset PDF again:

            \(setupCommand)
            """
        alert.addButton(withTitle: "Copy Command")
        alert.addButton(withTitle: "Cancel")
        if alert.runModal() == .alertFirstButtonReturn {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(setupCommand, forType: .string)
        }
    }

    private static func baseName(of url: URL?) -> String {
        url?.deletingPathExtension().lastPathComponent ?? "Untitled"
    }

    private static func presentError(_ message: String, detail: String) {
        let alert = NSAlert()
        alert.messageText = message
        alert.informativeText = detail
        alert.runModal()
    }
}
#endif
